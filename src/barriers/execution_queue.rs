use sqlx::SqlitePool;

use crate::error::DbResult;
use crate::id;
use crate::timestamp;

/// A queue entry returned by peek/dequeue operations.
pub struct QueueEntry {
    pub id: String,
    pub hook_slug: String,
    pub execution_id: String,
    pub position: i64,
}

/// Enqueue an execution for a hook. Returns the assigned position.
///
/// Uses an atomic INSERT-SELECT to compute the next position in a single
/// statement, avoiding TOCTOU races under concurrent enqueuers.
pub async fn enqueue(pool: &SqlitePool, hook_slug: &str, execution_id: &str) -> DbResult<i64> {
    let id = id::new_id();
    let queued_at = timestamp::now_utc();

    sqlx::query(
        "INSERT INTO execution_queue (id, hook_slug, execution_id, position, queued_at, status) \
         SELECT ?, ?, ?, COALESCE(MAX(position), 0) + 1, ?, 'waiting' \
         FROM execution_queue WHERE hook_slug = ?",
    )
    .bind(&id)
    .bind(hook_slug)
    .bind(execution_id)
    .bind(&queued_at)
    .bind(hook_slug)
    .execute(pool)
    .await?;

    // Read back the assigned position.
    let row: (i64,) = sqlx::query_as("SELECT position FROM execution_queue WHERE id = ?")
        .bind(&id)
        .fetch_one(pool)
        .await?;

    Ok(row.0)
}

/// Peek at the next waiting entry for a hook without changing its status.
/// Returns None if no waiting entries exist.
pub async fn peek_next(pool: &SqlitePool, hook_slug: &str) -> DbResult<Option<QueueEntry>> {
    let row: Option<(String, String, String, i64)> = sqlx::query_as(
        "SELECT id, hook_slug, execution_id, position \
         FROM execution_queue \
         WHERE hook_slug = ? AND status = 'waiting' \
         ORDER BY position ASC LIMIT 1",
    )
    .bind(hook_slug)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(id, hook_slug, execution_id, position)| QueueEntry {
        id,
        hook_slug,
        execution_id,
        position,
    }))
}

/// Transition a queue entry from 'waiting' to 'ready'.
pub async fn mark_ready(pool: &SqlitePool, queue_entry_id: &str) -> DbResult<()> {
    sqlx::query(
        "UPDATE execution_queue SET status = 'ready' WHERE id = ? AND status = 'waiting'",
    )
    .bind(queue_entry_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Count waiting entries for a hook.
pub async fn count_waiting(pool: &SqlitePool, hook_slug: &str) -> DbResult<i64> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM execution_queue WHERE hook_slug = ? AND status = 'waiting'",
    )
    .bind(hook_slug)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Expire the queue entry for a specific execution.
pub async fn expire_for_execution(pool: &SqlitePool, execution_id: &str) -> DbResult<()> {
    sqlx::query(
        "UPDATE execution_queue SET status = 'expired' \
         WHERE execution_id = ? AND status = 'waiting'",
    )
    .bind(execution_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::models::execution::{self, NewExecution};

    async fn test_pool() -> SqlitePool {
        let db = Db::new_in_memory().await.unwrap();
        db.migrate().await.unwrap();
        db.pool().clone()
    }

    /// Create a minimal execution record so the queue FK is satisfied.
    async fn create_exec(pool: &SqlitePool, id: &str) -> String {
        let exec = execution::create(
            pool,
            &NewExecution {
                id: Some(id),
                hook_slug: "test-hook",
                log_path: "data/logs/test",
                trigger_source: "127.0.0.1",
                request_payload: "{}",
                retry_of: None,
                status: None,
            },
        )
        .await
        .unwrap();
        exec.id
    }

    #[tokio::test]
    async fn enqueue_assigns_sequential_positions() {
        let pool = test_pool().await;
        let e1 = create_exec(&pool, "exec-1").await;
        let e2 = create_exec(&pool, "exec-2").await;
        let e3 = create_exec(&pool, "exec-3").await;

        let p1 = enqueue(&pool, "test-hook", &e1).await.unwrap();
        let p2 = enqueue(&pool, "test-hook", &e2).await.unwrap();
        let p3 = enqueue(&pool, "test-hook", &e3).await.unwrap();

        assert_eq!(p1, 1);
        assert_eq!(p2, 2);
        assert_eq!(p3, 3);
    }

    #[tokio::test]
    async fn peek_returns_oldest_waiting() {
        let pool = test_pool().await;
        let e1 = create_exec(&pool, "exec-1").await;
        let e2 = create_exec(&pool, "exec-2").await;

        enqueue(&pool, "test-hook", &e1).await.unwrap();
        enqueue(&pool, "test-hook", &e2).await.unwrap();

        let next = peek_next(&pool, "test-hook").await.unwrap().unwrap();
        assert_eq!(next.execution_id, e1);
        assert_eq!(next.position, 1);
    }

    #[tokio::test]
    async fn peek_does_not_change_status() {
        let pool = test_pool().await;
        let e1 = create_exec(&pool, "exec-1").await;
        enqueue(&pool, "test-hook", &e1).await.unwrap();

        // Peek twice -- should return the same entry both times
        let first = peek_next(&pool, "test-hook").await.unwrap().unwrap();
        let second = peek_next(&pool, "test-hook").await.unwrap().unwrap();
        assert_eq!(first.execution_id, second.execution_id);
        assert_eq!(count_waiting(&pool, "test-hook").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn mark_ready_transitions_status() {
        let pool = test_pool().await;
        let e1 = create_exec(&pool, "exec-1").await;
        let e2 = create_exec(&pool, "exec-2").await;

        enqueue(&pool, "test-hook", &e1).await.unwrap();
        enqueue(&pool, "test-hook", &e2).await.unwrap();

        let first = peek_next(&pool, "test-hook").await.unwrap().unwrap();
        mark_ready(&pool, &first.id).await.unwrap();

        // Next peek should return the second entry
        let next = peek_next(&pool, "test-hook").await.unwrap().unwrap();
        assert_eq!(next.execution_id, e2);
    }

    #[tokio::test]
    async fn peek_returns_none_when_empty() {
        let pool = test_pool().await;
        assert!(peek_next(&pool, "test-hook").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn count_waiting_accurate() {
        let pool = test_pool().await;
        let e1 = create_exec(&pool, "exec-1").await;
        let e2 = create_exec(&pool, "exec-2").await;
        let e3 = create_exec(&pool, "exec-3").await;

        enqueue(&pool, "test-hook", &e1).await.unwrap();
        enqueue(&pool, "test-hook", &e2).await.unwrap();
        enqueue(&pool, "test-hook", &e3).await.unwrap();

        assert_eq!(count_waiting(&pool, "test-hook").await.unwrap(), 3);

        let first = peek_next(&pool, "test-hook").await.unwrap().unwrap();
        mark_ready(&pool, &first.id).await.unwrap();

        assert_eq!(count_waiting(&pool, "test-hook").await.unwrap(), 2);
    }

    #[tokio::test]
    async fn expire_for_execution_marks_expired() {
        let pool = test_pool().await;
        let e1 = create_exec(&pool, "exec-1").await;
        enqueue(&pool, "test-hook", &e1).await.unwrap();

        expire_for_execution(&pool, &e1).await.unwrap();
        assert_eq!(count_waiting(&pool, "test-hook").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn different_hooks_independent() {
        let pool = test_pool().await;
        let e1 = create_exec(&pool, "exec-1").await;
        let e2 = create_exec(&pool, "exec-2").await;

        enqueue(&pool, "hook-a", &e1).await.unwrap();
        enqueue(&pool, "hook-b", &e2).await.unwrap();

        assert_eq!(count_waiting(&pool, "hook-a").await.unwrap(), 1);
        assert_eq!(count_waiting(&pool, "hook-b").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn concurrent_enqueue_positions_unique() {
        use std::collections::HashSet;

        let pool = test_pool().await;

        // Pre-create execution records to satisfy FK constraints
        for i in 1..=5u32 {
            create_exec(&pool, &format!("exec-{i}")).await;
        }

        // Spawn 5 concurrent enqueue tasks
        let handles: Vec<_> = (1..=5u32)
            .map(|i| {
                let pool = pool.clone();
                tokio::spawn(async move {
                    enqueue(&pool, "test-hook", &format!("exec-{i}")).await.unwrap()
                })
            })
            .collect();

        let mut positions: Vec<i64> = Vec::with_capacity(5);
        for h in handles {
            positions.push(h.await.expect("task panicked"));
        }

        // All positions must be unique and in range 1..=5
        let unique: HashSet<i64> = positions.iter().copied().collect();
        assert_eq!(unique.len(), 5, "positions must be unique: {positions:?}");
        assert!(
            unique.iter().all(|&p| (1..=5).contains(&p)),
            "positions out of range: {positions:?}"
        );
    }
}
