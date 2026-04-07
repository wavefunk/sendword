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
