use std::time::Instant;

use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;

use crate::config::HttpMethod;
use crate::models::ExecutionStatus;
use crate::models::execution;

use super::{ExecutionContext, ExecutionResult, ResolvedExecutor, prepare_log_files};

/// Run an HTTP request executor.
///
/// Makes an HTTP request to the configured URL. The response body (truncated to 4KB)
/// is written to stdout.log. Request/response metadata (method, URL, status, timing,
/// response headers) is written to stderr.log.
///
/// HTTP status 2xx → Success with exit_code = HTTP status code.
/// All other statuses → Failed with exit_code = HTTP status code.
/// Request error or spawn error → Failed with exit_code = None.
pub async fn run_http(
    pool: &SqlitePool,
    ctx: &ExecutionContext,
    client: &reqwest::Client,
) -> ExecutionResult {
    let log_dir_str = format!("{}/{}", ctx.logs_dir, ctx.execution_id);

    // 1. Prepare log files
    let (log_dir, mut stdout_file, mut stderr_file) =
        match prepare_log_files(&ctx.logs_dir, &ctx.execution_id, &ctx.payload_json).await {
            Ok(files) => files,
            Err(e) => {
                tracing::error!(
                    execution_id = %ctx.execution_id,
                    "failed to prepare log files: {e}"
                );
                return ExecutionResult {
                    status: ExecutionStatus::Failed,
                    exit_code: None,
                    log_dir: log_dir_str,
                };
            }
        };

    let log_dir_display = log_dir.display().to_string();

    // 2. Mark running in DB
    if let Err(e) = execution::mark_running(pool, &ctx.execution_id).await {
        tracing::error!(
            execution_id = %ctx.execution_id,
            "failed to mark execution as running: {e}"
        );
        return ExecutionResult {
            status: ExecutionStatus::Failed,
            exit_code: None,
            log_dir: log_dir_display,
        };
    }

    // 3. Extract HTTP config from the resolved executor
    let (method, url, headers, body, follow_redirects) = match &ctx.executor {
        ResolvedExecutor::Http {
            method,
            url,
            headers,
            body,
            follow_redirects,
        } => (
            *method,
            url.as_str(),
            headers,
            body.as_deref(),
            *follow_redirects,
        ),
        _ => {
            // Should not happen -- run_http is only called for Http executors
            tracing::error!(
                execution_id = %ctx.execution_id,
                "run_http called with non-Http executor"
            );
            let _ =
                execution::mark_completed(pool, &ctx.execution_id, ExecutionStatus::Failed, None)
                    .await;
            return ExecutionResult {
                status: ExecutionStatus::Failed,
                exit_code: None,
                log_dir: log_dir_display,
            };
        }
    };

    // 4. Build the request
    let reqwest_method = match method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Post => reqwest::Method::POST,
        HttpMethod::Put => reqwest::Method::PUT,
        HttpMethod::Patch => reqwest::Method::PATCH,
        HttpMethod::Delete => reqwest::Method::DELETE,
    };

    // If follow_redirects is disabled, build a one-off client with no-follow policy.
    // The shared client always follows redirects (default policy).
    let owned_client;
    let effective_client: &reqwest::Client = if !follow_redirects {
        owned_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        &owned_client
    } else {
        client
    };

    let mut request_builder = effective_client.request(reqwest_method.clone(), url);

    // Apply headers, resolving ${ENV_VAR} references in values
    for (key, value) in headers {
        let resolved_value = resolve_env_refs(value);
        request_builder = request_builder.header(key.as_str(), resolved_value);
    }

    if let Some(body_str) = body {
        request_builder = request_builder.body(body_str.to_owned());
    }

    // 5. Execute with timeout
    let start = Instant::now();
    let work = async move { request_builder.send().await };

    let outcome: Result<Result<reqwest::Response, reqwest::Error>, tokio::time::error::Elapsed> =
        tokio::time::timeout(ctx.timeout, work).await;
    let elapsed = start.elapsed();

    // 6. Handle result
    let (status, exit_code) = match outcome {
        Err(_elapsed) => {
            // Timeout
            let meta = format!(
                "method={} url={} timeout={}ms\n",
                reqwest_method,
                url,
                ctx.timeout.as_millis()
            );
            let _ = stderr_file.write_all(meta.as_bytes()).await;
            let _ =
                execution::mark_completed(pool, &ctx.execution_id, ExecutionStatus::TimedOut, None)
                    .await;
            return ExecutionResult {
                status: ExecutionStatus::TimedOut,
                exit_code: None,
                log_dir: log_dir_display,
            };
        }
        Ok(Err(e)) => {
            // Request error (connection refused, DNS failure, etc.)
            let msg = format!("http request failed: {e}\n");
            let _ = stderr_file.write_all(msg.as_bytes()).await;
            tracing::error!(
                execution_id = %ctx.execution_id,
                "http request error: {e}"
            );
            (ExecutionStatus::Failed, None)
        }
        Ok(Ok(response)) => {
            let http_status = response.status();
            let status_code = http_status.as_u16() as i32;

            // Write metadata to stderr.log
            let mut meta = format!(
                "method={} url={} status={} elapsed={}ms\n",
                reqwest_method,
                url,
                http_status,
                elapsed.as_millis()
            );
            for (name, value) in response.headers() {
                let v = value.to_str().unwrap_or("<non-utf8>");
                meta.push_str(&format!("response-header: {name}: {v}\n"));
            }
            let _ = stderr_file.write_all(meta.as_bytes()).await;

            // Write response body to stdout.log (truncated to 4KB)
            const MAX_BODY: usize = 4096;
            match response.bytes().await {
                Ok(bytes) => {
                    let truncated = if bytes.len() > MAX_BODY {
                        &bytes[..MAX_BODY]
                    } else {
                        &bytes
                    };
                    let _ = stdout_file.write_all(truncated).await;
                }
                Err(e) => {
                    tracing::warn!(
                        execution_id = %ctx.execution_id,
                        "failed to read response body: {e}"
                    );
                }
            }

            let exec_status = if http_status.is_success() {
                ExecutionStatus::Success
            } else {
                ExecutionStatus::Failed
            };

            (exec_status, Some(status_code))
        }
    };

    // 7. Mark completed in DB
    if let Err(e) =
        execution::mark_completed(pool, &ctx.execution_id, status.clone(), exit_code).await
    {
        tracing::error!(
            execution_id = %ctx.execution_id,
            "failed to mark execution as completed: {e}"
        );
    }

    ExecutionResult {
        status,
        exit_code,
        log_dir: log_dir_display,
    }
}

/// Resolve `${ENV_VAR}` references in a string to their current environment values.
/// Unset variables are replaced with an empty string.
fn resolve_env_refs(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let var_name: String = chars.by_ref().take_while(|&c| c != '}').collect();
            let val = std::env::var(&var_name).unwrap_or_default();
            result.push_str(&val);
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HttpMethod;
    use crate::db::Db;
    use crate::models::execution;
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    async fn test_pool() -> sqlx::SqlitePool {
        let db = Db::new_in_memory().await.expect("in-memory db");
        db.migrate().await.expect("migration");
        db.pool().clone()
    }

    async fn setup_execution(
        pool: &sqlx::SqlitePool,
        logs_dir: &str,
        url: &str,
    ) -> (ExecutionContext, String) {
        let exec = execution::create(
            pool,
            &execution::NewExecution {
                id: None,
                hook_slug: "test-hook",
                log_path: logs_dir,
                trigger_source: "127.0.0.1",
                request_payload: "{}",
                retry_of: None,
                status: None,
            },
        )
        .await
        .expect("create execution");

        let exec_id = exec.id.clone();
        let ctx = ExecutionContext {
            execution_id: exec.id,
            hook_slug: "test-hook".into(),
            executor: ResolvedExecutor::Http {
                method: HttpMethod::Get,
                url: url.into(),
                headers: HashMap::new(),
                body: None,
                follow_redirects: true,
            },
            env: HashMap::new(),
            cwd: None,
            timeout: Duration::from_secs(5),
            logs_dir: logs_dir.into(),
            payload_json: "{}".into(),
            http_client: None,
        };
        (ctx, exec_id)
    }

    async fn read_log(logs_dir: &str, exec_id: &str, file: &str) -> String {
        let path = std::path::Path::new(logs_dir).join(exec_id).join(file);
        tokio::fs::read_to_string(path).await.unwrap_or_default()
    }

    /// Spawn a minimal HTTP server that responds with a fixed status + body.
    /// Returns the bound address. The server runs until the returned `JoinHandle` is dropped.
    async fn spawn_stub_server(
        status: u16,
        body: &'static str,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let handle = tokio::spawn(async move {
            // Accept one connection and respond
            if let Ok((mut stream, _)) = listener.accept().await {
                let (reader, mut writer) = stream.split();
                let mut buf_reader = BufReader::new(reader);
                // Drain the request headers
                let mut line = String::new();
                loop {
                    line.clear();
                    let _ = buf_reader.read_line(&mut line).await;
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = writer.write_all(response.as_bytes()).await;
            }
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn http_200_succeeds() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8");
        let pool = test_pool().await;
        let (addr, _server) = spawn_stub_server(200, "ok").await;

        let url = format!("http://{addr}/");
        let (ctx, _exec_id) = setup_execution(&pool, logs_dir, &url).await;
        let client = reqwest::Client::new();
        let result = run_http(&pool, &ctx, &client).await;

        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.exit_code, Some(200));
    }

    #[tokio::test]
    async fn http_500_fails() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8");
        let pool = test_pool().await;
        let (addr, _server) = spawn_stub_server(500, "error").await;

        let url = format!("http://{addr}/");
        let (ctx, _exec_id) = setup_execution(&pool, logs_dir, &url).await;
        let client = reqwest::Client::new();
        let result = run_http(&pool, &ctx, &client).await;

        assert_eq!(result.status, ExecutionStatus::Failed);
        assert_eq!(result.exit_code, Some(500));
    }

    #[tokio::test]
    async fn http_logs_response_body() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8");
        let pool = test_pool().await;
        let (addr, _server) = spawn_stub_server(200, "hello world").await;

        let url = format!("http://{addr}/");
        let (ctx, exec_id) = setup_execution(&pool, logs_dir, &url).await;
        let client = reqwest::Client::new();
        run_http(&pool, &ctx, &client).await;

        let stdout = read_log(logs_dir, &exec_id, "stdout.log").await;
        assert!(stdout.contains("hello world"), "stdout: {stdout:?}");
    }

    #[tokio::test]
    async fn http_logs_metadata() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8");
        let pool = test_pool().await;
        let (addr, _server) = spawn_stub_server(200, "").await;

        let url = format!("http://{addr}/");
        let (ctx, exec_id) = setup_execution(&pool, logs_dir, &url).await;
        let client = reqwest::Client::new();
        run_http(&pool, &ctx, &client).await;

        let stderr = read_log(logs_dir, &exec_id, "stderr.log").await;
        assert!(stderr.contains("method=GET"), "stderr: {stderr:?}");
        assert!(stderr.contains("status=200"), "stderr: {stderr:?}");
    }

    #[tokio::test]
    async fn http_timeout() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8");
        let pool = test_pool().await;

        // Bind a listener but never accept / respond so the request hangs
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let _listener = listener; // Keep alive but never respond

        let url = format!("http://{addr}/");
        let (mut ctx, _exec_id) = setup_execution(&pool, logs_dir, &url).await;
        ctx.timeout = Duration::from_millis(100);

        let client = reqwest::Client::new();
        let start = std::time::Instant::now();
        let result = run_http(&pool, &ctx, &client).await;
        let elapsed = start.elapsed();

        assert_eq!(result.status, ExecutionStatus::TimedOut);
        assert!(result.exit_code.is_none());
        assert!(elapsed < Duration::from_secs(2), "elapsed: {elapsed:?}");
    }

    #[test]
    fn resolve_env_refs_known_var() {
        // Safety: single-threaded test, no other threads reading this var concurrently.
        unsafe { std::env::set_var("TEST_HTTP_TOKEN_12345", "mysecret") };
        let result = resolve_env_refs("Bearer ${TEST_HTTP_TOKEN_12345}");
        assert_eq!(result, "Bearer mysecret");
        // Safety: same as above.
        unsafe { std::env::remove_var("TEST_HTTP_TOKEN_12345") };
    }

    #[test]
    fn resolve_env_refs_unset_var() {
        let result = resolve_env_refs("Bearer ${SENDWORD_UNSET_XYZ_12345}");
        assert_eq!(result, "Bearer ");
    }

    #[test]
    fn resolve_env_refs_no_refs() {
        let result = resolve_env_refs("plain-value");
        assert_eq!(result, "plain-value");
    }
}
