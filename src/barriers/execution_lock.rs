use sqlx::SqlitePool;

use crate::error::DbResult;
use crate::timestamp;

/// Attempt to acquire an execution lock for a hook.
/// Returns true if the lock was acquired, false if another execution holds it.
pub async fn try_acquire(pool: &SqlitePool, hook_slug: &str, execution_id: &str) -> DbResult<bool> {
    let acquired_at = timestamp::now_utc();
    let result = sqlx::query(
        "INSERT OR IGNORE INTO execution_locks (hook_slug, execution_id, acquired_at) \
         VALUES (?, ?, ?)",
    )
    .bind(hook_slug)
    .bind(execution_id)
    .bind(&acquired_at)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Release the execution lock for a hook.
pub async fn release(pool: &SqlitePool, hook_slug: &str) -> DbResult<()> {
    sqlx::query("DELETE FROM execution_locks WHERE hook_slug = ?")
        .bind(hook_slug)
        .execute(pool)
        .await?;
    Ok(())
}

/// Hand off the lock atomically to a new execution without releasing it.
/// UPDATE replaces the holder in-place, preventing a race where a new
/// trigger steals the lock between a release and re-acquire.
pub async fn hand_off(pool: &SqlitePool, hook_slug: &str, next_execution_id: &str) -> DbResult<()> {
    let acquired_at = timestamp::now_utc();
    sqlx::query("UPDATE execution_locks SET execution_id = ?, acquired_at = ? WHERE hook_slug = ?")
        .bind(next_execution_id)
        .bind(&acquired_at)
        .bind(hook_slug)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get the execution_id currently holding the lock for a hook, if any.
pub async fn get_holder(pool: &SqlitePool, hook_slug: &str) -> DbResult<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT execution_id FROM execution_locks WHERE hook_slug = ?")
            .bind(hook_slug)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn test_pool() -> SqlitePool {
        let db = Db::new_in_memory().await.unwrap();
        db.migrate().await.unwrap();
        db.pool().clone()
    }

    #[tokio::test]
    async fn acquire_succeeds_when_no_lock() {
        let pool = test_pool().await;
        assert!(try_acquire(&pool, "hook-a", "exec-1").await.unwrap());
    }

    #[tokio::test]
    async fn acquire_fails_when_lock_held() {
        let pool = test_pool().await;
        assert!(try_acquire(&pool, "hook-a", "exec-1").await.unwrap());
        assert!(!try_acquire(&pool, "hook-a", "exec-2").await.unwrap());
    }

    #[tokio::test]
    async fn release_allows_reacquire() {
        let pool = test_pool().await;
        assert!(try_acquire(&pool, "hook-a", "exec-1").await.unwrap());
        release(&pool, "hook-a").await.unwrap();
        assert!(try_acquire(&pool, "hook-a", "exec-2").await.unwrap());
    }

    #[tokio::test]
    async fn different_hooks_independent() {
        let pool = test_pool().await;
        assert!(try_acquire(&pool, "hook-a", "exec-1").await.unwrap());
        assert!(try_acquire(&pool, "hook-b", "exec-2").await.unwrap());
    }

    #[tokio::test]
    async fn get_holder_returns_execution_id() {
        let pool = test_pool().await;
        try_acquire(&pool, "hook-a", "exec-1").await.unwrap();
        assert_eq!(
            get_holder(&pool, "hook-a").await.unwrap(),
            Some("exec-1".into())
        );
    }

    #[tokio::test]
    async fn get_holder_returns_none_when_no_lock() {
        let pool = test_pool().await;
        assert_eq!(get_holder(&pool, "hook-a").await.unwrap(), None);
    }

    #[tokio::test]
    async fn hand_off_replaces_holder() {
        let pool = test_pool().await;
        try_acquire(&pool, "hook-a", "exec-1").await.unwrap();
        hand_off(&pool, "hook-a", "exec-2").await.unwrap();
        assert_eq!(
            get_holder(&pool, "hook-a").await.unwrap(),
            Some("exec-2".into())
        );
    }

    #[tokio::test]
    async fn hand_off_does_not_release_lock() {
        let pool = test_pool().await;
        try_acquire(&pool, "hook-a", "exec-1").await.unwrap();
        hand_off(&pool, "hook-a", "exec-2").await.unwrap();
        // Lock is still held -- a new try_acquire should fail
        assert!(!try_acquire(&pool, "hook-a", "exec-3").await.unwrap());
    }
}
