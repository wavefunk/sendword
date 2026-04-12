use sqlx::SqlitePool;
use std::time::Duration;
use tokio::task::JoinHandle;

use crate::models::session;

/// Threshold for cleaning up stale rate limit counters. 48 hours is safely
/// past any realistic rate limit window a user would configure.
const RATE_LIMIT_COUNTER_TTL_HOURS: i64 = 48;

/// Interval between expired session sweeps.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60); // 1 hour

/// Spawn a background task that periodically deletes expired sessions.
/// Returns the JoinHandle so the caller can abort it on shutdown if needed.
pub fn spawn_session_sweep(pool: SqlitePool) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        // The first tick completes immediately; skip it so we don't
        // sweep on startup (sessions are filtered by expires_at on lookup anyway).
        interval.tick().await;

        loop {
            interval.tick().await;
            match session::delete_expired(&pool).await {
                Ok(0) => {
                    tracing::debug!("session sweep: no expired sessions");
                }
                Ok(count) => {
                    tracing::info!(count, "session sweep: deleted expired sessions");
                }
                Err(err) => {
                    tracing::error!(error = %err, "session sweep: failed to delete expired sessions");
                }
            }

            // Clean up stale rate limit counters (older than 48 hours).
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
                        "session sweep: cleaned stale rate limit counters"
                    );
                }
                Err(e) => tracing::warn!("session sweep: failed to clean rate limit counters: {e}"),
                _ => {}
            }
        }
    })
}
