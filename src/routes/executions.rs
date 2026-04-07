use std::path::Path as FsPath;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::AuthUser;
use crate::config::ExecutorConfig;
use crate::error::AppError;
use crate::masking::mask_secrets;
use crate::models::execution;
use crate::retry;
use crate::server::AppState;
use crate::templates::context;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/executions/{id}", get(execution_detail))
        .route("/executions/{id}/replay", post(replay_execution))
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
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Html<String>, AppError> {
    let config = state.config.load();
    let pool = state.db.pool();

    let exec = execution::get_by_id(pool, &id)
        .await
        .map_err(|e| match e {
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
            username => auth.username,
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
    let command = match &hook.executor {
        ExecutorConfig::Shell { command } => command.clone(),
    };
    let env = hook.env.clone();
    let cwd = hook.cwd.clone();
    let logs_dir = config.logs.dir.clone();

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
        },
    )
    .await?;

    // 5. Build execution context and spawn in a detached task
    let ctx = crate::executor::ExecutionContext {
        execution_id: exec.id.clone(),
        hook_slug: original.hook_slug,
        command,
        env,
        cwd,
        timeout,
        logs_dir,
        payload_json: original.request_payload,
    };

    let pool = pool.clone();
    tokio::spawn(async move {
        let result = retry::run_with_retries(&pool, ctx, &retry_config).await;
        tracing::info!(
            log_dir = %result.log_dir,
            status = %result.status,
            exit_code = ?result.exit_code,
            "replay execution completed"
        );
    });

    Ok(Json(ReplayResponse {
        execution_id: exec.id,
    }))
}
