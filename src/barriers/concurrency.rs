use sqlx::SqlitePool;

use crate::barriers::{BarrierOutcome, execution_lock, execution_queue};
use crate::config::{ConcurrencyConfig, ConcurrencyMode};
use crate::models::execution::{self, NewExecution};
use crate::models::trigger_attempt::TriggerAttemptStatus;

/// Evaluate concurrency barriers for a hook.
///
/// `exec_id`: pre-generated execution ID.
/// `new_exec`: template for creating the execution record in the queue/defer case.
pub async fn evaluate(
    pool: &SqlitePool,
    hook_slug: &str,
    exec_id: &str,
    config: &ConcurrencyConfig,
    new_exec: &NewExecution<'_>,
) -> BarrierOutcome {
    let acquired = match execution_lock::try_acquire(pool, hook_slug, exec_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(hook_slug = %hook_slug, "failed to acquire execution lock: {e}");
            return BarrierOutcome::Reject {
                status: TriggerAttemptStatus::ConcurrencyRejected,
                reason: "internal error acquiring lock".to_owned(),
            };
        }
    };

    if acquired {
        return BarrierOutcome::Proceed;
    }

    // Lock is held -- handle based on mode
    match config.mode {
        ConcurrencyMode::Mutex => BarrierOutcome::Reject {
            status: TriggerAttemptStatus::ConcurrencyRejected,
            reason: "another execution is in progress".to_owned(),
        },
        ConcurrencyMode::Queue => {
            let count = match execution_queue::count_waiting(pool, hook_slug).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(hook_slug = %hook_slug, "failed to count queue: {e}");
                    return BarrierOutcome::Reject {
                        status: TriggerAttemptStatus::ConcurrencyRejected,
                        reason: "internal error checking queue".to_owned(),
                    };
                }
            };

            if count >= config.queue_depth as i64 {
                return BarrierOutcome::Reject {
                    status: TriggerAttemptStatus::ConcurrencyRejected,
                    reason: format!("queue full ({count}/{})", config.queue_depth),
                };
            }

            // Create the execution record before enqueueing (queue FK requires it)
            if let Err(e) = execution::create(pool, new_exec).await {
                tracing::error!(hook_slug = %hook_slug, "failed to create execution for queue: {e}");
                return BarrierOutcome::Reject {
                    status: TriggerAttemptStatus::ConcurrencyRejected,
                    reason: "internal error creating queued execution".to_owned(),
                };
            }

            let position = match execution_queue::enqueue(pool, hook_slug, exec_id).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!(hook_slug = %hook_slug, "failed to enqueue execution: {e}");
                    return BarrierOutcome::Reject {
                        status: TriggerAttemptStatus::ConcurrencyRejected,
                        reason: "internal error enqueueing execution".to_owned(),
                    };
                }
            };

            BarrierOutcome::Defer {
                execution_id: exec_id.to_owned(),
                status: crate::models::ExecutionStatus::Pending,
                reason: format!("queued at position {position}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::models::execution::NewExecution;

    async fn test_pool() -> SqlitePool {
        let db = Db::new_in_memory().await.unwrap();
        db.migrate().await.unwrap();
        db.pool().clone()
    }

    fn mutex_config() -> ConcurrencyConfig {
        ConcurrencyConfig {
            mode: ConcurrencyMode::Mutex,
            queue_depth: 10,
        }
    }

    fn queue_config(depth: u32) -> ConcurrencyConfig {
        ConcurrencyConfig {
            mode: ConcurrencyMode::Queue,
            queue_depth: depth,
        }
    }

    fn new_exec_params<'a>(id: &'a str, hook_slug: &'a str) -> NewExecution<'a> {
        NewExecution {
            id: Some(id),
            hook_slug,
            log_path: "data/logs/test",
            trigger_source: "127.0.0.1",
            request_payload: "{}",
            retry_of: None,
            status: None,
        }
    }

    #[tokio::test]
    async fn mutex_proceeds_when_no_lock() {
        let pool = test_pool().await;
        let outcome = evaluate(
            &pool,
            "hook-a",
            "exec-1",
            &mutex_config(),
            &new_exec_params("exec-1", "hook-a"),
        )
        .await;
        assert!(matches!(outcome, BarrierOutcome::Proceed));
    }

    #[tokio::test]
    async fn mutex_rejects_when_lock_held() {
        let pool = test_pool().await;
        execution_lock::try_acquire(&pool, "hook-a", "exec-1")
            .await
            .unwrap();

        let outcome = evaluate(
            &pool,
            "hook-a",
            "exec-2",
            &mutex_config(),
            &new_exec_params("exec-2", "hook-a"),
        )
        .await;
        assert!(matches!(
            outcome,
            BarrierOutcome::Reject {
                status: TriggerAttemptStatus::ConcurrencyRejected,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn queue_proceeds_when_no_lock() {
        let pool = test_pool().await;
        let outcome = evaluate(
            &pool,
            "hook-a",
            "exec-1",
            &queue_config(5),
            &new_exec_params("exec-1", "hook-a"),
        )
        .await;
        assert!(matches!(outcome, BarrierOutcome::Proceed));
    }

    #[tokio::test]
    async fn queue_defers_when_lock_held() {
        let pool = test_pool().await;
        execution_lock::try_acquire(&pool, "hook-a", "exec-1")
            .await
            .unwrap();

        let exec = new_exec_params("exec-2", "hook-a");
        let outcome = evaluate(&pool, "hook-a", "exec-2", &queue_config(5), &exec).await;
        match outcome {
            BarrierOutcome::Defer {
                execution_id,
                reason,
                ..
            } => {
                assert_eq!(execution_id, "exec-2");
                assert!(reason.contains("position"));
            }
            other => {
                let tag = std::mem::discriminant(&other);
                panic!("expected Defer, got {tag:?}");
            }
        }
    }

    #[tokio::test]
    async fn queue_rejects_when_full() {
        let pool = test_pool().await;
        execution_lock::try_acquire(&pool, "hook-a", "exec-1")
            .await
            .unwrap();

        // Fill the queue (depth=2)
        evaluate(
            &pool,
            "hook-a",
            "exec-2",
            &queue_config(2),
            &new_exec_params("exec-2", "hook-a"),
        )
        .await;
        evaluate(
            &pool,
            "hook-a",
            "exec-3",
            &queue_config(2),
            &new_exec_params("exec-3", "hook-a"),
        )
        .await;

        // Third should be rejected (queue full at 2)
        let outcome = evaluate(
            &pool,
            "hook-a",
            "exec-4",
            &queue_config(2),
            &new_exec_params("exec-4", "hook-a"),
        )
        .await;
        assert!(matches!(
            outcome,
            BarrierOutcome::Reject {
                status: TriggerAttemptStatus::ConcurrencyRejected,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn different_hooks_independent_locks() {
        let pool = test_pool().await;
        execution_lock::try_acquire(&pool, "hook-a", "exec-1")
            .await
            .unwrap();

        let outcome = evaluate(
            &pool,
            "hook-b",
            "exec-2",
            &mutex_config(),
            &new_exec_params("exec-2", "hook-b"),
        )
        .await;
        assert!(matches!(outcome, BarrierOutcome::Proceed));
    }
}
