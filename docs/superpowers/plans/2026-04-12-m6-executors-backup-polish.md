# M6: Executors, Backup & Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add script and HTTP executors, config export/import, S3-compatible backups with scheduling, completion notifications, and UI polish including live log streaming. This completes the v1 feature set.

**Architecture:** The executor is refactored from a single file to a module with submodules per executor type, dispatched via enum (`ResolvedExecutor`). Backup is a self-contained subsystem (S3 client + tarball + scheduler). Notifications piggyback on the shared HTTP client. Config export/import requires `Serialize` on all config types. SSE for live logs is the only architecturally significant UI change.

**Design Spec:** `docs/superpowers/specs/2026-04-12-m6-executors-backup-polish-design.md`

**Note:** This spec was NOT reviewed by a separate reviewer. Implementers should verify assumptions against the actual codebase state and flag discrepancies.

**Parallelism:** Three independent tracks:
- Track A (tasks 1-4): Executor refactor → script → HTTP → notifications (sequential)
- Track B (tasks 5-6): Backup core → backup API/CLI/scheduler (sequential)
- Track C (tasks 7-8): Config export/import, UI polish (independent)

Tracks A, B, and C can run in parallel.

---

## File Structure

### New files to create

| File | Purpose |
|------|---------|
| `src/executor/mod.rs` | `ResolvedExecutor`, `ExecutionContext`, `ExecutionResult`, `run()` dispatch |
| `src/executor/shell.rs` | `run_shell()` extracted from current executor.rs |
| `src/executor/script.rs` | `run_script()` + tests |
| `src/executor/http.rs` | `run_http()` + tests |
| `src/notification.rs` | `send_notification()` + notification context builder |
| `src/backup/mod.rs` | Public API, `BackupConfig`, `RetentionConfig` |
| `src/backup/s3.rs` | S3 client wrapper (put, get, list, delete) |
| `src/backup/tarball.rs` | Create/extract `.tar.gz` |
| `src/backup/scheduler.rs` | Cron-based scheduled backups |
| `src/routes/api.rs` | `/api/config/*`, `/api/backup/*` endpoints |

### Existing files to modify

| File | Change |
|------|--------|
| `src/executor.rs` | Renamed to `src/executor/mod.rs` (git mv) |
| `src/lib.rs` | Add `pub mod notification;`, `pub mod backup;` |
| `src/config.rs` | Add `Serialize` to all config types; add `HttpMethod`, `NotificationConfig`, `NotifyOutcome`, `BackupConfig`, `RetentionConfig`; add `notification` to `HookConfig`; add `backup` to `AppConfig`; expand `ExecutorConfig` enum |
| `src/server.rs` | Add `http_client: reqwest::Client` to `AppState` |
| `src/config_writer.rs` | Add Script and Http executor serialization to `apply_hook_fields()` |
| `src/routes/hooks.rs` | Update `trigger_hook()` to resolve `ResolvedExecutor` from config + payload |
| `src/routes/executions.rs` | Add SSE log streaming endpoint; update replay to use `ResolvedExecutor` |
| `src/routes/mod.rs` | Add `mod api;` and merge API routes |
| `src/main.rs` | Add `Export`, `Import`, `Backup`, `Restore` CLI commands |
| `Cargo.toml` | Move `reqwest` to deps; add `rust-s3`, `cron`, `flate2`, `tar` |
| `templates/execution_detail.html` | Add SSE connection for running executions |
| `templates/base.html` | Add toast container |
| `static/ts/main.ts` | Add SSE handler + toast JS |

---

## Track A: Executors + Notifications

---

### Task 1: Executor module refactor

**Commit message:** `refactor: split executor.rs into executor module with shell submodule`

This is a pure refactor. Zero behavior change. The riskiest task -- touches the most files.

**Files:**
- Rename: `src/executor.rs` → `src/executor/mod.rs`
- Create: `src/executor/shell.rs`
- Modify: `src/routes/hooks.rs`, `src/routes/executions.rs`, `src/retry.rs`

**Steps:**

- [ ] Use `git mv src/executor.rs src/executor/mod.rs` to preserve history. Create the `src/executor/` directory first.

- [ ] In `src/executor/mod.rs`, replace `command: String` on `ExecutionContext` with `executor: ResolvedExecutor`:

```rust
#[derive(Clone)]
pub enum ResolvedExecutor {
    Shell { command: String },
}

#[derive(Clone)]
pub struct ExecutionContext {
    pub execution_id: String,
    pub hook_slug: String,
    pub executor: ResolvedExecutor,  // was: pub command: String
    pub env: HashMap<String, String>,
    pub cwd: Option<String>,
    pub timeout: Duration,
    pub logs_dir: String,
    pub payload_json: String,
}
```

Note: Only the `Shell` variant exists at this point. Script and Http are added in tasks 2-3.

- [ ] Extract the shell-specific logic from `run()` into `src/executor/shell.rs`:

```rust
// shell.rs
pub async fn run_shell(pool: &SqlitePool, ctx: &ExecutionContext, command: &str) -> ExecutionResult
```

Move the process spawning, stdout/stderr capture, timeout handling, and DB status updates into `run_shell`. Keep `prepare_log_files()` and `system_env_vars()` in `mod.rs` (shared by all executors).

- [ ] Update `run()` in `mod.rs` to dispatch:

```rust
pub async fn run(pool: &SqlitePool, ctx: ExecutionContext) -> ExecutionResult {
    match &ctx.executor {
        ResolvedExecutor::Shell { command } => shell::run_shell(pool, &ctx, command).await,
    }
}
```

- [ ] Re-export public types from `mod.rs`:

```rust
pub mod shell;

pub use self::shell::run_shell; // if needed by tests
```

- [ ] Update `src/routes/hooks.rs` -- where `ExecutionContext` is constructed, change `command: command` to `executor: ResolvedExecutor::Shell { command }`.

- [ ] Update `src/routes/executions.rs` replay handler -- same change.

- [ ] Update `src/retry.rs` -- `ExecutionContext` is cloned during retries. Verify `ResolvedExecutor` derives `Clone`. No logic change needed.

- [ ] Run `cargo check` to verify compilation.
- [ ] Run `cargo test` to verify all existing tests pass (pure refactor).

---

### Task 2: Script file executor

**Commit message:** `feat: add script file executor`

**Depends on:** Task 1.

**Files:**
- Create: `src/executor/script.rs`
- Modify: `src/config.rs`, `src/executor/mod.rs`, `src/config_writer.rs`, `src/routes/hooks.rs`

**Steps:**

- [ ] In `src/config.rs`, expand `ExecutorConfig`:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutorConfig {
    Shell { command: String },
    Script { path: String },
}
```

- [ ] Add `Script` variant to `ResolvedExecutor` in `src/executor/mod.rs`:

```rust
pub enum ResolvedExecutor {
    Shell { command: String },
    Script { path: PathBuf },
}
```

- [ ] Update `run()` dispatch:

```rust
ResolvedExecutor::Script { path } => script::run_script(pool, &ctx, path).await,
```

- [ ] Create `src/executor/script.rs`:

```rust
pub async fn run_script(pool: &SqlitePool, ctx: &ExecutionContext, path: &Path) -> ExecutionResult
```

Implementation:
1. Call `prepare_log_files()` (shared).
2. Call `execution::mark_running()`.
3. Build command: `tokio::process::Command::new(path)` (not via `sh -c`).
4. Set env: system vars + hook env + `SENDWORD_*` vars + payload field vars (`SENDWORD_FIELD_<NAME>=value` -- uppercase, dots → underscores).
5. Pipe stdout/stderr to log files.
6. Apply timeout via `tokio::time::timeout`.
7. Mark completed with status/exit_code.

The payload field env var logic: parse `ctx.payload_json` as `serde_json::Value`, walk all leaf values, and set `SENDWORD_FIELD_{UPPERCASED_PATH}=value`.

- [ ] Add config validation in `AppConfig::validate()`: for `Script` executor, check the script file exists and is executable. Use `std::fs::metadata()` to check existence and `std::os::unix::fs::PermissionsExt` for execute bit.

- [ ] Update `src/config_writer.rs` `apply_hook_fields()` to handle the `Script` variant when serializing to TOML.

- [ ] Update `src/routes/hooks.rs` to resolve `Script` config into `ResolvedExecutor::Script { path: PathBuf::from(&path) }`.

- [ ] Run `cargo check`.

**Tests** (in `script.rs`):

- [ ] `script_executes_and_captures_output` -- write a temp script that echoes, verify stdout.log
- [ ] `script_passes_payload_env_vars` -- script prints `$SENDWORD_FIELD_ACTION`, verify output
- [ ] `script_nonexistent_path_fails` -- spawn fails gracefully, marks as Failed
- [ ] `script_timeout_kills_process` -- slow script + short timeout = TimedOut
- [ ] `script_exit_code_captured` -- script exits with code 42, verify exit_code

---

### Task 3: HTTP executor + shared HTTP client

**Commit message:** `feat: add HTTP executor with shared reqwest client`

**Depends on:** Task 1.

**Files:**
- Create: `src/executor/http.rs`
- Modify: `Cargo.toml`, `src/config.rs`, `src/executor/mod.rs`, `src/server.rs`, `src/config_writer.rs`, `src/routes/hooks.rs`

**Steps:**

- [ ] In `Cargo.toml`, `reqwest` is already in `[dependencies]` with `["json"]`. Add `"rustls-tls"` to the features list:

```toml
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }
```

No move from dev-dependencies needed — it's already a production dependency.

- [ ] In `src/config.rs`, add `HttpMethod` and expand `ExecutorConfig`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get, Post, Put, Patch, Delete,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutorConfig {
    Shell { command: String },
    Script { path: String },
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
```

- [ ] Add `Http` variant to `ResolvedExecutor`:

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

- [ ] In `src/server.rs`, add `http_client: reqwest::Client` to `AppState` and initialize in `AppState::new()`:

```rust
pub struct AppState {
    pub config: ArcSwap<AppConfig>,
    pub config_writer: ConfigWriter,
    pub db: Db,
    pub templates: Templates,
    pub http_client: reqwest::Client,
}
```

Build the client with `reqwest::Client::builder().build().unwrap_or_default()`.

- [ ] Create `src/executor/http.rs`:

```rust
pub async fn run_http(pool: &SqlitePool, ctx: &ExecutionContext, client: &reqwest::Client) -> ExecutionResult
```

**Decision: add `http_client: Option<reqwest::Client>` to `ExecutionContext`.** Shell and script set it to `None`; HTTP sets it to `Some(client.clone())` at dispatch time in `trigger_hook`. This keeps the `run(pool, ctx)` signature unchanged (the retry module clones `ExecutionContext` without knowing which executor type is active). `reqwest::Client` is cheaply clonable (Arc internally). Alternative of changing `run()` to take an extra `Option<&reqwest::Client>` would leak HTTP concern into retry.rs — avoid that.

Implementation:
1. Call `prepare_log_files()`.
2. Call `execution::mark_running()`.
3. Build the request from `ResolvedExecutor::Http` fields.
4. Resolve `${ENV_VAR}` references in header values.
5. Apply timeout via `tokio::time::timeout`.
6. Execute request.
7. Write response body to `stdout.log` (truncated to 4KB).
8. Write request/response metadata to `stderr.log` (method, URL, status, timing, response headers).
9. Determine status: 2xx → Success, else → Failed. Set `exit_code` to HTTP status code as i32.
10. Mark completed.

- [ ] Update `src/routes/hooks.rs` to resolve `Http` config into `ResolvedExecutor::Http { .. }`, applying `{{field}}` interpolation to URL and body.

- [ ] Update `src/config_writer.rs` `apply_hook_fields()` to handle the `Http` variant.

- [ ] Run `cargo check`.

**Tests** (in `http.rs`):

- [ ] `http_200_succeeds` -- mock/stub server returns 200, verify Success + exit_code 200
- [ ] `http_500_fails` -- server returns 500, verify Failed + exit_code 500
- [ ] `http_timeout` -- server hangs, timeout fires, verify TimedOut
- [ ] `http_logs_response_body` -- verify stdout.log contains response body
- [ ] `http_logs_metadata` -- verify stderr.log contains method, URL, status

Note: HTTP tests can use a local TCP listener (`tokio::net::TcpListener`) serving canned responses, or a lightweight mock. Avoid external dependencies for tests.

---

### Task 4: Completion notifications

**Commit message:** `feat: add completion notifications for hook executions`

**Depends on:** Task 3 (shared HTTP client).

**Files:**
- Create: `src/notification.rs`
- Modify: `src/lib.rs`, `src/config.rs`, `src/routes/hooks.rs`

**Steps:**

- [ ] In `src/lib.rs`, add `pub mod notification;`.

- [ ] In `src/config.rs`, add notification types:

```rust
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

#[derive(Debug, Clone, Deserialize)]
pub struct NotificationConfig {
    pub url: String,
    #[serde(default = "default_notify_on")]
    pub on: Vec<NotifyOutcome>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    pub body: String,
}
```

- [ ] Add `notification: Option<NotificationConfig>` to `HookConfig` with `#[serde(default)]`.

**NOTE:** Adding a field to `HookConfig` requires updating ALL `HookConfig { ... }` struct literals in `tests/server_integration.rs` and elsewhere with `notification: None`. The same pattern applied when `concurrency`/`approval` were added in M5 batch 1. Search for `HookConfig {` in test files and add the new field.

- [ ] Create `src/notification.rs`:

```rust
use crate::config::{HookConfig, NotificationConfig, NotifyOutcome};
use crate::executor::ExecutionResult;
use crate::models::execution::Execution;
use crate::models::ExecutionStatus;
use crate::interpolation::interpolate_command;

/// Build a JSON context for notification template interpolation.
fn notification_context(
    hook: &HookConfig,
    result: &ExecutionResult,
    execution: &Execution,
) -> serde_json::Value {
    serde_json::json!({
        "hook_name": hook.name,
        "hook_slug": hook.slug,
        "status": result.status.to_string(),
        "exit_code": result.exit_code.map(|c| c.to_string()).unwrap_or_default(),
        "execution_id": execution.id,
        "duration": compute_duration(execution),
        "trigger_source": execution.trigger_source,
    })
}

/// Map ExecutionStatus to NotifyOutcome for matching.
fn status_to_outcome(status: &ExecutionStatus) -> Option<NotifyOutcome> {
    match status {
        ExecutionStatus::Success => Some(NotifyOutcome::Success),
        ExecutionStatus::Failed => Some(NotifyOutcome::Failure),
        ExecutionStatus::TimedOut => Some(NotifyOutcome::Timeout),
        _ => None,  // Rejected, Expired, etc. don't trigger notifications
    }
}

/// Send a completion notification if configured and the outcome matches.
pub async fn send_notification(
    client: &reqwest::Client,
    config: &NotificationConfig,
    hook: &HookConfig,
    result: &ExecutionResult,
    execution: &Execution,
) {
    // 1. Check if outcome matches config.on
    let Some(outcome) = status_to_outcome(&result.status) else { return };
    if !config.on.contains(&outcome) { return; }

    // 2. Build interpolated body
    let context = notification_context(hook, result, execution);
    let body = interpolate_command(&config.body, &context);

    // 3. POST to URL with headers, 10s timeout
    let mut req = client.post(&config.url).timeout(std::time::Duration::from_secs(10));
    for (k, v) in &config.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    req = req.body(body.into_owned());

    match req.send().await {
        Ok(resp) => tracing::info!(
            hook_slug = %hook.slug,
            status = resp.status().as_u16(),
            "notification sent"
        ),
        Err(e) => tracing::warn!(
            hook_slug = %hook.slug,
            "notification failed: {e}"
        ),
    }
}
```

- [ ] In `src/routes/hooks.rs`, in the spawned task after `run_with_retries` completes, add notification dispatch:

```rust
// After result = retry::run_with_retries(...)
if let Some(ref notification_config) = notification_config {
    if let Ok(exec) = execution::get_by_id(&pool, &ctx.execution_id).await {
        notification::send_notification(
            &http_client, notification_config, &hook_snapshot, &result, &exec,
        ).await;
    }
}
```

The spawned task must capture `notification_config: Option<NotificationConfig>`, `hook_snapshot: HookConfig` (or relevant fields), and `http_client: reqwest::Client` from the handler's state.

- [ ] Run `cargo check`.

**Tests** (in `notification.rs`):

- [ ] `notification_context_builds_all_fields` -- verify all template variables present
- [ ] `status_to_outcome_maps_correctly` -- Success/Failed/TimedOut map; Rejected returns None
- [ ] `notification_not_sent_when_outcome_not_in_on` -- Success result, `on: [failure]`, no POST made

---

## Track B: Backup

---

### Task 5: S3 backup core

**Commit message:** `feat: add S3 backup client with tarball and snapshot support`

**Files:**
- Create: `src/backup/mod.rs`, `src/backup/s3.rs`, `src/backup/tarball.rs`
- Modify: `Cargo.toml`, `src/lib.rs`, `src/config.rs`

**Steps:**

- [ ] In `Cargo.toml`, add dependencies:

```toml
rust-s3 = "0.35"
cron = "0.13"
flate2 = "1"
tar = "0.4"
```

- [ ] In `src/lib.rs`, add `pub mod backup;`.

- [ ] In `src/config.rs`, add backup config types:

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RetentionConfig {
    pub max_count: Option<u32>,
    #[serde(default, with = "humantime_serde::option")]
    pub max_age: Option<Duration>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackupConfig {
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    #[serde(default)]
    pub prefix: String,
    pub schedule: Option<String>,
    #[serde(default)]
    pub retention: RetentionConfig,
}
```

Add `backup: Option<BackupConfig>` to `AppConfig`.

- [ ] Add config validation: if `backup.schedule` is set, validate it parses as a cron expression.

- [ ] Create `src/backup/s3.rs` -- wrapper around `rust-s3`:

```rust
pub struct S3Client { bucket: s3::Bucket }

impl S3Client {
    pub fn new(config: &BackupConfig) -> Result<Self, ...>
    pub async fn put(&self, key: &str, data: &[u8]) -> Result<(), ...>
    pub async fn get(&self, key: &str) -> Result<Vec<u8>, ...>
    pub async fn list(&self, prefix: &str) -> Result<Vec<BackupEntry>, ...>
    pub async fn delete(&self, key: &str) -> Result<(), ...>
}

pub struct BackupEntry {
    pub key: String,
    pub size: u64,
    pub last_modified: String,
}
```

- [ ] Create `src/backup/tarball.rs`:

```rust
/// Create a .tar.gz from config files + DB snapshot.
pub fn create_tarball(
    config_path: &Path,
    json_config_path: Option<&Path>,
    db_snapshot_path: &Path,
    output_path: &Path,
) -> std::io::Result<()>

/// Extract a .tar.gz to a directory.
pub fn extract_tarball(tarball_path: &Path, output_dir: &Path) -> std::io::Result<()>
```

- [ ] Create `src/backup/mod.rs` with the public API:

```rust
/// Create a backup: snapshot DB, bundle config + DB, upload to S3.
pub async fn create_backup(pool: &SqlitePool, config: &BackupConfig, config_path: &Path) -> Result<String, BackupError>

/// Restore from a backup key: download, extract, validate, replace.
pub async fn restore_backup(config: &BackupConfig, key: &str, config_path: &Path, db_path: &Path) -> Result<(), BackupError>

/// Apply retention policy: delete old backups exceeding count/age.
pub async fn apply_retention(config: &BackupConfig) -> Result<(), BackupError>

/// List available backups.
pub async fn list_backups(config: &BackupConfig) -> Result<Vec<BackupEntry>, BackupError>
```

The DB snapshot uses `VACUUM INTO`:

```rust
let snapshot_path = temp_dir.join("sendword.db");
sqlx::query(&format!("VACUUM INTO '{}'", snapshot_path.display()))
    .execute(pool)
    .await?;
```

- [ ] Run `cargo check`.

**Tests** (in `tarball.rs` -- S3 tests require a live endpoint, so unit-test tarball logic only):

- [ ] `create_and_extract_roundtrip` -- create tarball from temp files, extract, verify contents match
- [ ] `extract_handles_missing_json_config` -- tarball without sendword.json extracts fine

---

### Task 6: Backup API + CLI + scheduled backups

**Commit message:** `feat: add backup API, CLI commands, and scheduled backups`

**Depends on:** Task 5.

**Files:**
- Create: `src/backup/scheduler.rs`, `src/routes/api.rs` (or add to existing)
- Modify: `src/routes/mod.rs`, `src/main.rs`

**Steps:**

- [ ] Create `src/routes/api.rs` with backup endpoints:

```rust
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/backup", post(create_backup))
        .route("/api/backup/list", get(list_backups))
        .route("/api/backup/restore", post(restore_backup))
}
```

Handlers: each requires `AuthUser`. `restore_backup` requires `{"confirm": true}` in request body. Restore writes to temp path first, validates, then atomically replaces files.

- [ ] In `src/routes/mod.rs`, add `mod api;` and merge the router.

- [ ] Create `src/backup/scheduler.rs`:

```rust
// NOTE: Takes Arc<AppState> for consistency with the M5 approval sweep pattern.
// AppState.config is ArcSwap<AppConfig> (not separately Arc-wrapped).
// AppState.http_client is also available here if needed for notifications.
pub fn spawn_backup_scheduler(
    state: Arc<AppState>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let config = state.config.load();
            let Some(backup) = &config.backup else {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            };
            let Some(schedule_str) = &backup.schedule else {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            };
            // Parse cron, compute next occurrence, sleep until then, run backup
            // After backup, apply retention
        }
    })
}
```

- [ ] In `src/main.rs`, add CLI commands:

```rust
Export,
Import { path: String },
Backup {
    #[command(subcommand)]
    action: BackupAction,
},
Restore {
    #[arg(long)]
    from: String,
},
```

Implement `export()`: load config, serialize as JSON, print to stdout.
Implement `import(path)`: read JSON, deserialize + validate, write as TOML to config path.
Implement `backup_create()`: load config, connect DB, create backup.
Implement `backup_list()`: load config, list from S3.
Implement `restore(key)`: load config, download + restore.

- [ ] Start the backup scheduler in `serve()` if `backup.schedule` is configured.

- [ ] Run `cargo check`.

**Tests:**

- [ ] `export_produces_valid_json` -- load config, export, parse back, verify roundtrip
- [ ] `import_validates_before_writing` -- import invalid JSON, verify config file unchanged
- [ ] Backup API + scheduler tests require S3 access -- skip in CI, document manual test procedure

---

## Track C: Config Export + UI Polish

---

### Task 7: Config export/import API + CLI

**Commit message:** `feat: add config export/import API and CLI commands`

**Files:**
- Modify: `src/config.rs`, `src/routes/api.rs`, `src/main.rs`

**Steps:**

- [ ] Add `#[derive(Serialize)]` to ALL config types in `src/config.rs`:

Types to update: `AppConfig`, `ServerConfig`, `DatabaseConfig`, `LogsConfig`, `AuthConfig`, `ScriptsConfig`, `RateLimitConfig`, `BackoffStrategy`, `RetryConfig`, `DefaultsConfig`, `ExecutorConfig`, `HmacAlgorithm`, `HookAuthConfig`, `HookConfig`, `TriggerRules`, `PayloadFilter`, `FilterOperator`, `TimeWindow`, `TriggerRateLimit`, `ConcurrencyConfig`, `ConcurrencyMode`, `ApprovalConfig`, `NotificationConfig`, `NotifyOutcome`, `HttpMethod`, `BackupConfig`, `RetentionConfig`, and any sub-types.

Also add `Serialize` to `PayloadSchema`, `PayloadField`, `FieldType` in `src/payload.rs` and `MaskingConfig` in `src/masking.rs`.

Run `cargo check` after each batch. Fix any types where `Serialize` doesn't derive cleanly (e.g., types using deserialize-only serde helpers). `humantime_serde` supports both, so Duration fields should work.

- [ ] Add export/import endpoints to `src/routes/api.rs`:

```rust
GET  /api/config/export    -- serialize config as JSON, return
POST /api/config/import    -- deserialize, validate, write to disk, reload
```

Export: `serde_json::to_string_pretty(&*state.config.load())`.
Import flow:
1. Deserialize request body as `AppConfig` via `serde_json::from_str`.
2. Call `config.validate()` — if errors, return 422.
3. Convert validated config to TOML via `toml::to_string` and write to the config path (available via `state.config_writer.path()`).
4. Call `state.reload_config()` which re-reads from disk using `AppConfig::load_from(toml_path, json_path)`.
5. Return 200.

Note: `AppConfig::load_from` takes a TOML path and an optional JSON overlay path. Import writes the JSON-sourced config as TOML (step 3), so reload reads the updated TOML. Do NOT call `AppConfig::load_from` directly with JSON — it expects TOML as the primary format.

- [ ] Add CLI commands in `src/main.rs` (if not already added in task 6):

`sendword export` -- loads config, prints JSON.
`sendword import <path>` -- reads JSON, validates, writes TOML.

- [ ] Run `cargo check` and `cargo test`.

**Tests:**

- [ ] `export_roundtrip` -- export config as JSON, parse back, verify key fields match
- [ ] `import_invalid_rejects` -- POST invalid JSON, verify 422 and config unchanged
- [ ] `import_valid_updates_and_reloads` -- POST valid config, verify config reloaded

---

### Task 8: UI polish

**Commit message:** `feat: add live log streaming, dashboard indicators, and toast notifications`

**Files:**
- Modify: `src/routes/executions.rs`, `templates/execution_detail.html`, `templates/base.html`, `templates/dashboard.html`, `static/ts/main.ts`

**Steps:**

- [ ] **SSE log streaming** -- add endpoint in `src/routes/executions.rs`:

```rust
GET /executions/:id/logs/stream
```

Implementation:
1. Look up execution by ID.
2. If terminal status: send full log content as one `stdout` event + `done` event, close.
3. If running: open log files, seek to end, enter tail loop (100ms poll interval).
4. Each new chunk → SSE event with `event: stdout` or `event: stderr`.
5. Poll DB for status change. On terminal → send `done` event with status JSON, close.
6. Return `Content-Type: text/event-stream` with `Cache-Control: no-cache`.

Use `axum::response::Sse` with a `tokio::sync::mpsc` channel or `async_stream`.

- [ ] **Frontend SSE handler** in `static/ts/main.ts`:

```typescript
// On execution detail page, if status is "running":
const source = new EventSource(`/executions/${id}/logs/stream`);
source.addEventListener('stdout', (e) => { /* append to pre.stdout */ });
source.addEventListener('stderr', (e) => { /* append to pre.stderr */ });
source.addEventListener('done', (e) => { /* update status, close source */ });
```

- [ ] **Dashboard hook status indicators** in `templates/dashboard.html`:

Add a colored dot next to each hook based on recent execution status. Query: for each hook, fetch the last 5 executions' statuses. Green = all success. Yellow = mixed. Red = last failed. Grey = none. The query can be done in the dashboard handler and passed to the template.

- [ ] **Execution history filters** -- add status/date/hook dropdown filters to the execution list pages. These are query parameter filters on existing list queries. Add `status`, `from_date`, `to_date` parameters to the relevant handler.

- [ ] **Toast notifications** in `templates/base.html`:

Add `<div id="toasts" class="fixed top-4 right-4 z-50"></div>` to the base template.

In `static/ts/main.ts`, add a `showToast(message, type)` function. Listen for `htmx:afterOnLoad` events and check for `HX-Trigger: showToast` header. Auto-dismiss after 5 seconds.

Update handlers (approve, reject, hook CRUD, backup) to include `HX-Trigger: showToast` header in responses.

- [ ] Run `cargo check` and `cargo test`.

**Tests:**

- [ ] `sse_returns_done_for_terminal_execution` -- GET stream for completed execution, verify `done` event
- [ ] `dashboard_shows_status_indicators` -- create executions with various statuses, verify dashboard HTML contains status classes

---

## Verification

After all 8 tasks are complete:

- [ ] `cargo check` passes with no warnings
- [ ] `cargo test` passes -- all existing tests + new tests
- [ ] `cargo clippy` passes with no warnings
- [ ] Manual test: trigger hooks with shell, script, and HTTP executors
- [ ] Manual test: export config, modify, import, verify reload
- [ ] Manual test: create backup (requires S3 endpoint), list, restore
- [ ] Manual test: verify notifications fire on execution completion
- [ ] Manual test: view live log streaming on execution detail page
