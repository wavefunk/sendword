use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::{DbError, DbResult};
use crate::id;
use crate::timestamp;

// --- Types ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TriggerAttemptStatus {
    Fired,
    AuthFailed,
    ValidationFailed,
    Filtered,
    RateLimited,
    ScheduleSkipped,
    CooldownSkipped,
    ConcurrencyRejected,
    PendingApproval,
}

impl TriggerAttemptStatus {
    /// Parse a status string. Returns `None` for unrecognised values.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fired" => Some(Self::Fired),
            "auth_failed" => Some(Self::AuthFailed),
            "validation_failed" => Some(Self::ValidationFailed),
            "filtered" => Some(Self::Filtered),
            "rate_limited" => Some(Self::RateLimited),
            "schedule_skipped" => Some(Self::ScheduleSkipped),
            "cooldown_skipped" => Some(Self::CooldownSkipped),
            "concurrency_rejected" => Some(Self::ConcurrencyRejected),
            "pending_approval" => Some(Self::PendingApproval),
            _ => None,
        }
    }
}

impl std::fmt::Display for TriggerAttemptStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Fired => "fired",
            Self::AuthFailed => "auth_failed",
            Self::ValidationFailed => "validation_failed",
            Self::Filtered => "filtered",
            Self::RateLimited => "rate_limited",
            Self::ScheduleSkipped => "schedule_skipped",
            Self::CooldownSkipped => "cooldown_skipped",
            Self::ConcurrencyRejected => "concurrency_rejected",
            Self::PendingApproval => "pending_approval",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TriggerAttempt {
    pub id: String,
    pub hook_slug: String,
    pub attempted_at: String,
    pub source_ip: String,
    #[sqlx(rename = "status")]
    pub status: TriggerAttemptStatus,
    pub reason: String,
    pub execution_id: Option<String>,
}

/// Parameters for inserting a new trigger attempt.
pub struct NewTriggerAttempt<'a> {
    pub hook_slug: &'a str,
    pub source_ip: &'a str,
    pub status: TriggerAttemptStatus,
    pub reason: &'a str,
    pub execution_id: Option<&'a str>,
}

// --- Query functions ---

/// Insert a new trigger attempt. Returns the created record.
pub async fn insert(pool: &SqlitePool, new: &NewTriggerAttempt<'_>) -> DbResult<TriggerAttempt> {
    let id = id::new_id();
    let attempted_at = timestamp::now_utc();
    let status = new.status.to_string();

    sqlx::query(
        "INSERT INTO trigger_attempts (id, hook_slug, attempted_at, source_ip, status, reason, execution_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(new.hook_slug)
    .bind(&attempted_at)
    .bind(new.source_ip)
    .bind(&status)
    .bind(new.reason)
    .bind(new.execution_id)
    .execute(pool)
    .await?;

    get_by_id(pool, &id).await
}

/// Fetch a single trigger attempt by primary key.
pub async fn get_by_id(pool: &SqlitePool, id: &str) -> DbResult<TriggerAttempt> {
    sqlx::query_as::<_, TriggerAttempt>(
        "SELECT id, hook_slug, attempted_at, source_ip, status, reason, execution_id \
         FROM trigger_attempts WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("trigger_attempt {id}")))
}

/// List trigger attempts for a specific hook, ordered by attempted_at DESC, paginated.
pub async fn list_by_hook(
    pool: &SqlitePool,
    hook_slug: &str,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<TriggerAttempt>> {
    let rows = sqlx::query_as::<_, TriggerAttempt>(
        "SELECT id, hook_slug, attempted_at, source_ip, status, reason, execution_id \
         FROM trigger_attempts WHERE hook_slug = ? \
         ORDER BY attempted_at DESC LIMIT ? OFFSET ?",
    )
    .bind(hook_slug)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// List trigger attempts for a specific hook filtered by status, ordered by attempted_at DESC.
pub async fn list_by_hook_filtered(
    pool: &SqlitePool,
    hook_slug: &str,
    status: &TriggerAttemptStatus,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<TriggerAttempt>> {
    let status_str = status.to_string();
    let rows = sqlx::query_as::<_, TriggerAttempt>(
        "SELECT id, hook_slug, attempted_at, source_ip, status, reason, execution_id \
         FROM trigger_attempts WHERE hook_slug = ? AND status = ? \
         ORDER BY attempted_at DESC LIMIT ? OFFSET ?",
    )
    .bind(hook_slug)
    .bind(&status_str)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// List recent trigger attempts across all hooks, ordered by attempted_at DESC.
pub async fn list_recent(pool: &SqlitePool, limit: i64) -> DbResult<Vec<TriggerAttempt>> {
    let rows = sqlx::query_as::<_, TriggerAttempt>(
        "SELECT id, hook_slug, attempted_at, source_ip, status, reason, execution_id \
         FROM trigger_attempts ORDER BY attempted_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn test_pool() -> SqlitePool {
        let db = Db::new_in_memory().await.expect("in-memory db");
        db.migrate().await.expect("migration");
        db.pool().clone()
    }

    fn make_attempt(status: TriggerAttemptStatus) -> NewTriggerAttempt<'static> {
        NewTriggerAttempt {
            hook_slug: "deploy-app",
            source_ip: "127.0.0.1",
            status,
            reason: "test reason",
            execution_id: None,
        }
    }

    // --- Display round-trip: verify each variant produces the exact DB-safe string ---

    #[test]
    fn status_display_matches_db_check_constraint() {
        let cases = [
            (TriggerAttemptStatus::Fired, "fired"),
            (TriggerAttemptStatus::AuthFailed, "auth_failed"),
            (TriggerAttemptStatus::ValidationFailed, "validation_failed"),
            (TriggerAttemptStatus::Filtered, "filtered"),
            (TriggerAttemptStatus::RateLimited, "rate_limited"),
            (TriggerAttemptStatus::ScheduleSkipped, "schedule_skipped"),
            (TriggerAttemptStatus::CooldownSkipped, "cooldown_skipped"),
            (TriggerAttemptStatus::ConcurrencyRejected, "concurrency_rejected"),
            (TriggerAttemptStatus::PendingApproval, "pending_approval"),
        ];
        for (variant, expected) in cases {
            assert_eq!(variant.to_string(), expected);
        }
    }

    // --- insert + get_by_id round-trip for every status variant ---

    #[tokio::test]
    async fn insert_and_get_by_id_round_trip_all_statuses() {
        let pool = test_pool().await;

        let all_statuses = [
            TriggerAttemptStatus::Fired,
            TriggerAttemptStatus::AuthFailed,
            TriggerAttemptStatus::ValidationFailed,
            TriggerAttemptStatus::Filtered,
            TriggerAttemptStatus::RateLimited,
            TriggerAttemptStatus::ScheduleSkipped,
            TriggerAttemptStatus::CooldownSkipped,
            TriggerAttemptStatus::ConcurrencyRejected,
            TriggerAttemptStatus::PendingApproval,
        ];

        for status in all_statuses {
            let expected_status = status.clone();
            let attempt = insert(&pool, &make_attempt(status)).await.unwrap();

            assert_eq!(attempt.status, expected_status);
            assert_eq!(attempt.hook_slug, "deploy-app");
            assert_eq!(attempt.source_ip, "127.0.0.1");
            assert_eq!(attempt.reason, "test reason");
            assert!(attempt.execution_id.is_none());
            assert!(!attempt.id.is_empty());
            assert!(!attempt.attempted_at.is_empty());

            // Fetch back from DB and verify the status survived the round-trip
            let fetched = get_by_id(&pool, &attempt.id).await.unwrap();
            assert_eq!(fetched.id, attempt.id);
            assert_eq!(fetched.status, expected_status);
        }
    }

    // --- get_by_id error case ---

    #[tokio::test]
    async fn get_by_id_returns_not_found_for_missing_id() {
        let pool = test_pool().await;
        let result = get_by_id(&pool, "nonexistent").await;
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }

    // --- Nullable execution_id ---

    #[tokio::test]
    async fn insert_with_execution_id_persists_it() {
        let pool = test_pool().await;

        // Create an execution first so the FK is satisfied
        use crate::models::execution::{self, NewExecution};
        let exec = execution::create(
            &pool,
            &NewExecution {
                id: None,
                hook_slug: "deploy-app",
                log_path: "data/logs/test",
                trigger_source: "127.0.0.1",
                request_payload: "{}",
                retry_of: None,
            },
        )
        .await
        .unwrap();

        let attempt = insert(
            &pool,
            &NewTriggerAttempt {
                hook_slug: "deploy-app",
                source_ip: "10.0.0.1",
                status: TriggerAttemptStatus::Fired,
                reason: "matched",
                execution_id: Some(&exec.id),
            },
        )
        .await
        .unwrap();

        assert_eq!(attempt.execution_id.as_deref(), Some(exec.id.as_str()));

        let fetched = get_by_id(&pool, &attempt.id).await.unwrap();
        assert_eq!(fetched.execution_id.as_deref(), Some(exec.id.as_str()));
    }

    #[tokio::test]
    async fn insert_without_execution_id_stores_null() {
        let pool = test_pool().await;
        let attempt = insert(&pool, &make_attempt(TriggerAttemptStatus::AuthFailed))
            .await
            .unwrap();
        assert!(attempt.execution_id.is_none());
    }

    // --- list_by_hook: ordering and pagination ---

    #[tokio::test]
    async fn list_by_hook_returns_descending_order() {
        let pool = test_pool().await;

        for _ in 0..3 {
            insert(&pool, &make_attempt(TriggerAttemptStatus::Fired))
                .await
                .unwrap();
        }

        let list = list_by_hook(&pool, "deploy-app", 10, 0).await.unwrap();
        assert_eq!(list.len(), 3);

        for pair in list.windows(2) {
            assert!(pair[0].attempted_at >= pair[1].attempted_at);
        }
    }

    #[tokio::test]
    async fn list_by_hook_respects_limit_and_offset() {
        let pool = test_pool().await;

        for _ in 0..5 {
            insert(&pool, &make_attempt(TriggerAttemptStatus::Filtered))
                .await
                .unwrap();
        }

        let page1 = list_by_hook(&pool, "deploy-app", 2, 0).await.unwrap();
        assert_eq!(page1.len(), 2);

        let page2 = list_by_hook(&pool, "deploy-app", 2, 2).await.unwrap();
        assert_eq!(page2.len(), 2);

        let page3 = list_by_hook(&pool, "deploy-app", 2, 4).await.unwrap();
        assert_eq!(page3.len(), 1);

        // Pages should not overlap
        assert_ne!(page1[0].id, page2[0].id);
        assert_ne!(page2[0].id, page3[0].id);
    }

    #[tokio::test]
    async fn list_by_hook_filters_by_slug() {
        let pool = test_pool().await;

        insert(&pool, &make_attempt(TriggerAttemptStatus::Fired))
            .await
            .unwrap();
        insert(
            &pool,
            &NewTriggerAttempt {
                hook_slug: "other-hook",
                source_ip: "10.0.0.1",
                status: TriggerAttemptStatus::RateLimited,
                reason: "too fast",
                execution_id: None,
            },
        )
        .await
        .unwrap();

        let list = list_by_hook(&pool, "deploy-app", 10, 0).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].hook_slug, "deploy-app");
    }

    // --- list_recent: ordering, limit, cross-hook ---

    #[tokio::test]
    async fn list_recent_returns_across_all_hooks() {
        let pool = test_pool().await;

        insert(&pool, &make_attempt(TriggerAttemptStatus::Fired))
            .await
            .unwrap();
        insert(
            &pool,
            &NewTriggerAttempt {
                hook_slug: "other-hook",
                source_ip: "10.0.0.1",
                status: TriggerAttemptStatus::AuthFailed,
                reason: "bad token",
                execution_id: None,
            },
        )
        .await
        .unwrap();

        let list = list_recent(&pool, 10).await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn list_recent_respects_limit() {
        let pool = test_pool().await;

        for _ in 0..5 {
            insert(&pool, &make_attempt(TriggerAttemptStatus::Fired))
                .await
                .unwrap();
        }

        let list = list_recent(&pool, 3).await.unwrap();
        assert_eq!(list.len(), 3);
    }

    #[tokio::test]
    async fn list_recent_returns_descending_order() {
        let pool = test_pool().await;

        for _ in 0..3 {
            insert(&pool, &make_attempt(TriggerAttemptStatus::Fired))
                .await
                .unwrap();
        }

        let list = list_recent(&pool, 10).await.unwrap();
        for pair in list.windows(2) {
            assert!(pair[0].attempted_at >= pair[1].attempted_at);
        }
    }
}
