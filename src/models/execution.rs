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
    /// Optional initial status. Defaults to `pending` if None.
    pub status: Option<ExecutionStatus>,
}

// --- Query functions ---

/// Insert a new execution with `pending` status. Returns the created record.
pub async fn create(pool: &SqlitePool, new: &NewExecution<'_>) -> DbResult<Execution> {
    let id = new.id.map(String::from).unwrap_or_else(id::new_id);
    let triggered_at = timestamp::now_utc();
    let status = new
        .status
        .as_ref()
        .unwrap_or(&ExecutionStatus::Pending)
        .to_string();

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

/// Get the most recent execution for a hook that has actually started
/// (started_at IS NOT NULL). Used by cooldown evaluation.
pub async fn get_latest_started_by_hook(
    pool: &SqlitePool,
    hook_slug: &str,
) -> DbResult<Option<Execution>> {
    let row = sqlx::query_as::<_, Execution>(
        "SELECT id, hook_slug, triggered_at, started_at, completed_at, \
                status, exit_code, log_path, trigger_source, request_payload, \
                retry_count, retry_of, approved_at, approved_by \
         FROM executions WHERE hook_slug = ? AND started_at IS NOT NULL \
         ORDER BY started_at DESC LIMIT 1",
    )
    .bind(hook_slug)
    .fetch_optional(pool)
    .await?;
    Ok(row)
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

/// Get the last N executions for a given hook slug, most recent first.
/// Used by the dashboard to render per-hook status indicators.
pub async fn list_recent_by_hook(
    pool: &SqlitePool,
    hook_slug: &str,
    limit: i64,
) -> DbResult<Vec<Execution>> {
    let rows = sqlx::query_as::<_, Execution>(
        "SELECT id, hook_slug, triggered_at, started_at, completed_at, \
                status, exit_code, log_path, trigger_source, request_payload, \
                retry_count, retry_of, approved_at, approved_by \
         FROM executions WHERE hook_slug = ? \
         ORDER BY triggered_at DESC LIMIT ?",
    )
    .bind(hook_slug)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
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

/// Filters for the execution list. All fields are optional; omitted fields are not applied.
pub struct ExecutionFilters<'a> {
    /// Filter by status string (e.g. "success", "failed"). None = all statuses.
    pub status: Option<&'a str>,
    /// Inclusive lower bound on triggered_at (ISO8601). None = no lower bound.
    pub from_date: Option<&'a str>,
    /// Inclusive upper bound on triggered_at (ISO8601). None = no upper bound.
    pub to_date: Option<&'a str>,
}

/// List executions for a hook with optional filters, ordered by triggered_at DESC.
///
/// Status filter is pushed into SQL. Date range filters are applied in Rust
/// (triggered_at is an ISO8601 string so lexicographic comparison is correct).
pub async fn list_by_hook_filtered(
    pool: &SqlitePool,
    hook_slug: &str,
    filters: &ExecutionFilters<'_>,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<Execution>> {
    let rows = match (filters.status, filters.from_date, filters.to_date) {
        (None, None, None) => list_by_hook(pool, hook_slug, limit, offset).await?,
        (Some(s), None, None) => sqlx::query_as::<_, Execution>(
                "SELECT id, hook_slug, triggered_at, started_at, completed_at, \
                        status, exit_code, log_path, trigger_source, request_payload, \
                        retry_count, retry_of, approved_at, approved_by \
                 FROM executions WHERE hook_slug = ? AND status = ? \
                 ORDER BY triggered_at DESC LIMIT ? OFFSET ?",
            )
            .bind(hook_slug).bind(s).bind(limit).bind(offset)
            .fetch_all(pool).await?,
        (status, from_date, to_date) => {
            // Fetch with status filter only, then apply dates in Rust.
            let candidates = if let Some(s) = status {
                sqlx::query_as::<_, Execution>(
                    "SELECT id, hook_slug, triggered_at, started_at, completed_at, \
                            status, exit_code, log_path, trigger_source, request_payload, \
                            retry_count, retry_of, approved_at, approved_by \
                     FROM executions WHERE hook_slug = ? AND status = ? \
                     ORDER BY triggered_at DESC LIMIT ? OFFSET ?",
                )
                .bind(hook_slug)
                .bind(s)
                .bind(limit + offset) // fetch enough to satisfy offset
                .bind(0i64)
                .fetch_all(pool)
                .await?
            } else {
                sqlx::query_as::<_, Execution>(
                    "SELECT id, hook_slug, triggered_at, started_at, completed_at, \
                            status, exit_code, log_path, trigger_source, request_payload, \
                            retry_count, retry_of, approved_at, approved_by \
                     FROM executions WHERE hook_slug = ? \
                     ORDER BY triggered_at DESC LIMIT ? OFFSET ?",
                )
                .bind(hook_slug)
                .bind(limit + offset)
                .bind(0i64)
                .fetch_all(pool)
                .await?
            };

            // Apply date range post-filter.
            let filtered: Vec<Execution> = candidates
                .into_iter()
                .filter(|e| {
                    if let Some(from) = from_date {
                        if e.triggered_at.as_str() < from { return false; }
                    }
                    if let Some(to) = to_date {
                        // Include everything up to and including the to_date day.
                        // triggered_at is an ISO8601 timestamp; compare prefix.
                        let day = &e.triggered_at[..to.len().min(e.triggered_at.len())];
                        if day > to { return false; }
                    }
                    true
                })
                .skip(offset as usize)
                .take(limit as usize)
                .collect();
            filtered
        }
    };
    Ok(rows)
}

/// Count executions for a hook with optional filters (for pagination).
pub async fn count_by_hook_filtered(
    pool: &SqlitePool,
    hook_slug: &str,
    filters: &ExecutionFilters<'_>,
) -> DbResult<i64> {
    match (filters.status, filters.from_date, filters.to_date) {
        (None, None, None) => count_by_hook(pool, hook_slug).await,
        (Some(s), None, None) => {
            let row: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM executions WHERE hook_slug = ? AND status = ?",
            )
            .bind(hook_slug)
            .bind(s)
            .fetch_one(pool)
            .await?;
            Ok(row.0)
        }
        (status, from_date, to_date) => {
            // For date-range queries, count by fetching all filtered rows.
            // This is less efficient but avoids dynamic SQL for a rarely-used path.
            let all = list_by_hook_filtered(
                pool,
                hook_slug,
                &ExecutionFilters { status, from_date, to_date },
                i64::MAX / 2,
                0,
            )
            .await?;
            Ok(all.len() as i64)
        }
    }
}

/// Transition pending_approval -> approved, recording who approved and when.
pub async fn mark_approved(pool: &SqlitePool, id: &str, approved_by: &str) -> DbResult<Execution> {
    let approved_at = timestamp::now_utc();
    let result = sqlx::query(
        "UPDATE executions SET status = 'approved', approved_at = ?, approved_by = ? \
         WHERE id = ? AND status = 'pending_approval'",
    )
    .bind(&approved_at)
    .bind(approved_by)
    .bind(id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(DbError::Conflict(format!(
            "execution {id} is not in pending_approval status"
        )));
    }
    get_by_id(pool, id).await
}

/// Transition pending_approval -> rejected.
pub async fn mark_rejected(pool: &SqlitePool, id: &str, rejected_by: &str) -> DbResult<Execution> {
    let completed_at = timestamp::now_utc();
    let result = sqlx::query(
        "UPDATE executions SET status = 'rejected', completed_at = ?, approved_at = ?, approved_by = ? \
         WHERE id = ? AND status = 'pending_approval'",
    )
    .bind(&completed_at)
    .bind(&completed_at)
    .bind(rejected_by)
    .bind(id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(DbError::Conflict(format!(
            "execution {id} is not in pending_approval status"
        )));
    }
    get_by_id(pool, id).await
}

/// Transition pending_approval -> expired (for timeout sweep).
pub async fn mark_expired(pool: &SqlitePool, id: &str) -> DbResult<()> {
    let completed_at = timestamp::now_utc();
    sqlx::query(
        "UPDATE executions SET status = 'expired', completed_at = ? \
         WHERE id = ? AND status = 'pending_approval'",
    )
    .bind(&completed_at)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Transition pending -> pending_approval (for queued items that reach the front and need approval).
pub async fn mark_pending_approval(pool: &SqlitePool, id: &str) -> DbResult<()> {
    let result = sqlx::query(
        "UPDATE executions SET status = 'pending_approval' WHERE id = ? AND status = 'pending'",
    )
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

/// List all executions with pending_approval status, most recent first.
pub async fn list_pending_approval(pool: &SqlitePool) -> DbResult<Vec<Execution>> {
    let rows = sqlx::query_as::<_, Execution>(
        "SELECT id, hook_slug, triggered_at, started_at, completed_at, \
                status, exit_code, log_path, trigger_source, request_payload, \
                retry_count, retry_of, approved_at, approved_by \
         FROM executions WHERE status = 'pending_approval' \
         ORDER BY triggered_at DESC",
    )
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

    fn test_new_execution() -> NewExecution<'static> {
        NewExecution {
            id: None,
            hook_slug: "deploy-app",
            log_path: "data/logs/test-id",
            trigger_source: "127.0.0.1",
            request_payload: r#"{"action": "opened"}"#,
            retry_of: None,
            status: None,
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
                status: None,
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
                status: None,
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
                status: None,
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
                status: None,
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
