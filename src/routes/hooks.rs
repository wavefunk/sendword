use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::config::{BackoffStrategy, ExecutorConfig};
use crate::config_writer::{self, HookFormData, RetryFormData, WriteError};
use crate::error::AppError;
use crate::models::execution;
use crate::retry;
use crate::server::AppState;
use crate::templates::context;

const EXECUTIONS_PER_PAGE: i64 = 20;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/hook/{slug}", post(trigger_hook))
        .route("/hooks/new", get(new_hook_form).post(create_hook))
        .route("/hooks/{slug}", get(hook_detail))
        .route("/hooks/{slug}/edit", get(edit_hook_form).post(update_hook))
        .route("/hooks/{slug}/delete", post(delete_hook))
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

// ---------------------------------------------------------------------------
// Hook form deserialization
// ---------------------------------------------------------------------------

/// Raw form data from the hook create/edit form.
///
/// Environment variables arrive as parallel arrays (`env_keys` and `env_values`)
/// because HTML forms cannot natively express maps.
#[derive(Deserialize)]
struct HookForm {
    name: String,
    slug: String,
    #[serde(default)]
    description: String,
    /// Checkbox: present with value "true" when checked, absent when unchecked.
    #[serde(default)]
    enabled: Option<String>,
    command: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    timeout: String,
    #[serde(default)]
    env_keys: Vec<String>,
    #[serde(default)]
    env_values: Vec<String>,
    #[serde(default)]
    retry_count: Option<String>,
    #[serde(default)]
    retry_backoff: Option<String>,
    #[serde(default)]
    retry_initial_delay: Option<String>,
    #[serde(default)]
    retry_max_delay: Option<String>,
}

/// Parse a human-readable duration string (e.g., "30s", "5m", "2h") into a `Duration`.
/// Returns `None` for empty strings.
fn parse_duration_field(s: &str) -> Result<Option<Duration>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    humantime::parse_duration(s)
        .map(Some)
        .map_err(|e| format!("invalid duration '{s}': {e}"))
}

/// Convert raw form data into `HookFormData` used by ConfigWriter.
fn parse_hook_form(form: &HookForm) -> Result<HookFormData, String> {
    let timeout = parse_duration_field(&form.timeout)?;

    // Build env map from parallel arrays, skipping rows with empty keys
    let mut env = HashMap::new();
    for (key, value) in form.env_keys.iter().zip(form.env_values.iter()) {
        let key = key.trim();
        if !key.is_empty() {
            env.insert(key.to_owned(), value.clone());
        }
    }

    // Parse retry config
    let retry_count: u32 = form
        .retry_count
        .as_deref()
        .unwrap_or("")
        .trim()
        .parse()
        .unwrap_or(0);

    let retries = if retry_count > 0 {
        let backoff = match form.retry_backoff.as_deref().unwrap_or("exponential") {
            "none" => BackoffStrategy::None,
            "linear" => BackoffStrategy::Linear,
            _ => BackoffStrategy::Exponential,
        };

        let initial_delay = parse_duration_field(
            form.retry_initial_delay.as_deref().unwrap_or(""),
        )?
        .unwrap_or(Duration::from_secs(1));

        let max_delay = parse_duration_field(
            form.retry_max_delay.as_deref().unwrap_or(""),
        )?
        .unwrap_or(Duration::from_secs(60));

        Some(RetryFormData {
            count: retry_count,
            backoff,
            initial_delay,
            max_delay,
        })
    } else {
        None
    };

    let cwd = if form.cwd.trim().is_empty() {
        None
    } else {
        Some(form.cwd.trim().to_owned())
    };

    Ok(HookFormData {
        name: form.name.trim().to_owned(),
        slug: form.slug.trim().to_owned(),
        description: form.description.trim().to_owned(),
        enabled: form.enabled.is_some(),
        command: form.command.trim().to_owned(),
        cwd,
        env,
        timeout,
        retries,
    })
}

// ---------------------------------------------------------------------------
// Flash query params
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct FlashParams {
    success: Option<String>,
    error: Option<String>,
}

// ---------------------------------------------------------------------------
// GET /hooks/new
// ---------------------------------------------------------------------------

async fn new_hook_form(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(flash): Query<FlashParams>,
) -> Result<Html<String>, AppError> {
    let html = state.templates.render(
        "hook_form.html",
        context! {
            is_new => true,
            form_name => "",
            form_slug => "",
            form_description => "",
            form_enabled => true,
            form_command => "",
            form_cwd => "",
            form_timeout => "",
            form_env => Vec::<()>::new(),
            form_retry_count => 0,
            form_retry_backoff => "exponential",
            form_retry_initial_delay => "",
            form_retry_max_delay => "",
            success => flash.success,
            error => flash.error,
        },
    )?;
    Ok(Html(html))
}

// ---------------------------------------------------------------------------
// POST /hooks/new
// ---------------------------------------------------------------------------

async fn create_hook(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<HookForm>,
) -> Response {
    let data = match parse_hook_form(&form) {
        Ok(d) => d,
        Err(msg) => {
            let encoded = urlencoding::encode(&msg);
            return Redirect::to(&format!("/hooks/new?error={encoded}")).into_response();
        }
    };

    let slug = data.slug.clone();

    if let Err(e) = state.config_writer.add_hook(&data) {
        let msg = write_error_message(&e);
        let encoded = urlencoding::encode(&msg);
        return Redirect::to(&format!("/hooks/new?error={encoded}")).into_response();
    }

    // Hot-reload config
    if let Err(e) = state.reload_config() {
        tracing::error!(error = %e, "failed to reload config after hook creation");
    }

    Redirect::to(&format!("/hooks/{slug}")).into_response()
}

// ---------------------------------------------------------------------------
// GET /hooks/:slug/edit
// ---------------------------------------------------------------------------

async fn edit_hook_form(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Query(flash): Query<FlashParams>,
) -> Result<Html<String>, AppError> {
    let config = state.config.load();

    let hook = config
        .hooks
        .iter()
        .find(|h| h.slug == slug)
        .ok_or(AppError::not_found("hook"))?;

    let (command, _) = match &hook.executor {
        ExecutorConfig::Shell { command } => (command.as_str(), "shell"),
    };

    let timeout_str = hook
        .timeout
        .map(config_writer::format_duration)
        .unwrap_or_default();

    let env_rows: Vec<_> = {
        let mut pairs: Vec<_> = hook.env.iter().collect();
        pairs.sort_by_key(|(k, _)| k.as_str());
        pairs
            .into_iter()
            .map(|(k, v)| context! { key => k, value => v })
            .collect()
    };

    let (retry_count, retry_backoff, retry_initial_delay, retry_max_delay) =
        if let Some(r) = &hook.retries {
            (
                r.count,
                config_writer::backoff_str(r.backoff),
                config_writer::format_duration(r.initial_delay),
                config_writer::format_duration(r.max_delay),
            )
        } else {
            (0, "exponential", String::new(), String::new())
        };

    let html = state.templates.render(
        "hook_form.html",
        context! {
            is_new => false,
            slug => &hook.slug,
            form_name => &hook.name,
            form_slug => &hook.slug,
            form_description => &hook.description,
            form_enabled => hook.enabled,
            form_command => command,
            form_cwd => hook.cwd.as_deref().unwrap_or(""),
            form_timeout => timeout_str,
            form_env => env_rows,
            form_retry_count => retry_count,
            form_retry_backoff => retry_backoff,
            form_retry_initial_delay => retry_initial_delay,
            form_retry_max_delay => retry_max_delay,
            success => flash.success,
            error => flash.error,
        },
    )?;
    Ok(Html(html))
}

// ---------------------------------------------------------------------------
// POST /hooks/:slug/edit
// ---------------------------------------------------------------------------

async fn update_hook(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Form(form): Form<HookForm>,
) -> Response {
    let data = match parse_hook_form(&form) {
        Ok(d) => d,
        Err(msg) => {
            let encoded = urlencoding::encode(&msg);
            return Redirect::to(&format!("/hooks/{slug}/edit?error={encoded}")).into_response();
        }
    };

    if let Err(e) = state.config_writer.update_hook(&slug, &data) {
        let msg = write_error_message(&e);
        let encoded = urlencoding::encode(&msg);
        return Redirect::to(&format!("/hooks/{slug}/edit?error={encoded}")).into_response();
    }

    // Hot-reload config
    if let Err(e) = state.reload_config() {
        tracing::error!(error = %e, "failed to reload config after hook update");
    }

    Redirect::to(&format!("/hooks/{slug}")).into_response()
}

// ---------------------------------------------------------------------------
// POST /hooks/:slug/delete
// ---------------------------------------------------------------------------

async fn delete_hook(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Response {
    if let Err(e) = state.config_writer.remove_hook(&slug) {
        let msg = write_error_message(&e);
        let encoded = urlencoding::encode(&msg);
        return Redirect::to(&format!("/hooks/{slug}?error={encoded}")).into_response();
    }

    // Hot-reload config
    if let Err(e) = state.reload_config() {
        tracing::error!(error = %e, "failed to reload config after hook deletion");
    }

    Redirect::to("/").into_response()
}

/// Convert a `WriteError` into a user-facing message.
fn write_error_message(e: &WriteError) -> String {
    match e {
        WriteError::SlugConflict(slug) => format!("A hook with slug '{slug}' already exists"),
        WriteError::HookNotFound(slug) => format!("Hook '{slug}' not found"),
        WriteError::Validation(inner) => format!("Validation error: {inner}"),
        WriteError::Io(inner) => {
            tracing::error!(error = %inner, "config write IO error");
            "Failed to write config file".to_owned()
        }
        WriteError::Parse(inner) => {
            tracing::error!(error = %inner, "config parse error");
            "Failed to parse config file".to_owned()
        }
    }
}

// ---------------------------------------------------------------------------
// Execution list and helpers
// ---------------------------------------------------------------------------

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
