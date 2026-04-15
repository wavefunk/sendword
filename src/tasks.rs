use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use sqlx::SqlitePool;
use tokio::task::JoinHandle;

use crate::barriers::{self, execution_lock, execution_queue};
use crate::config::AppConfig;
use crate::models::execution;
use crate::server::AppState;

/// Threshold for cleaning up stale rate limit counters. 48 hours is safely
/// past any realistic rate limit window a user would configure.
const RATE_LIMIT_COUNTER_TTL_HOURS: i64 = 48;

/// Interval between rate limit counter sweeps.
const RATE_LIMIT_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60); // 1 hour

/// Spawn a background task that expires pending_approval executions that have
/// timed out. Checks every 60 seconds.
pub fn spawn_approval_sweep(pool: SqlitePool, state: Arc<AppState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        // Skip first tick to avoid sweeping immediately on startup
        interval.tick().await;

        loop {
            interval.tick().await;
            expire_pending_approvals(&pool, &state.config, &state).await;
        }
    })
}

async fn expire_pending_approvals(
    pool: &SqlitePool,
    config: &ArcSwap<AppConfig>,
    state: &Arc<AppState>,
) {
    let cfg = config.load();
    let now = chrono::Utc::now();

    for hook in &cfg.hooks {
        let Some(approval) = &hook.approval else {
            continue;
        };
        let Some(timeout) = approval.timeout else {
            continue;
        };

        let cutoff = (now
            - chrono::Duration::from_std(timeout)
                .unwrap_or(chrono::Duration::try_seconds(0).unwrap_or_default()))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

        let rows: Result<Vec<(String,)>, _> = sqlx::query_as(
            "SELECT id FROM executions \
             WHERE hook_slug = ? AND status = 'pending_approval' AND triggered_at < ?",
        )
        .bind(&hook.slug)
        .bind(&cutoff)
        .fetch_all(pool)
        .await;

        if let Ok(rows) = rows {
            for (id,) in rows {
                if let Err(e) = execution::mark_expired(pool, &id).await {
                    tracing::warn!(execution_id = %id, "failed to expire pending approval: {e}");
                    continue;
                }

                tracing::info!(
                    execution_id = %id,
                    hook_slug = %hook.slug,
                    "expired pending approval (timeout)"
                );

                // Expire any queue entry for this execution
                let _ = execution_queue::expire_for_execution(pool, &id).await;

                // If this execution held the lock, hand off or release
                if let Ok(Some(holder)) = execution_lock::get_holder(pool, &hook.slug).await
                    && holder == id
                {
                    barriers::on_execution_complete(
                        state,
                        &hook.slug,
                        hook.concurrency.clone(),
                        hook.approval.clone(),
                    )
                    .await;
                }
            }
        }
    }
}

/// Spawn a background task that periodically deletes stale rate limit counters.
///
/// Session sweeping is handled by allowthem (via its own session management).
/// This task only cleans up the sendword-specific rate_limit_counters table.
pub fn spawn_rate_limit_sweep(pool: SqlitePool) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(RATE_LIMIT_SWEEP_INTERVAL);
        // Skip first tick to avoid sweeping immediately on startup.
        interval.tick().await;

        loop {
            interval.tick().await;
            let cutoff = (chrono::Utc::now()
                - chrono::Duration::hours(RATE_LIMIT_COUNTER_TTL_HOURS))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
            match sqlx::query("DELETE FROM rate_limit_counters WHERE window_start < ?")
                .bind(&cutoff)
                .execute(&pool)
                .await
            {
                Ok(r) if r.rows_affected() > 0 => {
                    tracing::debug!(
                        deleted = r.rows_affected(),
                        "rate limit sweep: cleaned stale counters"
                    );
                }
                Err(e) => {
                    tracing::warn!("rate limit sweep: failed to clean counters: {e}");
                }
                _ => {}
            }
        }
    })
}
