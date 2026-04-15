pub mod http;
pub mod script;
pub mod shell;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::fs;

use crate::config::HttpMethod;
use crate::models::ExecutionStatus;

/// Which executor to use for a hook.
#[derive(Clone)]
pub enum ResolvedExecutor {
    Shell {
        command: String,
    },
    Script {
        path: PathBuf,
    },
    Http {
        method: HttpMethod,
        url: String,
        headers: HashMap<String, String>,
        body: Option<String>,
        follow_redirects: bool,
    },
}

/// Everything the executor needs to run a command.
#[derive(Clone)]
pub struct ExecutionContext {
    /// The execution record ID (UUIDv7 string).
    pub execution_id: String,
    /// The hook slug, passed as SENDWORD_HOOK_SLUG env var.
    pub hook_slug: String,
    /// Which executor to dispatch to.
    pub executor: ResolvedExecutor,
    /// Additional environment variables for the process.
    pub env: HashMap<String, String>,
    /// Working directory. If None, inherits from the server process.
    pub cwd: Option<String>,
    /// Maximum execution time. Process is killed on expiry.
    pub timeout: Duration,
    /// Base directory for log files (e.g., "data/logs").
    pub logs_dir: String,
    /// Raw JSON payload from the trigger request. Set as SENDWORD_PAYLOAD
    /// env var and written to payload.json in the log directory.
    pub payload_json: String,
    /// Shared HTTP client for HTTP executor. Shell and script set this to None.
    /// reqwest::Client is cheaply clonable (Arc internally).
    pub http_client: Option<reqwest::Client>,
}

/// The outcome of an execution attempt.
pub struct ExecutionResult {
    /// Terminal status: Success, Failed, or TimedOut.
    pub status: ExecutionStatus,
    /// Process exit code. None if the process was killed or failed to spawn.
    pub exit_code: Option<i32>,
    /// Path to the log directory (data/logs/{execution_id}).
    pub log_dir: String,
}

/// Create the log directory and open stdout/stderr files for writing.
/// Returns (log_dir_path, stdout_file, stderr_file).
///
/// Files are opened in append mode so that retry attempts append to
/// existing log files rather than truncating them.
pub(crate) async fn prepare_log_files(
    logs_dir: &str,
    execution_id: &str,
    payload_json: &str,
) -> std::io::Result<(PathBuf, fs::File, fs::File)> {
    let log_dir = Path::new(logs_dir).join(execution_id);
    fs::create_dir_all(&log_dir).await?;

    // Write payload.json (uses write, not append, so retries overwrite
    // with identical content rather than duplicating)
    fs::write(log_dir.join("payload.json"), payload_json.as_bytes()).await?;

    let stdout_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("stdout.log"))
        .await?;
    let stderr_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("stderr.log"))
        .await?;

    Ok((log_dir, stdout_file, stderr_file))
}

/// Collect system environment variables that should be passed to child processes.
/// Returns only the vars that are actually set in the current process.
pub(crate) fn system_env_vars() -> HashMap<String, String> {
    const INHERIT_VARS: &[&str] = &["PATH", "HOME", "USER", "LANG"];

    let mut vars = HashMap::with_capacity(INHERIT_VARS.len());
    for &name in INHERIT_VARS {
        if let Ok(val) = std::env::var(name) {
            vars.insert(name.into(), val);
        }
    }
    vars
}

/// Dispatch to the appropriate executor based on the resolved executor type.
pub async fn run(pool: &SqlitePool, ctx: ExecutionContext) -> ExecutionResult {
    match &ctx.executor {
        ResolvedExecutor::Shell { command } => shell::run_shell(pool, &ctx, command).await,
        ResolvedExecutor::Script { path } => script::run_script(pool, &ctx, path).await,
        ResolvedExecutor::Http { .. } => {
            let client = ctx.http_client.clone().unwrap_or_default();
            http::run_http(pool, &ctx, &client).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::models::execution;
    use sqlx::SqlitePool;

    // --- Unit tests ---

    #[tokio::test]
    async fn prepare_log_files_creates_directory_and_files() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let exec_id = "test-exec-001";

        let (log_dir, _stdout, _stderr) = prepare_log_files(logs_dir, exec_id, "{}")
            .await
            .expect("prepare_log_files");

        assert!(log_dir.exists());
        assert!(log_dir.join("stdout.log").exists());
        assert!(log_dir.join("stderr.log").exists());
    }

    #[test]
    fn system_env_vars_includes_path() {
        let vars = system_env_vars();
        assert!(
            vars.contains_key("PATH"),
            "PATH should be present in system env vars"
        );
    }

    #[test]
    fn system_env_vars_excludes_arbitrary_vars() {
        // Safety: test-only env var, unique name avoids collisions
        unsafe { std::env::set_var("SENDWORD_TEST_ARBITRARY_XYZ_999", "leaked") };
        let vars = system_env_vars();
        assert!(
            !vars.contains_key("SENDWORD_TEST_ARBITRARY_XYZ_999"),
            "arbitrary env vars should not be inherited"
        );
        unsafe { std::env::remove_var("SENDWORD_TEST_ARBITRARY_XYZ_999") };
    }

    #[tokio::test]
    async fn prepare_log_files_creates_nested_parents() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let nested = tmp.path().join("a").join("b").join("logs");
        let logs_dir = nested.to_str().expect("utf-8 path");
        let exec_id = "test-exec-002";

        let (log_dir, _stdout, _stderr) = prepare_log_files(logs_dir, exec_id, "{}")
            .await
            .expect("prepare_log_files");

        assert!(log_dir.exists());
        assert!(log_dir.join("stdout.log").exists());
        assert!(log_dir.join("stderr.log").exists());
    }

    // --- Integration test helpers ---

    async fn test_pool() -> SqlitePool {
        let db = Db::new_in_memory().await.expect("in-memory db");
        db.migrate().await.expect("migration");
        db.pool().clone()
    }

    /// Create a pending execution record and return a matching ExecutionContext.
    async fn setup_execution(pool: &SqlitePool, logs_dir: &str, command: &str) -> ExecutionContext {
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

        ExecutionContext {
            execution_id: exec.id,
            hook_slug: "test-hook".into(),
            executor: ResolvedExecutor::Shell {
                command: command.into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: Duration::from_secs(10),
            logs_dir: logs_dir.into(),
            payload_json: "{}".into(),
            http_client: None,
        }
    }

    /// Read a log file to a string.
    async fn read_log(logs_dir: &str, exec_id: &str, file: &str) -> String {
        let path = Path::new(logs_dir).join(exec_id).join(file);
        tokio::fs::read_to_string(path).await.unwrap_or_default()
    }

    // --- Integration tests ---

    #[tokio::test]
    async fn successful_command_returns_success() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let ctx = setup_execution(&pool, logs_dir, "echo hello").await;
        let exec_id = ctx.execution_id.clone();

        let result = run(&pool, ctx).await;

        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.exit_code, Some(0));

        // Verify stdout.log
        let stdout = read_log(logs_dir, &exec_id, "stdout.log").await;
        assert_eq!(stdout.trim(), "hello");

        // Verify stderr.log is empty
        let stderr = read_log(logs_dir, &exec_id, "stderr.log").await;
        assert!(stderr.is_empty());

        // Verify DB record
        let exec = execution::get_by_id(&pool, &exec_id).await.expect("get");
        assert_eq!(exec.status, ExecutionStatus::Success);
        assert!(exec.started_at.is_some());
        assert!(exec.completed_at.is_some());
        assert_eq!(exec.exit_code, Some(0));
    }

    #[tokio::test]
    async fn failing_command_returns_failed_with_exit_code() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let ctx = setup_execution(&pool, logs_dir, "exit 42").await;
        let exec_id = ctx.execution_id.clone();

        let result = run(&pool, ctx).await;

        assert_eq!(result.status, ExecutionStatus::Failed);
        assert_eq!(result.exit_code, Some(42));

        let exec = execution::get_by_id(&pool, &exec_id).await.expect("get");
        assert_eq!(exec.status, ExecutionStatus::Failed);
        assert_eq!(exec.exit_code, Some(42));
    }

    #[tokio::test]
    async fn stderr_output_is_captured() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let ctx = setup_execution(&pool, logs_dir, "echo error >&2").await;
        let exec_id = ctx.execution_id.clone();

        let _result = run(&pool, ctx).await;

        let stderr = read_log(logs_dir, &exec_id, "stderr.log").await;
        assert_eq!(stderr.trim(), "error");
    }

    #[tokio::test]
    async fn timeout_kills_long_running_command() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let mut ctx = setup_execution(&pool, logs_dir, "sleep 60").await;
        ctx.timeout = Duration::from_secs(1);
        let exec_id = ctx.execution_id.clone();

        let start = std::time::Instant::now();
        let result = run(&pool, ctx).await;
        let elapsed = start.elapsed();

        assert_eq!(result.status, ExecutionStatus::TimedOut);
        assert!(result.exit_code.is_none());
        assert!(
            elapsed < Duration::from_secs(5),
            "timeout test should complete quickly, took {elapsed:?}"
        );

        let exec = execution::get_by_id(&pool, &exec_id).await.expect("get");
        assert_eq!(exec.status, ExecutionStatus::TimedOut);
    }

    #[tokio::test]
    async fn hook_env_vars_are_passed_to_command() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let mut ctx = setup_execution(&pool, logs_dir, "echo $MY_VAR").await;
        ctx.env.insert("MY_VAR".into(), "hello".into());
        let exec_id = ctx.execution_id.clone();

        let _result = run(&pool, ctx).await;

        let stdout = read_log(logs_dir, &exec_id, "stdout.log").await;
        assert_eq!(stdout.trim(), "hello");
    }

    #[tokio::test]
    async fn sendword_env_vars_are_set() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let ctx = setup_execution(
            &pool,
            logs_dir,
            "echo $SENDWORD_EXECUTION_ID $SENDWORD_HOOK_SLUG",
        )
        .await;
        let exec_id = ctx.execution_id.clone();

        let _result = run(&pool, ctx).await;

        let stdout = read_log(logs_dir, &exec_id, "stdout.log").await;
        let parts: Vec<&str> = stdout.trim().split_whitespace().collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], exec_id);
        assert_eq!(parts[1], "test-hook");
    }

    #[tokio::test]
    async fn working_directory_is_respected() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let work_dir = tempfile::TempDir::new().expect("work dir");
        let work_path = work_dir.path().canonicalize().expect("canonical path");

        let mut ctx = setup_execution(&pool, logs_dir, "pwd").await;
        ctx.cwd = Some(work_path.to_str().expect("utf-8").into());
        let exec_id = ctx.execution_id.clone();

        let _result = run(&pool, ctx).await;

        let stdout = read_log(logs_dir, &exec_id, "stdout.log").await;
        assert_eq!(stdout.trim(), work_path.to_str().expect("utf-8"));
    }

    #[tokio::test]
    async fn invalid_cwd_results_in_failed() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let mut ctx = setup_execution(&pool, logs_dir, "echo should-not-run").await;
        ctx.cwd = Some("/nonexistent/path/that/does/not/exist".into());
        let exec_id = ctx.execution_id.clone();

        let result = run(&pool, ctx).await;

        assert_eq!(result.status, ExecutionStatus::Failed);

        let stderr = read_log(logs_dir, &exec_id, "stderr.log").await;
        assert!(
            !stderr.is_empty(),
            "stderr.log should contain spawn error message"
        );
    }

    #[tokio::test]
    async fn environment_is_clean_no_inherited_server_vars() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        // Safety: test-only env var, unique name avoids collisions
        unsafe { std::env::set_var("SENDWORD_TEST_UNIQUE_VAR_12345", "leaked") };

        let ctx = setup_execution(
            &pool,
            logs_dir,
            "echo ${SENDWORD_TEST_UNIQUE_VAR_12345:-clean}",
        )
        .await;
        let exec_id = ctx.execution_id.clone();

        let _result = run(&pool, ctx).await;

        unsafe { std::env::remove_var("SENDWORD_TEST_UNIQUE_VAR_12345") };

        let stdout = read_log(logs_dir, &exec_id, "stdout.log").await;
        assert_eq!(
            stdout.trim(),
            "clean",
            "server env vars should not leak to child process"
        );
    }

    #[tokio::test]
    async fn prepare_log_files_creates_payload_json() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let exec_id = "test-exec-payload";
        let payload = r#"{"test":true}"#;

        let (log_dir, _stdout, _stderr) = prepare_log_files(logs_dir, exec_id, payload)
            .await
            .expect("prepare_log_files");

        assert!(log_dir.join("payload.json").exists());
        let contents = tokio::fs::read_to_string(log_dir.join("payload.json"))
            .await
            .expect("read payload.json");
        assert_eq!(contents, payload);
    }

    #[tokio::test]
    async fn payload_json_file_is_written_to_log_dir() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let mut ctx = setup_execution(&pool, logs_dir, "echo ok").await;
        ctx.payload_json = r#"{"action":"deploy","count":3}"#.into();
        let exec_id = ctx.execution_id.clone();

        let _result = run(&pool, ctx).await;

        let payload_path = Path::new(logs_dir).join(&exec_id).join("payload.json");
        let contents = tokio::fs::read_to_string(&payload_path)
            .await
            .expect("payload.json should exist");
        assert_eq!(contents, r#"{"action":"deploy","count":3}"#);
    }

    #[tokio::test]
    async fn sendword_payload_env_var_is_set() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let mut ctx = setup_execution(&pool, logs_dir, "echo $SENDWORD_PAYLOAD").await;
        ctx.payload_json = r#"{"key":"value"}"#.into();
        let exec_id = ctx.execution_id.clone();

        let _result = run(&pool, ctx).await;

        let stdout = read_log(logs_dir, &exec_id, "stdout.log").await;
        assert_eq!(stdout.trim(), r#"{"key":"value"}"#);
    }

    #[tokio::test]
    async fn empty_payload_json_written_to_log_dir() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let ctx = setup_execution(&pool, logs_dir, "echo ok").await;
        let exec_id = ctx.execution_id.clone();

        let _result = run(&pool, ctx).await;

        let payload_path = Path::new(logs_dir).join(&exec_id).join("payload.json");
        let contents = tokio::fs::read_to_string(&payload_path)
            .await
            .expect("payload.json should exist even for empty payload");
        assert_eq!(contents, "{}");
    }

    #[tokio::test]
    async fn sendword_payload_env_var_set_for_empty_payload() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        // setup_execution defaults payload_json to "{}"
        let ctx = setup_execution(&pool, logs_dir, "echo $SENDWORD_PAYLOAD").await;
        let exec_id = ctx.execution_id.clone();

        let _result = run(&pool, ctx).await;

        let stdout = read_log(logs_dir, &exec_id, "stdout.log").await;
        assert_eq!(stdout.trim(), "{}");
    }

    #[tokio::test]
    async fn payload_json_overwritten_not_appended_on_retry() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let exec_id = "test-exec-overwrite";

        // Write payload.json twice with different content to simulate retry
        let payload_v1 = r#"{"version":1}"#;
        let payload_v2 = r#"{"version":2}"#;

        let _ = prepare_log_files(logs_dir, exec_id, payload_v1)
            .await
            .expect("first prepare");
        let (log_dir, _, _) = prepare_log_files(logs_dir, exec_id, payload_v2)
            .await
            .expect("second prepare");

        let contents = tokio::fs::read_to_string(log_dir.join("payload.json"))
            .await
            .expect("read payload.json");
        assert_eq!(
            contents, payload_v2,
            "payload.json should be overwritten, not appended"
        );
    }
}
