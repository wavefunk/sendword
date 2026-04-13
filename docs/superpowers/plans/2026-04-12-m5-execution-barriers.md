# M5: Execution Barriers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement concurrency control (mutex and queue modes) and approval gates for the webhook trigger pipeline, so that hooks can enforce one-at-a-time execution, queue overflow, and human approval before running commands.

**Architecture:** A new `barriers` module with a shared `BarrierOutcome` type and four submodules: execution locks, execution queue, concurrency evaluator, and approval logic. Barriers sit after M4 trigger rule evaluators in the pipeline and control execution timing/ordering. The pipeline is restructured to pre-generate execution IDs before barrier evaluation (barriers may need to create execution records for deferred cases).

**Design Spec:** `docs/superpowers/specs/2026-04-12-m5-execution-barriers-design.md`

**Dependency on M4:** This plan assumes M4 trigger rules are already wired into the pipeline. Barriers are evaluated after trigger rules pass.

**Parallelism:** Tasks 1 and 2 are sequential (2 depends on types/tables from 1). Tasks 3 and 4 are parallel (both depend on 1+2). Task 5 depends on 3+4.

---

## File Structure

### New files to create

| File | Purpose |
|------|---------|
| `migrations/20260412000005_create_execution_barriers.sql` | `execution_locks` and `execution_queue` tables |
| `src/barriers/mod.rs` | `BarrierOutcome` enum, `on_execution_complete()`, `recover_barriers()`, re-exports |
| `src/barriers/execution_lock.rs` | Lock acquire/release DB functions + tests |
| `src/barriers/execution_queue.rs` | Queue enqueue/dequeue/expire DB functions + tests |
| `src/barriers/concurrency.rs` | Concurrency evaluator (mutex + queue) + tests |
| `src/barriers/approval.rs` | Approval evaluation helper + expiry sweep + tests |
| `templates/approvals.html` | Pending approvals list page |

### Existing files to modify

| File | Change |
|------|--------|
| `src/lib.rs` | Add `pub mod barriers;` |
| `src/config.rs` | Add `ConcurrencyConfig`, `ConcurrencyMode`, `ApprovalConfig` types; add `concurrency` and `approval` fields to `HookConfig`; add config validation |
| `src/models/execution.rs` | Add `status` field to `NewExecution`; add `mark_approved()`, `mark_rejected()`, `mark_expired()`, `list_pending_approval()` functions |
| `src/routes/executions.rs` | Add `approve_execution()`, `reject_execution()`, `list_pending_approvals()` handlers + routes |
| `src/routes/hooks.rs` | Wire barriers into `trigger_hook()`; restructure execution creation; modify spawned task to handle lock release + dequeue |
| `src/tasks.rs` | Add approval expiry sweep |
| `src/main.rs` | Call `recover_barriers()` on startup |
| `tests/server_integration.rs` | Integration tests for barrier pipeline |

---

## Task 1: Config types + migration + execution DB functions

**Commit message:** `feat: add barrier config types, migration, and execution status functions`

**Files:**
- Create: `migrations/20260412000005_create_execution_barriers.sql`
- Modify: `src/config.rs`, `src/models/execution.rs`

**Steps:**

- [x] Create `migrations/20260412000005_create_execution_barriers.sql` — **ALREADY EXISTS**, matches spec exactly. Skip this step.

```sql
CREATE TABLE execution_locks (
    hook_slug    TEXT PRIMARY KEY NOT NULL,
    execution_id TEXT NOT NULL,
    acquired_at  TEXT NOT NULL
);

CREATE TABLE execution_queue (
    id           TEXT PRIMARY KEY NOT NULL,
    hook_slug    TEXT NOT NULL,
    execution_id TEXT NOT NULL REFERENCES executions(id),
    position     INTEGER NOT NULL,
    queued_at    TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'waiting'
                 CHECK (status IN ('waiting', 'ready', 'expired')),
    UNIQUE (hook_slug, position)
);

CREATE INDEX idx_execution_queue_hook_status ON execution_queue(hook_slug, status, position);
```

- [ ] In `src/config.rs`, add config types before `HookConfig`:

```rust
fn default_queue_depth() -> u32 {
    10
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyMode {
    Mutex,
    Queue,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConcurrencyConfig {
    pub mode: ConcurrencyMode,
    #[serde(default = "default_queue_depth")]
    pub queue_depth: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApprovalConfig {
    pub required: bool,
    #[serde(default, with = "humantime_serde::option")]
    pub timeout: Option<Duration>,
}
```

- [ ] In `src/config.rs`, add fields to `HookConfig`:

```rust
pub struct HookConfig {
    // ... existing fields ...
    #[serde(default)]
    pub concurrency: Option<ConcurrencyConfig>,
    #[serde(default)]
    pub approval: Option<ApprovalConfig>,
}
```

- [ ] In `src/config.rs`, add config validation in `AppConfig::validate()` inside the hook loop (after trigger_rules validation):

```rust
if let Some(concurrency) = &hook.concurrency {
    if concurrency.mode == ConcurrencyMode::Queue && concurrency.queue_depth == 0 {
        errors.push(format!(
            "{prefix}.concurrency.queue_depth must be greater than 0 in queue mode"
        ));
    }
}

if let Some(approval) = &hook.approval {
    if let Some(timeout) = approval.timeout {
        if timeout.is_zero() {
            errors.push(format!(
                "{prefix}.approval.timeout must be greater than 0 if set"
            ));
        }
    }
}
```

- [ ] In `src/models/execution.rs`, add an optional `status` field to `NewExecution`:

```rust
pub struct NewExecution<'a> {
    pub id: Option<&'a str>,
    pub hook_slug: &'a str,
    pub log_path: &'a str,
    pub trigger_source: &'a str,
    pub request_payload: &'a str,
    pub retry_of: Option<&'a str>,
    /// Optional initial status. Defaults to `pending` if None.
    pub status: Option<ExecutionStatus>,
}
```

Update `create()` to use `new.status.as_ref().unwrap_or(&ExecutionStatus::Pending).to_string()` instead of hardcoded `ExecutionStatus::Pending.to_string()`.

- [ ] Update all existing callers of `NewExecution` to include `status: None`. Grep for `NewExecution {` in `src/routes/hooks.rs` and `src/routes/executions.rs`.

- [ ] Add new execution DB functions in `src/models/execution.rs`:

```rust
/// Transition pending_approval → approved, recording who approved and when.
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

/// Transition pending_approval → rejected.
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

/// Transition pending_approval → expired (for timeout sweep).
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

/// Transition pending → pending_approval (for queued items that reach the front and need approval).
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
```

- [ ] Run `cargo check` to verify compilation.

**Tests** (in `execution.rs` test module):

- [ ] `mark_approved_transitions_status` -- create execution with `pending_approval`, approve it, verify status and `approved_by`
- [ ] `mark_approved_wrong_status_errors` -- create execution with `pending`, try to approve, expect `Conflict`
- [ ] `mark_rejected_transitions_status` -- create `pending_approval`, reject, verify `rejected` status
- [ ] `mark_expired_transitions_status` -- create `pending_approval`, expire, verify `expired` status
- [ ] `list_pending_approval_filters_correctly` -- create executions with various statuses, verify only `pending_approval` returned
- [ ] `create_with_status_pending_approval` -- create with explicit `pending_approval` status, verify

---

## Task 2: Lock and queue DB modules

**Commit message:** `feat: add execution lock and queue database modules`

**Depends on:** Task 1 (migration + types).

**Files:**
- Create: `src/barriers/mod.rs`, `src/barriers/execution_lock.rs`, `src/barriers/execution_queue.rs`
- Modify: `src/lib.rs`

**Steps:**

- [ ] In `src/lib.rs`, add `pub mod barriers;` (alphabetically, between `mod auth` and `mod config`).

- [ ] Create `src/barriers/mod.rs`:

```rust
pub mod execution_lock;
pub mod execution_queue;
pub mod concurrency;
pub mod approval;

use crate::models::trigger_attempt::TriggerAttemptStatus;
use crate::models::ExecutionStatus;

/// Outcome of an execution barrier check.
pub enum BarrierOutcome {
    /// Execution proceeds immediately.
    Proceed,
    /// Request is rejected -- no execution record created.
    Reject {
        status: TriggerAttemptStatus,
        reason: String,
    },
    /// Execution is deferred -- record created but not run yet.
    Defer {
        execution_id: String,
        status: ExecutionStatus,
        reason: String,
    },
}
```

Note: `concurrency.rs` and `approval.rs` are placeholders (empty or `// TODO`) until tasks 3 and 4.

- [ ] Create `src/barriers/execution_lock.rs`:

```rust
use sqlx::SqlitePool;
use crate::error::DbResult;
use crate::timestamp;

/// Attempt to acquire an execution lock for a hook.
/// Returns true if the lock was acquired, false if another execution holds it.
pub async fn try_acquire(pool: &SqlitePool, hook_slug: &str, execution_id: &str) -> DbResult<bool> {
    let acquired_at = timestamp::now_utc();
    let result = sqlx::query(
        "INSERT OR IGNORE INTO execution_locks (hook_slug, execution_id, acquired_at) VALUES (?, ?, ?)",
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

/// Get the execution_id currently holding the lock for a hook, if any.
pub async fn get_holder(pool: &SqlitePool, hook_slug: &str) -> DbResult<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT execution_id FROM execution_locks WHERE hook_slug = ?",
    )
    .bind(hook_slug)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}

/// Hand off the lock atomically to a new execution without releasing it.
/// This UPDATE replaces the holder in-place, preventing a race where a new
/// trigger steals the lock between a release and re-acquire.
pub async fn hand_off(pool: &SqlitePool, hook_slug: &str, next_execution_id: &str) -> DbResult<()> {
    let acquired_at = timestamp::now_utc();
    sqlx::query(
        "UPDATE execution_locks SET execution_id = ?, acquired_at = ? WHERE hook_slug = ?",
    )
    .bind(next_execution_id)
    .bind(&acquired_at)
    .bind(hook_slug)
    .execute(pool)
    .await?;
    Ok(())
}
```

**Important: No FK on `execution_locks.execution_id`.** In the Proceed path, `try_acquire` is called *before* `execution::create` (the lock must be claimed before the execution record exists). With `PRAGMA foreign_keys = ON` (set in `Db::new`), a `REFERENCES executions(id)` constraint would fail. The queue table retains its FK because the execution record is always created by barrier code before the queue row is inserted.

- [ ] Create `src/barriers/execution_queue.rs`:

```rust
use sqlx::SqlitePool;
use crate::error::{DbError, DbResult};
use crate::id;
use crate::timestamp;

/// A queue entry returned by dequeue operations.
pub struct QueueEntry {
    pub id: String,
    pub hook_slug: String,
    pub execution_id: String,
    pub position: i64,
}

/// Enqueue an execution for a hook. Returns the assigned position.
///
/// Uses an atomic INSERT-SELECT to compute the next position and insert
/// in a single statement, avoiding TOCTOU races under concurrent enqueuers.
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
    let row: (i64,) = sqlx::query_as(
        "SELECT position FROM execution_queue WHERE id = ?",
    )
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
    sqlx::query("UPDATE execution_queue SET status = 'ready' WHERE id = ? AND status = 'waiting'")
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
        "UPDATE execution_queue SET status = 'expired' WHERE execution_id = ? AND status = 'waiting'",
    )
    .bind(execution_id)
    .execute(pool)
    .await?;
    Ok(())
}
```

- [ ] Run `cargo check` to verify compilation.

**Tests** (in `execution_lock.rs`):

- [ ] `acquire_succeeds_when_no_lock` -- acquire returns true
- [ ] `acquire_fails_when_lock_held` -- acquire for same hook_slug returns false
- [ ] `release_allows_reacquire` -- acquire, release, acquire again succeeds
- [ ] `different_hooks_independent` -- lock on hook-a does not block hook-b
- [ ] `get_holder_returns_execution_id` -- acquire, get_holder returns correct ID
- [ ] `get_holder_returns_none_when_no_lock` -- no lock, returns None
- [ ] `hand_off_replaces_holder` -- acquire with exec-1, hand_off to exec-2, get_holder returns exec-2
- [ ] `hand_off_does_not_release_lock` -- acquire, hand_off, new try_acquire still fails (lock held)

**Tests** (in `execution_queue.rs`):

- [ ] `enqueue_assigns_sequential_positions` -- enqueue 3 items, positions are 1, 2, 3
- [ ] `peek_returns_oldest_waiting` -- enqueue 3, peek returns position 1
- [ ] `peek_does_not_change_status` -- peek, count_waiting unchanged
- [ ] `mark_ready_transitions_status` -- enqueue, mark_ready, peek returns next item (position 2)
- [ ] `peek_returns_none_when_empty` -- no items, returns None
- [ ] `count_waiting_accurate` -- enqueue 3, mark_ready 1, count is 2
- [ ] `expire_for_execution_marks_expired` -- enqueue, expire, count is 0
- [ ] `concurrent_enqueue_positions_unique` -- enqueue concurrently, all positions unique (no UNIQUE constraint violation)

All tests use `Db::new_in_memory()` with migrations.

---

## Task 3: Concurrency evaluator

**Commit message:** `feat: add concurrency evaluator with mutex and queue modes`

**Depends on:** Tasks 1 + 2.

**Files:**
- Modify: `src/barriers/concurrency.rs` (replace placeholder)

**Steps:**

- [ ] Implement `src/barriers/concurrency.rs`:

```rust
use sqlx::SqlitePool;
use crate::config::{ConcurrencyConfig, ConcurrencyMode};
use crate::models::{execution, trigger_attempt::TriggerAttemptStatus, ExecutionStatus};
use crate::barriers::{BarrierOutcome, execution_lock, execution_queue};

/// Evaluate concurrency barriers for a hook.
///
/// `exec_id`: pre-generated execution ID.
/// `new_exec`: template for creating the execution record (used only in queue defer case).
pub async fn evaluate(
    pool: &SqlitePool,
    hook_slug: &str,
    exec_id: &str,
    config: &ConcurrencyConfig,
    new_exec: &execution::NewExecution<'_>,
) -> BarrierOutcome
```

Implementation:
1. Attempt `execution_lock::try_acquire(pool, hook_slug, exec_id)`.
2. If acquired: return `BarrierOutcome::Proceed`.
3. If not acquired, branch on `config.mode`:
   - `Mutex`: return `BarrierOutcome::Reject { status: ConcurrencyRejected, reason: "another execution is in progress" }`.
   - `Queue`:
     a. Call `execution_queue::count_waiting(pool, hook_slug)`.
     b. If count >= `config.queue_depth as i64`: return `Reject { status: ConcurrencyRejected, reason: "queue full ({count}/{queue_depth})" }`.
     c. Create execution record: `execution::create(pool, new_exec)` (status defaults to `pending`).
     d. Enqueue: `execution_queue::enqueue(pool, hook_slug, exec_id)`.
     e. Return `Defer { execution_id: exec_id, status: Pending, reason: "queued at position {position}" }`.

- [ ] Run `cargo check` to verify compilation.

**Tests** (in `concurrency.rs`, `#[tokio::test]` with in-memory DB):

- [ ] `mutex_proceeds_when_no_lock` -- first request proceeds
- [ ] `mutex_rejects_when_lock_held` -- second request on same hook rejected
- [ ] `queue_proceeds_when_no_lock` -- first request proceeds
- [ ] `queue_defers_when_lock_held` -- second request queued, returns Defer with position
- [ ] `queue_rejects_when_full` -- queue_depth=2, lock held + 2 waiting, third rejected
- [ ] `different_hooks_independent_locks` -- lock on hook-a does not block hook-b

---

## Task 4: Approval logic + API endpoints + expiry sweep

**Commit message:** `feat: add approval gates with approve/reject API and expiry sweep`

**Depends on:** Tasks 1 + 2.

**Files:**
- Modify: `src/barriers/approval.rs` (replace placeholder), `src/routes/executions.rs`, `src/tasks.rs`
- Create: `templates/approvals.html`

**Steps:**

- [ ] Implement `src/barriers/approval.rs`:

```rust
/// Check if a hook requires approval and the execution should be deferred.
/// Returns true if approval is required, false otherwise.
pub fn requires_approval(approval: Option<&ApprovalConfig>) -> bool {
    approval.map_or(false, |a| a.required)
}
```

This is a simple helper. The actual approval deferral logic is in the pipeline wiring (task 5), and the approve/reject actions are in route handlers below.

- [ ] Add approval expiry sweep to `src/tasks.rs`. Create a new function:

```rust
// NOTE: Takes SqlitePool + Arc<AppState> (not Arc<ArcSwap<AppConfig>> directly).
// AppState.config is ArcSwap<AppConfig> (not Arc-wrapped), so pass the whole state.
// In main.rs, wrap AppState in Arc before spawning and before calling server::run.
pub fn spawn_approval_sweep(pool: SqlitePool, state: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            expire_pending_approvals(&pool, &state.config).await;
        }
    })
}

async fn expire_pending_approvals(pool: &SqlitePool, config: &ArcSwap<AppConfig>) {
    let config = config.load();
    let now = chrono::Utc::now();

    for hook in &config.hooks {
        let Some(approval) = &hook.approval else { continue };
        let Some(timeout) = approval.timeout else { continue };

        // Find pending_approval executions for this hook older than timeout
        let cutoff = (now - chrono::Duration::from_std(timeout).unwrap_or(chrono::Duration::zero()))
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();

        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT id FROM executions WHERE hook_slug = ? AND status = 'pending_approval' AND triggered_at < ?",
        )
        .bind(&hook.slug)
        .bind(&cutoff)
        .fetch_all(pool)
        .await;

        if let Ok(rows) = rows {
            for (id,) in rows {
                if let Err(e) = execution::mark_expired(pool, &id).await {
                    tracing::warn!(execution_id = %id, "failed to expire approval: {e}");
                } else {
                    tracing::info!(execution_id = %id, hook_slug = %hook.slug, "expired pending approval");
                    // Also expire any queue entry for this execution
                    let _ = execution_queue::expire_for_execution(pool, &id).await;
                    // If this execution held a lock, hand off to next queued item or release.
                    // This prevents queued items from being stuck indefinitely after an approval expires.
                    if let Ok(Some(holder)) = execution_lock::get_holder(pool, &hook.slug).await {
                        if holder == id {
                            // Peek for next queued item
                            if let Ok(Some(next)) = execution_queue::peek_next(pool, &hook.slug).await {
                                let _ = execution_lock::hand_off(pool, &hook.slug, &next.execution_id).await;
                                let _ = execution_queue::mark_ready(pool, &next.id).await;
                                // Note: the dequeued item may itself need approval.
                                // A full on_execution_complete call would handle this,
                                // but the sweep lacks AppState for spawning. Log and let
                                // the next trigger or sweep iteration pick it up.
                                tracing::info!(
                                    execution_id = %next.execution_id,
                                    "handed off lock to queued execution after approval expiry"
                                );
                            } else {
                                let _ = execution_lock::release(pool, &hook.slug).await;
                            }
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] In `src/routes/executions.rs`, add routes to the router:

```rust
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/executions/{id}", get(execution_detail))
        .route("/executions/{id}/replay", post(replay_execution))
        .route("/executions/{id}/approve", post(approve_execution))  // NEW
        .route("/executions/{id}/reject", post(reject_execution))    // NEW
        .route("/approvals", get(list_pending_approvals))            // NEW
}
```

- [ ] Implement `approve_execution` handler:

```rust
async fn approve_execution(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let pool = state.db.pool();

    // Mark approved
    let exec = execution::mark_approved(pool, &id, &_user.username)
        .await
        .map_err(|e| match e {
            DbError::Conflict(_) => StatusCode::CONFLICT.into_response(),
            _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        })?;

    // Look up hook config to check if concurrency is configured
    let config = state.config.load();
    let hook = config.hooks.iter().find(|h| h.slug == exec.hook_slug);

    if let Some(hook) = hook {
        // Transition approved → pending, then spawn execution
        // Reset status to pending so executor::run can transition pending → running
        let _ = sqlx::query("UPDATE executions SET status = 'pending' WHERE id = ? AND status = 'approved'")
            .bind(&id)
            .execute(pool)
            .await;

        // Build ExecutionContext and spawn (same logic as trigger_hook's spawn path)
        // ... reconstruct ctx from exec.request_payload + hook config ...
        // ... spawn task with lock release on completion ...
    }

    // Return redirect for HTMX, JSON for API
    // Check HX-Request header to decide response format
    Ok(Redirect::to(&format!("/executions/{id}")).into_response())
}
```

Note: The full spawn logic involves reconstructing `ExecutionContext` from the stored `request_payload` and current hook config. This is the same pattern as `replay_execution`. Extract a shared helper `spawn_execution_for_hook()` if the duplication is significant (task 5 will finalize this).

- [ ] Implement `reject_execution` handler:

```rust
async fn reject_execution(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let pool = state.db.pool();

    let exec = execution::mark_rejected(pool, &id, &_user.username)
        .await
        .map_err(|e| match e {
            DbError::Conflict(_) => StatusCode::CONFLICT.into_response(),
            _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        })?;

    // If this execution held a lock, hand off to next queued item or release
    let config = state.config.load();
    if let Some(hook) = config.hooks.iter().find(|h| h.slug == exec.hook_slug) {
        if let Ok(Some(holder)) = execution_lock::get_holder(pool, &exec.hook_slug).await {
            if holder == id {
                // Use the same on_execution_complete logic to hand off or release
                // state here is Arc<AppState> (from State(state): State<Arc<AppState>>)
                barriers::on_execution_complete(
                    &state, &exec.hook_slug,
                    hook.concurrency.as_ref(),
                    hook.approval.as_ref(),
                ).await;
                // Note: on_execution_complete already handles expire_for_execution internally
                // when it marks ready. The explicit expire_for_execution below handles the
                // rejected execution's own queue entry (not the dequeued next item).
            }
        }
        // Also expire any queue entry for this execution
        let _ = execution_queue::expire_for_execution(pool, &id).await;
    }

    Ok(Redirect::to(&format!("/executions/{id}")).into_response())
}
```

- [ ] Implement `list_pending_approvals` handler:

```rust
async fn list_pending_approvals(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
) -> Result<Html<String>, AppError> {
    let pool = state.db.pool();
    let executions = execution::list_pending_approval(pool).await?;

    state.templates.render("approvals.html", context! {
        executions => executions,
    })
}
```

- [ ] Create `templates/approvals.html` -- a minimal list page showing pending approval executions with Approve/Reject buttons (HTMX POST to the approve/reject endpoints). Extends `base.html`. List columns: hook slug, triggered at, trigger source, payload snippet. Two buttons per row.

- [ ] Run `cargo check` to verify compilation.

**Tests** (in `tests/server_integration.rs`):

- [ ] `approve_execution_transitions_and_runs` -- create hook with approval, trigger, verify pending_approval, approve via POST, verify execution runs
- [ ] `reject_execution_marks_rejected` -- trigger, reject, verify status is rejected and command never ran
- [ ] `approve_wrong_status_returns_409` -- try to approve a running execution, expect 409
- [ ] `pending_approvals_page_lists_pending` -- trigger a gated hook, GET /approvals, verify it appears

---

## Task 5: Pipeline wiring + completion hook + startup recovery

**Commit message:** `feat: wire execution barriers into trigger pipeline with dequeue and recovery`

**Depends on:** Tasks 3 + 4.

**Files:**
- Modify: `src/routes/hooks.rs`, `src/barriers/mod.rs`, `src/main.rs`
- Modify: `tests/server_integration.rs`

**Steps:**

- [ ] In `src/routes/hooks.rs`, restructure `trigger_hook()` to pre-generate execution ID before barriers:

Move the `exec_id` and `log_path` generation to before the barrier evaluation block. Create a `NewExecution` template that can be passed to barrier logic:

```rust
// Pre-generate execution ID (needed by barrier logic)
let exec_id = crate::id::new_id();
let log_path = format!("{logs_dir}/{exec_id}");
let new_exec = execution::NewExecution {
    id: Some(&exec_id),
    hook_slug: &slug,
    log_path: &log_path,
    trigger_source: &source_ip,
    request_payload: &payload_str,
    retry_of: None,
    status: None,
};
```

- [ ] Add concurrency barrier evaluation after trigger rules and before execution creation:

```rust
if let Some(concurrency) = &hook.concurrency {
    match barriers::concurrency::evaluate(pool, &slug, &exec_id, concurrency, &new_exec).await {
        BarrierOutcome::Proceed => {
            // Lock acquired, continue to approval check or execution
        }
        BarrierOutcome::Reject { status, reason } => {
            let _ = trigger_attempt::insert(pool, &NewTriggerAttempt {
                hook_slug: &slug,
                source_ip: &source_ip,
                status: status.clone(),
                reason: &reason,
                execution_id: None,
            }).await;
            return Err(StatusCode::SERVICE_UNAVAILABLE.into_response());
        }
        BarrierOutcome::Defer { execution_id, status, reason } => {
            let _ = trigger_attempt::insert(pool, &NewTriggerAttempt {
                hook_slug: &slug,
                source_ip: &source_ip,
                status: TriggerAttemptStatus::Fired,
                reason: &reason,
                execution_id: Some(&execution_id),
            }).await;
            return Ok(Json(serde_json::json!({
                "execution_id": execution_id,
                "status": "queued",
                "reason": reason,
            })).into_response());
        }
    }
}
```

- [ ] Add approval barrier after concurrency (for the Proceed path):

```rust
if barriers::approval::requires_approval(hook.approval.as_ref()) {
    // NOTE: NewExecution does not derive Clone. Use explicit struct rebuild, not ..new_exec spread.
    // new_exec was constructed earlier in this function; repeat all fields with status overridden.
    let exec = execution::create(pool, &execution::NewExecution {
        id: Some(&exec_id),
        hook_slug: &slug,
        log_path: &log_path,
        trigger_source: &source_ip,
        request_payload: &payload_str,
        retry_of: None,
        status: Some(ExecutionStatus::PendingApproval),
    }).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;

    let _ = trigger_attempt::insert(pool, &NewTriggerAttempt {
        hook_slug: &slug,
        source_ip: &source_ip,
        status: TriggerAttemptStatus::PendingApproval,
        reason: "pending approval",
        execution_id: Some(&exec.id),
    }).await;

    return Ok(Json(serde_json::json!({
        "execution_id": exec.id,
        "status": "pending_approval",
    })).into_response());
}
```

- [ ] Modify the spawned task to handle lock release on completion. Pass `Arc<AppState>` into the task:

```rust
// NOTE: trigger_hook receives State(state): State<Arc<AppState>>, so state is already Arc<AppState>.
// Clone it for the spawn task.
let state_clone = Arc::clone(&state);
let hook_slug_clone = slug.clone();
let concurrency_config = hook.concurrency.clone();
let approval_config = hook.approval.clone();

tokio::spawn(async move {
    let result = retry::run_with_retries(&pool, ctx, &retry_config).await;

    // Release lock and dequeue next if concurrency configured
    if concurrency_config.is_some() {
        barriers::on_execution_complete(
            &state_clone,
            &hook_slug_clone,
            concurrency_config.as_ref(),
            approval_config.as_ref(),
        ).await;
    }

    tracing::info!(
        log_dir = %result.log_dir,
        status = %result.status,
        exit_code = ?result.exit_code,
        "execution completed"
    );
});
```

- [ ] Implement `on_execution_complete` in `src/barriers/mod.rs`:

```rust
/// Called when an execution reaches a terminal state. Hands off the lock
/// to the next queued item (if any) or releases it.
///
/// Uses hand_off (UPDATE) instead of release+acquire to prevent a race
/// where a new trigger steals the lock between the two operations.
// NOTE: Takes Arc<AppState> so it can be cloned into the spawned dequeue task.
// All callers (spawn task in trigger_hook, reject handler, approval sweep) hold Arc<AppState>.
pub async fn on_execution_complete(
    state: &Arc<AppState>,
    hook_slug: &str,
    concurrency: Option<&ConcurrencyConfig>,
    approval: Option<&ApprovalConfig>,
) {
    let pool = state.db.pool();

    // Only process queue for queue mode
    let Some(config) = concurrency else {
        let _ = execution_lock::release(pool, hook_slug).await;
        return;
    };
    if config.mode != ConcurrencyMode::Queue {
        let _ = execution_lock::release(pool, hook_slug).await;
        return;
    }

    // 1. Peek at the queue (do NOT dequeue yet -- we need to hand off the lock first)
    let next = execution_queue::peek_next(pool, hook_slug).await.ok().flatten();

    match next {
        None => {
            // No queued items -- release the lock
            let _ = execution_lock::release(pool, hook_slug).await;
        }
        Some(queued) => {
            // 2. Hand off the lock atomically to the queued execution (UPDATE, not DELETE+INSERT)
            if let Err(e) = execution_lock::hand_off(pool, hook_slug, &queued.execution_id).await {
                tracing::warn!(hook_slug = %hook_slug, "failed to hand off lock: {e}");
                return;
            }

            // 3. Mark the queue entry as ready
            let _ = execution_queue::mark_ready(pool, &queued.id).await;

            // 4. Check if approval is needed
            if approval::requires_approval(approval) {
                if let Err(e) = execution::mark_pending_approval(pool, &queued.execution_id).await {
                    tracing::warn!(
                        execution_id = %queued.execution_id,
                        "failed to transition to pending_approval: {e}"
                    );
                }
                tracing::info!(execution_id = %queued.execution_id, "dequeued execution awaiting approval");
                return;
            }

            // 5. Spawn the execution
            let app_config = state.config.load();
            let Some(hook) = app_config.hooks.iter().find(|h| h.slug == hook_slug) else {
                tracing::warn!(hook_slug = %hook_slug, "hook not found in config after dequeue, releasing lock");
                let _ = execution_lock::release(pool, hook_slug).await;
                return;
            };

            // Reconstruct ExecutionContext from stored execution record + current hook config
            // Same pattern as replay_execution: fetch exec, parse request_payload, resolve command, spawn
            // ... fetch execution record, build ctx, spawn task with lock release on completion ...
        }
    }
}
```

The spawn-from-dequeue logic mirrors `replay_execution` in `executions.rs`. Extract a shared `spawn_execution_for_hook()` helper to avoid duplication between approve, dequeue, and replay paths.

- [ ] Implement `recover_barriers` in `src/barriers/mod.rs`:

```rust
/// Clean up stale barrier state on server startup.
/// Called after migrations, before accepting requests.
pub async fn recover_barriers(pool: &SqlitePool) {
    let now = crate::timestamp::now_utc();

    // 1. Mark stuck running executions as failed
    let result = sqlx::query(
        "UPDATE executions SET status = 'failed', completed_at = ? WHERE status = 'running'",
    )
    .bind(&now)
    .execute(pool)
    .await;
    if let Ok(r) = result {
        if r.rows_affected() > 0 {
            tracing::info!(count = r.rows_affected(), "recovered stuck running executions");
        }
    }

    // 2. Clean up orphaned locks (execution in terminal state)
    let result = sqlx::query(
        "DELETE FROM execution_locks WHERE execution_id IN \
         (SELECT id FROM executions WHERE status IN ('success', 'failed', 'timed_out', 'rejected', 'expired'))",
    )
    .execute(pool)
    .await;
    if let Ok(r) = result {
        if r.rows_affected() > 0 {
            tracing::info!(count = r.rows_affected(), "cleaned orphaned execution locks");
        }
    }

    // 3. Expire stale queue entries
    let result = sqlx::query(
        "UPDATE execution_queue SET status = 'expired' \
         WHERE status = 'waiting' AND execution_id IN \
         (SELECT id FROM executions WHERE status IN ('rejected', 'expired', 'failed'))",
    )
    .execute(pool)
    .await;
    if let Ok(r) = result {
        if r.rows_affected() > 0 {
            tracing::info!(count = r.rows_affected(), "expired stale queue entries");
        }
    }
}
```

- [ ] In `src/main.rs`, call `recover_barriers()` in `serve()` after `db.migrate()` and before `server::run()`:

```rust
sendword::barriers::recover_barriers(db.pool()).await;
tracing::info!("barrier recovery complete");
```

- [ ] In `src/main.rs`, wrap `AppState` in `Arc` before spawning the sweep (so both the sweep and `server::run` can share ownership):

```rust
// Wrap in Arc so we can clone for the sweep task
let state = Arc::new(sendword::server::AppState::new(config, "sendword.toml", db, templates));

let _approval_sweep = sendword::tasks::spawn_approval_sweep(
    state.db.pool().clone(),
    Arc::clone(&state),
);
tracing::info!("approval sweep task started");

sendword::server::run(state).await?;
```

Note: `server::run` already takes `Arc<AppState>`. Current `main.rs` passes `AppState` directly (implicit `Arc::from` coercion). The fix above makes the wrapping explicit so the sweep can clone the same `Arc`. No change to `server::run` signature needed.

- [ ] Run `cargo check` to verify compilation.
- [ ] Run `cargo test` to verify all existing tests still pass.

**Integration tests** (in `tests/server_integration.rs`):

- [ ] `mutex_blocks_concurrent_execution` -- trigger mutex hook, trigger again while running, second gets 503
- [ ] `queue_defers_and_processes` -- trigger queue hook twice, first runs, second queued (202), first completes, second auto-starts
- [ ] `approval_defers_execution` -- trigger approval hook, get 202 with pending_approval, POST approve, execution runs
- [ ] `approval_reject_prevents_execution` -- trigger, reject, verify execution never ran
- [ ] `mutex_plus_approval_holds_lock` -- trigger mutex+approval hook, verify lock held during approval, second trigger rejected
- [ ] `recovery_cleans_orphaned_locks` -- insert orphaned lock row, call recover_barriers, verify lock cleaned

---

## Verification

After all 5 tasks are complete:

- [ ] `cargo check` passes with no warnings
- [ ] `cargo test` passes -- all existing tests + new unit tests + new integration tests
- [ ] `cargo clippy` passes with no warnings
- [ ] Manual test: configure hooks with mutex, queue, and approval, verify correct behavior through the trigger pipeline
