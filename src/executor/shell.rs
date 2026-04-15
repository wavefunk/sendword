use std::process::Stdio;

use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;

use crate::models::ExecutionStatus;
use crate::models::execution;

use super::{ExecutionContext, ExecutionResult, prepare_log_files, system_env_vars};

/// Run a shell command via `sh -c`.
///
/// Spawns `sh -c <command>`, captures stdout/stderr to log files,
/// enforces a timeout, and updates the execution record in the database
/// through its lifecycle: pending -> running -> success/failed/timed_out.
pub async fn run_shell(
    pool: &SqlitePool,
    ctx: &ExecutionContext,
    command: &str,
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

    // 3. Build the command
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command);
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
