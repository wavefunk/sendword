use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::models::execution;
use crate::models::trigger_attempt::TriggerAttemptStatus;

use super::EvalOutcome;

pub async fn evaluate(pool: &SqlitePool, hook_slug: &str, cooldown: Duration) -> EvalOutcome {
    evaluate_at(pool, hook_slug, cooldown, Utc::now()).await
}

pub async fn evaluate_at(
    pool: &SqlitePool,
    hook_slug: &str,
    cooldown: Duration,
    now: DateTime<Utc>,
) -> EvalOutcome {
    let latest = match execution::get_latest_started_by_hook(pool, hook_slug).await {
        Ok(Some(exec)) => exec,
        Ok(None) => return EvalOutcome::Allow,
        Err(e) => {
            tracing::warn!(hook_slug = %hook_slug, "cooldown check failed: {e}");
            return EvalOutcome::Allow;
        }
    };

    let Some(started_at_str) = &latest.started_at else {
        return EvalOutcome::Allow;
    };

    let Ok(started_at) = DateTime::parse_from_rfc3339(started_at_str)
        .or_else(|_| {
            // Timestamps in the DB may not have timezone suffix; assume UTC.
            chrono::NaiveDateTime::parse_from_str(started_at_str, "%Y-%m-%dT%H:%M:%S")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(started_at_str, "%Y-%m-%dT%H:%M:%S%.f"))
                .map(|ndt| ndt.and_utc().fixed_offset())
        })
    else {
        tracing::warn!(
            hook_slug = %hook_slug,
            started_at = %started_at_str,
            "cooldown check: failed to parse started_at timestamp"
        );
        return EvalOutcome::Allow;
    };

    let started_at_utc: DateTime<Utc> = started_at.into();
    let elapsed = now.signed_duration_since(started_at_utc);

    let cooldown_chrono = match chrono::Duration::from_std(cooldown) {
        Ok(d) => d,
        Err(_) => return EvalOutcome::Allow,
    };

    if elapsed < cooldown_chrono {
        let remaining = cooldown_chrono - elapsed;
        let remaining_secs = remaining.num_seconds().max(0);
        EvalOutcome::Reject {
            status: TriggerAttemptStatus::CooldownSkipped,
            reason: format!(
                "cooldown active, {}s remaining",
                remaining_secs,
            ),
        }
    } else {
        EvalOutcome::Allow
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::models::execution::{self, NewExecution};
    use chrono::TimeZone;

    async fn setup_db() -> SqlitePool {
        let db = Db::new_in_memory().await.unwrap();
        db.migrate().await.unwrap();
        db.pool().clone()
    }

    fn utc(year: i32, month: u32, day: u32, hour: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, min, 0)
            .unwrap()
    }

    fn parse_timestamp(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
                    .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f"))
                    .map(|ndt| ndt.and_utc())
            })
            .unwrap()
    }

    async fn create_execution(pool: &SqlitePool, hook_slug: &str) -> execution::Execution {
        execution::create(
            pool,
            &NewExecution {
                id: None,
                hook_slug,
                log_path: "test/logs",
                trigger_source: "127.0.0.1",
                request_payload: "{}",
                retry_of: None,
            },
        )
        .await
        .unwrap()
    }

    async fn mark_running(pool: &SqlitePool, id: &str) {
        execution::mark_running(pool, id).await.unwrap();
    }

    #[tokio::test]
    async fn no_prior_execution_allows() {
        let pool = setup_db().await;
        let result = evaluate_at(&pool, "test-hook", Duration::from_secs(300), utc(2026, 4, 13, 10, 0)).await;
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[tokio::test]
    async fn within_cooldown_rejects() {
        let pool = setup_db().await;
        let exec = create_execution(&pool, "test-hook").await;
        mark_running(&pool, &exec.id).await;

        // Execution started ~now. Check cooldown 5 minutes later.
        // started_at is set by mark_running to "now" in DB.
        // We evaluate at a time 2 minutes after "now" with a 5-minute cooldown.
        // Since started_at is written as timestamp::now_utc(), we need to query it.
        let exec = execution::get_by_id(&pool, &exec.id).await.unwrap();
        let started_at = parse_timestamp(exec.started_at.as_deref().unwrap());

        // 2 minutes after start, 5 min cooldown -> should reject
        let check_time = started_at + chrono::Duration::minutes(2);
        let result = evaluate_at(&pool, "test-hook", Duration::from_secs(300), check_time).await;
        assert!(matches!(result, EvalOutcome::Reject { .. }));

        if let EvalOutcome::Reject { status, reason } = result {
            assert_eq!(status, TriggerAttemptStatus::CooldownSkipped);
            assert!(reason.contains("remaining"));
        }
    }

    #[tokio::test]
    async fn after_cooldown_allows() {
        let pool = setup_db().await;
        let exec = create_execution(&pool, "test-hook").await;
        mark_running(&pool, &exec.id).await;

        let exec = execution::get_by_id(&pool, &exec.id).await.unwrap();
        let started_at = parse_timestamp(exec.started_at.as_deref().unwrap());

        // 10 minutes after start, 5 min cooldown -> should allow
        let check_time = started_at + chrono::Duration::minutes(10);
        let result = evaluate_at(&pool, "test-hook", Duration::from_secs(300), check_time).await;
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[tokio::test]
    async fn pending_execution_without_started_at_ignored() {
        let pool = setup_db().await;
        // Create execution but don't mark it as running (started_at is NULL)
        create_execution(&pool, "test-hook").await;

        let result = evaluate_at(&pool, "test-hook", Duration::from_secs(300), utc(2026, 4, 13, 10, 0)).await;
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[tokio::test]
    async fn reason_includes_remaining_time() {
        let pool = setup_db().await;
        let exec = create_execution(&pool, "test-hook").await;
        mark_running(&pool, &exec.id).await;

        let exec = execution::get_by_id(&pool, &exec.id).await.unwrap();
        let started_at = parse_timestamp(exec.started_at.as_deref().unwrap());

        // 1 minute after start, 5 min cooldown -> 240s remaining
        let check_time = started_at + chrono::Duration::minutes(1);
        let result = evaluate_at(&pool, "test-hook", Duration::from_secs(300), check_time).await;

        let EvalOutcome::Reject { reason, .. } = result else {
            panic!("expected Reject");
        };
        assert!(reason.contains("remaining"), "reason should mention remaining: {reason}");
    }

    #[tokio::test]
    async fn different_hooks_independent() {
        let pool = setup_db().await;
        let exec = create_execution(&pool, "hook-a").await;
        mark_running(&pool, &exec.id).await;

        let exec = execution::get_by_id(&pool, &exec.id).await.unwrap();
        let started_at = parse_timestamp(exec.started_at.as_deref().unwrap());

        // hook-a has a running execution, but hook-b should not be affected
        let check_time = started_at + chrono::Duration::minutes(1);
        let result = evaluate_at(&pool, "hook-b", Duration::from_secs(300), check_time).await;
        assert!(matches!(result, EvalOutcome::Allow));
    }
}
