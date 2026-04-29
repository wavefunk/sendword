use std::convert::Infallible;
use std::path::Path as FsPath;
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

use crate::barriers::{self, execution_lock, execution_queue};
use crate::config::ExecutorConfig;
use crate::error::{AppError, DbError};
use crate::executor::ResolvedExecutor;
use crate::extractors::AuthUser;
use crate::interpolation::interpolate_command;
use crate::masking::mask_secrets;
use crate::models::execution;
use crate::retry;
use crate::server::AppState;
use crate::templates::context;

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

/// Read a log file, returning its contents or a fallback message.
async fn read_log_file(logs_dir: &str, execution_id: &str, filename: &str) -> String {
    let path = FsPath::new(logs_dir).join(execution_id).join(filename);
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) if !contents.is_empty() => contents,
        _ => "No output captured.".into(),
    }
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
    let stdout = read_log_file(logs_dir, &exec.id, "stdout.log").await;
    let stderr = read_log_file(logs_dir, &exec.id, "stderr.log").await;

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
    let stdout = mask_secrets(&stdout, &config.masking, &hook_env);
    let stderr = mask_secrets(&stderr, &config.masking, &hook_env);

    let duration = compute_duration(&exec.started_at, &exec.completed_at);

    let html = state.templates.render(
        "execution_detail.html",
        context! {
            id => exec.id,
            hook_slug => exec.hook_slug,
            status => exec.status.to_string(),
            exit_code => exec.exit_code,
            triggered_at => exec.triggered_at,
            started_at => exec.started_at,
            completed_at => exec.completed_at,
            duration => duration,
            trigger_source => exec.trigger_source,
            retry_count => exec.retry_count,
            retry_of => exec.retry_of,
            stdout => stdout,
            stderr => stderr,
            username => auth.email.as_str(),
            nav_active => "hooks",
        },
    )?;

    Ok(Html(html))
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
    let resolved_executor = match &hook.executor {
        ExecutorConfig::Shell { command } => {
            let interpolated = if let Ok(payload_value) =
                serde_json::from_str::<serde_json::Value>(&original.request_payload)
            {
                interpolate_command(command, &payload_value).into_owned()
            } else {
                command.clone()
            };
            ResolvedExecutor::Shell {
                command: interpolated,
            }
        }
        ExecutorConfig::Script { path } => ResolvedExecutor::Script {
            path: std::path::PathBuf::from(path),
        },
        ExecutorConfig::Http {
            method,
            url,
            headers,
            body,
            follow_redirects,
        } => {
            let payload_value: serde_json::Value = serde_json::from_str(&original.request_payload)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            let interpolated_url = interpolate_command(url, &payload_value).into_owned();
            let interpolated_body = body
                .as_deref()
                .map(|b| interpolate_command(b, &payload_value).into_owned());
            ResolvedExecutor::Http {
                method: *method,
                url: interpolated_url,
                headers: headers.clone(),
                body: interpolated_body,
                follow_redirects: *follow_redirects,
            }
        }
    };

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
        let resolved_executor = match &hook.executor {
            ExecutorConfig::Shell { command } => {
                let interpolated = if let Ok(payload_value) =
                    serde_json::from_str::<serde_json::Value>(&exec.request_payload)
                {
                    interpolate_command(command, &payload_value).into_owned()
                } else {
                    command.clone()
                };
                ResolvedExecutor::Shell {
                    command: interpolated,
                }
            }
            ExecutorConfig::Script { path } => ResolvedExecutor::Script {
                path: std::path::PathBuf::from(path),
            },
            ExecutorConfig::Http {
                method,
                url,
                headers,
                body,
                follow_redirects,
            } => {
                let payload_value: serde_json::Value = serde_json::from_str(&exec.request_payload)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let interpolated_url = interpolate_command(url, &payload_value).into_owned();
                let interpolated_body = body
                    .as_deref()
                    .map(|b| interpolate_command(b, &payload_value).into_owned());
                ResolvedExecutor::Http {
                    method: *method,
                    url: interpolated_url,
                    headers: headers.clone(),
                    body: interpolated_body,
                    follow_redirects: *follow_redirects,
                }
            }
        };

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

    let exec_list: Vec<serde_json::Value> = executions
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "hook_slug": e.hook_slug,
                "triggered_at": e.triggered_at,
                "trigger_source": e.trigger_source,
            })
        })
        .collect();

    let html = state.templates.render(
        "approvals.html",
        context! {
            executions => exec_list,
            username => user.email.as_str(),
            nav_active => "approvals",
        },
    )?;

    Ok(Html(html))
}

/// GET /executions/:id/logs/stream
///
/// Streams log output as Server-Sent Events. For terminal executions, sends the
/// full log content then closes. For running executions, polls log files at 200ms
/// intervals and sends new chunks until the execution reaches a terminal state.
///
/// Events emitted:
/// - `stdout` — new stdout content
/// - `stderr` — new stderr content
/// - `done`   — JSON `{"status": "..."}` when terminal
async fn log_stream(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let pool = state.db.pool().clone();
    let config = state.config.load();
    let logs_dir = config.logs.dir.clone();
    drop(config);

    let exec = execution::get_by_id(&pool, &id)
        .await
        .map_err(|e| match e {
            crate::error::DbError::NotFound(_) => AppError::not_found("execution"),
            other => AppError::from(other),
        })?;

    let is_terminal = exec.status.is_terminal();
    let log_dir = format!("{logs_dir}/{id}");

    let s = stream! {
        if is_terminal {
            // Execution already done — send full log content then close.
            let stdout = tokio::fs::read_to_string(format!("{log_dir}/stdout.log"))
                .await
                .unwrap_or_default();
            let stderr = tokio::fs::read_to_string(format!("{log_dir}/stderr.log"))
                .await
                .unwrap_or_default();
            if !stdout.is_empty() {
                yield Ok::<Event, Infallible>(Event::default().event("stdout").data(html_escape(&stdout)));
            }
            if !stderr.is_empty() {
                yield Ok(Event::default().event("stderr").data(html_escape(&stderr)));
            }
            let status = execution::get_by_id(&pool, &id)
                .await
                .map(|e| e.status.to_string())
                .unwrap_or_else(|_| "unknown".into());
            let tag_class = match status.as_str() {
                "success" => "ok",
                "failed" => "err",
                _ => "",
            };
            yield Ok(Event::default().event("done").data(
                format!(r#"<span class="wf-tag {tag_class}"><span class="dot"></span>{}</span>"#, status.to_uppercase())
            ));
        } else {
            // Execution is running — tail log files.
            let stdout_path = format!("{log_dir}/stdout.log");
            let stderr_path = format!("{log_dir}/stderr.log");
            let mut stdout_offset: u64 = 0;
            let mut stderr_offset: u64 = 0;

            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                // Read new stdout bytes.
                if let Ok(content) = tokio::fs::read(&stdout_path).await
                    && content.len() as u64 > stdout_offset {
                        let new = &content[stdout_offset as usize..];
                        stdout_offset = content.len() as u64;
                        if let Ok(text) = std::str::from_utf8(new)
                            && !text.is_empty() {
                                yield Ok::<Event, Infallible>(
                                    Event::default().event("stdout").data(html_escape(text))
                                );
                            }
                    }

                // Read new stderr bytes.
                if let Ok(content) = tokio::fs::read(&stderr_path).await
                    && content.len() as u64 > stderr_offset {
                        let new = &content[stderr_offset as usize..];
                        stderr_offset = content.len() as u64;
                        if let Ok(text) = std::str::from_utf8(new)
                            && !text.is_empty() {
                                yield Ok::<Event, Infallible>(
                                    Event::default().event("stderr").data(html_escape(text))
                                );
                            }
                    }

                // Check if execution has reached a terminal state.
                match execution::get_by_id(&pool, &id).await {
                    Ok(e) if e.status.is_terminal() => {
                        let status = e.status.to_string();
                        let tag_class = match status.as_str() {
                            "success" => "ok",
                            "failed" => "err",
                            _ => "",
                        };
                        yield Ok(Event::default().event("done").data(
                            format!(r#"<span class="wf-tag {tag_class}"><span class="dot"></span>{}</span>"#, status.to_uppercase())
                        ));
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
        let templates =
            crate::templates::Templates::new(crate::templates::Templates::default_dir());
        let state = AppState::new(config, &config_path, db, templates, ath, auth_client);
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
}
