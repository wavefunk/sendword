# M4: Trigger Rule Evaluators Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement four trigger rule evaluators (payload filter, time window, cooldown, rate limiter) and wire them into the webhook trigger pipeline, so that hooks can conditionally fire based on payload content, time-of-day, minimum intervals, and request rate.

**Architecture:** A new `trigger_rules` module with a shared `EvalOutcome` type and four evaluator submodules. Each evaluator is a free function returning `EvalOutcome::Allow` or `EvalOutcome::Reject`. The pipeline in `trigger_hook()` calls evaluators in order (payload filter -> time window -> cooldown -> rate limit), short-circuiting on the first rejection. Config types (`TriggerRules`, `PayloadFilter`, `TimeWindow`, `TriggerRateLimit`) are already implemented in `src/config.rs`.

**Design Spec:** `docs/superpowers/specs/2026-04-12-m4-trigger-rules-design.md`

**Parallelism:** Tasks 1-4 are independent and can be implemented in parallel. Task 5 depends on all four.

---

## File Structure

### New files to create

| File | Purpose |
|------|---------|
| `src/trigger_rules/mod.rs` | `EvalOutcome` enum, re-exports |
| `src/trigger_rules/payload_filter.rs` | Payload filter evaluator + tests |
| `src/trigger_rules/time_window.rs` | Time window evaluator + tests |
| `src/trigger_rules/cooldown.rs` | Cooldown evaluator + tests |
| `src/trigger_rules/rate_limit.rs` | Rate limiter evaluator + tests |
| `migrations/20260412000001_create_rate_limit_counters.sql` | Rate limit counter table |

### Existing files to modify

| File | Change |
|------|--------|
| `src/lib.rs` | Add `pub mod trigger_rules;` |
| `src/config.rs` | Add config validation for trigger rules fields in `AppConfig::validate()` |
| `src/models/execution.rs` | Add `get_latest_started_by_hook()` query for cooldown |
| `src/tasks.rs` | Add rate limit counter cleanup to session sweep |
| `src/routes/hooks.rs` | Wire evaluators into `trigger_hook()`, add `log_rejection()` helper |
| `tests/server_integration.rs` | Integration tests for trigger rule pipeline |

---

## Task 1: Module scaffold + payload filter evaluator

**Commit message:** `feat: add trigger_rules module with payload filter evaluator`

**Files:**
- Create: `src/trigger_rules/mod.rs`, `src/trigger_rules/payload_filter.rs`
- Modify: `src/lib.rs`, `src/config.rs`

**Steps:**

- [ ] In `src/lib.rs`, add `pub mod trigger_rules;` (alphabetically between `mod templates` and `mod timestamp`).

- [ ] Create `src/trigger_rules/mod.rs`:

```rust
pub mod payload_filter;
pub mod time_window;
pub mod cooldown;
pub mod rate_limit;

use crate::models::trigger_attempt::TriggerAttemptStatus;

/// Outcome of a trigger rule evaluation.
pub enum EvalOutcome {
    /// Request passes this evaluator.
    Allow,
    /// Request is rejected by this evaluator.
    Reject {
        status: TriggerAttemptStatus,
        reason: String,
    },
}
```

Note: `time_window`, `cooldown`, and `rate_limit` submodules will be empty initially (just `// TODO` or empty file). They will be implemented in tasks 2-4.

- [ ] Create `src/trigger_rules/payload_filter.rs` with:

```rust
pub fn evaluate(filters: &[PayloadFilter], payload: &serde_json::Value) -> EvalOutcome
```

Implementation:
1. Iterate over each `PayloadFilter`.
2. Resolve the field using `crate::payload::resolve_field(payload, &filter.field)`.
3. Match on `filter.operator`:
   - `Exists`: return `Reject` if field is `None` or `Null`.
   - `Equals`: compare the resolved field's JSON string representation with `filter.value`. For strings, compare the unquoted string value. For booleans/numbers, compare their `to_string()` form.
   - `NotEquals`: inverse of `Equals`.
   - `Contains`: if field is a string, check `str::contains`. If field is an array, iterate elements and check string representation match. Otherwise reject with type error reason.
   - `Regex`: compile pattern from `filter.value` using `regex::Regex::new()`. Match against field's string representation. Compilation is per-request (not cached -- `regex` crate is fast; see design decisions below).
   - `Gt`, `Lt`, `Gte`, `Lte`: parse both the field value and `filter.value` as `f64`. Compare. Reject with type error if either is non-numeric.
4. If any filter fails, return `EvalOutcome::Reject { status: TriggerAttemptStatus::Filtered, reason }` where `reason` describes which filter failed and why.
5. If all pass, return `EvalOutcome::Allow`.

- [ ] Add config validation for payload filter regex patterns in `AppConfig::validate()` (`src/config.rs`):

In the hook validation loop, after existing checks, add:

```rust
if let Some(rules) = &hook.trigger_rules {
    if let Some(filters) = &rules.payload_filters {
        for (j, filter) in filters.iter().enumerate() {
            if filter.operator == FilterOperator::Regex {
                if let Some(pattern) = &filter.value {
                    if regex::Regex::new(pattern).is_err() {
                        errors.push(format!(
                            "{prefix}.trigger_rules.payload_filters[{j}].value is not a valid regex"
                        ));
                    }
                } else {
                    errors.push(format!(
                        "{prefix}.trigger_rules.payload_filters[{j}].value is required for regex operator"
                    ));
                }
            }
        }
    }
}
```

- [ ] Run `cargo check` to verify compilation.

**Design decision -- regex compilation:** Validate regex patterns at config load time in `AppConfig::validate()` (compile to check validity, then discard). At evaluation time, compile fresh per-request. The `regex` crate compiles fast enough for this use case. Caching compiled regexes alongside the config would complicate the `ArcSwap<AppConfig>` and config reload path. If profiling shows this is a bottleneck, add `OnceLock`-based caching later.

**Tests** (in `payload_filter.rs`):

- [ ] `equals_string_field_passes` -- field "action" equals "released", payload has `{"action": "released"}`
- [ ] `equals_string_field_rejects` -- field "action" equals "released", payload has `{"action": "push"}`
- [ ] `equals_boolean_field` -- field "draft" equals "false", payload has `{"draft": false}`
- [ ] `equals_number_field` -- field "count" equals "42", payload has `{"count": 42}`
- [ ] `not_equals_passes` -- field "action" not_equals "push", payload has `{"action": "released"}`
- [ ] `not_equals_rejects` -- field "action" not_equals "push", payload has `{"action": "push"}`
- [ ] `contains_substring` -- field "message" contains "deploy", payload has `{"message": "deploy to prod"}`
- [ ] `contains_array_element` -- field "labels" contains "deploy", payload has `{"labels": ["deploy", "release"]}`
- [ ] `contains_rejects_on_type_mismatch` -- field is a number, contains operator rejects
- [ ] `regex_matches` -- field "branch" regex "^main$", payload has `{"branch": "main"}`
- [ ] `regex_no_match` -- field "branch" regex "^main$", payload has `{"branch": "develop"}`
- [ ] `exists_passes` -- field "action" exists, payload has `{"action": "any"}`
- [ ] `exists_rejects_missing` -- field "action" exists, payload is `{}`
- [ ] `exists_rejects_null` -- field "action" exists, payload has `{"action": null}`
- [ ] `gt_passes` -- field "count" gt "5", payload has `{"count": 10}`
- [ ] `gt_rejects` -- field "count" gt "5", payload has `{"count": 3}`
- [ ] `lt_gte_lte_basic` -- one test per numeric operator confirming pass/reject boundary
- [ ] `numeric_comparison_rejects_non_number` -- field is string, gt operator rejects with type error
- [ ] `dot_notation_nested_field` -- field "repo.name" equals "myapp", payload has `{"repo": {"name": "myapp"}}`
- [ ] `missing_field_with_equals_rejects` -- field is not in payload, equals operator rejects
- [ ] `multiple_filters_all_pass` -- two filters, both match, returns Allow
- [ ] `multiple_filters_first_fails` -- two filters, first fails, returns Reject (short-circuits)
- [ ] `empty_filters_allows` -- empty filter list, returns Allow

---

## Task 2: Time window evaluator

**Commit message:** `feat: add time window evaluator for trigger rules`

**Files:**
- Create: `src/trigger_rules/time_window.rs` (replace empty placeholder)
- Modify: `src/config.rs`

**Steps:**

- [ ] Implement `src/trigger_rules/time_window.rs`:

```rust
pub fn evaluate(windows: &[TimeWindow]) -> EvalOutcome
```

Implementation:
1. Get the current UTC time using `chrono::Utc::now()` (already a dependency).
2. For each `TimeWindow`:
   a. Parse `start_time` and `end_time` as `NaiveTime` (HH:MM format).
   b. Get the current UTC day-of-week as a 3-letter abbreviation (e.g., "Mon").
   c. If `days` is non-empty, check if the current day is in the list (case-insensitive comparison).
   d. If `days` is empty, all days match.
   e. Check if the current time is in `[start_time, end_time)`.
3. If any window matches, return `Allow`.
4. If no window matches, return `Reject { status: ScheduleSkipped, reason }`.

Provide a testable variant that accepts a `chrono::DateTime<Utc>` parameter so tests can control the clock:

```rust
pub fn evaluate(windows: &[TimeWindow]) -> EvalOutcome {
    evaluate_at(windows, chrono::Utc::now())
}

pub fn evaluate_at(windows: &[TimeWindow], now: chrono::DateTime<chrono::Utc>) -> EvalOutcome {
    // ... actual logic
}
```

- [ ] Add config validation for time windows in `AppConfig::validate()` (`src/config.rs`):

In the trigger_rules validation block (started in task 1), add:

```rust
if let Some(windows) = &rules.time_windows {
    for (j, window) in windows.iter().enumerate() {
        let prefix_w = format!("{prefix}.trigger_rules.time_windows[{j}]");
        // Validate start_time and end_time parse as HH:MM
        if chrono::NaiveTime::parse_from_str(&window.start_time, "%H:%M").is_err() {
            errors.push(format!("{prefix_w}.start_time must be HH:MM format"));
        }
        if chrono::NaiveTime::parse_from_str(&window.end_time, "%H:%M").is_err() {
            errors.push(format!("{prefix_w}.end_time must be HH:MM format"));
        }
        // Validate start_time < end_time (no overnight windows)
        if let (Ok(start), Ok(end)) = (
            chrono::NaiveTime::parse_from_str(&window.start_time, "%H:%M"),
            chrono::NaiveTime::parse_from_str(&window.end_time, "%H:%M"),
        ) {
            if start >= end {
                errors.push(format!("{prefix_w}.start_time must be before end_time"));
            }
        }
        // Validate day names
        const VALID_DAYS: &[&str] = &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        for day in &window.days {
            if !VALID_DAYS.iter().any(|d| d.eq_ignore_ascii_case(day)) {
                errors.push(format!(
                    "{prefix_w}.days contains invalid day '{day}' (expected Mon-Sun)"
                ));
            }
        }
    }
}
```

- [ ] Run `cargo check` to verify compilation.

**Tests** (in `time_window.rs`):

- [ ] `within_window_allows` -- Monday 10:00 UTC, window Mon-Fri 09:00-17:00
- [ ] `outside_window_rejects` -- Monday 20:00 UTC, window Mon-Fri 09:00-17:00
- [ ] `wrong_day_rejects` -- Saturday 10:00 UTC, window Mon-Fri 09:00-17:00
- [ ] `empty_days_matches_all` -- Tuesday 12:00, window with empty days 09:00-17:00
- [ ] `multiple_windows_or_logic` -- Saturday 11:00, weekday window rejects but Saturday window allows
- [ ] `edge_exact_start_time_allows` -- Monday 09:00, window 09:00-17:00 (inclusive start)
- [ ] `edge_exact_end_time_rejects` -- Monday 17:00, window 09:00-17:00 (exclusive end)
- [ ] `case_insensitive_day_matching` -- "mon" matches "Mon"
- [ ] `no_windows_allows` -- empty windows list returns Allow (vacuously true handled by caller)

---

## Task 3: Rate limit migration + evaluator

**Commit message:** `feat: add rate limit evaluator with counter table`

**Files:**
- Create: `migrations/20260412000001_create_rate_limit_counters.sql`, `src/trigger_rules/rate_limit.rs` (replace empty placeholder)
- Modify: `src/tasks.rs`

**Steps:**

- [ ] Create `migrations/20260412000001_create_rate_limit_counters.sql`:

```sql
CREATE TABLE rate_limit_counters (
    hook_slug    TEXT NOT NULL,
    window_start TEXT NOT NULL,
    count        INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (hook_slug, window_start)
);
```

- [ ] Implement `src/trigger_rules/rate_limit.rs`:

```rust
pub async fn evaluate(pool: &SqlitePool, hook_slug: &str, config: &TriggerRateLimit) -> EvalOutcome
```

Implementation:
1. Compute the window boundary: truncate `chrono::Utc::now()` to the configured `config.window` duration. For example, if `window` is 1 hour, truncate to the current hour (`2026-04-12T14:00:00Z`). Use integer division of the Unix timestamp by `window.as_secs()`, then multiply back.
2. Format as ISO 8601 string for the `window_start` column.
3. Execute two SQL statements:
   a. `INSERT OR IGNORE INTO rate_limit_counters (hook_slug, window_start, count) VALUES (?, ?, 0)`
   b. `UPDATE rate_limit_counters SET count = count + 1 WHERE hook_slug = ? AND window_start = ? AND count < ?` (bind `config.max_requests` as the limit).
4. Check `rows_affected()` on the UPDATE:
   - 1: under limit, return `Allow`.
   - 0: limit reached, return `Reject { status: RateLimited, reason: "rate limit exceeded ({max_requests} per {window})" }`.

- [ ] Add rate limit config validation in `AppConfig::validate()` (in the trigger_rules block):

```rust
if let Some(rl) = &rules.rate_limit {
    if rl.max_requests == 0 {
        errors.push(format!(
            "{prefix}.trigger_rules.rate_limit.max_requests must be greater than 0"
        ));
    }
    if rl.window.is_zero() {
        errors.push(format!(
            "{prefix}.trigger_rules.rate_limit.window must be greater than 0"
        ));
    }
}
```

- [ ] Add stale counter cleanup to `src/tasks.rs`. In `spawn_session_sweep()`, after the session deletion, add:

```rust
// Clean up stale rate limit counters (older than 48 hours).
// 48 hours is a safe conservative threshold: well past any realistic rate limit window
// (max_requests per window, where window is user-configured). A per-hook threshold
// would require joining against the config; 48 hours is simpler and correct.
let cutoff = (chrono::Utc::now() - chrono::Duration::hours(48))
    .format("%Y-%m-%dT%H:%M:%SZ")
    .to_string();
match sqlx::query("DELETE FROM rate_limit_counters WHERE window_start < ?")
    .bind(&cutoff)
    .execute(&pool)
    .await
{
    Ok(result) if result.rows_affected() > 0 => {
        tracing::debug!(deleted = result.rows_affected(), "cleaned stale rate limit counters");
    }
    Err(e) => tracing::warn!("failed to clean rate limit counters: {e}"),
    _ => {}
}
```

- [ ] Run `cargo check` to verify compilation.

**Tests** (in `rate_limit.rs`):

- [ ] `under_limit_allows` -- limit 5/min, first request returns Allow
- [ ] `at_limit_rejects` -- limit 2/min, first two requests Allow, third Reject
- [ ] `different_windows_independent` -- requests in different time windows don't interfere
- [ ] `different_hooks_independent` -- requests for different hook slugs don't interfere
- [ ] `concurrent_requests_safe` -- spawn two tasks that both try to claim the last slot, exactly one succeeds (use `tokio::test`)

All rate limit tests need a database. Use `Db::new_in_memory()` and run migrations.

---

## Task 4: Cooldown evaluator

**Commit message:** `feat: add cooldown evaluator for trigger rules`

**Files:**
- Create: `src/trigger_rules/cooldown.rs` (replace empty placeholder)
- Modify: `src/models/execution.rs`

**Steps:**

- [ ] Add `get_latest_started_by_hook()` to `src/models/execution.rs`:

```rust
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
```

- [ ] Implement `src/trigger_rules/cooldown.rs`:

```rust
pub async fn evaluate(pool: &SqlitePool, hook_slug: &str, cooldown: Duration) -> EvalOutcome
```

Implementation:
1. Call `execution::get_latest_started_by_hook(pool, hook_slug).await`.
2. If `None`, return `Allow` (no prior execution, cooldown not applicable).
3. If `Some(exec)`, parse `exec.started_at` as an ISO 8601 timestamp.
4. Compute `elapsed = now - started_at`.
5. If `elapsed < cooldown`, return `Reject { status: CooldownSkipped, reason: "cooldown active, {remaining} remaining" }`.
6. Otherwise, return `Allow`.

Provide a testable variant that accepts a `now` parameter:

```rust
pub async fn evaluate(pool: &SqlitePool, hook_slug: &str, cooldown: Duration) -> EvalOutcome {
    evaluate_at(pool, hook_slug, cooldown, chrono::Utc::now()).await
}

pub async fn evaluate_at(
    pool: &SqlitePool,
    hook_slug: &str,
    cooldown: Duration,
    now: chrono::DateTime<chrono::Utc>,
) -> EvalOutcome {
    // ... actual logic
}
```

- [ ] Run `cargo check` to verify compilation.

**Tests** (in `cooldown.rs`, all `#[tokio::test]` with in-memory DB):

- [ ] `no_prior_execution_allows` -- hook has never run, cooldown passes
- [ ] `within_cooldown_rejects` -- last execution started 2 minutes ago, cooldown is 5 minutes, rejects
- [ ] `after_cooldown_allows` -- last execution started 10 minutes ago, cooldown is 5 minutes, allows
- [ ] `pending_execution_without_started_at_ignored` -- execution exists with `started_at = NULL` (still pending), cooldown passes
- [ ] `reason_includes_remaining_time` -- verify the reject reason contains human-readable remaining time

---

## Task 5: Wire evaluators into trigger pipeline

**Commit message:** `feat: wire trigger rule evaluators into webhook pipeline`

**Depends on:** Tasks 1-4 must be complete.

**Files:**
- Modify: `src/routes/hooks.rs`
- Modify: `tests/server_integration.rs`

**Steps:**

- [ ] **First**: Change `trigger_hook()`'s return type from `Result<Json<TriggerResponse>, Response>` to `Result<Response, Response>`. Wrap the existing success return value with `.into_response()`. The rejection variants need to return a different JSON shape than `TriggerResponse`, which the old return type cannot express. Do this before touching any other code — all subsequent steps assume this new signature.

- [ ] In `src/routes/hooks.rs`, add imports:

```rust
use crate::trigger_rules::{self, EvalOutcome};
use crate::trigger_rules::{payload_filter, time_window, cooldown, rate_limit};
```

- [ ] Add a `log_rejection()` helper function (private to the module):

```rust
async fn log_rejection(
    pool: &SqlitePool,
    hook_slug: &str,
    source_ip: &str,
    status: TriggerAttemptStatus,
    reason: &str,
) {
    let _ = trigger_attempt::insert(
        pool,
        &NewTriggerAttempt {
            hook_slug,
            source_ip,
            status,
            reason,
            execution_id: None,
        },
    )
    .await;
}
```

**Note:** `NewTriggerAttempt.reason` is `&str`, not `Option<&str>`. The design spec's pipeline integration example incorrectly shows `reason: Some(reason)` -- use `reason` directly.

- [ ] In `trigger_hook()`, after the payload parsing block (after `let payload_str = ...`) and before execution creation, add the trigger rule evaluation block:

```rust
// Parse payload as Value for filter evaluation (reuse if already parsed above)
let payload_value: serde_json::Value = serde_json::from_str(&payload_str)
    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

if let Some(rules) = &hook.trigger_rules {
    // 1. Payload filters
    if let Some(filters) = &rules.payload_filters {
        if !filters.is_empty() {
            if let EvalOutcome::Reject { status, reason } =
                payload_filter::evaluate(filters, &payload_value)
            {
                log_rejection(pool, &slug, &source_ip, status.clone(), &reason).await;
                return Ok(Json(serde_json::json!({
                    "status": status.to_string(),
                    "reason": reason,
                })).into_response());
            }
        }
    }

    // 2. Time windows
    if let Some(windows) = &rules.time_windows {
        if !windows.is_empty() {
            if let EvalOutcome::Reject { status, reason } =
                time_window::evaluate(windows)
            {
                log_rejection(pool, &slug, &source_ip, status.clone(), &reason).await;
                return Ok(Json(serde_json::json!({
                    "status": status.to_string(),
                    "reason": reason,
                })).into_response());
            }
        }
    }

    // 3. Cooldown
    if let Some(cd) = rules.cooldown {
        if let EvalOutcome::Reject { status, reason } =
            cooldown::evaluate(pool, &slug, cd).await
        {
            log_rejection(pool, &slug, &source_ip, status.clone(), &reason).await;
            return Ok(Json(serde_json::json!({
                "status": status.to_string(),
                "reason": reason,
            })).into_response());
        }
    }

    // 4. Rate limit
    if let Some(rl) = &rules.rate_limit {
        if let EvalOutcome::Reject { status, reason } =
            rate_limit::evaluate(pool, &slug, rl).await
        {
            log_rejection(pool, &slug, &source_ip, status.clone(), &reason).await;
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "status": status.to_string(),
                    "reason": reason,
                })),
            ).into_response());
        }
    }
}
```

- [ ] Run `cargo check` to verify compilation.

- [ ] Run `cargo test` to verify all existing tests still pass.

**Integration tests** (in `tests/server_integration.rs`):

- [ ] `trigger_payload_filter_rejects_non_matching` -- POST to hook with payload filter `action = "released"`, send `{"action": "push"}`, expect 200 with `{"status": "filtered", ...}`
- [ ] `trigger_payload_filter_allows_matching` -- same hook, send `{"action": "released"}`, expect 200 with `execution_id`
- [ ] `trigger_cooldown_rejects_within_window` -- trigger hook twice within cooldown period, second returns 200 with `cooldown_skipped`
- [ ] `trigger_rate_limit_rejects_over_limit` -- hook with rate limit 2/1min, fire 3 times, third returns 429
- [ ] `trigger_no_rules_fires_normally` -- hook with no `trigger_rules`, fires as before (regression test)
- [ ] `trigger_attempts_logged_for_rejections` -- verify trigger_attempt records created with correct status for each rejection type

---

## Verification

After all 5 tasks are complete:

- [ ] `cargo check` passes with no warnings
- [ ] `cargo test` passes -- all existing tests + new unit tests + new integration tests
- [ ] `cargo clippy` passes with no warnings
- [ ] Manual test: create a hook with trigger rules in `sendword.toml`, trigger with matching and non-matching payloads, verify correct behavior and trigger_attempts logging
