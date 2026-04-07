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
async fn prepare_log_files(
    logs_dir: &str,
    execution_id: &str,
) -> std::io::Result<(PathBuf, fs::File, fs::File)> {
    let log_dir = Path::new(logs_dir).join(execution_id);
    fs::create_dir_all(&log_dir).await?;

    let stdout_file = fs::File::create(log_dir.join("stdout.log")).await?;
    let stderr_file = fs::File::create(log_dir.join("stderr.log")).await?;

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
        match prepare_log_files(&ctx.logs_dir, &ctx.execution_id).await {
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

    #[tokio::test]
    async fn prepare_log_files_creates_directory_and_files() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let exec_id = "test-exec-001";

        let (log_dir, _stdout, _stderr) = prepare_log_files(logs_dir, exec_id)
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
        // logs_dir itself does not exist yet
        let nested = tmp.path().join("a").join("b").join("logs");
        let logs_dir = nested.to_str().expect("utf-8 path");
        let exec_id = "test-exec-002";

        let (log_dir, _stdout, _stderr) = prepare_log_files(logs_dir, exec_id)
            .await
            .expect("prepare_log_files");

        assert!(log_dir.exists());
        assert!(log_dir.join("stdout.log").exists());
        assert!(log_dir.join("stderr.log").exists());
    }
}
