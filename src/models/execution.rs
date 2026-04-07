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
    pub hook_slug: &'a str,
    pub log_path: &'a str,
    pub trigger_source: &'a str,
    pub request_payload: &'a str,
    pub retry_of: Option<&'a str>,
}

// --- Query functions ---

/// Insert a new execution with `pending` status. Returns the created record.
pub async fn create(pool: &SqlitePool, new: &NewExecution<'_>) -> DbResult<Execution> {
    let id = id::new_id();
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
