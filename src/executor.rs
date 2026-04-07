use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::models::execution;
use crate::models::ExecutionStatus;

/// Everything the executor needs to run a command.
#[derive(Clone)]
pub struct ExecutionContext {
    /// The execution record ID (UUIDv7 string).
    pub execution_id: String,
    /// The hook slug, passed as SENDWORD_HOOK_SLUG env var.
    pub hook_slug: String,
    /// Shell command to run via `sh -c`.
    pub command: String,
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
async fn prepare_log_files(
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
fn system_env_vars() -> HashMap<String, String> {
    const INHERIT_VARS: &[&str] = &["PATH", "HOME", "USER", "LANG"];

    let mut vars = HashMap::with_capacity(INHERIT_VARS.len());
    for &name in INHERIT_VARS {
        if let Ok(val) = std::env::var(name) {
            vars.insert(name.into(), val);
        }
    }
    vars
}

/// Run a shell command with the given execution context.
///
/// Spawns `sh -c <command>`, captures stdout/stderr to log files,
/// enforces a timeout, and updates the execution record in the database
/// through its lifecycle: pending -> running -> success/failed/timed_out.
pub async fn run(pool: &SqlitePool, ctx: ExecutionContext) -> ExecutionResult {
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

    // 3. Build the command
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(&ctx.command);
    cmd.env_clear();

    // System env vars first, then hook env vars (hook overrides system)
    let sys_env = system_env_vars();
    cmd.envs(&sys_env);
    cmd.envs(&ctx.env);
    cmd.env("SENDWORD_EXECUTION_ID", &ctx.execution_id);
    cmd.env("SENDWORD_HOOK_SLUG", &ctx.hook_slug);
    cmd.env("SENDWORD_PAYLOAD", &ctx.payload_json);

    if let Some(cwd) = &ctx.cwd {
        cmd.current_dir(cwd);
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // 4. Spawn the child
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let msg = format!("failed to spawn command: {e}\n");
            let _ = stderr_file.write_all(msg.as_bytes()).await;
            let _ = execution::mark_completed(
                pool,
                &ctx.execution_id,
                ExecutionStatus::Failed,
                None,
            )
            .await;
            return ExecutionResult {
                status: ExecutionStatus::Failed,
                exit_code: None,
                log_dir: log_dir_display,
            };
        }
    };

    // 5. Stream output to files
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_copy = async {
        if let Some(mut pipe) = stdout_pipe {
            tokio::io::copy(&mut pipe, &mut stdout_file).await
        } else {
            Ok(0)
        }
    };

    let stderr_copy = async {
        if let Some(mut pipe) = stderr_pipe {
            tokio::io::copy(&mut pipe, &mut stderr_file).await
        } else {
            Ok(0)
        }
    };

    // 6. Wait with timeout
    let exec_id = ctx.execution_id.clone();
    let work = async {
        let (wait_result, stdout_result, stderr_result) =
            tokio::join!(child.wait(), stdout_copy, stderr_copy);

        if let Err(e) = stdout_result {
            tracing::warn!(execution_id = %exec_id, "stdout copy error: {e}");
        }
        if let Err(e) = stderr_result {
            tracing::warn!(execution_id = %exec_id, "stderr copy error: {e}");
        }

        wait_result
    };

    let outcome = tokio::time::timeout(ctx.timeout, work).await;

    // 7. Determine result
    let (status, exit_code) = match outcome {
        Ok(Ok(exit_status)) => {
            if exit_status.success() {
                (ExecutionStatus::Success, exit_status.code())
            } else {
                (ExecutionStatus::Failed, exit_status.code())
            }
        }
        Ok(Err(e)) => {
            tracing::error!(
                execution_id = %ctx.execution_id,
                "child wait failed: {e}"
            );
            (ExecutionStatus::Failed, None)
        }
        Err(_elapsed) => {
            // Timeout expired -- kill the child
            let _ = child.kill().await;
            (ExecutionStatus::TimedOut, None)
        }
    };

    // 8. Mark completed in DB
    if let Err(e) =
        execution::mark_completed(pool, &ctx.execution_id, status.clone(), exit_code).await
    {
        tracing::error!(
            execution_id = %ctx.execution_id,
            "failed to mark execution as completed: {e}"
        );
    }

    // 9. Return result
    ExecutionResult {
        status,
        exit_code,
        log_dir: log_dir_display,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
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
    async fn setup_execution(
        pool: &SqlitePool,
        logs_dir: &str,
        command: &str,
    ) -> ExecutionContext {
        let exec = execution::create(
            pool,
            &execution::NewExecution {
                id: None,
                hook_slug: "test-hook",
                log_path: logs_dir,
                trigger_source: "127.0.0.1",
                request_payload: "{}",
                retry_of: None,
            },
        )
        .await
        .expect("create execution");

        ExecutionContext {
            execution_id: exec.id,
            hook_slug: "test-hook".into(),
            command: command.into(),
            env: HashMap::new(),
            cwd: None,
            timeout: Duration::from_secs(10),
            logs_dir: logs_dir.into(),
            payload_json: "{}".into(),
        }
    }

    /// Read a log file to a string.
    async fn read_log(logs_dir: &str, exec_id: &str, file: &str) -> String {
        let path = Path::new(logs_dir).join(exec_id).join(file);
        tokio::fs::read_to_string(path)
            .await
            .unwrap_or_default()
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
        assert_eq!(
            stdout.trim(),
            work_path.to_str().expect("utf-8")
        );
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
}
