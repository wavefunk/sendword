use std::path::Path;
use std::process::Stdio;

use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;

use crate::models::execution;
use crate::models::ExecutionStatus;

use super::{prepare_log_files, system_env_vars, ExecutionContext, ExecutionResult};

/// Run a script file directly (not via `sh -c`).
///
/// The script is executed with the path as the command. Payload fields are
/// exposed as `SENDWORD_FIELD_<UPPERCASED_PATH>=value` env vars. All other
/// env var behavior (system vars, hook env, SENDWORD_* vars, timeout,
/// stdout/stderr capture) matches the shell executor.
pub async fn run_script(pool: &SqlitePool, ctx: &ExecutionContext, path: &Path) -> ExecutionResult {
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

    // 3. Build the command -- execute the script directly, not via sh -c
    let mut cmd = tokio::process::Command::new(path);
    cmd.env_clear();

    // System env vars first, then hook env vars (hook overrides system)
    let sys_env = system_env_vars();
    cmd.envs(&sys_env);
    cmd.envs(&ctx.env);
    cmd.env("SENDWORD_EXECUTION_ID", &ctx.execution_id);
    cmd.env("SENDWORD_HOOK_SLUG", &ctx.hook_slug);
    cmd.env("SENDWORD_PAYLOAD", &ctx.payload_json);

    // Set payload fields as SENDWORD_FIELD_<UPPERCASED_PATH>=value
    if let Ok(payload_value) = serde_json::from_str::<serde_json::Value>(&ctx.payload_json) {
        for (key, value) in flatten_json_fields(&payload_value) {
            let env_key = format!("SENDWORD_FIELD_{}", key.to_uppercase().replace('.', "_"));
            cmd.env(env_key, value);
        }
    }

    if let Some(cwd) = &ctx.cwd {
        cmd.current_dir(cwd);
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // 4. Spawn the child
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let msg = format!("failed to spawn script: {e}\n");
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

/// Flatten a JSON value into key=value pairs for environment variables.
/// Only leaf values (strings, numbers, booleans) are included.
/// Nested keys use dot notation (e.g., "user.name").
fn flatten_json_fields(value: &serde_json::Value) -> Vec<(String, String)> {
    let mut result = Vec::new();
    flatten_recursive(value, String::new(), &mut result);
    result
}

fn flatten_recursive(value: &serde_json::Value, prefix: String, result: &mut Vec<(String, String)>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                let new_prefix = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_recursive(val, new_prefix, result);
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, val) in arr.iter().enumerate() {
                let new_prefix = if prefix.is_empty() {
                    i.to_string()
                } else {
                    format!("{prefix}.{i}")
                };
                flatten_recursive(val, new_prefix, result);
            }
        }
        serde_json::Value::String(s) => {
            if !prefix.is_empty() {
                result.push((prefix, s.clone()));
            }
        }
        serde_json::Value::Number(n) => {
            if !prefix.is_empty() {
                result.push((prefix, n.to_string()));
            }
        }
        serde_json::Value::Bool(b) => {
            if !prefix.is_empty() {
                result.push((prefix, b.to_string()));
            }
        }
        serde_json::Value::Null => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::models::execution;
    use std::collections::HashMap;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    async fn test_pool() -> sqlx::SqlitePool {
        let db = Db::new_in_memory().await.expect("in-memory db");
        db.migrate().await.expect("migration");
        db.pool().clone()
    }

    async fn setup_execution(
        pool: &sqlx::SqlitePool,
        logs_dir: &str,
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
            executor: crate::executor::ResolvedExecutor::Script {
                path: std::path::PathBuf::from("/tmp/dummy"), // placeholder
            },
            env: HashMap::new(),
            cwd: None,
            timeout: Duration::from_secs(10),
            logs_dir: logs_dir.into(),
            payload_json: "{}".into(),
            http_client: None,
        };
        (ctx, exec_id)
    }

    fn write_script(content: &str) -> (tempfile::TempPath, std::path::PathBuf) {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(content.as_bytes()).expect("write script");
        let path = file.path().to_path_buf();

        // Make it executable
        let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("set permissions");

        // Close the write fd via into_temp_path() to avoid ETXTBSY when executing.
        // TempPath keeps the file alive (deletes on drop) without holding a write handle.
        let temp_path = file.into_temp_path();
        (temp_path, path)
    }

    async fn read_log(logs_dir: &str, exec_id: &str, file: &str) -> String {
        let path = std::path::Path::new(logs_dir).join(exec_id).join(file);
        tokio::fs::read_to_string(path).await.unwrap_or_default()
    }

    #[tokio::test]
    async fn script_executes_and_captures_output() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let (script_file, script_path) = write_script("#!/bin/sh\necho hello_from_script\n");

        let (mut ctx, exec_id) = setup_execution(&pool, logs_dir).await;
        ctx.executor = crate::executor::ResolvedExecutor::Script { path: script_path.clone() };

        let result = run_script(&pool, &ctx, &script_path).await;

        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.exit_code, Some(0));

        let stdout = read_log(logs_dir, &exec_id, "stdout.log").await;
        assert_eq!(stdout.trim(), "hello_from_script");

        drop(script_file);
    }

    #[tokio::test]
    async fn script_passes_payload_env_vars() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let (script_file, script_path) = write_script("#!/bin/sh\necho $SENDWORD_FIELD_ACTION\n");

        let (mut ctx, exec_id) = setup_execution(&pool, logs_dir).await;
        ctx.executor = crate::executor::ResolvedExecutor::Script { path: script_path.clone() };
        ctx.payload_json = r#"{"action":"deploy"}"#.into();

        let result = run_script(&pool, &ctx, &script_path).await;

        assert_eq!(result.status, ExecutionStatus::Success);
        let stdout = read_log(logs_dir, &exec_id, "stdout.log").await;
        assert_eq!(stdout.trim(), "deploy");

        drop(script_file);
    }

    #[tokio::test]
    async fn script_nonexistent_path_fails() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let nonexistent = std::path::Path::new("/tmp/sendword_test_nonexistent_script_xyz.sh");
        let (mut ctx, _exec_id) = setup_execution(&pool, logs_dir).await;
        ctx.executor = crate::executor::ResolvedExecutor::Script { path: nonexistent.to_path_buf() };

        let result = run_script(&pool, &ctx, nonexistent).await;

        assert_eq!(result.status, ExecutionStatus::Failed);
        assert!(result.exit_code.is_none());
    }

    #[tokio::test]
    async fn script_timeout_kills_process() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let (script_file, script_path) = write_script("#!/bin/sh\nsleep 60\n");

        let (mut ctx, _exec_id) = setup_execution(&pool, logs_dir).await;
        ctx.executor = crate::executor::ResolvedExecutor::Script { path: script_path.clone() };
        ctx.timeout = Duration::from_secs(1);

        let start = std::time::Instant::now();
        let result = run_script(&pool, &ctx, &script_path).await;
        let elapsed = start.elapsed();

        assert_eq!(result.status, ExecutionStatus::TimedOut);
        assert!(result.exit_code.is_none());
        assert!(elapsed < Duration::from_secs(5));

        drop(script_file);
    }

    #[tokio::test]
    async fn script_exit_code_captured() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let (script_file, script_path) = write_script("#!/bin/sh\nexit 42\n");

        let (mut ctx, exec_id) = setup_execution(&pool, logs_dir).await;
        ctx.executor = crate::executor::ResolvedExecutor::Script { path: script_path.clone() };

        let result = run_script(&pool, &ctx, &script_path).await;

        assert_eq!(result.status, ExecutionStatus::Failed);
        assert_eq!(result.exit_code, Some(42));

        let exec = execution::get_by_id(&pool, &exec_id).await.expect("get");
        assert_eq!(exec.exit_code, Some(42));

        drop(script_file);
    }

    #[test]
    fn flatten_json_fields_simple() {
        let v: serde_json::Value = serde_json::from_str(r#"{"action":"deploy","count":3}"#).unwrap();
        let fields = flatten_json_fields(&v);
        let map: std::collections::HashMap<_, _> = fields.into_iter().collect();
        assert_eq!(map.get("action").map(|s| s.as_str()), Some("deploy"));
        assert_eq!(map.get("count").map(|s| s.as_str()), Some("3"));
    }

    #[test]
    fn flatten_json_fields_nested() {
        let v: serde_json::Value = serde_json::from_str(r#"{"user":{"name":"alice"}}"#).unwrap();
        let fields = flatten_json_fields(&v);
        let map: std::collections::HashMap<_, _> = fields.into_iter().collect();
        assert_eq!(map.get("user.name").map(|s| s.as_str()), Some("alice"));
    }
}
