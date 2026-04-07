use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::config::ExecutorConfig;
use crate::error::AppError;
use crate::models::execution;
use crate::retry;
use crate::server::AppState;
use crate::templates::context;

const EXECUTIONS_PER_PAGE: i64 = 20;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/hook/{slug}", post(trigger_hook))
        .route("/hooks/{slug}", get(hook_detail))
        .route("/hooks/{slug}/executions", get(execution_list))
}

#[derive(Serialize)]
struct TriggerResponse {
    execution_id: String,
}

async fn trigger_hook(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<TriggerResponse>, StatusCode> {
    let config = state.config.load();

    let hook = config
        .hooks
        .iter()
        .find(|h| h.slug == slug && h.enabled)
        .ok_or(StatusCode::NOT_FOUND)?;

    let timeout = hook.timeout.unwrap_or(config.defaults.timeout);

    let command = match &hook.executor {
        ExecutorConfig::Shell { command } => command.clone(),
    };

    let env = hook.env.clone();
    let cwd = hook.cwd.clone();
    let logs_dir = config.logs.dir.clone();

    let retry_config = retry::resolve_retry_config(hook, &config.defaults.retries);

    // Pre-generate the execution ID so we can set the correct log_path
    let exec_id = crate::id::new_id();
    let log_path = format!("{logs_dir}/{exec_id}");

    let pool = state.db.pool();

    let exec = execution::create(
        pool,
        &execution::NewExecution {
            id: Some(&exec_id),
            hook_slug: &slug,
            log_path: &log_path,
            trigger_source: "127.0.0.1", // TODO: extract from request
            request_payload: "{}",        // TODO: extract from request body
            retry_of: None,
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let ctx = crate::executor::ExecutionContext {
        execution_id: exec.id.clone(),
        hook_slug: slug,
        command,
        env,
        cwd,
        timeout,
        logs_dir,
    };

    let pool = pool.clone();
    tokio::spawn(async move {
        let result = retry::run_with_retries(&pool, ctx, &retry_config).await;
        tracing::info!(
            log_dir = %result.log_dir,
            status = %result.status,
            exit_code = ?result.exit_code,
            "execution completed"
        );
    });

    Ok(Json(TriggerResponse {
        execution_id: exec.id,
    }))
}

#[derive(Deserialize)]
struct PaginationParams {
    page: Option<i64>,
}

async fn hook_detail(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Html<String>, AppError> {
    let config = state.config.load();

    let hook = config
        .hooks
        .iter()
        .find(|h| h.slug == slug)
        .ok_or(AppError::not_found("hook"))?;

    let pool = state.db.pool();
    let total = execution::count_by_hook(pool, &slug).await?;
    let executions = execution::list_by_hook(pool, &slug, EXECUTIONS_PER_PAGE, 0).await?;

    let total_pages = (total + EXECUTIONS_PER_PAGE - 1) / EXECUTIONS_PER_PAGE;
    let has_more = total_pages > 1;

    let (executor_command, executor_type) = match &hook.executor {
        ExecutorConfig::Shell { command } => (command.as_str(), "shell"),
    };

    let timeout_display = hook
        .timeout
        .unwrap_or(config.defaults.timeout)
        .as_secs();

    let env_vars: Vec<_> = hook.env.keys().collect();

    let execution_rows = build_execution_rows(&executions);

    let html = state.templates.render(
        "hook_detail.html",
        context! {
            name => hook.name,
            slug => hook.slug,
            description => hook.description,
            enabled => hook.enabled,
            executor_type => executor_type,
            executor_command => executor_command,
            cwd => hook.cwd,
            timeout_secs => timeout_display,
            env_vars => env_vars,
            executions => execution_rows,
            total => total,
            page => 1,
            total_pages => total_pages,
            has_more => has_more,
        },
    )?;

    Ok(Html(html))
}

async fn execution_list(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Result<Html<String>, AppError> {
    let config = state.config.load();

    // Verify hook exists
    let _hook = config
        .hooks
        .iter()
        .find(|h| h.slug == slug)
        .ok_or(AppError::not_found("hook"))?;

    let page = params.page.unwrap_or(1).max(1);
    let offset = (page - 1) * EXECUTIONS_PER_PAGE;

    let pool = state.db.pool();
    let total = execution::count_by_hook(pool, &slug).await?;
    let executions = execution::list_by_hook(pool, &slug, EXECUTIONS_PER_PAGE, offset).await?;

    let total_pages = (total + EXECUTIONS_PER_PAGE - 1) / EXECUTIONS_PER_PAGE;
    let has_more = page < total_pages;

    let execution_rows = build_execution_rows(&executions);

    let html = state.templates.render(
        "partials/execution_list.html",
        context! {
            slug => slug,
            executions => execution_rows,
            total => total,
            page => page,
            total_pages => total_pages,
            has_more => has_more,
        },
    )?;

    Ok(Html(html))
}

fn build_execution_rows(executions: &[execution::Execution]) -> Vec<minijinja::Value> {
    executions
        .iter()
        .map(|e| {
            let duration = compute_duration(&e.started_at, &e.completed_at);
            context! {
                id => e.id,
                triggered_at => e.triggered_at,
                status => e.status.to_string(),
                exit_code => e.exit_code,
                duration => duration,
            }
        })
        .collect()
}

/// Compute duration string from ISO8601 timestamps.
/// Returns None if either timestamp is missing.
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
