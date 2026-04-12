# M5: Execution Barriers Design Spec

Adds concurrency control (mutex and queue modes) and approval gates to the webhook trigger pipeline. These are execution barriers -- they sit after trigger rule evaluation and control whether, when, and how an execution proceeds.

## Motivation

M4 trigger rules determine *whether* a webhook should fire. Execution barriers determine *how* it proceeds once accepted. A deploy hook should not run two deploys concurrently (mutex). A CI hook might queue builds rather than rejecting them (queue). A production deploy might require human approval before running (approval gate). These are distinct from trigger rules: a request passes all filters and rate limits, but still needs to be serialized, queued, or gated.

## Pipeline Position

```
Auth → Payload validation → Payload filters → Time window → Cooldown → Rate limit → Concurrency check → Approval gate → Execute
```

Barriers come after all trigger rule evaluators. A request that passes rate limiting is a valid, accepted request. Barriers control the execution timing and ordering, not the accept/reject decision.

## Barrier Outcomes

Trigger rule evaluators (M4) are binary: allow or reject. Barriers have three outcomes:

```rust
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

- **Proceed** -- create execution record, spawn it immediately (current behavior).
- **Reject** -- no execution record. Log trigger attempt with `concurrency_rejected`. Return 503.
- **Defer** -- execution record is created with a non-running status (`pending_approval`, or `pending` in queue). Return 202 with the execution ID. The execution will proceed later when conditions are met.

## Execution ID Pre-generation

**Important structural change from current code:** Today, `trigger_hook` generates the execution ID and creates the execution record *after* all checks pass. With barriers, the execution ID must be pre-generated *before* barrier evaluation in some cases (queue mode needs to create a record and enqueue it). The pipeline is restructured:

```
1. Generate exec_id + log_path (always, unconditionally)
2. Evaluate trigger rules (no DB write)
3. Evaluate concurrency barrier:
   - Reject → return 503, no DB write
   - Defer (queue) → create execution record (pending), enqueue, return 202
   - Proceed → continue
4. Evaluate approval barrier:
   - Defer → create execution record (pending_approval), return 202
   - Proceed → continue
5. Create execution record (pending/running), spawn
```

This means `execution::create` may be called inside the barrier logic (for deferred cases) or at the normal point (for the proceed path). The execution ID is always pre-generated at step 1 so it can be passed to DB functions in any step.

## 1. Concurrency Control

### Purpose

Control how many executions of the same hook can run simultaneously. Two modes: mutex (one at a time) and queue (one at a time, but queue overflow instead of rejecting).

### Config

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ConcurrencyConfig {
    /// Concurrency control mode.
    pub mode: ConcurrencyMode,
    /// Maximum queue depth (only for queue mode). Default: 10.
    #[serde(default = "default_queue_depth")]
    pub queue_depth: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyMode {
    /// One execution at a time. New triggers while one is running are rejected.
    Mutex,
    /// One execution at a time. New triggers are queued up to queue_depth.
    Queue,
}

fn default_queue_depth() -> u32 {
    10
}
```

### TOML config example

```toml
[[hooks]]
name = "deploy-app"
slug = "deploy-app"

[hooks.concurrency]
mode = "mutex"

[[hooks]]
name = "build-ci"
slug = "build-ci"

[hooks.concurrency]
mode = "queue"
queue_depth = 20
```

### HookConfig change

```rust
pub struct HookConfig {
    // ... existing fields ...
    pub concurrency: Option<ConcurrencyConfig>,
    pub approval: Option<ApprovalConfig>,
}
```

Concurrency and approval are top-level hook fields, not nested under `trigger_rules`. They are execution barriers, not trigger rules.

### Database schema: execution_locks

```sql
CREATE TABLE execution_locks (
    hook_slug    TEXT PRIMARY KEY NOT NULL,
    execution_id TEXT NOT NULL REFERENCES executions(id),
    acquired_at  TEXT NOT NULL
);
```

One row per hook. `PRIMARY KEY` on `hook_slug` enforces the mutex invariant -- at most one active lock per hook. SQLite's write serialization means concurrent `INSERT` attempts are safe: one succeeds, the rest get `UNIQUE constraint failed`.

### Database schema: execution_queue

```sql
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

### Lock lifecycle

**Acquiring the lock:**

```rust
pub async fn try_acquire(pool: &SqlitePool, hook_slug: &str, execution_id: &str) -> DbResult<bool>
```

Attempts `INSERT INTO execution_locks`. Returns `true` if the row was inserted (lock acquired), `false` if it already exists (another execution holds the lock).

**Releasing the lock:**

```rust
pub async fn release(pool: &SqlitePool, hook_slug: &str) -> DbResult<()>
```

`DELETE FROM execution_locks WHERE hook_slug = ?`. Called when an execution reaches a terminal state.

**The lock is held through retries.** When `run_with_retries` retries a failed command, the lock remains held. Release happens only when the entire retry sequence completes (success, final failure, or timeout).

### Mutex mode evaluation

1. Attempt `try_acquire(pool, hook_slug, exec_id)`.
2. If acquired: return `Proceed`. Execution record is created in the normal path and spawned.
3. If not acquired: return `Reject { status: ConcurrencyRejected, reason: "another execution is in progress" }`. No execution record created.

### Queue mode evaluation

1. Attempt `try_acquire(pool, hook_slug, exec_id)`.
2. If acquired: return `Proceed`. Execution record is created in the normal path and spawned.
3. If not acquired:
   a. Count `waiting` entries for this hook in `execution_queue`.
   b. If count >= `queue_depth`: return `Reject { status: ConcurrencyRejected, reason: "queue full ({count}/{queue_depth})" }`.
   c. Otherwise: create execution record (status `pending`), insert into queue with next position, return `Defer { execution_id: exec_id, status: Pending, reason: "queued at position {n}" }`.

Note: in case 3c, `execution::create` is called inside the barrier logic, not at the normal pipeline step. The pre-generated `exec_id` is used.

### Queue processing (dequeue on completion)

When an execution completes and its lock is released, the completion path checks for queued items:

```rust
pub async fn on_execution_complete(pool: &SqlitePool, hook_slug: &str, concurrency: &ConcurrencyConfig) {
    // 1. Release the lock
    execution_lock::release(pool, hook_slug).await;

    // 2. Dequeue next waiting item (if any)
    if let Some(queued) = execution_queue::dequeue_next(pool, hook_slug).await {
        // 3. Check if approval is also needed for this hook
        //    This requires access to the current hook config. Pass it in or
        //    look it up from state. If approval required, move execution to
        //    pending_approval. Otherwise acquire lock and spawn execution.
    }
}
```

`dequeue_next` finds the oldest `waiting` entry, marks it as `ready`, and returns it. This is event-driven, not polled -- the completion of one execution triggers the next.

```rust
/// Dequeue the next waiting item for a hook. Returns None if the queue is empty.
pub async fn dequeue_next(pool: &SqlitePool, hook_slug: &str) -> DbResult<Option<QueueEntry>> {
    // Within a single query: find the oldest waiting entry, mark it as ready
    // SQLite write serialization ensures only one caller can dequeue
}
```

**Note:** `on_execution_complete` needs access to the hook's approval config to decide whether to prompt for approval when dequeuing. The spawned task must capture both `concurrency_config` and `approval_config` from the config snapshot at trigger time. If the config is reloaded between trigger and completion, the captured snapshot is used -- matching how M4 handles config snapshots.

### Queue position tracking

Position is assigned at enqueue time as `MAX(position) + 1` for the hook. When an item is dequeued, its position is not rewritten -- positions are monotonically increasing, not compacted. The queue is ordered by `position ASC` for dequeue priority.

## 2. Approval Gates

### Purpose

Require human approval before an execution runs. The execution record is created and visible in the UI, but the command does not execute until someone approves it.

### Config

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ApprovalConfig {
    /// Whether approval is required for this hook.
    pub required: bool,
    /// How long to wait for approval before auto-expiring.
    /// If None, pending approvals never expire.
    #[serde(default, with = "humantime_serde::option")]
    pub timeout: Option<Duration>,
}
```

### TOML config example

```toml
[[hooks]]
name = "deploy-prod"
slug = "deploy-prod"

[hooks.approval]
required = true
timeout = "1h"
```

### Approval flow

When a hook with `approval.required = true` is triggered (and passes all trigger rules and concurrency checks):

1. Create execution record with status `pending_approval` (using the pre-generated `exec_id`).
2. Log trigger attempt with status `PendingApproval` and execution ID.
3. Return 202 with `{ "execution_id": "...", "status": "pending_approval" }`.
4. The command does **not** run.

The execution appears in the UI on a "Pending Approvals" page. A user can:
- **Approve** -- execution proceeds to `approved` (transient), then `pending`, then `running`.
- **Reject** -- execution marked `rejected`, command never runs.

### New execution DB functions needed

The current `execution.rs` handles `pending → running → terminal`. New functions needed for barriers:

```rust
/// Create an execution with an explicit status (for pending_approval and queued cases).
pub async fn create_with_status(pool: &SqlitePool, new: &NewExecution<'_>, status: ExecutionStatus) -> DbResult<Execution>

/// Transition pending_approval → approved, recording who approved and when.
pub async fn mark_approved(pool: &SqlitePool, id: &str, approved_by: &str) -> DbResult<Execution>

/// Transition pending_approval → rejected.
pub async fn mark_rejected(pool: &SqlitePool, id: &str, rejected_by: &str) -> DbResult<Execution>

/// Transition pending_approval → expired (for timeout sweep).
pub async fn mark_expired(pool: &SqlitePool, id: &str) -> DbResult<Execution>
```

The existing `approved_at` and `approved_by` columns on `executions` are already present in the migration (no schema changes needed).

### Approval API endpoints

```
POST /executions/:id/approve    (requires AuthUser)
POST /executions/:id/reject     (requires AuthUser)
```

**Approve handler:**

1. Verify execution exists and status is `pending_approval`.
2. Call `execution::mark_approved(pool, &id, &user.username)`.
3. If concurrency is configured for this hook:
   a. Attempt to acquire the lock.
   b. If acquired: transition to `pending`, then spawn the execution.
   c. If not acquired (another execution started while awaiting approval): re-enqueue with a `waiting` status in the execution queue. The execution will proceed when the lock is next available.
4. If no concurrency: transition to `pending`, spawn the execution.
5. Return 200 with execution details.

**Reject handler:**

1. Verify execution exists and status is `pending_approval`.
2. Call `execution::mark_rejected(pool, &id, &user.username)`.
3. Return 200 with execution details.

Both handlers use conditional updates (`WHERE status = 'pending_approval'`). If two users act simultaneously, only one succeeds; the other sees no rows affected and returns 409 Conflict.

### Approval timeout / auto-expiry

If `approval.timeout` is configured, executions that sit in `pending_approval` longer than the timeout are automatically expired:

- A background task (separate sweep, or combined with session sweep) scans for `pending_approval` executions where `triggered_at + timeout < now()`.
- Expired executions are transitioned to status `expired` via `execution::mark_expired`.
- If the execution was also in the queue, the queue entry is marked `expired`.

The sweep interval should be 1 minute (or configurable). This is acceptable latency for an expiry mechanism.

### Execution status lifecycle with barriers

```
pending ──────────────────────────────────────→ running → success/failed/timed_out
    │
    │  (approval required)
    └──→ pending_approval ──→ approved ──→ pending ──→ running → ...
              │
              ├──→ rejected
              └──→ expired
```

The `approved` status is transient -- after approval, the execution transitions to `pending` before being spawned. This reuses the existing `pending → running` transition in `executor::run` without modification.

Note: `pending_approval`, `approved`, `rejected`, and `expired` statuses are already defined in `ExecutionStatus` and in the `executions` table `CHECK` constraint. No schema changes needed.

## Interaction Between Concurrency and Approval

When a hook has both `concurrency` and `approval` configured, the barriers interact:

### Case 1: Lock available, approval required

1. Acquire lock (using pre-generated `exec_id`).
2. Create execution with status `pending_approval`.
3. Lock is **held** while awaiting approval. This prevents another execution from starting while one is pending approval.
4. On approve: transition to `approved` → `pending`, spawn execution.
5. On reject/expire: release lock, dequeue next if in queue mode.

### Case 2: Lock unavailable (queue mode), approval required

1. Cannot acquire lock.
2. Create execution with status `pending` (not `pending_approval` yet).
3. Enqueue the execution.
4. When it reaches the front of the queue and the lock is acquired, check if approval is needed:
   - If yes: move execution to `pending_approval`. Lock is held.
   - If no: execute immediately.

### Case 3: Lock unavailable (mutex mode), approval required

1. Cannot acquire lock.
2. Reject with `concurrency_rejected`. No execution record created.
3. Approval is never reached.

This means: in mutex mode, concurrency rejection takes priority over approval. In queue mode, the execution is queued first, then approval is checked when it is next to run.

## HTTP Response Contract Changes

The current `trigger_hook` always returns 200 with `{ "execution_id": "..." }`. With barriers, the response depends on the outcome:

| Outcome | HTTP Status | Body |
|---------|-------------|------|
| Normal execution | 200 | `{ "execution_id": "..." }` |
| Queued | 202 | `{ "execution_id": "...", "status": "queued", "position": N }` |
| Pending approval | 202 | `{ "execution_id": "...", "status": "pending_approval" }` |
| Concurrency rejected | 503 | `{ "status": "concurrency_rejected", "reason": "..." }` |

The 200 case is unchanged. 202 indicates the request was accepted but execution is deferred. 503 indicates temporary unavailability (the caller may retry later).

## Database Migration

One new migration: `20260412000005_create_execution_barriers.sql`

```sql
CREATE TABLE execution_locks (
    hook_slug    TEXT PRIMARY KEY NOT NULL,
    execution_id TEXT NOT NULL REFERENCES executions(id),
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

No changes to the existing `executions` table -- the `approved_at`, `approved_by`, and `pending_approval`/`approved`/`rejected`/`expired` statuses are already present in the migration `20260407000001_create_executions.sql`.

## New Source Files

```
src/
  barriers/
    mod.rs              -- BarrierOutcome type, re-exports
    execution_lock.rs   -- lock acquire/release + tests
    execution_queue.rs  -- queue enqueue/dequeue/expire + tests
    concurrency.rs      -- concurrency evaluator (mutex + queue) + tests
    approval.rs         -- approval evaluation logic + tests
```

Added to `lib.rs`:

```rust
pub mod barriers;
```

### Approval route additions

In `src/routes/executions.rs`, add:

```rust
POST /executions/:id/approve   -- approve_execution()
POST /executions/:id/reject    -- reject_execution()
```

### Pending approvals page

New route in `src/routes/executions.rs`:

```rust
GET /approvals                 -- list_pending_approvals()
```

Lists all executions with status `pending_approval`, most recent first.

## Pipeline Integration

In `trigger_hook()`, the execution ID is pre-generated before barrier evaluation. The restructured flow:

```rust
// After: trigger rules (payload filter, time window, cooldown, rate limit)

// Pre-generate execution ID (needed by barrier logic before DB write)
let exec_id = crate::id::new_id();
let log_path = format!("{logs_dir}/{exec_id}");
let new_exec_template = execution::NewExecution {
    id: Some(&exec_id),
    hook_slug: &slug,
    log_path: &log_path,
    trigger_source: &source_ip,
    request_payload: &payload_str,
    retry_of: None,
};

// Concurrency check
if let Some(concurrency) = &hook.concurrency {
    match concurrency::evaluate(pool, &slug, &exec_id, concurrency, &new_exec_template).await {
        BarrierOutcome::Proceed => {
            // Lock acquired; fall through to execution creation + spawn
        }
        BarrierOutcome::Reject { status, reason } => {
            let _ = trigger_attempt::insert(pool, &NewTriggerAttempt {
                hook_slug: &slug,
                source_ip: &source_ip,
                status,
                reason: &reason,
                execution_id: None,
            }).await;
            return Err(StatusCode::SERVICE_UNAVAILABLE.into_response());
        }
        BarrierOutcome::Defer { execution_id, status, reason } => {
            // Execution already created by barrier (queue case)
            let _ = trigger_attempt::insert(pool, &NewTriggerAttempt {
                hook_slug: &slug,
                source_ip: &source_ip,
                status: TriggerAttemptStatus::Fired,
                reason: &reason,
                execution_id: Some(&execution_id),
            }).await;
            return Ok(Json(/* 202 response with execution_id, status, position */));
        }
    }
}

// Approval check (when lock was acquired or no concurrency configured)
if let Some(approval) = &hook.approval {
    if approval.required {
        let exec = execution::create_with_status(
            pool, &new_exec_template, ExecutionStatus::PendingApproval,
        ).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;

        let _ = trigger_attempt::insert(pool, &NewTriggerAttempt {
            hook_slug: &slug,
            source_ip: &source_ip,
            status: TriggerAttemptStatus::PendingApproval,
            reason: "pending approval",
            execution_id: Some(&exec.id),
        }).await;
        return Ok(Json(/* 202 response with execution_id, pending_approval */));
    }
}

// Normal path: create execution and spawn
let exec = execution::create(pool, &new_exec_template).await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
// ... spawn task ...
```

### Execution completion hook

After `run_with_retries` completes, the spawned task must handle barrier cleanup. The task captures `concurrency_config` and `approval_config` by value from the config snapshot at trigger time:

```rust
tokio::spawn(async move {
    let result = retry::run_with_retries(&pool, ctx, &retry_config).await;

    // Release lock and process queue (if concurrency is configured)
    if let Some(ref concurrency_config) = concurrency_config {
        barriers::on_execution_complete(
            &pool, &hook_slug, concurrency_config, approval_config.as_ref(),
        ).await;
    }

    tracing::info!(/* ... */);
});
```

This is the dequeue trigger point -- event-driven, not polled. Config changes after trigger time do not affect in-flight executions.

## Recovery on Server Restart

Stale state can accumulate if the server crashes mid-execution:

### Orphaned locks

A lock row exists but the referenced execution is in a terminal state (server crashed after execution completed but before lock release). On startup:

```sql
DELETE FROM execution_locks
WHERE execution_id IN (
    SELECT id FROM executions
    WHERE status IN ('success', 'failed', 'timed_out', 'rejected', 'expired')
);
```

### Stuck running executions

Executions with status `running` after a crash will never complete. On startup, mark them as `failed`:

```sql
UPDATE executions SET status = 'failed', completed_at = ?
WHERE status = 'running';
```

### Stuck approved executions

Executions with status `approved` were awaiting lock acquisition when the server crashed. On startup, if no concurrency is configured, spawn them directly. If concurrency is configured, re-attempt lock acquisition and either spawn or re-enqueue. In practice, treating `approved` as needing re-evaluation on startup is safest:

```sql
-- No automatic status change; let on_startup_recover handle them case-by-case
```

### Expired approvals

Scan for `pending_approval` executions past their timeout and expire them. This is also handled by the periodic sweep task, but running it on startup catches any that expired during downtime.

### Stale queue entries

Queue entries referencing executions in terminal states should be cleaned up:

```sql
UPDATE execution_queue SET status = 'expired'
WHERE status = 'waiting'
AND execution_id IN (
    SELECT id FROM executions
    WHERE status IN ('rejected', 'expired')
);
```

After cleanup, process any unblocked queue entries (dequeue and execute/gate).

All recovery logic runs in a `recover_barriers()` function called during server startup, after migrations but before accepting requests.

## Edge Cases

### Approval rejected while in queue

If a user rejects an execution that is in the queue (status `pending`, in `execution_queue` with `waiting`), the queue entry is marked `expired` and the execution is marked `rejected`. The queue advances to the next item when the current execution completes.

### Lock held during approval

When concurrency + approval are both configured, the lock is held while awaiting approval. This is intentional -- it prevents a race where two executions are approved simultaneously. The tradeoff is that the queue is blocked while waiting for human action. The approval timeout mitigates indefinite blocking.

### Queue overflow during approval wait

If the lock is held for approval and the queue fills up, new requests are rejected with `concurrency_rejected`. The queue depth includes the pending-approval execution in its count (it holds the lock, not a queue slot).

### Concurrent approve/reject

Two users clicking approve and reject simultaneously. Both handlers use a conditional update (`WHERE status = 'pending_approval'`), so only one succeeds. The other sees zero rows affected and returns 409 Conflict.

### Config change removes concurrency

If a hook previously had concurrency configured and the config is reloaded without it, existing locks and queue entries become orphaned. The recovery logic on startup handles this. Mid-operation, in-flight executions complete normally and the lock is released -- the dequeue path checks if concurrency is still configured before processing the queue (using the captured config snapshot, not the reloaded config).

### Replay of a gated execution

When replaying an execution (`POST /executions/:id/replay`) for a hook with barriers, the replay goes through the full pipeline including concurrency and approval checks. It is not auto-approved.

### No `mark_running` for deferred executions

The current `trigger_hook` calls `execution::create` (which sets status `pending`) then the executor calls `mark_running`. Deferred executions (queued or pending approval) are created with `pending` or `pending_approval` directly and are only `mark_running`'d when actually spawned. The executor's `run()` function must not be called until the execution is truly dequeued/approved.

## Non-Goals

- **Distributed locking** -- sendword is single-instance. SQLite write serialization is the concurrency primitive. No distributed lock manager.
- **Queue priority** -- all queue entries are FIFO. No priority levels or queue reordering.
- **Partial approval** -- one approver, binary approve/reject. No multi-approver workflows, quorum, or escalation chains.
- **Queue introspection API** -- no API to inspect or reorder the queue. Queue state is visible via the UI only.
- **Notification on pending approval** -- the approval sits in the UI until acted on or expired. Integration with external notification (Slack, email) is a future feature.
- **Barrier-specific trigger attempt statuses** -- uses existing `ConcurrencyRejected` and `PendingApproval` statuses already defined in `TriggerAttemptStatus`. No new variants needed.
- **Web UI for editing barrier config** -- config editor updates are out of scope for this spec.

## Full TOML Config Example

```toml
[defaults]
timeout = "30s"

[[hooks]]
name = "Deploy Production"
slug = "deploy-prod"
description = "Deploys to production with approval gate and mutex"
enabled = true

[hooks.executor]
type = "shell"
command = "scripts/deploy-prod.sh --tag {{release.tag_name}}"

[hooks.auth]
mode = "hmac"
header = "X-Hub-Signature-256"
algorithm = "sha256"
secret = "${GITHUB_WEBHOOK_SECRET}"

[hooks.trigger_rules]
cooldown = "10m"
payload_filters = [
    { field = "action", operator = "equals", value = "released" },
]

[hooks.concurrency]
mode = "mutex"

[hooks.approval]
required = true
timeout = "1h"

[[hooks]]
name = "Build CI"
slug = "build-ci"
description = "Queued CI builds -- one at a time, overflow queued"

[hooks.executor]
type = "shell"
command = "scripts/build.sh"

[hooks.concurrency]
mode = "queue"
queue_depth = 20
```
