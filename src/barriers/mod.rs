pub mod approval;
pub mod concurrency;
pub mod execution_lock;
pub mod execution_queue;

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::config::{ApprovalConfig, ConcurrencyConfig, ConcurrencyMode, ExecutorConfig};
use crate::executor::ResolvedExecutor;
use crate::interpolation::interpolate_command;
use crate::models::trigger_attempt::TriggerAttemptStatus;
use crate::models::{execution, ExecutionStatus};
use crate::server::AppState;

/// Outcome of an execution barrier check.
pub enum BarrierOutcome {
    /// Execution proceeds immediately.
    Proceed,
    /// Request is rejected -- no execution record created.
    Reject {
        status: TriggerAttemptStatus,
        reason: String,
    },
    /// Execution is deferred -- record created but not run yet.
    Defer {
        execution_id: String,
        status: ExecutionStatus,
        reason: String,
    },
}

/// Called when an execution reaches a terminal state. Hands off the lock to the
/// next queued execution (if any) or releases it. In queue mode, handles the
/// dequeue → approval check → spawn sequence atomically to prevent race conditions.
pub async fn on_execution_complete(
    state: &Arc<AppState>,
    hook_slug: &str,
    concurrency: Option<ConcurrencyConfig>,
    approval: Option<ApprovalConfig>,
) {
    let pool = state.db.pool();

    let Some(config) = &concurrency else {
        let _ = execution_lock::release(pool, hook_slug).await;
        return;
    };

    if config.mode != ConcurrencyMode::Queue {
        let _ = execution_lock::release(pool, hook_slug).await;
        return;
    }

    // Peek at the queue without changing status yet
    let next = execution_queue::peek_next(pool, hook_slug).await.ok().flatten();

    match next {
        None => {
            let _ = execution_lock::release(pool, hook_slug).await;
        }
        Some(queued) => {
            // Hand off the lock atomically (UPDATE, not DELETE+INSERT)
            if let Err(e) = execution_lock::hand_off(pool, hook_slug, &queued.execution_id).await {
                tracing::warn!(hook_slug = %hook_slug, "failed to hand off lock: {e}");
                return;
            }

            // Mark the queue entry as ready (no longer waiting)
            let _ = execution_queue::mark_ready(pool, &queued.id).await;

            // Check if approval is needed for the dequeued execution
            if approval::requires_approval(approval.as_ref()) {
                if let Err(e) =
                    execution::mark_pending_approval(pool, &queued.execution_id).await
                {
                    tracing::warn!(
                        execution_id = %queued.execution_id,
                        "failed to transition dequeued execution to pending_approval: {e}"
                    );
                }
                tracing::info!(
                    execution_id = %queued.execution_id,
                    hook_slug = %hook_slug,
                    "dequeued execution is awaiting approval"
                );
                return;
            }

            // Spawn the dequeued execution in a separate task.
            // Passing owned values avoids lifetime/Send issues with the recursive async pattern.
            spawn_dequeued_task(
                Arc::clone(state),
                hook_slug.to_owned(),
                queued.execution_id.clone(),
                concurrency,
                approval,
            );
        }
    }
}

/// Spawn a task that runs a dequeued execution, then calls back into
/// on_execution_complete. This is a regular fn (not async) to avoid type
/// inference cycles with the mutual recursion.
fn spawn_dequeued_task(
    state: Arc<AppState>,
    hook_slug: String,
    execution_id: String,
    concurrency: Option<ConcurrencyConfig>,
    approval: Option<ApprovalConfig>,
) {
    tokio::spawn(run_dequeued(state, hook_slug, execution_id, concurrency, approval));
}

/// Run a dequeued execution to completion, then hand off to the next queue item.
async fn run_dequeued(
    state: Arc<AppState>,
    hook_slug: String,
    execution_id: String,
    concurrency: Option<ConcurrencyConfig>,
    approval: Option<ApprovalConfig>,
) {
    let pool = state.db.pool();

    // Fetch the stored execution record
    let exec = match execution::get_by_id(pool, &execution_id).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(execution_id = %execution_id, "failed to fetch dequeued execution: {e}");
            let _ = execution_lock::release(pool, &hook_slug).await;
            return;
        }
    };

    // Find hook config
    let app_config = state.config.load();
    let hook = match app_config.hooks.iter().find(|h| h.slug == hook_slug) {
        Some(h) => h,
        None => {
            tracing::warn!(
                hook_slug = %hook_slug,
                "hook not found in config after dequeue, releasing lock"
            );
            let _ = execution_lock::release(pool, &hook_slug).await;
            return;
        }
    };

    let timeout = hook.timeout.unwrap_or(app_config.defaults.timeout);
    let resolved_executor = match &hook.executor {
        ExecutorConfig::Shell { command } => {
            let interpolated = if let Ok(payload_value) =
                serde_json::from_str::<serde_json::Value>(&exec.request_payload)
            {
                interpolate_command(command, &payload_value).into_owned()
            } else {
                command.clone()
            };
            ResolvedExecutor::Shell { command: interpolated }
        }
        ExecutorConfig::Script { path } => {
            ResolvedExecutor::Script { path: std::path::PathBuf::from(path) }
        }
        ExecutorConfig::Http { method, url, headers, body, follow_redirects } => {
            let payload_value: serde_json::Value =
                serde_json::from_str(&exec.request_payload)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            let interpolated_url = interpolate_command(url, &payload_value).into_owned();
            let interpolated_body = body.as_deref()
                .map(|b| interpolate_command(b, &payload_value).into_owned());
            ResolvedExecutor::Http {
                method: *method,
                url: interpolated_url,
                headers: headers.clone(),
                body: interpolated_body,
                follow_redirects: *follow_redirects,
            }
        }
    };

    let ctx = crate::executor::ExecutionContext {
        execution_id: exec.id.clone(),
        hook_slug: exec.hook_slug.clone(),
        executor: resolved_executor,
        env: hook.env.clone(),
        cwd: hook.cwd.clone(),
        timeout,
        logs_dir: app_config.logs.dir.clone(),
        payload_json: exec.request_payload.clone(),
        http_client: Some(state.http_client.clone()),
    };

    let retry_config = crate::retry::resolve_retry_config(hook, &app_config.defaults.retries);
    let pool_clone = pool.clone();

    let result = crate::retry::run_with_retries(&pool_clone, ctx, &retry_config).await;
    tracing::info!(
        log_dir = %result.log_dir,
        status = %result.status,
        "dequeued execution completed"
    );

    // After the execution finishes, hand off to the next queue item (or release the lock).
    // Calling on_execution_complete here is safe because run_dequeued is only ever called
    // from within tokio::spawn (via spawn_dequeued_task), so this recursive call simply
    // runs the completion handler in the same task context without spawning again.
    on_execution_complete(&state, &hook_slug, concurrency, approval).await;
}

/// Clean up stale barrier state on server startup. Called after migrations, before
/// accepting requests. Handles the case where the server crashed while executions
/// were in-flight.
pub async fn recover_barriers(pool: &SqlitePool) {
    let now = crate::timestamp::now_utc();

    // 1. Mark stuck running executions as failed
    match sqlx::query(
        "UPDATE executions SET status = 'failed', completed_at = ? WHERE status = 'running'",
    )
    .bind(&now)
    .execute(pool)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!(count = r.rows_affected(), "recovered stuck running executions");
        }
        Err(e) => tracing::warn!("failed to recover running executions: {e}"),
        _ => {}
    }

    // 2. Clean up orphaned locks (lock held by a terminal execution)
    match sqlx::query(
        "DELETE FROM execution_locks WHERE execution_id IN \
         (SELECT id FROM executions WHERE status IN ('success', 'failed', 'timed_out', 'rejected', 'expired'))",
    )
    .execute(pool)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!(count = r.rows_affected(), "cleaned orphaned execution locks");
        }
        Err(e) => tracing::warn!("failed to clean orphaned locks: {e}"),
        _ => {}
    }

    // 3. Expire stale queue entries (waiting entries for terminated executions)
    match sqlx::query(
        "UPDATE execution_queue SET status = 'expired' \
         WHERE status = 'waiting' AND execution_id IN \
         (SELECT id FROM executions WHERE status IN ('rejected', 'expired', 'failed'))",
    )
    .execute(pool)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!(count = r.rows_affected(), "expired stale queue entries");
        }
        Err(e) => tracing::warn!("failed to expire stale queue entries: {e}"),
        _ => {}
    }
}
