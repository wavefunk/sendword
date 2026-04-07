use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::{DbError, DbResult};
use crate::id;
use crate::timestamp;

// --- Types ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Pending,
    PendingApproval,
    Approved,
    Rejected,
    Expired,
    Running,
    Success,
    Failed,
    TimedOut,
}

impl ExecutionStatus {
    /// Returns true if this status represents a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Success | Self::Failed | Self::TimedOut | Self::Rejected | Self::Expired
        )
    }
}

impl std::fmt::Display for ExecutionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::PendingApproval => "pending_approval",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Running => "running",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Execution {
    pub id: String,
    pub hook_slug: String,
    pub triggered_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    #[sqlx(rename = "status")]
    pub status: ExecutionStatus,
    pub exit_code: Option<i32>,
    pub log_path: String,
    pub trigger_source: String,
    pub request_payload: String,
    pub retry_count: i32,
    pub retry_of: Option<String>,
    pub approved_at: Option<String>,
    pub approved_by: Option<String>,
}

/// Parameters for creating a new execution.
pub struct NewExecution<'a> {
    /// Pre-generated ID. If None, a new UUIDv7 is generated.
    pub id: Option<&'a str>,
    pub hook_slug: &'a str,
    pub log_path: &'a str,
    pub trigger_source: &'a str,
    pub request_payload: &'a str,
    pub retry_of: Option<&'a str>,
}

// --- Query functions ---

/// Insert a new execution with `pending` status. Returns the created record.
pub async fn create(pool: &SqlitePool, new: &NewExecution<'_>) -> DbResult<Execution> {
    let id = new.id.map(String::from).unwrap_or_else(id::new_id);
    let triggered_at = timestamp::now_utc();
    let status = ExecutionStatus::Pending.to_string();

    sqlx::query(
        "INSERT INTO executions (id, hook_slug, triggered_at, status, log_path, trigger_source, request_payload, retry_of) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(new.hook_slug)
    .bind(&triggered_at)
    .bind(&status)
    .bind(new.log_path)
    .bind(new.trigger_source)
    .bind(new.request_payload)
    .bind(new.retry_of)
    .execute(pool)
    .await?;

    get_by_id(pool, &id).await
}

/// Fetch a single execution by primary key.
pub async fn get_by_id(pool: &SqlitePool, id: &str) -> DbResult<Execution> {
    sqlx::query_as::<_, Execution>(
        "SELECT id, hook_slug, triggered_at, started_at, completed_at, \
                status, exit_code, log_path, trigger_source, request_payload, \
                retry_count, retry_of, approved_at, approved_by \
         FROM executions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("execution {id}")))
}

/// Transition an execution from pending to running.
pub async fn mark_running(pool: &SqlitePool, id: &str) -> DbResult<()> {
    let started_at = timestamp::now_utc();
    let status = ExecutionStatus::Running.to_string();

    let result = sqlx::query(
        "UPDATE executions SET status = ?, started_at = ? \
         WHERE id = ? AND status = 'pending'",
    )
    .bind(&status)
    .bind(&started_at)
    .bind(id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(DbError::Conflict(format!(
            "execution {id} is not in pending status"
        )));
    }
    Ok(())
}

/// Transition an execution from running to a terminal status.
pub async fn mark_completed(
    pool: &SqlitePool,
    id: &str,
    status: ExecutionStatus,
    exit_code: Option<i32>,
) -> DbResult<()> {
    let completed_at = timestamp::now_utc();
    let status_str = status.to_string();

    let result = sqlx::query(
        "UPDATE executions SET status = ?, completed_at = ?, exit_code = ? \
         WHERE id = ? AND status = 'running'",
    )
    .bind(&status_str)
    .bind(&completed_at)
    .bind(exit_code)
    .bind(id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(DbError::Conflict(format!(
            "execution {id} is not in running status"
        )));
    }
    Ok(())
}

/// Increment the retry count for an execution.
pub async fn increment_retry_count(pool: &SqlitePool, id: &str) -> DbResult<()> {
    sqlx::query("UPDATE executions SET retry_count = retry_count + 1 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// List executions for a specific hook, ordered by triggered_at DESC, paginated.
pub async fn list_by_hook(
    pool: &SqlitePool,
    hook_slug: &str,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<Execution>> {
    let rows = sqlx::query_as::<_, Execution>(
        "SELECT id, hook_slug, triggered_at, started_at, completed_at, \
                status, exit_code, log_path, trigger_source, request_payload, \
                retry_count, retry_of, approved_at, approved_by \
         FROM executions WHERE hook_slug = ? \
         ORDER BY triggered_at DESC LIMIT ? OFFSET ?",
    )
    .bind(hook_slug)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// List recent executions across all hooks, ordered by triggered_at DESC.
pub async fn list_recent(pool: &SqlitePool, limit: i64) -> DbResult<Vec<Execution>> {
    let rows = sqlx::query_as::<_, Execution>(
        "SELECT id, hook_slug, triggered_at, started_at, completed_at, \
                status, exit_code, log_path, trigger_source, request_payload, \
                retry_count, retry_of, approved_at, approved_by \
         FROM executions ORDER BY triggered_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Get the most recent execution for a given hook slug.
pub async fn get_latest_by_hook(
    pool: &SqlitePool,
    hook_slug: &str,
) -> DbResult<Option<Execution>> {
    let row = sqlx::query_as::<_, Execution>(
        "SELECT id, hook_slug, triggered_at, started_at, completed_at, \
                status, exit_code, log_path, trigger_source, request_payload, \
                retry_count, retry_of, approved_at, approved_by \
         FROM executions WHERE hook_slug = ? \
         ORDER BY triggered_at DESC LIMIT 1",
    )
    .bind(hook_slug)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Count total executions for a hook (for pagination).
pub async fn count_by_hook(pool: &SqlitePool, hook_slug: &str) -> DbResult<i64> {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM executions WHERE hook_slug = ?")
            .bind(hook_slug)
            .fetch_one(pool)
            .await?;
    Ok(row.0)
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

    fn test_new_execution() -> NewExecution<'static> {
        NewExecution {
            id: None,
            hook_slug: "deploy-app",
            log_path: "data/logs/test-id",
            trigger_source: "127.0.0.1",
            request_payload: r#"{"action": "opened"}"#,
            retry_of: None,
        }
    }

    #[tokio::test]
    async fn create_returns_execution_with_pending_status() {
        let pool = test_pool().await;
        let exec = create(&pool, &test_new_execution()).await.unwrap();

        assert_eq!(exec.hook_slug, "deploy-app");
        assert_eq!(exec.status, ExecutionStatus::Pending);
        assert_eq!(exec.log_path, "data/logs/test-id");
        assert_eq!(exec.trigger_source, "127.0.0.1");
        assert_eq!(exec.request_payload, r#"{"action": "opened"}"#);
        assert_eq!(exec.retry_count, 0);
        assert!(exec.retry_of.is_none());
        assert!(exec.started_at.is_none());
        assert!(exec.completed_at.is_none());
        assert!(exec.exit_code.is_none());
        assert!(!exec.id.is_empty());
        assert!(!exec.triggered_at.is_empty());
    }

    #[tokio::test]
    async fn get_by_id_returns_existing_execution() {
        let pool = test_pool().await;
        let created = create(&pool, &test_new_execution()).await.unwrap();
        let fetched = get_by_id(&pool, &created.id).await.unwrap();
        assert_eq!(created.id, fetched.id);
        assert_eq!(created.hook_slug, fetched.hook_slug);
    }

    #[tokio::test]
    async fn get_by_id_returns_not_found_for_missing_id() {
        let pool = test_pool().await;
        let result = get_by_id(&pool, "nonexistent").await;
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }

    #[tokio::test]
    async fn mark_running_sets_status_and_started_at() {
        let pool = test_pool().await;
        let exec = create(&pool, &test_new_execution()).await.unwrap();

        mark_running(&pool, &exec.id).await.unwrap();
        let updated = get_by_id(&pool, &exec.id).await.unwrap();

        assert_eq!(updated.status, ExecutionStatus::Running);
        assert!(updated.started_at.is_some());
    }

    #[tokio::test]
    async fn mark_running_rejects_non_pending_execution() {
        let pool = test_pool().await;
        let exec = create(&pool, &test_new_execution()).await.unwrap();

        mark_running(&pool, &exec.id).await.unwrap();
        // Try to mark running again -- should fail
        let result = mark_running(&pool, &exec.id).await;
        assert!(matches!(result, Err(DbError::Conflict(_))));
    }

    #[tokio::test]
    async fn mark_completed_success_sets_fields() {
        let pool = test_pool().await;
        let exec = create(&pool, &test_new_execution()).await.unwrap();
        mark_running(&pool, &exec.id).await.unwrap();

        mark_completed(&pool, &exec.id, ExecutionStatus::Success, Some(0))
            .await
            .unwrap();
        let updated = get_by_id(&pool, &exec.id).await.unwrap();

        assert_eq!(updated.status, ExecutionStatus::Success);
        assert!(updated.completed_at.is_some());
        assert_eq!(updated.exit_code, Some(0));
    }

    #[tokio::test]
    async fn mark_completed_failed_with_exit_code() {
        let pool = test_pool().await;
        let exec = create(&pool, &test_new_execution()).await.unwrap();
        mark_running(&pool, &exec.id).await.unwrap();

        mark_completed(&pool, &exec.id, ExecutionStatus::Failed, Some(1))
            .await
            .unwrap();
        let updated = get_by_id(&pool, &exec.id).await.unwrap();

        assert_eq!(updated.status, ExecutionStatus::Failed);
        assert_eq!(updated.exit_code, Some(1));
    }

    #[tokio::test]
    async fn mark_completed_timed_out() {
        let pool = test_pool().await;
        let exec = create(&pool, &test_new_execution()).await.unwrap();
        mark_running(&pool, &exec.id).await.unwrap();

        mark_completed(&pool, &exec.id, ExecutionStatus::TimedOut, None)
            .await
            .unwrap();
        let updated = get_by_id(&pool, &exec.id).await.unwrap();

        assert_eq!(updated.status, ExecutionStatus::TimedOut);
        assert!(updated.exit_code.is_none());
    }

    #[tokio::test]
    async fn mark_completed_rejects_non_running_execution() {
        let pool = test_pool().await;
        let exec = create(&pool, &test_new_execution()).await.unwrap();
        // Still pending, not running
        let result = mark_completed(&pool, &exec.id, ExecutionStatus::Success, Some(0)).await;
        assert!(matches!(result, Err(DbError::Conflict(_))));
    }

    #[tokio::test]
    async fn increment_retry_count_increments() {
        let pool = test_pool().await;
        let exec = create(&pool, &test_new_execution()).await.unwrap();
        assert_eq!(exec.retry_count, 0);

        increment_retry_count(&pool, &exec.id).await.unwrap();
        let updated = get_by_id(&pool, &exec.id).await.unwrap();
        assert_eq!(updated.retry_count, 1);

        increment_retry_count(&pool, &exec.id).await.unwrap();
        let updated = get_by_id(&pool, &exec.id).await.unwrap();
        assert_eq!(updated.retry_count, 2);
    }

    #[tokio::test]
    async fn list_by_hook_returns_executions_in_descending_order() {
        let pool = test_pool().await;

        // Create 3 executions for the same hook
        for _ in 0..3 {
            create(&pool, &test_new_execution()).await.unwrap();
        }

        let list = list_by_hook(&pool, "deploy-app", 10, 0).await.unwrap();
        assert_eq!(list.len(), 3);

        // Most recent first (UUIDv7 IDs are time-ordered, triggered_at is also monotonic)
        for pair in list.windows(2) {
            assert!(pair[0].triggered_at >= pair[1].triggered_at);
        }
    }

    #[tokio::test]
    async fn list_by_hook_respects_limit_and_offset() {
        let pool = test_pool().await;

        for _ in 0..5 {
            create(&pool, &test_new_execution()).await.unwrap();
        }

        let page1 = list_by_hook(&pool, "deploy-app", 2, 0).await.unwrap();
        assert_eq!(page1.len(), 2);

        let page2 = list_by_hook(&pool, "deploy-app", 2, 2).await.unwrap();
        assert_eq!(page2.len(), 2);

        let page3 = list_by_hook(&pool, "deploy-app", 2, 4).await.unwrap();
        assert_eq!(page3.len(), 1);

        // Pages should not overlap
        assert_ne!(page1[0].id, page2[0].id);
    }

    #[tokio::test]
    async fn list_by_hook_filters_by_slug() {
        let pool = test_pool().await;

        create(&pool, &test_new_execution()).await.unwrap();
        create(
            &pool,
            &NewExecution {
                id: None,
                hook_slug: "other-hook",
                log_path: "data/logs/other",
                trigger_source: "127.0.0.1",
                request_payload: "{}",
                retry_of: None,
            },
        )
        .await
        .unwrap();

        let list = list_by_hook(&pool, "deploy-app", 10, 0).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].hook_slug, "deploy-app");
    }

    #[tokio::test]
    async fn list_recent_returns_across_all_hooks() {
        let pool = test_pool().await;

        create(&pool, &test_new_execution()).await.unwrap();
        create(
            &pool,
            &NewExecution {
                id: None,
                hook_slug: "other-hook",
                log_path: "data/logs/other",
                trigger_source: "10.0.0.1",
                request_payload: "{}",
                retry_of: None,
            },
        )
        .await
        .unwrap();

        let list = list_recent(&pool, 10).await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn get_latest_by_hook_returns_most_recent() {
        let pool = test_pool().await;

        let first = create(&pool, &test_new_execution()).await.unwrap();
        let second = create(&pool, &test_new_execution()).await.unwrap();

        let latest = get_latest_by_hook(&pool, "deploy-app").await.unwrap();
        assert!(latest.is_some());
        let latest = latest.unwrap();
        assert_eq!(latest.id, second.id);
        assert_ne!(latest.id, first.id);
    }

    #[tokio::test]
    async fn get_latest_by_hook_returns_none_for_unknown_slug() {
        let pool = test_pool().await;
        let latest = get_latest_by_hook(&pool, "nonexistent").await.unwrap();
        assert!(latest.is_none());
    }

    #[tokio::test]
    async fn create_with_retry_of_links_to_original() {
        let pool = test_pool().await;
        let original = create(&pool, &test_new_execution()).await.unwrap();

        let replay = create(
            &pool,
            &NewExecution {
                id: None,
                hook_slug: "deploy-app",
                log_path: "data/logs/replay",
                trigger_source: "127.0.0.1",
                request_payload: r#"{"action": "opened"}"#,
                retry_of: Some(&original.id),
            },
        )
        .await
        .unwrap();

        assert_eq!(replay.retry_of.as_deref(), Some(original.id.as_str()));
    }

    #[tokio::test]
    async fn count_by_hook_returns_correct_count() {
        let pool = test_pool().await;

        assert_eq!(count_by_hook(&pool, "deploy-app").await.unwrap(), 0);

        create(&pool, &test_new_execution()).await.unwrap();
        assert_eq!(count_by_hook(&pool, "deploy-app").await.unwrap(), 1);

        create(&pool, &test_new_execution()).await.unwrap();
        assert_eq!(count_by_hook(&pool, "deploy-app").await.unwrap(), 2);

        // Different hook should not affect count
        create(
            &pool,
            &NewExecution {
                id: None,
                hook_slug: "other-hook",
                log_path: "data/logs/other",
                trigger_source: "127.0.0.1",
                request_payload: "{}",
                retry_of: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(count_by_hook(&pool, "deploy-app").await.unwrap(), 2);
    }

    #[test]
    fn execution_status_is_terminal() {
        assert!(!ExecutionStatus::Pending.is_terminal());
        assert!(!ExecutionStatus::Running.is_terminal());
        assert!(!ExecutionStatus::PendingApproval.is_terminal());
        assert!(!ExecutionStatus::Approved.is_terminal());
        assert!(ExecutionStatus::Success.is_terminal());
        assert!(ExecutionStatus::Failed.is_terminal());
        assert!(ExecutionStatus::TimedOut.is_terminal());
        assert!(ExecutionStatus::Rejected.is_terminal());
        assert!(ExecutionStatus::Expired.is_terminal());
    }
}
