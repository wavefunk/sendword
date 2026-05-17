use std::collections::HashMap;
use std::convert::Infallible;
use std::path::Path as FsPath;
use std::path::PathBuf;
use std::sync::Arc;

use async_stream::stream;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_core::Stream;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};

use crate::barriers::{self, execution_lock, execution_queue};
use crate::error::{AppError, DbError};
use crate::executor::resolve_executor;
use crate::extractors::AuthUser;
use crate::masking::{MaskingConfig, mask_secrets};
use crate::models::execution;
use crate::retry;
use crate::server::AppState;
use crate::views::approvals::{ApprovalRow, ApprovalsPage, render_approvals_page};
use crate::views::execution_detail::{ExecutionDetailPage, render_execution_detail_page};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/executions/{id}", get(execution_detail))
        .route("/executions/{id}/logs/stream", get(log_stream))
        .route("/executions/{id}/replay", post(replay_execution))
        .route("/executions/{id}/approve", post(approve_execution))
        .route("/executions/{id}/reject", post(reject_execution))
        .route("/approvals", get(list_pending_approvals))
}

/// HTML-escape a string for safe insertion via HTMX SSE swap.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn mask_and_escape(
    text: &str,
    masking: &MaskingConfig,
    hook_env: &HashMap<String, String>,
) -> String {
    if masking.env_vars.is_empty() && masking.compiled_patterns.is_empty() {
        return html_escape(text);
    }

    html_escape(&mask_secrets(text, masking, hook_env))
}

fn mask_resolved_env_values_and_escape(text: &str, secret_values: &[String]) -> String {
    if secret_values.is_empty() {
        return html_escape(text);
    }

    let mut masked = text.to_owned();
    for value in secret_values {
        masked = masked.replace(value, "***");
    }

    html_escape(&masked)
}

fn stream_mask_tail_chars(secret_values: &[String]) -> usize {
    secret_values
        .iter()
        .map(|value| value.chars().count())
        .max()
        .unwrap_or(0)
}

fn stream_mask_secret_values(
    masking: &MaskingConfig,
    hook_env: &HashMap<String, String>,
) -> Vec<String> {
    masking
        .env_vars
        .iter()
        .filter_map(|name| {
            hook_env
                .get(name.as_str())
                .cloned()
                .or_else(|| std::env::var(name).ok())
        })
        .filter(|value| !value.is_empty())
        .collect()
}

#[derive(Clone, Debug)]
struct LogStreamMasker {
    secret_values: Vec<String>,
    buffer: String,
    tail_chars: usize,
}

impl LogStreamMasker {
    fn new(masking: &MaskingConfig, hook_env: HashMap<String, String>) -> Self {
        let secret_values = stream_mask_secret_values(masking, &hook_env);
        let tail_chars = stream_mask_tail_chars(&secret_values);
        Self {
            secret_values,
            buffer: String::new(),
            tail_chars,
        }
    }

    fn push(&mut self, text: &str) -> Option<String> {
        self.buffer.push_str(text);

        let emit_len =
            safe_emit_prefix_len(&self.buffer, self.tail_chars, self.secret_values.as_slice());
        if emit_len == 0 {
            return None;
        }

        let tail = self.buffer.split_off(emit_len);
        let chunk = std::mem::replace(&mut self.buffer, tail);
        Some(mask_resolved_env_values_and_escape(
            &chunk,
            self.secret_values.as_slice(),
        ))
        .filter(|value| !value.is_empty())
    }

    fn flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }

        let chunk = std::mem::take(&mut self.buffer);
        Some(mask_resolved_env_values_and_escape(
            &chunk,
            self.secret_values.as_slice(),
        ))
        .filter(|value| !value.is_empty())
    }
}

const LOG_TAIL_READ_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct LogTail {
    path: PathBuf,
    file: Option<tokio::fs::File>,
    offset: u64,
}

impl LogTail {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            file: None,
            offset: 0,
        }
    }

    async fn read_new_text(&mut self) -> Option<String> {
        if self.file.is_none() {
            self.file = tokio::fs::File::open(&self.path).await.ok();
        }

        let file = self.file.as_mut()?;
        let len = match file.metadata().await {
            Ok(metadata) => metadata.len(),
            Err(_) => {
                self.file = None;
                self.offset = 0;
                return None;
            }
        };

        if len < self.offset {
            self.offset = 0;
        }

        if len == self.offset {
            return None;
        }

        let remaining = len.saturating_sub(self.offset);
        let read_len = remaining.min(LOG_TAIL_READ_CHUNK_BYTES as u64) as usize;
        if file.seek(SeekFrom::Start(self.offset)).await.is_err() {
            self.file = None;
            self.offset = 0;
            return None;
        }

        let mut bytes = vec![0; read_len];
        let bytes_read = match file.read(&mut bytes).await {
            Ok(bytes_read) => bytes_read,
            Err(_) => {
                self.file = None;
                self.offset = 0;
                return None;
            }
        };

        if bytes_read == 0 {
            return None;
        }

        bytes.truncate(bytes_read);
        self.offset = self.offset.saturating_add(bytes_read as u64);

        String::from_utf8(bytes).ok()
    }
}

fn safe_emit_prefix_len(text: &str, tail_chars: usize, secret_values: &[String]) -> usize {
    let mut emit_len = emit_prefix_len(text, tail_chars);

    loop {
        if emit_len == 0 {
            return 0;
        }

        let adjusted = adjust_emit_len_for_secret_prefixes(text, emit_len, secret_values);
        if adjusted == emit_len {
            return emit_len;
        }
        emit_len = adjusted;
    }
}

fn adjust_emit_len_for_secret_prefixes(
    text: &str,
    emit_len: usize,
    secret_values: &[String],
) -> usize {
    let emitted = &text[..emit_len];
    let mut adjusted = emit_len;

    for secret in secret_values {
        for (prefix_end, _) in secret.char_indices().skip(1) {
            let prefix = &secret[..prefix_end];
            if emitted.ends_with(prefix) {
                adjusted = adjusted.min(emit_len - prefix.len());
            }
        }
    }

    adjusted
}

fn emit_prefix_len(text: &str, tail_chars: usize) -> usize {
    if tail_chars == 0 {
        return text.len();
    }

    let mut retained_chars = 0;
    for (index, _) in text.char_indices().rev() {
        retained_chars += 1;
        if retained_chars == tail_chars {
            return index;
        }
    }

    0
}

/// Read a log file, returning non-empty contents when present.
async fn read_log_file(logs_dir: &str, execution_id: &str, filename: &str) -> Option<String> {
    let path = FsPath::new(logs_dir).join(execution_id).join(filename);
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) if !contents.is_empty() => Some(contents),
        _ => None,
    }
}

async fn masked_log_file_event(
    logs_dir: &str,
    execution_id: &str,
    filename: &str,
    event_name: &'static str,
    masking: &MaskingConfig,
    hook_env: &HashMap<String, String>,
) -> Option<Event> {
    let contents = read_log_file(logs_dir, execution_id, filename).await?;
    Some(
        Event::default()
            .event(event_name)
            .data(mask_and_escape(&contents, masking, hook_env)),
    )
}

fn done_event(status: &str) -> Event {
    let tag_class = match status {
        "success" => "ok",
        "failed" => "err",
        _ => "",
    };
    Event::default().event("done").data(format!(
        r#"<span class="wf-tag {tag_class}"><span class="dot"></span>{}</span>"#,
        status.to_uppercase()
    ))
}

/// Compute a human-readable duration string from ISO8601 timestamps.
fn compute_duration(started_at: &Option<String>, completed_at: &Option<String>) -> Option<String> {
    let started = started_at.as_ref()?;
    let completed = completed_at.as_ref()?;

    let start = chrono::DateTime::parse_from_rfc3339(started).ok()?;
    let end = chrono::DateTime::parse_from_rfc3339(completed).ok()?;
    let dur = end.signed_duration_since(start);

    let secs = dur.num_seconds();
    if secs < 0 {
        return None;
    }

    if secs < 60 {
        let ms = dur.num_milliseconds() % 1000;
        Some(format!("{secs}.{ms:03}s"))
    } else if secs < 3600 {
        Some(format!("{}m {}s", secs / 60, secs % 60))
    } else {
        Some(format!("{}h {}m", secs / 3600, (secs % 3600) / 60))
    }
}

async fn execution_detail(
    AuthUser(auth): AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Html<String>, AppError> {
    let config = state.config.load();
    let pool = state.db.pool();

    let exec = execution::get_by_id(pool, &id).await.map_err(|e| match e {
        crate::error::DbError::NotFound(_) => AppError::not_found("execution"),
        other => AppError::from(other),
    })?;

    let logs_dir = &config.logs.dir;
    let (stdout, stderr) = if exec.status == execution::ExecutionStatus::Running {
        (String::new(), String::new())
    } else {
        let stdout = read_log_file(logs_dir, &exec.id, "stdout.log")
            .await
            .unwrap_or_else(|| "No output captured.".into());
        let stderr = read_log_file(logs_dir, &exec.id, "stderr.log")
            .await
            .unwrap_or_default();

        // Apply secret masking to log output before rendering.
        // If the hook has been removed from config, hook_env is empty and only
        // system env vars and regex patterns are used for masking.
        let hook_env = config
            .hooks
            .iter()
            .find(|h| h.slug == exec.hook_slug)
            .map(|h| &h.env)
            .cloned()
            .unwrap_or_default();
        (
            mask_secrets(&stdout, &config.masking, &hook_env),
            mask_secrets(&stderr, &config.masking, &hook_env),
        )
    };

    let duration = compute_duration(&exec.started_at, &exec.completed_at);

    let page = ExecutionDetailPage::new(
        exec.id,
        exec.hook_slug,
        &exec.status,
        exec.exit_code,
        exec.triggered_at,
        exec.started_at,
        exec.completed_at,
        duration,
        exec.trigger_source,
        exec.retry_count,
        exec.retry_of,
        stdout,
        stderr,
    );

    render_execution_detail_page(auth.email.as_str(), &page)
}

#[derive(Serialize)]
struct ReplayResponse {
    execution_id: String,
}

/// Re-trigger an execution with the same payload.
///
/// Looks up the original execution, clones its payload, creates a new execution
/// record linked via `retry_of`, and spawns the executor in a detached task.
async fn replay_execution(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ReplayResponse>, AppError> {
    let config = state.config.load();
    let pool = state.db.pool();

    // 1. Look up the original execution
    let original = execution::get_by_id(pool, &id)
        .await
        .map_err(|_| AppError::not_found("execution"))?;

    // 2. Look up the hook config (must still exist)
    let hook = config
        .hooks
        .iter()
        .find(|h| h.slug == original.hook_slug)
        .ok_or(AppError::not_found("hook"))?;

    // 3. Prepare execution parameters from hook config
    let timeout = hook.timeout.unwrap_or(config.defaults.timeout);
    let resolved_executor = resolve_executor(&hook.executor, &original.request_payload);

    let env = hook.env.clone();
    let cwd = hook.cwd.clone();
    let logs_dir = config.logs.dir.clone();
    let notification_config = hook.notification.clone();
    let hook_snapshot = hook.clone();

    let retry_config = retry::resolve_retry_config(hook, &config.defaults.retries);

    // 4. Create a new execution record linked to the original
    let exec_id = crate::id::new_id();
    let log_path = format!("{logs_dir}/{exec_id}");

    let exec = execution::create(
        pool,
        &execution::NewExecution {
            id: Some(&exec_id),
            hook_slug: &original.hook_slug,
            log_path: &log_path,
            trigger_source: &original.trigger_source,
            request_payload: &original.request_payload,
            retry_of: Some(&original.id),
            status: None,
        },
    )
    .await?;

    // 5. Build execution context and spawn in a detached task
    let ctx = crate::executor::ExecutionContext {
        execution_id: exec.id.clone(),
        hook_slug: original.hook_slug,
        executor: resolved_executor,
        env,
        cwd,
        timeout,
        logs_dir,
        payload_json: original.request_payload,
        http_client: Some(state.http_client.clone()),
    };

    let pool = pool.clone();
    let state_clone = Arc::clone(&state);
    let execution_id = exec.id.clone();
    tokio::spawn(async move {
        let result = retry::run_with_retries(&pool, ctx, &retry_config).await;
        tracing::info!(
            log_dir = %result.log_dir,
            status = %result.status,
            exit_code = ?result.exit_code,
            "replay execution completed"
        );
        if let Some(ref nc) = notification_config
            && let Ok(exec_record) = crate::models::execution::get_by_id(&pool, &execution_id).await
        {
            crate::notification::send_notification(
                &state_clone.http_client,
                nc,
                &hook_snapshot,
                &result,
                &exec_record,
            )
            .await;
        }
    });

    Ok(Json(ReplayResponse {
        execution_id: exec.id,
    }))
}

/// Approve a pending_approval execution. Transitions to approved, then spawns execution.
async fn approve_execution(
    AuthUser(user): AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let pool = state.db.pool();

    let exec = execution::mark_approved(pool, &id, user.email.as_str())
        .await
        .map_err(|e| match e {
            DbError::Conflict(_) => StatusCode::CONFLICT.into_response(),
            DbError::NotFound(_) => StatusCode::NOT_FOUND.into_response(),
            _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        })?;

    let config = state.config.load();
    let hook = config.hooks.iter().find(|h| h.slug == exec.hook_slug);

    if let Some(hook) = hook {
        let timeout = hook.timeout.unwrap_or(config.defaults.timeout);
        let resolved_executor = resolve_executor(&hook.executor, &exec.request_payload);

        let env = hook.env.clone();
        let cwd = hook.cwd.clone();
        let logs_dir = config.logs.dir.clone();
        let retry_config = retry::resolve_retry_config(hook, &config.defaults.retries);
        let concurrency_config = hook.concurrency.clone();
        let approval_config = hook.approval.clone();
        let notification_config = hook.notification.clone();
        let hook_snapshot = hook.clone();
        let hook_slug = exec.hook_slug.clone();
        let state_clone = Arc::clone(&state);

        // Reset to pending so executor can transition pending -> running
        let _ = sqlx::query(
            "UPDATE executions SET status = 'pending' WHERE id = ? AND status = 'approved'",
        )
        .bind(&exec.id)
        .execute(pool)
        .await;

        let ctx = crate::executor::ExecutionContext {
            execution_id: exec.id.clone(),
            hook_slug: exec.hook_slug.clone(),
            executor: resolved_executor,
            env,
            cwd,
            timeout,
            logs_dir,
            payload_json: exec.request_payload.clone(),
            http_client: Some(state.http_client.clone()),
        };

        let execution_id = exec.id.clone();
        let pool_clone = pool.clone();
        tokio::spawn(async move {
            let result = retry::run_with_retries(&pool_clone, ctx, &retry_config).await;
            tracing::info!(
                log_dir = %result.log_dir,
                status = %result.status,
                "approved execution completed"
            );
            if let Some(ref nc) = notification_config
                && let Ok(exec_record) =
                    crate::models::execution::get_by_id(&pool_clone, &execution_id).await
            {
                crate::notification::send_notification(
                    &state_clone.http_client,
                    nc,
                    &hook_snapshot,
                    &result,
                    &exec_record,
                )
                .await;
            }
            if concurrency_config.is_some() {
                barriers::on_execution_complete(
                    &state_clone,
                    &hook_slug,
                    concurrency_config,
                    approval_config,
                )
                .await;
            }
        });
    }

    Ok(Redirect::to(&format!("/executions/{id}")).into_response())
}

/// Reject a pending_approval execution.
async fn reject_execution(
    AuthUser(user): AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let pool = state.db.pool();

    let exec = execution::mark_rejected(pool, &id, user.email.as_str())
        .await
        .map_err(|e| match e {
            DbError::Conflict(_) => StatusCode::CONFLICT.into_response(),
            DbError::NotFound(_) => StatusCode::NOT_FOUND.into_response(),
            _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        })?;

    // Expire any queue entry for the rejected execution
    let _ = execution_queue::expire_for_execution(pool, &exec.id).await;

    // If this execution held a lock, hand off to next queued or release
    let config = state.config.load();
    if let Some(hook) = config.hooks.iter().find(|h| h.slug == exec.hook_slug)
        && let Ok(Some(holder)) = execution_lock::get_holder(pool, &exec.hook_slug).await
        && holder == id
    {
        barriers::on_execution_complete(
            &state,
            &exec.hook_slug,
            hook.concurrency.clone(),
            hook.approval.clone(),
        )
        .await;
    }

    Ok(Redirect::to(&format!("/executions/{id}")).into_response())
}

/// List all pending_approval executions.
async fn list_pending_approvals(
    AuthUser(user): AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let pool = state.db.pool();
    let executions = execution::list_pending_approval(pool).await?;

    let rows = executions
        .into_iter()
        .map(ApprovalRow::from_execution)
        .collect();
    let page = ApprovalsPage::new(rows);

    render_approvals_page(user.email.as_str(), &page)
}

/// GET /executions/:id/logs/stream
///
/// Streams log output as Server-Sent Events. For terminal executions, sends the
/// full log content then closes. For running executions without regex masks,
/// polls log files at 200ms intervals and sends new chunks until terminal.
/// Regex-masked streams withhold live chunks and emit masked full logs at
/// terminal state, since arbitrary regex matches are not boundary-safe.
///
/// Events emitted:
/// - `stdout` — new stdout content
/// - `stderr` — new stderr content
/// - `done`   — status badge HTML when terminal
async fn log_stream(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let pool = state.db.pool().clone();
    let config = state.config.load();
    let logs_dir = config.logs.dir.clone();

    let exec = execution::get_by_id(&pool, &id)
        .await
        .map_err(|e| match e {
            crate::error::DbError::NotFound(_) => AppError::not_found("execution"),
            other => AppError::from(other),
        })?;
    let hook_env = config
        .hooks
        .iter()
        .find(|hook| hook.slug == exec.hook_slug)
        .map(|hook| hook.env.clone())
        .unwrap_or_default();
    let masking = config.masking.clone();
    let defer_live_streaming = !masking.compiled_patterns.is_empty();

    let is_terminal = exec.status.is_terminal();
    let log_dir = format!("{logs_dir}/{id}");

    let s = stream! {
        if is_terminal {
            // Execution already done — send full log content then close.
            if let Some(event) = masked_log_file_event(
                &logs_dir,
                &id,
                "stdout.log",
                "stdout",
                &masking,
                &hook_env,
            ).await {
                yield Ok::<Event, Infallible>(event);
            }
            if let Some(event) = masked_log_file_event(
                &logs_dir,
                &id,
                "stderr.log",
                "stderr",
                &masking,
                &hook_env,
            ).await {
                yield Ok::<Event, Infallible>(event);
            }
            let status = execution::get_by_id(&pool, &id)
                .await
                .map(|e| e.status.to_string())
                .unwrap_or_else(|_| "unknown".into());
            yield Ok(done_event(&status));
        } else if defer_live_streaming {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                match execution::get_by_id(&pool, &id).await {
                    Ok(e) if e.status.is_terminal() => {
                        let status = e.status.to_string();
                        if let Some(event) = masked_log_file_event(
                            &logs_dir,
                            &id,
                            "stdout.log",
                            "stdout",
                            &masking,
                            &hook_env,
                        ).await {
                            yield Ok::<Event, Infallible>(event);
                        }
                        if let Some(event) = masked_log_file_event(
                            &logs_dir,
                            &id,
                            "stderr.log",
                            "stderr",
                            &masking,
                            &hook_env,
                        ).await {
                            yield Ok::<Event, Infallible>(event);
                        }
                        yield Ok(done_event(&status));
                        break;
                    }
                    _ => {}
                }
            }
        } else {
            // Execution is running — tail log files.
            let mut stdout_tail = LogTail::new(format!("{log_dir}/stdout.log"));
            let mut stderr_tail = LogTail::new(format!("{log_dir}/stderr.log"));
            let mut stdout_masker = LogStreamMasker::new(&masking, hook_env.clone());
            let mut stderr_masker = LogStreamMasker::new(&masking, hook_env.clone());

            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                if let Some(text) = stdout_tail.read_new_text().await
                    && let Some(data) = stdout_masker.push(&text) {
                        yield Ok::<Event, Infallible>(
                            Event::default().event("stdout").data(data)
                        );
                    }

                if let Some(text) = stderr_tail.read_new_text().await
                    && let Some(data) = stderr_masker.push(&text) {
                        yield Ok::<Event, Infallible>(
                            Event::default().event("stderr").data(data)
                        );
                    }

                // Check if execution has reached a terminal state.
                match execution::get_by_id(&pool, &id).await {
                    Ok(e) if e.status.is_terminal() => {
                        let status = e.status.to_string();
                        while let Some(text) = stdout_tail.read_new_text().await {
                            if let Some(data) = stdout_masker.push(&text) {
                                yield Ok::<Event, Infallible>(
                                    Event::default().event("stdout").data(data)
                                );
                            }
                        }
                        if let Some(data) = stdout_masker.flush() {
                            yield Ok::<Event, Infallible>(
                                Event::default().event("stdout").data(data)
                            );
                        }

                        while let Some(text) = stderr_tail.read_new_text().await {
                            if let Some(data) = stderr_masker.push(&text) {
                                yield Ok::<Event, Infallible>(
                                    Event::default().event("stderr").data(data)
                                );
                            }
                        }
                        if let Some(data) = stderr_masker.flush() {
                            yield Ok::<Event, Infallible>(
                                Event::default().event("stderr").data(data)
                            );
                        }
                        yield Ok(done_event(&status));
                        break;
                    }
                    _ => {}
                }
            }
        }
    };

    Ok(Sse::new(s).keep_alive(KeepAlive::default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use tower::ServiceExt;

    use crate::config::AppConfig;
    use crate::db::Db;
    use crate::models::execution::{ExecutionStatus, NewExecution};
    use crate::server::AppState;
    use allowthem_core::{AllowThemBuilder, EmbeddedAuthClient};

    async fn test_state() -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let config_path = dir.path().join("sendword.toml");
        std::fs::write(&config_path, "[server]\nport = 8080\n").unwrap();
        let config =
            AppConfig::load_from(config_path.to_str().unwrap(), "nonexistent_overlay.json")
                .expect("load config");
        let db = Db::new_in_memory().await.expect("db");
        db.migrate().await.expect("migrate");
        let ath = AllowThemBuilder::with_pool(db.pool().clone())
            .cookie_secure(false)
            .build()
            .await
            .expect("allowthem build");
        let auth_client = Arc::new(EmbeddedAuthClient::new(ath.clone(), "/login"));
        let state = AppState::new(config, &config_path, db, ath, auth_client);
        (state, dir)
    }

    #[tokio::test]
    async fn sse_route_requires_auth() {
        let (state, _dir) = test_state().await;

        // Create a completed execution.
        let exec = crate::models::execution::create(
            state.db.pool(),
            &NewExecution {
                id: None,
                hook_slug: "test-hook",
                log_path: "/tmp/logs",
                trigger_source: "test",
                request_payload: "{}",
                retry_of: None,
                status: Some(ExecutionStatus::Success),
            },
        )
        .await
        .expect("create execution");

        let app = Router::new().merge(router()).with_state(Arc::clone(&state));

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/executions/{}/logs/stream", exec.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Without auth cookie the auth middleware redirects to /login (3xx).
        assert!(
            resp.status().is_redirection() || resp.status() == StatusCode::UNAUTHORIZED,
            "expected redirect or unauthorized, got {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn read_log_file_returns_none_for_empty_or_missing_logs() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let exec_id = "exec-1";
        let log_dir = dir.path().join(exec_id);
        tokio::fs::create_dir_all(&log_dir).await.expect("log dir");
        tokio::fs::write(log_dir.join("stdout.log"), "hello\n")
            .await
            .expect("stdout");
        tokio::fs::write(log_dir.join("stderr.log"), "")
            .await
            .expect("stderr");

        assert_eq!(
            read_log_file(dir.path().to_str().unwrap(), exec_id, "stdout.log").await,
            Some("hello\n".to_owned())
        );
        assert_eq!(
            read_log_file(dir.path().to_str().unwrap(), exec_id, "stderr.log").await,
            None
        );
        assert_eq!(
            read_log_file(dir.path().to_str().unwrap(), exec_id, "missing.log").await,
            None
        );
    }

    #[test]
    fn log_stream_masker_holds_complete_secret_until_it_can_mask_it() {
        let mut hook_env = HashMap::new();
        hook_env.insert("SECRET_TOKEN".to_owned(), "live-secret-value".to_owned());
        let masking = MaskingConfig {
            env_vars: vec!["SECRET_TOKEN".to_owned()],
            ..Default::default()
        };
        let mut masker = LogStreamMasker::new(&masking, hook_env);

        assert_eq!(masker.push("live-secret-value\n"), None);
        let flushed = masker.flush().expect("masked tail");

        assert_eq!(flushed, "***\n");
        assert!(!flushed.contains("live-secret-value"));
        assert!(!flushed.contains("live-"));
    }

    #[test]
    fn log_stream_masker_does_not_emit_split_secret_prefixes() {
        let mut hook_env = HashMap::new();
        hook_env.insert("SECRET_TOKEN".to_owned(), "live-secret-value".to_owned());
        let masking = MaskingConfig {
            env_vars: vec!["SECRET_TOKEN".to_owned()],
            ..Default::default()
        };
        let mut masker = LogStreamMasker::new(&masking, hook_env);
        let mut output = String::new();

        for chunk in [
            "prefix live-",
            "secret-value suffix with enough padding to emit",
        ] {
            if let Some(data) = masker.push(chunk) {
                output.push_str(&data);
            }
        }
        if let Some(data) = masker.flush() {
            output.push_str(&data);
        }

        assert!(output.contains("prefix *** suffix"));
        assert!(!output.contains("live-secret-value"));
        assert!(!output.contains("live-"));
    }

    #[tokio::test]
    async fn log_tail_reads_new_bytes_without_replaying_old_content() {
        use tokio::io::AsyncWriteExt;

        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("stdout.log");
        let mut tail = LogTail::new(&path);

        assert_eq!(tail.read_new_text().await, None);

        tokio::fs::write(&path, "first").await.expect("write first");
        assert_eq!(tail.read_new_text().await, Some("first".to_owned()));
        assert_eq!(tail.read_new_text().await, None);

        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .expect("open append");
        file.write_all(b"second").await.expect("append second");
        file.flush().await.expect("flush");

        assert_eq!(tail.read_new_text().await, Some("second".to_owned()));
    }

    #[tokio::test]
    async fn log_tail_reads_large_appends_in_bounded_chunks() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("stdout.log");
        let mut tail = LogTail::new(&path);
        let log_text = format!("{}tail", "a".repeat(LOG_TAIL_READ_CHUNK_BYTES));

        tokio::fs::write(&path, &log_text).await.expect("write log");

        let mut chunks = Vec::new();
        while let Some(chunk) = tail.read_new_text().await {
            assert!(chunk.len() <= LOG_TAIL_READ_CHUNK_BYTES);
            chunks.push(chunk);
        }

        assert!(chunks.len() > 1);
        assert_eq!(chunks.concat(), log_text);
    }
}
