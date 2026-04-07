use std::time::Duration;

use sqlx::SqlitePool;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

use crate::config::{BackoffStrategy, HookConfig, RetryConfig};
use crate::executor::{self, ExecutionContext, ExecutionResult};
use crate::models::execution;
use crate::models::ExecutionStatus;

/// Resolved retry configuration for a single execution.
/// Uses hook-level overrides when present, otherwise falls back to global defaults.
#[derive(Debug, Clone)]
pub struct EffectiveRetryConfig {
    pub count: u32,
    pub backoff: BackoffStrategy,
    pub initial_delay: Duration,
    pub max_delay: Duration,
}

/// Resolve the effective retry config for a hook.
/// Hook-level retries override global defaults entirely (not field-by-field).
pub fn resolve_retry_config(
    hook: &HookConfig,
    global: &RetryConfig,
) -> EffectiveRetryConfig {
    let retry = hook.retries.as_ref().unwrap_or(global);
    EffectiveRetryConfig {
        count: retry.count,
        backoff: retry.backoff,
        initial_delay: retry.initial_delay,
        max_delay: retry.max_delay,
    }
}

/// Calculate the delay before the given retry attempt (1-indexed).
///
/// - `None` / `BackoffStrategy::None`: no delay (returns Duration::ZERO)
/// - `Linear`: initial_delay * attempt, capped at max_delay
/// - `Exponential`: initial_delay * 2^(attempt-1), capped at max_delay
pub fn calculate_backoff(
    strategy: BackoffStrategy,
    attempt: u32,
    initial_delay: Duration,
    max_delay: Duration,
) -> Duration {
    let delay = match strategy {
        BackoffStrategy::None => Duration::ZERO,
        BackoffStrategy::Linear => initial_delay.saturating_mul(attempt),
        BackoffStrategy::Exponential => {
            // 2^(attempt-1), with saturating exponent to avoid overflow
            let exp = attempt.saturating_sub(1).min(31);
            let multiplier = 1u32.checked_shl(exp).unwrap_or(u32::MAX);
            initial_delay.saturating_mul(multiplier)
        }
    };
    delay.min(max_delay)
}

/// Append a retry attempt marker to both stdout.log and stderr.log.
async fn append_retry_marker(logs_dir: &str, execution_id: &str, attempt: u32) {
    let marker = format!("\n--- RETRY ATTEMPT {attempt} ---\n");
    let log_dir = std::path::Path::new(logs_dir).join(execution_id);

    for filename in &["stdout.log", "stderr.log"] {
        let path = log_dir.join(filename);
        if let Ok(mut file) = OpenOptions::new().append(true).open(&path).await {
            let _ = file.write_all(marker.as_bytes()).await;
        }
    }
}

/// Run a command with automatic retries on non-zero exit codes.
///
/// On the first attempt, delegates directly to `executor::run`. If the command
/// fails with a non-zero exit code and retries remain, waits the calculated
/// backoff duration, appends a retry marker to the log files, increments the
/// retry count in the database, resets the execution record to pending, and
/// runs the command again.
///
/// Returns the result of the final attempt.
pub async fn run_with_retries(
    pool: &SqlitePool,
    ctx: ExecutionContext,
    retry_config: &EffectiveRetryConfig,
) -> ExecutionResult {
    // First attempt
    let mut result = executor::run(pool, ctx.clone()).await;

    for attempt in 1..=retry_config.count {
        // Only retry on failed with a non-zero exit code.
        // Don't retry on: success, timed_out, or spawn failures (exit_code == None).
        let should_retry = result.status == ExecutionStatus::Failed
            && result.exit_code.is_some();

        if !should_retry {
            break;
        }

        // Calculate and apply backoff delay
        let delay = calculate_backoff(
            retry_config.backoff,
            attempt,
            retry_config.initial_delay,
            retry_config.max_delay,
        );
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        tracing::info!(
            execution_id = %ctx.execution_id,
            attempt = attempt,
            max_retries = retry_config.count,
            delay_ms = delay.as_millis() as u64,
            "retrying execution"
        );

        // Append retry marker to existing log files
        append_retry_marker(&ctx.logs_dir, &ctx.execution_id, attempt).await;

        // Increment retry count in the database
        if let Err(e) = execution::increment_retry_count(pool, &ctx.execution_id).await {
            tracing::error!(
                execution_id = %ctx.execution_id,
                "failed to increment retry count: {e}"
            );
        }

        // Reset execution to pending so executor::run can transition it
        if let Err(e) = reset_to_pending(pool, &ctx.execution_id).await {
            tracing::error!(
                execution_id = %ctx.execution_id,
                "failed to reset execution to pending: {e}"
            );
            break;
        }

        // Retry the execution
        result = executor::run(pool, ctx.clone()).await;
    }

    result
}

/// Reset an execution record back to pending status so it can be re-run.
async fn reset_to_pending(pool: &SqlitePool, id: &str) -> crate::error::DbResult<()> {
    let result = sqlx::query(
        "UPDATE executions SET status = 'pending', started_at = NULL, completed_at = NULL, exit_code = NULL WHERE id = ?",
    )
    .bind(id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(crate::error::DbError::NotFound(format!(
            "execution {id}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackoffStrategy, HookConfig, ExecutorConfig, RetryConfig};
    use std::collections::HashMap;

    // --- Backoff calculation tests ---

    #[test]
    fn backoff_none_returns_zero() {
        let delay = calculate_backoff(
            BackoffStrategy::None,
            1,
            Duration::from_secs(1),
            Duration::from_secs(60),
        );
        assert_eq!(delay, Duration::ZERO);
    }

    #[test]
    fn backoff_none_returns_zero_for_any_attempt() {
        for attempt in 1..=10 {
            let delay = calculate_backoff(
                BackoffStrategy::None,
                attempt,
                Duration::from_secs(5),
                Duration::from_secs(300),
            );
            assert_eq!(delay, Duration::ZERO);
        }
    }

    #[test]
    fn backoff_linear_scales_with_attempt() {
        let initial = Duration::from_secs(2);
        let max = Duration::from_secs(60);

        assert_eq!(
            calculate_backoff(BackoffStrategy::Linear, 1, initial, max),
            Duration::from_secs(2)
        );
        assert_eq!(
            calculate_backoff(BackoffStrategy::Linear, 2, initial, max),
            Duration::from_secs(4)
        );
        assert_eq!(
            calculate_backoff(BackoffStrategy::Linear, 3, initial, max),
            Duration::from_secs(6)
        );
        assert_eq!(
            calculate_backoff(BackoffStrategy::Linear, 5, initial, max),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn backoff_linear_caps_at_max_delay() {
        let initial = Duration::from_secs(10);
        let max = Duration::from_secs(30);

        assert_eq!(
            calculate_backoff(BackoffStrategy::Linear, 3, initial, max),
            Duration::from_secs(30)
        );
        assert_eq!(
            calculate_backoff(BackoffStrategy::Linear, 5, initial, max),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn backoff_exponential_doubles_each_attempt() {
        let initial = Duration::from_secs(1);
        let max = Duration::from_secs(300);

        // attempt 1: 1 * 2^0 = 1s
        assert_eq!(
            calculate_backoff(BackoffStrategy::Exponential, 1, initial, max),
            Duration::from_secs(1)
        );
        // attempt 2: 1 * 2^1 = 2s
        assert_eq!(
            calculate_backoff(BackoffStrategy::Exponential, 2, initial, max),
            Duration::from_secs(2)
        );
        // attempt 3: 1 * 2^2 = 4s
        assert_eq!(
            calculate_backoff(BackoffStrategy::Exponential, 3, initial, max),
            Duration::from_secs(4)
        );
        // attempt 4: 1 * 2^3 = 8s
        assert_eq!(
            calculate_backoff(BackoffStrategy::Exponential, 4, initial, max),
            Duration::from_secs(8)
        );
    }

    #[test]
    fn backoff_exponential_caps_at_max_delay() {
        let initial = Duration::from_secs(1);
        let max = Duration::from_secs(10);

        // attempt 5: 1 * 2^4 = 16s, capped to 10s
        assert_eq!(
            calculate_backoff(BackoffStrategy::Exponential, 5, initial, max),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn backoff_exponential_handles_large_attempt_without_overflow() {
        let initial = Duration::from_secs(1);
        let max = Duration::from_secs(60);

        // Very large attempt number should not panic, just cap at max_delay
        let delay = calculate_backoff(BackoffStrategy::Exponential, 100, initial, max);
        assert_eq!(delay, max);
    }

    #[test]
    fn backoff_linear_handles_large_attempt_without_overflow() {
        let initial = Duration::from_secs(1);
        let max = Duration::from_secs(60);

        let delay = calculate_backoff(BackoffStrategy::Linear, u32::MAX, initial, max);
        assert_eq!(delay, max);
    }

    #[test]
    fn backoff_with_sub_second_initial_delay() {
        let initial = Duration::from_millis(500);
        let max = Duration::from_secs(10);

        assert_eq!(
            calculate_backoff(BackoffStrategy::Exponential, 1, initial, max),
            Duration::from_millis(500)
        );
        assert_eq!(
            calculate_backoff(BackoffStrategy::Exponential, 2, initial, max),
            Duration::from_millis(1000)
        );
        assert_eq!(
            calculate_backoff(BackoffStrategy::Linear, 3, initial, max),
            Duration::from_millis(1500)
        );
    }

    // --- Config resolution tests ---

    fn make_hook(retries: Option<RetryConfig>) -> HookConfig {
        HookConfig {
            name: "test".into(),
            slug: "test".into(),
            description: String::new(),
            enabled: true,
            auth: None,
            executor: ExecutorConfig::Shell {
                command: "echo ok".into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries,
            rate_limit: None,
            payload: None,
        }
    }

    #[test]
    fn resolve_uses_global_defaults_when_hook_has_no_retries() {
        let hook = make_hook(None);
        let global = RetryConfig {
            count: 3,
            backoff: BackoffStrategy::Linear,
            initial_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(30),
        };

        let effective = resolve_retry_config(&hook, &global);
        assert_eq!(effective.count, 3);
        assert_eq!(effective.backoff, BackoffStrategy::Linear);
        assert_eq!(effective.initial_delay, Duration::from_secs(2));
        assert_eq!(effective.max_delay, Duration::from_secs(30));
    }

    #[test]
    fn resolve_uses_hook_overrides_when_present() {
        let hook = make_hook(Some(RetryConfig {
            count: 5,
            backoff: BackoffStrategy::Exponential,
            initial_delay: Duration::from_secs(10),
            max_delay: Duration::from_secs(120),
        }));
        let global = RetryConfig::default();

        let effective = resolve_retry_config(&hook, &global);
        assert_eq!(effective.count, 5);
        assert_eq!(effective.backoff, BackoffStrategy::Exponential);
        assert_eq!(effective.initial_delay, Duration::from_secs(10));
        assert_eq!(effective.max_delay, Duration::from_secs(120));
    }

    #[test]
    fn resolve_with_zero_count_means_no_retries() {
        let hook = make_hook(Some(RetryConfig {
            count: 0,
            backoff: BackoffStrategy::Exponential,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
        }));
        let global = RetryConfig {
            count: 3,
            ..RetryConfig::default()
        };

        let effective = resolve_retry_config(&hook, &global);
        assert_eq!(effective.count, 0);
    }

    // --- Integration tests (retry loop with real executor) ---

    use crate::db::Db;

    async fn test_pool() -> SqlitePool {
        let db = Db::new_in_memory().await.expect("in-memory db");
        db.migrate().await.expect("migration");
        db.pool().clone()
    }

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
        }
    }

    #[tokio::test]
    async fn no_retries_when_count_is_zero() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let ctx = setup_execution(&pool, logs_dir, "exit 1").await;
        let exec_id = ctx.execution_id.clone();

        let config = EffectiveRetryConfig {
            count: 0,
            backoff: BackoffStrategy::None,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        };

        let result = run_with_retries(&pool, ctx, &config).await;

        assert_eq!(result.status, ExecutionStatus::Failed);
        assert_eq!(result.exit_code, Some(1));

        let exec = execution::get_by_id(&pool, &exec_id).await.expect("get");
        assert_eq!(exec.retry_count, 0);
    }

    #[tokio::test]
    async fn successful_command_not_retried() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let ctx = setup_execution(&pool, logs_dir, "echo hello").await;
        let exec_id = ctx.execution_id.clone();

        let config = EffectiveRetryConfig {
            count: 3,
            backoff: BackoffStrategy::None,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        };

        let result = run_with_retries(&pool, ctx, &config).await;

        assert_eq!(result.status, ExecutionStatus::Success);

        let exec = execution::get_by_id(&pool, &exec_id).await.expect("get");
        assert_eq!(exec.retry_count, 0);
    }

    #[tokio::test]
    async fn retries_on_non_zero_exit_code() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        // Command always fails
        let ctx = setup_execution(&pool, logs_dir, "exit 1").await;
        let exec_id = ctx.execution_id.clone();

        let config = EffectiveRetryConfig {
            count: 2,
            backoff: BackoffStrategy::None,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        };

        let result = run_with_retries(&pool, ctx, &config).await;

        assert_eq!(result.status, ExecutionStatus::Failed);
        assert_eq!(result.exit_code, Some(1));

        let exec = execution::get_by_id(&pool, &exec_id).await.expect("get");
        assert_eq!(exec.retry_count, 2);
    }

    #[tokio::test]
    async fn retry_markers_appended_to_log_files() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let ctx = setup_execution(&pool, logs_dir, "echo attempt >&2; exit 1").await;
        let exec_id = ctx.execution_id.clone();

        let config = EffectiveRetryConfig {
            count: 2,
            backoff: BackoffStrategy::None,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        };

        let _result = run_with_retries(&pool, ctx, &config).await;

        let stderr_path = std::path::Path::new(logs_dir)
            .join(&exec_id)
            .join("stderr.log");
        let stderr = tokio::fs::read_to_string(&stderr_path)
            .await
            .unwrap_or_default();

        assert!(
            stderr.contains("--- RETRY ATTEMPT 1 ---"),
            "stderr should contain retry marker 1, got: {stderr}"
        );
        assert!(
            stderr.contains("--- RETRY ATTEMPT 2 ---"),
            "stderr should contain retry marker 2, got: {stderr}"
        );
    }

    #[tokio::test]
    async fn timed_out_command_not_retried() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let pool = test_pool().await;

        let mut ctx = setup_execution(&pool, logs_dir, "sleep 60").await;
        ctx.timeout = Duration::from_secs(1);
        let exec_id = ctx.execution_id.clone();

        let config = EffectiveRetryConfig {
            count: 3,
            backoff: BackoffStrategy::None,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        };

        let result = run_with_retries(&pool, ctx, &config).await;

        assert_eq!(result.status, ExecutionStatus::TimedOut);

        let exec = execution::get_by_id(&pool, &exec_id).await.expect("get");
        assert_eq!(exec.retry_count, 0, "timed out commands should not be retried");
    }
}
