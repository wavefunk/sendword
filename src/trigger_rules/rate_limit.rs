use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::config::TriggerRateLimit;
use crate::models::trigger_attempt::TriggerAttemptStatus;

use super::EvalOutcome;

pub async fn evaluate(
    pool: &SqlitePool,
    hook_slug: &str,
    config: &TriggerRateLimit,
) -> EvalOutcome {
    evaluate_at(pool, hook_slug, config, Utc::now()).await
}

pub async fn evaluate_at(
    pool: &SqlitePool,
    hook_slug: &str,
    config: &TriggerRateLimit,
    now: DateTime<Utc>,
) -> EvalOutcome {
    let window_secs = config.window.as_secs().max(1);
    let now_secs = now.timestamp() as u64;
    let window_start_secs = (now_secs / window_secs) * window_secs;
    let window_start = DateTime::from_timestamp(window_start_secs as i64, 0)
        .unwrap_or(now)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    // Ensure the counter row exists.
    let _ = sqlx::query(
        "INSERT OR IGNORE INTO rate_limit_counters (hook_slug, window_start, count) \
         VALUES (?, ?, 0)",
    )
    .bind(hook_slug)
    .bind(&window_start)
    .execute(pool)
    .await;

    // Atomically increment only if under limit.
    let result = sqlx::query(
        "UPDATE rate_limit_counters SET count = count + 1 \
         WHERE hook_slug = ? AND window_start = ? AND count < ?",
    )
    .bind(hook_slug)
    .bind(&window_start)
    .bind(config.max_requests as i64)
    .execute(pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 1 => EvalOutcome::Allow,
        Ok(_) => {
            let window_human = humantime::format_duration(config.window).to_string();
            EvalOutcome::Reject {
                status: TriggerAttemptStatus::RateLimited,
                reason: format!(
                    "rate limit exceeded ({} per {})",
                    config.max_requests, window_human
                ),
            }
        }
        Err(e) => {
            tracing::warn!("rate limit counter update failed: {e}");
            // Fail open: allow the request if the DB is broken.
            EvalOutcome::Allow
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TriggerRateLimit;
    use crate::db::Db;
    use std::time::Duration;

    async fn test_pool() -> SqlitePool {
        let db = Db::new_in_memory().await.expect("in-memory db");
        db.migrate().await.expect("migration");
        db.pool().clone()
    }

    fn config(max_requests: u64, window_secs: u64) -> TriggerRateLimit {
        TriggerRateLimit {
            max_requests,
            window: Duration::from_secs(window_secs),
        }
    }

    fn fixed_now() -> DateTime<Utc> {
        // 2026-04-12T14:30:00Z — well within an hour window
        DateTime::from_timestamp(1744464600, 0).unwrap()
    }

    #[tokio::test]
    async fn under_limit_allows() {
        let pool = test_pool().await;
        let cfg = config(5, 60);
        let result = evaluate_at(&pool, "deploy", &cfg, fixed_now()).await;
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[tokio::test]
    async fn at_limit_rejects() {
        let pool = test_pool().await;
        let cfg = config(2, 60);
        let now = fixed_now();

        let r1 = evaluate_at(&pool, "deploy", &cfg, now).await;
        let r2 = evaluate_at(&pool, "deploy", &cfg, now).await;
        let r3 = evaluate_at(&pool, "deploy", &cfg, now).await;

        assert!(matches!(r1, EvalOutcome::Allow));
        assert!(matches!(r2, EvalOutcome::Allow));
        assert!(matches!(r3, EvalOutcome::Reject { .. }));
    }

    #[tokio::test]
    async fn different_windows_independent() {
        let pool = test_pool().await;
        let cfg = config(1, 3600); // 1 per hour

        let hour1 = DateTime::from_timestamp(1744464600, 0).unwrap(); // 14:30 UTC
        let hour2 = DateTime::from_timestamp(1744468200, 0).unwrap(); // 15:30 UTC

        let r1 = evaluate_at(&pool, "deploy", &cfg, hour1).await;
        let r2 = evaluate_at(&pool, "deploy", &cfg, hour2).await;

        assert!(matches!(r1, EvalOutcome::Allow));
        assert!(matches!(r2, EvalOutcome::Allow));
    }

    #[tokio::test]
    async fn different_hooks_independent() {
        let pool = test_pool().await;
        let cfg = config(1, 60);
        let now = fixed_now();

        let r1 = evaluate_at(&pool, "hook-a", &cfg, now).await;
        let r2 = evaluate_at(&pool, "hook-b", &cfg, now).await;

        assert!(matches!(r1, EvalOutcome::Allow));
        assert!(matches!(r2, EvalOutcome::Allow));
    }

    #[tokio::test]
    async fn concurrent_requests_safe() {
        let pool = test_pool().await;
        let cfg = config(1, 60);
        let now = fixed_now();

        // Two concurrent requests for the last slot — exactly one must succeed.
        let pool1 = pool.clone();
        let pool2 = pool.clone();
        let cfg1 = cfg.clone();
        let cfg2 = cfg.clone();

        let (r1, r2) = tokio::join!(
            evaluate_at(&pool1, "deploy", &cfg1, now),
            evaluate_at(&pool2, "deploy", &cfg2, now),
        );

        let allows = [&r1, &r2]
            .iter()
            .filter(|r| matches!(r, EvalOutcome::Allow))
            .count();
        let rejects = [&r1, &r2]
            .iter()
            .filter(|r| matches!(r, EvalOutcome::Reject { .. }))
            .count();

        assert_eq!(allows, 1, "exactly one request should be allowed");
        assert_eq!(rejects, 1, "exactly one request should be rejected");
    }
}
