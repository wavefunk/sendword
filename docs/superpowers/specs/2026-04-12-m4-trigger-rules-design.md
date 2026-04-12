# M4: Trigger Rule Evaluators Design Spec

Adds four evaluators to the webhook trigger pipeline: payload filters, time window constraints, cooldown checking, and rate limiting. Together they control whether a validated webhook request proceeds to execution.

## Motivation

After auth and payload validation (M2/M3), sendword currently fires every valid webhook. Real deployments need finer control: only deploy on `action: "released"`, block triggers outside business hours, prevent burst re-deploys within 5 minutes, and enforce per-hook request budgets. These four evaluators fill that gap.

## Evaluation Order

```
Auth → Payload validation → Payload filters → Time window → Cooldown → Rate limit → Execute
```

This matches the v1 design spec. The order is intentional:

1. **Payload filters first** — cheap, pure computation, no I/O. Filters out irrelevant events before they consume rate limit budget or cooldown state. A GitHub repo receiving 100 push events/minute but only deploying on `action: "released"` should not have irrelevant events count against its rate limit.
2. **Time window second** — also pure computation. No point checking cooldown/rate state for requests outside allowed windows.
3. **Cooldown third** — single DB read (latest execution timestamp). Prevents rapid re-execution of the same hook.
4. **Rate limit last** — DB read + write (counter increment). Only requests that pass all prior checks consume rate limit budget.

Short-circuit on first rejection. Each rejection is logged to `trigger_attempts` with the appropriate status and reason.

## Shared Evaluation Interface

The codebase uses free functions throughout (executor, retry, webhook_auth). A trait would force a least-common-denominator function signature across evaluators with fundamentally different inputs (payload filter needs `&Value`, time window needs wall clock, cooldown/rate limiter need `&SqlitePool`). Instead, all evaluators return a shared result type:

```rust
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

Each evaluator is a standalone module with a public `evaluate` function:

```rust
// payload_filter.rs
pub fn evaluate(filters: &[PayloadFilter], payload: &serde_json::Value) -> EvalOutcome

// time_window.rs
pub fn evaluate(windows: &[TimeWindow]) -> EvalOutcome

// cooldown.rs
pub async fn evaluate(pool: &SqlitePool, hook_slug: &str, cooldown: Duration) -> EvalOutcome

// rate_limit.rs
pub async fn evaluate(pool: &SqlitePool, hook_slug: &str, config: &TriggerRateLimit) -> EvalOutcome
```

The pipeline caller in `trigger_hook()` calls each in sequence, returning early on any `Reject`.

## 1. Payload Filter Engine

### Purpose

Evaluate conditions on JSON payload fields. Multiple filters ANDed together per hook. If no filters defined, the hook fires on any valid payload.

### Config

The types are already implemented in `src/config.rs`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct PayloadFilter {
    /// Dot-notation path into the JSON payload (e.g., "pull_request.merged").
    pub field: String,
    /// Comparison operator.
    pub operator: FilterOperator,
    /// Expected value to compare against. Interpretation depends on `operator`.
    /// Stored as a string; numeric comparisons parse the string as f64 at evaluation time.
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOperator {
    /// Field value equals expected value.
    Equals,
    /// Field value does not equal expected value.
    NotEquals,
    /// String field contains expected substring, or array contains expected element.
    Contains,
    /// String field matches expected regex pattern.
    Regex,
    /// Field exists and is not null. No `value` needed.
    Exists,
    /// Numeric greater-than.
    Gt,
    /// Numeric less-than.
    Lt,
    /// Numeric greater-than-or-equal.
    Gte,
    /// Numeric less-than-or-equal.
    Lte,
}
```

**Note:** `value` is `Option<String>` rather than `Option<serde_json::Value>`. The evaluator parses numeric strings to `f64` for `gt`/`lt`/`gte`/`lte`. Boolean and null comparisons use string matching against the JSON serialization. This keeps the TOML config and web UI simple. A `not_exists` operator is not included; use `exists` with inverted logic at the hook level (create a second hook without the filter).

### TOML config example

```toml
[[hooks]]
name = "deploy-on-release"
slug = "deploy-on-release"

[hooks.trigger_rules]
payload_filters = [
    { field = "action", operator = "equals", value = "released" },
    { field = "release.draft", operator = "equals", value = "false" },
]
```

### Evaluation logic

1. Resolve each filter's `field` using `payload::resolve_field()` (existing dot-notation walker).
2. Apply the operator:
   - `equals` / `not_equals` — compare field's JSON string representation with `value`. For JSON strings, compare unquoted. For booleans and numbers, compare their string form.
   - `contains` — if the resolved field is a string, check substring. If an array, check element membership (string match on each element's JSON representation). Otherwise reject with type error.
   - `regex` — `value` must be a string containing a valid regex pattern. Match against the resolved field's string representation. Regex is compiled once at config load time and cached.
   - `exists` — check whether `resolve_field` returns `Some(non-null)`. `value` field is ignored.
   - `gt` / `lt` / `gte` / `lte` — parse both the resolved field and `value` as `f64`. Reject with type error if either is non-numeric.
3. All filters must pass (AND). First failing filter produces `Reject { status: Filtered, reason }` with the filter details.

### Regex compilation

Regex patterns in payload filters are compiled once at config validation time (`AppConfig::validate()`). A filter with an invalid regex pattern causes a config validation error, not a runtime error. The compiled regex is stored alongside the config (separate from the serializable `PayloadFilter` — a `CompiledPayloadFilter` wrapper or a parallel vec of compiled regexes).

### Non-matching result

Returns HTTP 200 (not an error — the webhook sender should not retry). Logged as `trigger_attempts` with status `filtered` and a reason describing which filter failed.

## 2. Time Window Evaluator

### Purpose

Allow or deny hook execution based on time-of-day and day-of-week windows. If no windows configured, always allowed. Multiple windows ORed (any match is sufficient).

### Config

The type is already implemented in `src/config.rs`:

```rust
/// A time window during which a hook is allowed to execute.
/// `days` is a list of day name strings (e.g. ["Mon", "Tue", "Wed"]).
/// Times are UTC strings in "HH:MM" 24-hour format.
#[derive(Debug, Clone, Deserialize)]
pub struct TimeWindow {
    pub days: Vec<String>,
    pub start_time: String,
    pub end_time: String,
}
```

**Note:** Timezone support is not included in this implementation. All time windows are evaluated in UTC. The `days` field uses plain strings (e.g. `"Mon"`, `"Tue"`) rather than a typed `Weekday` enum; the evaluator matches case-insensitively against the standard three-letter English abbreviations. A typed enum can be introduced later without a breaking config change.

### TOML config example

```toml
[[hooks]]
name = "deploy-weekdays"
slug = "deploy-weekdays"

[hooks.trigger_rules]
time_windows = [
    { days = ["Mon", "Tue", "Wed", "Thu", "Fri"], start_time = "09:00", end_time = "17:00" },
    { days = ["Sat"], start_time = "10:00", end_time = "14:00" },
]
```

### Evaluation logic

1. Get the current UTC wall clock time.
2. For each configured window, check if the current UTC day-of-week is in `days` (empty = all days) and the current UTC time is within `[start_time, end_time)`.
3. If any window matches, return `Allow`.
4. If no window matches, return `Reject { status: ScheduleSkipped, reason }` with the current time and configured windows.

### Overnight windows

Windows where `start_time >= end_time` are rejected at config validation time with a clear error. Overnight windows (spanning midnight) are a non-goal for the initial implementation.

## 3. Cooldown Checker

### Purpose

Enforce a minimum time between executions of the same hook. Prevents rapid re-triggering (e.g., a deploy hook firing 10 times in 30 seconds due to multiple git pushes).

### Config

Already implemented on `TriggerRules` in `src/config.rs`:

```rust
// Inside TriggerRules:
#[serde(default, with = "humantime_serde::option")]
pub cooldown: Option<Duration>,
```

### TOML config example

```toml
[[hooks]]
name = "deploy-app"
slug = "deploy-app"

[hooks.trigger_rules]
cooldown = "5m"
```

### Evaluation logic

1. Query the most recent execution for this hook that reached at least `running` status. Use the existing `execution::get_latest_by_hook()` function — it returns the latest execution ordered by `triggered_at DESC`.
2. Check the `started_at` timestamp. If `now - started_at < cooldown`, return `Reject { status: CooldownSkipped, reason }` with the remaining cooldown time.
3. If no previous execution exists, or the cooldown has elapsed, return `Allow`.

### Cooldown reference point

The cooldown timer resets when an execution **starts** (`started_at`), not when it completes. This means: if a command takes 10 minutes and the cooldown is 5 minutes, a new trigger is allowed 5 minutes after the command started, even if it is still running.

### No new database state

Cooldown requires no new tables or columns. The `executions` table already has `hook_slug`, `started_at`, and status columns. The existing `get_latest_by_hook()` query provides the needed data. If a more specific query is needed (e.g., only considering executions with `started_at IS NOT NULL`), a new query function will be added to `execution.rs`.

## 4. Rate Limiter

### Purpose

Enforce a maximum request rate per hook using a sliding window counter. Prevents abuse and protects downstream systems.

### Config

A new `TriggerRateLimit` type is implemented in `src/config.rs`, stored inside `TriggerRules`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct TriggerRateLimit {
    pub max_requests: u64,
    #[serde(with = "humantime_serde")]
    pub window: Duration,
}

// Inside TriggerRules:
pub rate_limit: Option<TriggerRateLimit>,
```

**Note:** This is distinct from the existing `RateLimitConfig` (`max_per_minute`) on `HookConfig` and `DefaultsConfig`. That global config is currently parsed but unused. The trigger-rule rate limit (`TriggerRateLimit`) is per-hook with a configurable window, lives inside `trigger_rules`, and is what M4 evaluates. The `HookConfig.rate_limit` field is a separate concern; both can coexist.

**Resolution needed:** During implementation, decide whether to unify these or keep them separate. If unified, remove `RateLimitConfig` from `HookConfig`/`DefaultsConfig` and replace with `TriggerRateLimit`. For M4, implement enforcement for `TriggerRules.rate_limit` only.

### TOML config example

```toml
[[hooks]]
name = "deploy-app"
slug = "deploy-app"

[hooks.trigger_rules]
rate_limit = { max_requests = 10, window = "1h" }
```

### Database schema

One new migration adds the `rate_limit_counters` table:

```sql
CREATE TABLE rate_limit_counters (
    hook_slug    TEXT NOT NULL,
    window_start TEXT NOT NULL,
    count        INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (hook_slug, window_start)
);
```

`window_start` is an ISO 8601 timestamp truncated to the start of the configured window (e.g., `2026-04-12T14:00:00Z` for an hourly window). For sub-minute windows, truncate to the second.

### Evaluation logic

Sliding window counter:

1. Compute the window boundary: `now` truncated to the configured `window` duration.
2. `INSERT OR IGNORE` a row for `(hook_slug, window_start)` with `count = 0`.
3. `UPDATE rate_limit_counters SET count = count + 1 WHERE hook_slug = ? AND window_start = ? AND count < max_requests`. Check rows affected:
   - 1 row updated: under the limit, return `Allow`.
   - 0 rows updated: limit reached, return `Reject { status: RateLimited, reason }`.
4. SQLite's write serialization ensures correctness under concurrent requests.

### Stale counter cleanup

A periodic background task deletes rows where `window_start` is older than 5× the configured window. This can piggyback on the existing session sweep task or run as a new sweep. Prevents unbounded table growth.

## Pipeline Integration

### Where evaluators are called

In `trigger_hook()` (`src/routes/hooks.rs`), after the existing auth check and payload validation/parsing, before execution record creation:

```rust
// Existing: auth check
// Existing: payload parsing + schema validation

// NEW: Trigger rule evaluation
if let Some(rules) = &hook.trigger_rules {
    // 1. Payload filters
    if let Some(filters) = &rules.payload_filters {
        if !filters.is_empty() {
            if let EvalOutcome::Reject { status, reason } =
                payload_filter::evaluate(filters, &payload_value)
            {
                log_rejection(pool, &slug, &source_ip, status, &reason).await;
                return Ok(/* 200 with rejection body */);
            }
        }
    }

    // 2. Time windows
    if let Some(windows) = &rules.time_windows {
        if !windows.is_empty() {
            if let EvalOutcome::Reject { status, reason } =
                time_window::evaluate(windows)
            {
                log_rejection(pool, &slug, &source_ip, status, &reason).await;
                return Ok(/* 200 with rejection body */);
            }
        }
    }

    // 3. Cooldown
    if let Some(cooldown) = rules.cooldown {
        if let EvalOutcome::Reject { status, reason } =
            cooldown::evaluate(pool, &slug, cooldown).await
        {
            log_rejection(pool, &slug, &source_ip, status, &reason).await;
            return Ok(/* 200 with rejection body */);
        }
    }

    // 4. Rate limit
    if let Some(rl) = &rules.rate_limit {
        if let EvalOutcome::Reject { status, reason } =
            rate_limit::evaluate(pool, &slug, rl).await
        {
            log_rejection(pool, &slug, &source_ip, status, &reason).await;
            return Err(StatusCode::TOO_MANY_REQUESTS.into_response());
        }
    }
}

// Existing: create execution, spawn task
```

### HTTP status codes for rejections

- **Payload filter miss** — 200 OK. The request is valid; it just doesn't match the trigger criteria. The webhook sender should not retry.
- **Time window miss** — 200 OK. Same reasoning.
- **Cooldown active** — 200 OK. The request is valid but skipped. The webhook sender should not retry.
- **Rate limited** — 429 Too Many Requests. The sender may back off and retry.

### Response body for rejections

All rejections return a JSON body describing the outcome:

```json
{
    "status": "filtered",
    "reason": "filter failed: field 'action' equals 'push', expected 'released'"
}
```

### Trigger attempt logging

A helper function `log_rejection()` inserts into `trigger_attempts`:

```rust
async fn log_rejection(
    pool: &SqlitePool,
    hook_slug: &str,
    source_ip: &str,
    status: TriggerAttemptStatus,
    reason: &str,
) {
    let _ = trigger_attempt::insert(pool, &NewTriggerAttempt {
        hook_slug,
        source_ip,
        status,
        reason: Some(reason),
        execution_id: None,
    }).await;
}
```

## Config Types Summary

### Types already implemented in `src/config.rs`

```rust
pub struct TriggerRules {
    pub payload_filters: Option<Vec<PayloadFilter>>,
    pub time_windows: Option<Vec<TimeWindow>>,
    pub cooldown: Option<Duration>,          // humantime_serde::option
    pub rate_limit: Option<TriggerRateLimit>,
}

pub struct PayloadFilter {
    pub field: String,
    pub operator: FilterOperator,
    pub value: Option<String>,
}

pub enum FilterOperator {
    Equals, NotEquals, Contains, Regex, Exists,
    Gt, Lt, Gte, Lte,
}

pub struct TimeWindow {
    pub days: Vec<String>,      // e.g. ["Mon", "Tue"]
    pub start_time: String,     // "HH:MM" UTC
    pub end_time: String,       // "HH:MM" UTC (exclusive)
}

pub struct TriggerRateLimit {
    pub max_requests: u64,
    pub window: Duration,       // humantime_serde
}
```

### HookConfig

```rust
pub struct HookConfig {
    // ... existing fields ...
    pub trigger_rules: Option<TriggerRules>,
    pub rate_limit: Option<RateLimitConfig>,  // existing, currently unenforced
}
```

`trigger_rules` is purely per-hook with no global defaults. The existing `rate_limit` field (global default in `DefaultsConfig`) remains separate.

## Database Migration

One new migration: `20260412000001_create_rate_limit_counters.sql`

```sql
CREATE TABLE rate_limit_counters (
    hook_slug    TEXT NOT NULL,
    window_start TEXT NOT NULL,
    count        INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (hook_slug, window_start)
);
```

No other schema changes needed. Payload filters and time windows are pure computation. Cooldown uses the existing `executions` table.

## New Source Files

```
src/
  trigger_rules/
    mod.rs              -- EvalOutcome type, re-exports
    payload_filter.rs   -- payload filter evaluator + tests
    time_window.rs      -- time window evaluator + tests
    cooldown.rs         -- cooldown evaluator + tests
    rate_limit.rs       -- rate limiter evaluator + tests
```

Added to `lib.rs`:

```rust
pub mod trigger_rules;
```

## Edge Cases

### Concurrent rate limit requests

SQLite serializes all writes. The `INSERT OR IGNORE` + conditional `UPDATE ... AND count < max_requests` pattern means two concurrent requests cannot both succeed when only one slot remains. One will see `rows_affected = 0` and be rejected.

### Clock skew

Time windows and cooldowns use the server's wall clock. In single-instance deployments (sendword's target), clock skew is not an issue. If the system clock jumps (e.g., NTP correction), cooldown and rate limit windows may be slightly off. This is acceptable.

### Empty payload with filters

If no payload body is sent and the hook has payload filters, the evaluator receives an empty JSON object `{}`. Filters checking for specific fields will fail as expected (field does not exist → `exists` rejects).

### Regex denial of service

Payload filter regex patterns are user-configured. Pathological patterns (e.g., `(a+)+$`) could cause catastrophic backtracking. Mitigation: use the `regex` crate, which guarantees linear-time matching and does not backtrack. This is already a transitive dependency.

### Config reload during evaluation

Config is read once at the start of `trigger_hook()` via `state.config.load()` (atomic `Arc` swap). In-flight evaluations use the config snapshot they started with. A config reload mid-request has no effect on that request.

### Cooldown with no prior executions

If a hook has never been executed, `get_latest_by_hook()` returns no result. Cooldown evaluator treats this as "cooldown elapsed" and returns `Allow`.

### Rate limit counter overflow

With `max_requests` as `u64` and the counter column as `INTEGER` (SQLite i64), there is no overflow risk for realistic values.

### `equals` operator with non-string JSON fields

The `value` field in `PayloadFilter` is a string. For boolean payloads (`{"draft": false}`), the evaluator compares the JSON serialization (`"false"`) against the config value string. Users must write `value = "false"` in TOML, not `value = false`. This is a known limitation of the string-based value design. Document clearly in user-facing help text.

## Non-Goals

- **Complex boolean logic for filters** — no OR groups, no nested AND/OR trees. All filters are ANDed. Users who need OR can create multiple hooks with different filter sets.
- **Per-filter evaluation logging** — only the first failing filter is logged in the trigger attempt reason. Individual filter pass/fail is not persisted.
- **Rate limit headers** — `X-RateLimit-Remaining`, `Retry-After`, etc. are not included in responses. Can be added later.
- **Overnight time windows** — windows where `start_time >= end_time` (spanning midnight) are rejected at config validation time. Can be relaxed in a follow-up.
- **Timezone-aware time windows** — all windows are evaluated in UTC. Timezone support requires adding a `tz` field and the `jiff` crate. Non-goal for M4.
- **Global trigger rules** — trigger rules are per-hook only. No global filter/schedule/cooldown configuration.
- **Concurrency control and approval gates** — these are separate M5 execution barrier features and are not covered by this spec.
- **`not_exists` operator** — not included. Achieve the same effect with a separate hook that fires when the field is absent.
- **Enforcing existing `RateLimitConfig`** — the `hook.rate_limit` (max_per_minute) field is already parsed but unenforced. M4 only enforces `trigger_rules.rate_limit`. Unifying the two rate limit configs is deferred.

## Full TOML Config Example

```toml
[defaults]
timeout = "30s"

[defaults.retries]
count = 2
backoff = "exponential"
initial_delay = "1s"
max_delay = "60s"

[[hooks]]
name = "Deploy on Release"
slug = "deploy-on-release"
description = "Deploys the application when a GitHub release is published"
enabled = true

[hooks.auth]
mode = "hmac"
header = "X-Hub-Signature-256"
algorithm = "sha256"
secret = "${GITHUB_WEBHOOK_SECRET}"

[hooks.executor]
type = "shell"
command = "scripts/deploy.sh --tag {{release.tag_name}}"

[hooks.payload]
fields = [
    { name = "action", type = "string", required = true },
    { name = "release", type = "object", required = true },
    { name = "release.tag_name", type = "string", required = true },
]

[hooks.trigger_rules]
cooldown = "5m"
payload_filters = [
    { field = "action", operator = "equals", value = "released" },
    { field = "release.draft", operator = "equals", value = "false" },
]
rate_limit = { max_requests = 10, window = "1h" }
time_windows = [
    { days = ["Mon", "Tue", "Wed", "Thu", "Fri"], start_time = "09:00", end_time = "17:00" },
]
```
