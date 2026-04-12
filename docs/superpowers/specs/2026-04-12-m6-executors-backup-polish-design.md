# M6: Executors, Backup & Polish Design Spec

Adds script and HTTP executors, config export/import, S3-compatible backups, completion notifications, and UI polish including live log streaming. This is the final milestone, completing the v1 feature set.

## Motivation

Milestone 1-5 established the core loop: receive webhook, evaluate rules and barriers, execute shell command, log results. M6 expands the executor model (script files, HTTP calls), adds operational capabilities (backup/restore, config portability), improves observability (notifications, live logs), and polishes the UI.

## 1. Executor Expansion

### Design Decision: Enum Dispatch, Not a Trait

The v1 spec proposes an `#[async_trait] trait Executor`. The codebase uses enum dispatch throughout (`ExecutorConfig`, `HookAuthConfig`, `BackoffStrategy`). A `Box<dyn Executor>` would add heap allocation and dynamic dispatch for exactly 3 variants known at compile time. Instead, expand the existing `ExecutorConfig` enum with `Script` and `Http` variants, and branch in `executor::run()`.

### ExecutorConfig expansion

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutorConfig {
    Shell {
        command: String,
    },
    Script {
        path: String,
    },
    Http {
        method: HttpMethod,
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default = "default_true")]
        follow_redirects: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}
```

### TOML config examples

```toml
# Shell executor (existing)
[[hooks]]
name = "deploy-shell"
slug = "deploy-shell"
[hooks.executor]
type = "shell"
command = "deploy.sh --repo {{repo.name}}"

# Script file executor
[[hooks]]
name = "deploy-script"
slug = "deploy-script"
[hooks.executor]
type = "script"
path = "scripts/deploy.sh"

# HTTP executor
[[hooks]]
name = "notify-api"
slug = "notify-api"
[hooks.executor]
type = "http"
method = "POST"
url = "https://api.example.com/deploy"
body = '{"repo": "{{repo.name}}", "action": "{{action}}"}'
follow_redirects = true
[hooks.executor.headers]
Authorization = "Bearer ${DEPLOY_TOKEN}"
Content-Type = "application/json"
```

### ResolvedExecutor

`ExecutionContext` currently carries `command: String` -- shell-specific. Replace with a `ResolvedExecutor` enum that carries already-interpolated, ready-to-execute parameters:

```rust
pub enum ResolvedExecutor {
    Shell { command: String },
    Script { path: PathBuf },
    Http {
        method: HttpMethod,
        url: String,
        headers: HashMap<String, String>,
        body: Option<String>,
        follow_redirects: bool,
    },
}
```

The interpolation step in `trigger_hook()` produces the `ResolvedExecutor` from config + payload. `ExecutionContext` changes:

```rust
pub struct ExecutionContext {
    pub execution_id: String,
    pub hook_slug: String,
    pub executor: ResolvedExecutor,  // replaces `command: String`
    pub env: HashMap<String, String>,
    pub cwd: Option<String>,
    pub timeout: Duration,
    pub logs_dir: String,
    pub payload_json: String,
}
```

### executor::run() dispatch

```rust
pub async fn run(pool: &SqlitePool, ctx: ExecutionContext) -> ExecutionResult {
    match &ctx.executor {
        ResolvedExecutor::Shell { command } => run_shell(pool, &ctx, command).await,
        ResolvedExecutor::Script { path } => run_script(pool, &ctx, path).await,
        ResolvedExecutor::Http { .. } => run_http(pool, &ctx).await,
    }
}
```

The retry module calls `executor::run()` unchanged -- it does not know which executor type is running.

### Shell executor

Unchanged from current implementation. `run_shell()` extracts the existing logic from `executor::run()`.

### Script file executor

```rust
async fn run_script(pool: &SqlitePool, ctx: &ExecutionContext, path: &Path) -> ExecutionResult
```

- Runs the script file directly (not via `sh -c`). The file must be executable (`chmod +x`).
- Payload fields passed as environment variables: `SENDWORD_FIELD_<NAME>=value`. Field names are uppercased, dots replaced with underscores (e.g., `repo.name` becomes `SENDWORD_FIELD_REPO_NAME`).
- Same stdout/stderr capture, timeout, process management, and DB status tracking as shell executor.
- Log directory structure identical to shell executor.

**Config validation**: `AppConfig::validate()` checks that the script file exists and is executable. This means config reload fails if a referenced script is missing -- correct behavior (fail-fast on bad config).

### HTTP executor

```rust
async fn run_http(pool: &SqlitePool, ctx: &ExecutionContext) -> ExecutionResult
```

- Uses a shared `reqwest::Client` (see below).
- URL and body template support `{{field}}` interpolation (already resolved in `ResolvedExecutor`).
- Header values support `${ENV_VAR}` references (resolved at execution time, same as auth config).
- Response status determines success: 2xx = `Success`, anything else = `Failed`.
- `exit_code` is set to the HTTP status code (e.g., 200, 500) for HTTP executions.
- Log files:
  - `stdout.log` -- response body (truncated to 4KB by default, configurable).
  - `stderr.log` -- request/response metadata: method, URL, response status, timing, response headers.
- Timeout applies to the entire HTTP call (connection + response).

**Shared HTTP client**: A `reqwest::Client` is stored in `AppState`. It is reused for HTTP executions, completion notifications, and any future outgoing HTTP calls. Connection pooling and TLS session reuse happen automatically.

```rust
pub struct AppState {
    pub config: ArcSwap<AppConfig>,
    pub config_writer: ConfigWriter,
    pub db: Db,
    pub templates: Templates,
    pub http_client: reqwest::Client,  // NEW
}
```

**Dependency change**: `reqwest` is already in `[dependencies]` with `["json"]`. Add `"rustls-tls"` to the features list. No move from dev-dependencies needed.

## 2. Config Export/Import

### Purpose

Config portability: export the running config as JSON, import a config from JSON. Useful for backup, migration between instances, and programmatic config generation.

### API endpoints

```
GET  /api/config/export    (requires AuthUser)
POST /api/config/import    (requires AuthUser)
```

**Export**: Serializes the current `AppConfig` as JSON. Returns `Content-Type: application/json` with the full config (hooks, defaults, server settings, etc.). Sensitive fields (auth tokens, secrets) are included -- the endpoint is auth-protected.

**Import**: Accepts a JSON body containing a full or partial config. Flow:

1. Deserialize and validate the incoming config (`AppConfig::load_from` equivalent).
2. If validation fails, return 422 with error details. The running server is unaffected.
3. If valid, write to disk (TOML format, replacing the existing config file).
4. Call `AppState::reload_config()` to hot-reload.
5. Return 200 with the new config.

Import validates before writing -- a bad import never breaks the running server.

### CLI commands

```
sendword export > backup.json
sendword import backup.json
```

Added to the `Command` enum in `main.rs`:

```rust
#[derive(Subcommand)]
enum Command {
    Serve,
    User { #[command(subcommand)] action: UserAction },
    /// Export current config as JSON to stdout
    Export,
    /// Import config from a JSON file
    Import {
        /// Path to the JSON config file
        path: String,
    },
}
```

**Export**: loads config, serializes as JSON, writes to stdout. Does not need a running server.

**Import**: loads the JSON file, validates, writes to the TOML config path, prints confirmation. If a server is running, it picks up the change on the next config reload (or the user can restart).

### Hot reload

The reload mechanism already exists: `AppState::reload_config()` atomically swaps the `ArcSwap<AppConfig>`. In-flight requests use their existing config snapshot. New requests see the updated config.

## 3. S3-Compatible Backups

### Purpose

Automated and manual backups to S3-compatible storage (AWS S3, MinIO, R2, Backblaze B2).

### Config

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct BackupConfig {
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    #[serde(default)]
    pub prefix: String,
    /// Cron expression for scheduled backups (e.g., "0 2 * * *").
    pub schedule: Option<String>,
    #[serde(default)]
    pub retention: RetentionConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetentionConfig {
    /// Maximum number of backups to keep.
    pub max_count: Option<u32>,
    /// Maximum age of backups.
    #[serde(default, with = "humantime_serde::option")]
    pub max_age: Option<Duration>,
}
```

### TOML config example

```toml
[backup]
endpoint = "https://s3.amazonaws.com"
bucket = "sendword-backups"
access_key = "${SENDWORD_BACKUP_ACCESS_KEY}"
secret_key = "${SENDWORD_BACKUP_SECRET_KEY}"
prefix = "instance-1/"
schedule = "0 2 * * *"

[backup.retention]
max_count = 30
max_age = "90d"
```

### Backup contents

A backup is a compressed tarball (`.tar.gz`) containing:

- `sendword.toml` -- config file
- `sendword.json` -- JSON config overlay (if exists)
- `sendword.db` -- SQLite database snapshot

**Log files are excluded by default.** Log directories can grow unboundedly and would make backups impractical for long-running instances. This is a deliberate scope constraint -- logs are preserved on disk and can be backed up separately via standard tools if needed.

The SQLite snapshot uses `VACUUM INTO` to create a consistent, compact copy without blocking the running database.

### S3 client

Use the `rust-s3` crate. It is purpose-built for S3-compatible storage, lighter than the full AWS SDK, and covers put/get/list/delete which is all sendword needs.

### Backup key format

`{prefix}sendword-backup-{ISO8601-timestamp}.tar.gz`

Example: `instance-1/sendword-backup-2026-04-12T02:00:00Z.tar.gz`

### API endpoints

```
POST /api/backup         (requires AuthUser) -- trigger manual backup, returns backup key
GET  /api/backup/list    (requires AuthUser) -- list available backups with timestamps and sizes
POST /api/backup/restore (requires AuthUser) -- download and restore a specific backup
```

**Backup flow**:
1. Create temp directory.
2. Copy config files.
3. `VACUUM INTO` the SQLite database to create a snapshot.
4. Create `.tar.gz` from the temp directory.
5. Upload to S3.
6. Apply retention policy (delete old backups exceeding count/age limits).
7. Return the backup key.

**Restore flow**:
1. Download the tarball from S3.
2. Extract to temp directory.
3. Validate the extracted config file.
4. Replace config file on disk.
5. Replace SQLite database (requires briefly closing the pool -- schedule during low-traffic or accept a brief interruption).
6. Reload config.
7. Return success.

**Restore is a destructive operation.** The API should require explicit confirmation (e.g., a `confirm: true` field in the request body). The current state is overwritten.

### CLI commands

```rust
/// Backup management commands
Backup {
    #[command(subcommand)]
    action: BackupAction,
},
/// Restore from a backup
Restore {
    /// Backup key to restore from
    #[arg(long)]
    from: String,
},

#[derive(Subcommand)]
enum BackupAction {
    /// Create a backup now
    Create,
    /// List available backups
    List,
}
```

### Scheduled backups

If `backup.schedule` is set, a background task parses the cron expression and triggers backups at the scheduled times. Uses the `cron` crate for parsing. The task runs alongside the existing session sweep task.

```rust
pub fn spawn_backup_scheduler(
    config: Arc<ArcSwap<AppConfig>>,
    pool: SqlitePool,
    http_client: reqwest::Client,
) -> tokio::task::JoinHandle<()>
```

After each scheduled backup, the retention policy is applied.

## 4. Completion Notifications

### Purpose

Notify external systems when a hook execution completes. Useful for Slack alerts, PagerDuty integration, or chaining hooks.

### Config

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct NotificationConfig {
    /// Webhook URL to POST to.
    pub url: String,
    /// Which outcomes trigger the notification.
    #[serde(default = "default_notify_on")]
    pub on: Vec<NotifyOutcome>,
    /// Additional headers for the request.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Body template with {{placeholder}} interpolation.
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyOutcome {
    Success,
    Failure,
    Timeout,
}

fn default_notify_on() -> Vec<NotifyOutcome> {
    vec![NotifyOutcome::Failure, NotifyOutcome::Timeout]
}
```

### TOML config example

```toml
[[hooks]]
name = "deploy-prod"
slug = "deploy-prod"

[hooks.notification]
url = "https://hooks.slack.com/services/T.../B.../..."
on = ["failure", "timeout"]
body = '{"text": "Hook {{hook_name}} {{status}}: exit {{exit_code}} ({{duration}})"}'
[hooks.notification.headers]
Content-Type = "application/json"
```

### Template interpolation

Notification body templates use `{{placeholder}}` syntax. Available variables:

| Variable | Description |
|----------|-------------|
| `{{hook_name}}` | Hook display name |
| `{{hook_slug}}` | Hook slug |
| `{{status}}` | Execution status (success, failed, timed_out) |
| `{{exit_code}}` | Process exit code (empty if none) |
| `{{execution_id}}` | Execution UUID |
| `{{duration}}` | Human-readable duration (e.g., "2.3s") |
| `{{trigger_source}}` | IP address of the trigger |

This uses the same interpolation infrastructure as command templates (`interpolation.rs`), but with a different set of variables. A `notification_context()` function builds a `serde_json::Value` from the execution result, then `interpolate_command()` is reused.

### Notification dispatch

After `run_with_retries` completes (including all retry attempts), if the hook has a notification config and the outcome matches `on`:

1. Build the interpolated body.
2. POST to `notification.url` with the configured headers.
3. Use the shared `reqwest::Client` from `AppState`.
4. Timeout: 10 seconds (hardcoded, not configurable -- notifications are best-effort).
5. Log the notification result (success/failure) but do **not** affect execution status.

```rust
pub async fn send_notification(
    client: &reqwest::Client,
    config: &NotificationConfig,
    hook: &HookConfig,
    result: &ExecutionResult,
    execution: &Execution,
) {
    // Check if outcome matches config.on
    // Build interpolated body
    // POST to URL
    // Log result
}
```

### HookConfig change

```rust
pub struct HookConfig {
    // ... existing fields ...
    pub notification: Option<NotificationConfig>,
}
```

## 5. UI Polish

### Live log streaming (SSE)

The most architecturally significant UI addition. Provides real-time log output for running executions.

**Endpoint**: `GET /executions/:id/logs/stream`

Returns a `text/event-stream` (Server-Sent Events) response that pushes new log lines as they are written.

**Implementation**:

1. Check execution status. If terminal, return the full log content as a single event and close.
2. If running, open `stdout.log` and `stderr.log`, seek to current position.
3. Tail both files using `tokio::fs::File` + periodic poll (100ms interval).
4. Each new chunk is sent as an SSE event with a `data:` prefix and event type (`stdout` or `stderr`).
5. When the execution reaches a terminal status (detected by polling the DB or watching for the file to stop growing + status check), send a `done` event and close the stream.

**SSE event format**:

```
event: stdout
data: Building project...

event: stderr
data: warning: unused variable

event: done
data: {"status": "success", "exit_code": 0}
```

**Frontend**: the execution detail page connects to the SSE endpoint for executions with status `running`. Log output appends to the `<pre>` element in real time. When a `done` event arrives, the connection closes and the page updates with the final status.

**Auth**: SSE endpoint requires `AuthUser` (session cookie). The `EventSource` API in browsers automatically sends cookies.

### Dashboard improvements

- **Hook status indicators**: each hook shows a colored dot based on the last 5 executions. Green = all succeeded. Yellow = some failures. Red = last execution failed. Grey = no executions.
- **Search/filter**: text input filters hooks by name/slug. Status dropdown filters by enabled/disabled.

### Execution history improvements

- **Status filter**: dropdown to filter executions by status (all, success, failed, timed_out, pending, running).
- **Date range filter**: start/end date inputs to narrow the history view.
- **Hook filter**: on the global executions page, dropdown to filter by hook.

### Toast notifications

HTMX-powered toast notifications for UI actions:

- Approve/reject execution
- Config import
- Backup create/restore
- Hook create/edit/delete (already exists as flash messages -- convert to toasts)

Implementation: a `<div id="toasts">` container in the base template. HTMX responses include an `HX-Trigger` header that adds a toast. JavaScript in `main.ts` handles rendering and auto-dismiss (5 seconds).

## Database Changes

No new tables for M6. The existing schema supports all features:

- Executors use the existing `executions` table.
- Config export/import is file-based (no DB).
- Backups snapshot the entire database.
- Notifications are fire-and-forget (no persistence).
- UI polish queries existing tables with new filter parameters.

## New Dependencies

| Crate | Purpose | Features |
|-------|---------|----------|
| `reqwest` (move to deps) | HTTP executor, notifications | `json`, `rustls-tls` |
| `rust-s3` | S3-compatible backup storage | default |
| `cron` | Scheduled backup parsing | default |
| `flate2` | Gzip compression for backup tarballs | default |
| `tar` | Tarball creation/extraction | default |

## New Source Files

```
src/
  executor/
    mod.rs          -- ResolvedExecutor, run() dispatch, ExecutionContext, ExecutionResult
    shell.rs        -- run_shell() (extracted from current executor.rs)
    script.rs       -- run_script() + tests
    http.rs         -- run_http() + tests
  backup/
    mod.rs          -- BackupConfig, public API
    s3.rs           -- S3 client wrapper (put, get, list, delete)
    tarball.rs      -- create/extract tar.gz
    scheduler.rs    -- cron-based scheduled backups
  notification.rs   -- send_notification() + template context builder
  routes/
    api.rs          -- /api/config/export, /api/config/import, /api/backup/*
```

Note: `executor.rs` becomes `executor/mod.rs` with submodules. The public API (`run`, `ExecutionContext`, `ExecutionResult`) is re-exported from `mod.rs` so callers don't change.

## Edge Cases

### HTTP executor timeout vs hook timeout

The hook-level `timeout` applies to the HTTP executor the same way it applies to shell: the entire operation (connection + transfer + response) must complete within the timeout. `reqwest::Client` has its own timeout; the outer `tokio::time::timeout` in the executor provides the authoritative bound.

### HTTP executor redirect loops

`follow_redirects: true` uses `reqwest`'s built-in redirect policy (max 10 redirects). Redirect loops are caught by the limit and result in a `Failed` status.

### Script path traversal

The script `path` is validated at config time to be within the configured `scripts.dir` (or an absolute path if explicitly configured). Path traversal attempts (`../../etc/passwd`) are rejected during config validation.

### Backup during active execution

`VACUUM INTO` creates a consistent snapshot of the SQLite database at a point in time. Active writes (execution status updates) that occur during the vacuum are included if committed before the vacuum starts, excluded otherwise. This is acceptable -- the backup captures a consistent state.

### Restore replaces active database

Restore requires replacing the SQLite file. The pool must be closed, the file replaced, and a new pool opened. During this window (milliseconds), incoming requests will fail. This is documented as expected behavior. The restore API should be called during maintenance windows.

### Notification to a down endpoint

If the notification URL is unreachable or returns an error, the failure is logged at `warn` level. No retry. No persistence. Notifications are best-effort.

### Config import with running hooks

Importing a config that removes or renames hooks does not affect in-flight executions -- they hold a reference to the old config snapshot via `Arc`. New triggers use the new config.

## Non-Goals

- **Executor plugins / dynamic loading** -- only the three built-in executor types. No WASM, shared library, or other plugin mechanisms.
- **Notification retry** -- notifications are fire-and-forget. No retry queue, no dead letter, no delivery guarantees.
- **Incremental backup** -- every backup is a full snapshot. No delta/incremental backup support.
- **Log file backup** -- logs are excluded from backups to keep backup size bounded. Operational log backup is the user's responsibility.
- **Backup encryption** -- backups are stored as-is. Encryption at rest is the storage provider's responsibility (S3 server-side encryption).
- **Multi-destination notifications** -- one notification config per hook. Users who need multiple destinations can chain through a webhook aggregator.
- **WebSocket log streaming** -- SSE is simpler, unidirectional (server to client), and sufficient. WebSocket adds complexity for no benefit here.
- **Prometheus metrics** -- explicitly a v1 non-goal per the design spec.
- **Built-in TLS** -- use a reverse proxy.

## Full TOML Config Example

```toml
[server]
bind = "0.0.0.0"
port = 8080

[database]
path = "data/sendword.db"

[logs]
dir = "data/logs"

[auth]
session_lifetime = "24h"
secure_cookie = false

[scripts]
dir = "scripts"

[defaults]
timeout = "30s"
rate_limit.max_per_minute = 60

[defaults.retries]
count = 2
backoff = "exponential"
initial_delay = "1s"
max_delay = "60s"

[backup]
endpoint = "https://s3.amazonaws.com"
bucket = "sendword-backups"
access_key = "${SENDWORD_BACKUP_ACCESS_KEY}"
secret_key = "${SENDWORD_BACKUP_SECRET_KEY}"
prefix = "prod/"
schedule = "0 2 * * *"
[backup.retention]
max_count = 30
max_age = "90d"

[[hooks]]
name = "Deploy Production"
slug = "deploy-prod"
description = "Deploys via script with approval gate"
enabled = true
rate_limit.max_per_minute = 5

[hooks.auth]
mode = "hmac"
header = "X-Hub-Signature-256"
algorithm = "sha256"
secret = "${GITHUB_WEBHOOK_SECRET}"

[hooks.executor]
type = "script"
path = "scripts/deploy-prod.sh"

[hooks.payload]
fields = [
    { name = "action", type = "string", required = true },
    { name = "release.tag_name", type = "string", required = true },
]

[hooks.trigger_rules]
cooldown = "10m"
# NOTE: actual field is `payload_filters` (not `filters`), operator key is `operator` (not `op`)
payload_filters = [
    { field = "action", operator = "equals", value = "released" },
]

# NOTE: actual fields are `start_time`/`end_time` (not `start`/`end`); no `tz` support (UTC only)
[[hooks.trigger_rules.time_windows]]
days = ["mon", "tue", "wed", "thu", "fri"]
start_time = "09:00"
end_time = "17:00"

[hooks.concurrency]
mode = "mutex"

[hooks.approval]
required = true
timeout = "1h"

[hooks.notification]
url = "https://hooks.slack.com/services/T.../B.../..."
on = ["success", "failure", "timeout"]
body = '{"text": "Deploy {{hook_slug}} {{status}}: {{release.tag_name}} (exit {{exit_code}}, {{duration}})"}'
[hooks.notification.headers]
Content-Type = "application/json"

[[hooks]]
name = "CI Build"
slug = "ci-build"
description = "Queued CI builds via HTTP callback"

[hooks.executor]
type = "http"
method = "POST"
url = "https://ci.internal/api/build"
body = '{"repo": "{{repo.name}}", "branch": "{{ref}}"}'
follow_redirects = true
[hooks.executor.headers]
Authorization = "Bearer ${CI_TOKEN}"
Content-Type = "application/json"

[hooks.concurrency]
mode = "queue"
queue_depth = 20

[hooks.notification]
url = "https://hooks.slack.com/services/T.../B.../..."
on = ["failure"]
body = '{"text": "CI build failed for {{repo.name}}"}'
[hooks.notification.headers]
Content-Type = "application/json"
```
