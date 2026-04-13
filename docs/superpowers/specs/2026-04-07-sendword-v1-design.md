# sendword v1 Design Spec

Simple HTTP webhook to command runner sidecar. Receives webhooks from external systems or manual HTTP triggers, validates and filters them, runs commands (or other executors), and logs everything. Config lives on disk (TOML/JSON/env via figment); SQLite stores execution history, logs metadata, and runtime state.

## Request Processing Pipeline

Every incoming trigger flows through this pipeline in order:

```
Route match → Auth → Payload validation → Payload filters → Schedule check → Cooldown check → Rate limit → Concurrency check → Approval gate → Execute
```

At each stage, a request can be accepted (continue), skipped (200, logged), or rejected (4xx, logged). Skipped vs rejected distinction: skipped means "valid request, doesn't meet trigger criteria" (webhook sender shouldn't retry); rejected means "bad request or limit hit" (sender may retry).

## Config Architecture

Figment-layered configuration with merge order:

1. `sendword.toml` (primary, hand-editable)
2. `sendword.json` (override, good for programmatic generation)
3. Environment variables (highest priority, for secrets)

The config file is the source of truth for all hook definitions, auth, trigger rules, rate limits, barrier config, and executor config. The web UI reads and edits this file. SQLite is used only for execution history, logs metadata, trigger attempts, and runtime state (concurrency locks, approval queue).

### Top-level config shape

```
server:
  bind: "0.0.0.0"
  port: 8080

database:
  path: "data/sendword.db"

logs:
  dir: "data/logs"

defaults:
  rate_limit:
    max_per_minute: 60
  timeout: "30s"
  retries:
    count: 0
    backoff: "exponential"
    initial_delay: "1s"
    max_delay: "60s"

backup:
  endpoint: "https://s3.amazonaws.com"
  bucket: "sendword-backups"
  access_key: "${SENDWORD_BACKUP_ACCESS_KEY}"
  secret_key: "${SENDWORD_BACKUP_SECRET_KEY}"
  prefix: "instance-1/"
  schedule: "0 2 * * *"
  retention:
    max_count: 30
    max_age: "90d"

hooks:
  - name: "deploy-app"
    slug: "deploy-app"
    description: "Triggers app deployment"
    enabled: true
    auth: { ... }
    payload: { ... }
    trigger_rules: { ... }
    rate_limit: { ... }
    concurrency: { ... }
    approval: { ... }
    executor: { ... }
    notification: { ... }
```

## Milestone 1: Foundation

End-to-end loop: load config, receive HTTP request, run shell command, log result, view in UI.

### Config loading

- Figment loads `sendword.toml` + `sendword.json` + env vars.
- Config struct with server settings, database path, log directory, global defaults, and hook list.
- Hook definition at this stage: name, slug, description, enabled, executor config (shell command), env vars, working directory, timeout, retry config.

### Server startup

- Axum server with shared state: config, DB pool, templates.
- Static file serving for CSS/JS from `static/dist/`.
- Auto-run SQLite migrations on startup.
- Health check endpoint: `GET /healthz` returns server + DB status.
- Graceful shutdown: on SIGTERM, stop accepting new triggers, wait for running executions to complete (up to a configurable drain timeout), then exit.

### Routes

- `POST /hook/:slug` — trigger a hook. Look up slug in config, run the executor, return 200 + execution ID.
- `GET /` — dashboard listing all hooks with status indicators and recent executions.
- `GET /hooks/:slug` — hook detail: config summary + execution history.
- `GET /executions/:id` — execution detail: stdout, stderr, exit code, timing.
- `POST /executions/:id/replay` — re-trigger with the same payload. Creates a new execution.
- `GET /healthz` — health check.

### Shell executor

- Runs command via `tokio::process::Command` with `sh -c`.
- Captures stdout and stderr to files: `data/logs/{execution_id}/stdout.log`, `stderr.log`.
- Per-hook configurable environment variables passed to the process.
- Per-hook configurable working directory.
- Configurable timeout (per-hook with global default). Timeout kills the process and marks execution as `timed_out`.

### Retry logic

- Per-hook config: retry count (default: 0), backoff strategy (none, linear, exponential), initial delay, max delay.
- Global defaults in top-level config, per-hook overrides.
- Retries happen automatically on non-zero exit codes.
- Retries increment `retry_count` on the execution record. Individual retry timing and outcomes are captured in the log files (appended with attempt markers).

### Execution logging

- SQLite `executions` table: id, hook_slug, triggered_at, started_at, completed_at, status (pending, running, success, failed, timed_out), exit_code, log_path, trigger_source (IP), request_payload (JSON), retry_count, retry_of (nullable, links to original execution for replays).
- Log files on disk under `data/logs/{execution_id}/`.
- Executor writes stdout/stderr to files in real-time during execution.

### Web UI

- Base template (exists). Add: dashboard page, hook detail page, execution detail page.
- Dashboard: list all hooks, show enabled/disabled, last execution status.
- Hook detail: config summary, execution history list.
- Execution detail: metadata, stdout/stderr display, replay button.
- HTMX for loading execution history without full page reloads.

## Milestone 2: Auth & Payload Validation

### Auth

Per-hook auth configuration. Three modes:

- **none** — public, no auth required.
- **bearer** — caller sends `Authorization: Bearer <token>`. Token in config (supports env var references like `${HOOK_TOKEN}`).
- **hmac** — webhook provider signs the payload. Config: header name (e.g. `X-Hub-Signature-256`), algorithm (sha256), shared secret. Sendword recomputes the signature and compares.

Auth check is the first thing after route matching. Failed auth returns 401.

### Trigger attempts table

SQLite `trigger_attempts` table: id, hook_slug, attempted_at, source_ip, status (fired, auth_failed, validation_failed, filtered, rate_limited, schedule_skipped, cooldown_skipped, concurrency_rejected, pending_approval), reason (human-readable).

All non-execution outcomes are logged here. Successful triggers that lead to execution are also recorded (status: fired) with a reference to the execution ID.

### Payload definitions

Per-hook schema definition in config:

```
payload:
  fields:
    - name: "action"
      type: "string"
      required: true
    - name: "repo"
      type: "object"
      required: false
    - name: "repo.name"
      type: "string"
      required: false
```

Types: string, number, boolean, object, array.

On ingest: validate incoming JSON against schema. Missing required fields or type mismatches return 422 with a descriptive error.

### Payload interpolation

Validated fields are available for interpolation into command templates:

```
executor:
  type: "shell"
  command: "deploy.sh --repo {{repo.name}} --action {{action}}"
```

Syntax: `{{field_name}}` with dot-notation for nested access. All interpolated values are shell-escaped to prevent injection.

If no payload definition is set, the hook accepts any JSON payload (or no body). The raw payload is available to the executor via the `SENDWORD_PAYLOAD` environment variable (JSON string) and written to `data/logs/{execution_id}/payload.json`.

### Secret masking

Configurable list of patterns to redact from log output displayed in the UI:

- Env var names whose values should be masked (e.g. `SENDWORD_BACKUP_SECRET_KEY`).
- Regex patterns to match and replace with `***`.
- Applied when reading log files for UI display, not when writing them (raw logs preserved on disk for debugging).

### UI additions

- Hook detail shows auth mode and payload schema.
- Trigger attempts visible on hook detail page, filterable by status.

## Milestone 3: Trigger Rules & Rate Limiting

### Payload filters

Per-hook list of conditions. All must match (AND logic) for the hook to fire.

```
trigger_rules:
  filters:
    - field: "action"
      op: "eq"
      value: "opened"
    - field: "pull_request.merged"
      op: "eq"
      value: true
    - field: "label"
      op: "in"
      value: ["deploy", "release"]
```

Operators: `eq`, `neq`, `in`, `contains` (substring), `exists`, `not_exists`.

If no filters defined, the hook fires on any valid request.

Non-matching requests return 200 (don't trigger sender retries), logged in `trigger_attempts` with status `filtered` and the reason.

### Scheduling constraints

Per-hook, optional. Two types:

**Time windows:**
```
trigger_rules:
  schedule:
    windows:
      - days: ["mon", "tue", "wed", "thu", "fri"]
        start: "09:00"
        end: "17:00"
        tz: "America/New_York"
```

Hook only fires during matching windows. Multiple windows supported (OR — any window match is sufficient).

**Cooldown:**
```
trigger_rules:
  cooldown: "5m"
```

Minimum duration between executions. If the hook was triggered within the cooldown period, skip. Cooldown is based on the last successful trigger, tracked via the executions table.

Requests outside time windows or within cooldown return 200 "skipped", logged in `trigger_attempts` with appropriate status and reason.

### Rate limiting

```
# Global default (top-level config)
defaults:
  rate_limit:
    max_per_minute: 60

# Per-hook override
hooks:
  - name: "deploy"
    rate_limit:
      max_per_minute: 5
```

Implemented as a sliding window counter. State stored in SQLite (survives restarts).

Rate-limited requests return 429, logged in `trigger_attempts`.

### Pipeline order (as of M3)

Route match → Auth → Payload validation → Payload filters → Schedule check → Cooldown check → Rate limit → Execute.

Note: M4 inserts concurrency check and approval gate between rate limit and execute. See the full pipeline in the Request Processing Pipeline section at the top.

### UI additions

- Hook detail shows trigger rules, schedule, cooldown, and rate limit config.
- Trigger attempts history with filtering by status.

## Milestone 4: Execution Barriers

### Concurrency control

Per-hook config:

```
concurrency:
  mode: "mutex"   # or "queue"
  queue_depth: 10  # only for queue mode
```

**mutex** — one execution at a time. New triggers while one is running are rejected with status `concurrency_rejected` in `trigger_attempts`.

**queue** — new triggers are queued. Configurable max queue depth (default: 10). Queue full → rejected.

If unset, no concurrency control — unlimited parallel executions.

State tracked in SQLite:
- `execution_locks` table: hook_slug, execution_id, acquired_at. Mutex check is a row existence check.
- `execution_queue` table: id, hook_slug, execution_id, position, queued_at, payload, status (waiting, ready, expired). Queue processing picks the oldest `waiting` entry when the lock is released.

### Approval gates

Per-hook config:

```
approval:
  required: true
  timeout: "1h"
```

When a gated hook is triggered:
1. Execution record created with status `pending_approval`.
2. Command does not run.
3. UI shows the pending execution on the "Pending Approvals" page.

Actions via UI:
- **Approve** — command runs, execution proceeds normally.
- **Reject** — execution marked `rejected`, command never runs.

Auto-expiry: if `timeout` is set and no action is taken within the window, execution is marked `expired`.

Approval metadata stored in executions table: approved_at, approved_by (placeholder for future UI auth), approval_status (pending, approved, rejected, expired).

### Interaction between barriers

Full pipeline: ... → Rate limit → Concurrency check → Approval gate → Execute.

A queued execution that also requires approval: enters the queue, and when it reaches the front, moves to `pending_approval` instead of executing immediately.

### UI additions

- "Pending Approvals" page listing gated executions awaiting action.
- Approve/Reject buttons via HTMX.
- Hook detail shows concurrency mode and approval config.
- Execution detail shows approval status and timing.

## Milestone 5: Executors, Backups & Polish

### Script file executor

```
executor:
  type: "script"
  path: "/opt/scripts/deploy.sh"
```

- Sendword checks file exists and is executable at config load time (fail-fast on startup).
- Payload fields passed as environment variables: `SENDWORD_<FIELD_NAME>=value` (uppercased, dots replaced with underscores).
- Same stdout/stderr capture, timeout, and retry behavior as shell executor.

### HTTP executor

```
executor:
  type: "http"
  method: "POST"
  url: "https://api.example.com/deploy"
  headers:
    Authorization: "Bearer ${DEPLOY_TOKEN}"
    Content-Type: "application/json"
  body: '{"repo": "{{repo.name}}", "action": "{{action}}"}'
  timeout: "30s"
  follow_redirects: true
```

- Body template supports `{{field}}` interpolation (same as shell command templates).
- Logs: request URL, method, response status, response body (truncated to configurable max, default 4KB), timing.
- Response status determines success: 2xx = success, anything else = failed.
- stdout.log contains the response body, stderr.log contains request/response metadata.

### Executor trait

```rust
#[async_trait]
trait Executor: Send + Sync {
    async fn run(&self, ctx: ExecutionContext) -> ExecutionResult;
}
```

Shell, script, and HTTP all implement this trait. `ExecutionContext` contains: validated payload, env vars, working directory, log paths, timeout. `ExecutionResult` contains: exit status, timing.

### Config export/import

API:
- `GET /api/config/export` — returns current hook config as JSON.
- `POST /api/config/import` — accepts JSON config, validates, writes to disk, triggers config reload.

CLI:
- `sendword export > backup.json`
- `sendword import backup.json`

Config reload is hot — no server restart required. New requests use the new config; in-flight executions complete with the old config.

### S3-compatible backups

Config:
```
backup:
  endpoint: "https://s3.amazonaws.com"
  bucket: "sendword-backups"
  access_key: "${SENDWORD_BACKUP_ACCESS_KEY}"
  secret_key: "${SENDWORD_BACKUP_SECRET_KEY}"
  prefix: "instance-1/"
  schedule: "0 2 * * *"
  retention:
    max_count: 30
    max_age: "90d"
```

Backup contents: config files (TOML + JSON) + SQLite database snapshot + log files, bundled as a compressed tarball.

API:
- `POST /api/backup` — trigger manual backup, returns backup key.
- `GET /api/backup/list` — list available backups with timestamps and sizes.
- `POST /api/backup/restore` — download and restore a specific backup.

CLI:
- `sendword backup` — manual backup.
- `sendword backup list` — list backups.
- `sendword restore --from <key>` — restore from a specific backup.

Scheduled backups via cron expression in config. Retention policy: keep last N backups and/or max age. Old backups auto-deleted from S3.

Compatible with: AWS S3, MinIO, R2, Backblaze B2, any S3-compatible store.

### Completion notifications

Per-hook, optional:

```
notification:
  url: "https://hooks.slack.com/services/..."
  on: ["failure", "timeout"]  # or ["success", "failure", "timeout"]
  headers:
    Content-Type: "application/json"
  body: '{"text": "Hook {{hook_name}} {{status}}: exit {{exit_code}}"}'
```

- Fires after execution completes (including retries).
- Body supports interpolation: `{{hook_name}}`, `{{hook_slug}}`, `{{status}}`, `{{exit_code}}`, `{{execution_id}}`, `{{duration}}`.
- Notification failures are logged but don't affect execution status.

### UI polish

- Dashboard: hook status indicators (enabled/disabled, healthy/failing based on recent executions), search and filter.
- Execution history: pagination, filtering by status, hook, and date range.
- Live log streaming: SSE endpoint (`GET /executions/:id/logs/stream`) pushes new log lines as the execution runs. Execution detail page connects automatically for running executions.
- Toast notifications for UI actions (approve, reject, import, backup).

## Data Model Summary

### SQLite tables

**executions**
- id (TEXT PK, UUIDv7)
- hook_slug (TEXT)
- triggered_at (TEXT, ISO8601)
- started_at (TEXT, nullable)
- completed_at (TEXT, nullable)
- status (TEXT: pending, pending_approval, approved, rejected, expired, running, success, failed, timed_out)
- exit_code (INTEGER, nullable)
- log_path (TEXT)
- trigger_source (TEXT, IP address)
- request_payload (TEXT, JSON)
- retry_count (INTEGER, default 0)
- retry_of (TEXT, nullable FK to executions.id)
- approved_at (TEXT, nullable)
- approved_by (TEXT, nullable)

**trigger_attempts**
- id (TEXT PK, UUIDv7)
- hook_slug (TEXT)
- attempted_at (TEXT, ISO8601)
- source_ip (TEXT)
- status (TEXT: fired, auth_failed, validation_failed, filtered, rate_limited, schedule_skipped, cooldown_skipped, concurrency_rejected, pending_approval)
- reason (TEXT, human-readable)
- execution_id (TEXT, nullable FK to executions.id)

**execution_locks**
- hook_slug (TEXT PK)
- execution_id (TEXT)
- acquired_at (TEXT, ISO8601)

**execution_queue**
- id (TEXT PK, UUIDv7)
- hook_slug (TEXT)
- execution_id (TEXT)
- position (INTEGER)
- queued_at (TEXT, ISO8601)
- payload (TEXT, JSON)
- status (TEXT: waiting, ready, expired)

**rate_limit_counters**
- hook_slug (TEXT)
- window_start (TEXT, ISO8601)
- count (INTEGER)
- PRIMARY KEY (hook_slug, window_start)

## Non-goals for v1

- Built-in TLS (use a reverse proxy)
- Prometheus metrics export
- Webhook delivery signatures on outgoing callbacks
- Multi-user auth for the web UI
- Distributed/clustered deployment
