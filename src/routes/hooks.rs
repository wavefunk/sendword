use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::interpolation::interpolate_command;
use crate::payload::{FieldType, PayloadField, PayloadSchema};
use crate::config::{BackoffStrategy, ExecutorConfig, HmacAlgorithm, HookAuthConfig};
use crate::webhook_auth;
use crate::config_writer::{self, HookFormData, RetryFormData, WriteError};
use crate::error::AppError;
use crate::models::{execution, trigger_attempt};
use crate::models::trigger_attempt::{NewTriggerAttempt, TriggerAttemptStatus};
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

/// Extract the client IP address from the request.
///
/// Prefers the first address in `X-Forwarded-For` (set by reverse proxies),
/// falls back to the peer socket address from `ConnectInfo`.
fn extract_source_ip(headers: &HeaderMap, peer: &SocketAddr) -> String {
    if let Some(forwarded) = headers.get("x-forwarded-for") {
        if let Ok(val) = forwarded.to_str() {
            if let Some(first) = val.split(',').next() {
                let trimmed = first.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_owned();
                }
            }
        }
    }
    peer.ip().to_string()
}

async fn trigger_hook(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<TriggerResponse>, Response> {
    let source_ip = extract_source_ip(&headers, &peer);
    let config = state.config.load();

    let hook = config
        .hooks
        .iter()
        .find(|h| h.slug == slug && h.enabled)
        .ok_or(StatusCode::NOT_FOUND.into_response())?;

    let pool = state.db.pool();

    // Auth check
    match webhook_auth::verify(hook.auth.as_ref(), &headers, &body) {
        webhook_auth::AuthResult::Ok => {}
        webhook_auth::AuthResult::Denied(reason) => {
            tracing::debug!(hook_slug = %slug, reason = %reason, "webhook auth denied");
            let _ = trigger_attempt::insert(
                pool,
                &NewTriggerAttempt {
                    hook_slug: &slug,
                    source_ip: &source_ip,
                    status: TriggerAttemptStatus::AuthFailed,
                    reason: &reason,
                    execution_id: None,
                },
            )
            .await;
            return Err(StatusCode::UNAUTHORIZED.into_response());
        }
    }

    // Parse body and validate against payload schema (if defined).
    // Only enforce JSON parsing when a schema exists. Without a schema,
    // store the raw body as-is (best-effort JSON, fall back to raw string).
    let payload_str = if let Some(schema) = &hook.payload {
        let payload_value: serde_json::Value = if body.is_empty() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    let reason = format!("invalid JSON: {e}");
                    let _ = trigger_attempt::insert(
                        pool,
                        &NewTriggerAttempt {
                            hook_slug: &slug,
                            source_ip: &source_ip,
                            status: TriggerAttemptStatus::ValidationFailed,
                            reason: &reason,
                            execution_id: None,
                        },
                    )
                    .await;
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": "invalid JSON",
                            "message": e.to_string(),
                        })),
                    )
                        .into_response());
                }
            }
        };

        if let Err(errors) = schema.validate(&payload_value) {
            let reason = format!("payload validation failed: {errors:?}");
            let _ = trigger_attempt::insert(
                pool,
                &NewTriggerAttempt {
                    hook_slug: &slug,
                    source_ip: &source_ip,
                    status: TriggerAttemptStatus::ValidationFailed,
                    reason: &reason,
                    execution_id: None,
                },
            )
            .await;
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "payload validation failed",
                    "details": errors,
                })),
            )
                .into_response());
        }

        serde_json::to_string(&payload_value)
            .unwrap_or_else(|_| "{}".to_owned())
    } else if body.is_empty() {
        "{}".to_owned()
    } else {
        // No schema: store body as-is if valid JSON, otherwise store raw string
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(v) => serde_json::to_string(&v).unwrap_or_else(|_| "{}".to_owned()),
            Err(_) => String::from_utf8_lossy(&body).into_owned(),
        }
    };

    let timeout = hook.timeout.unwrap_or(config.defaults.timeout);

    let command = match &hook.executor {
        ExecutorConfig::Shell { command } => command.clone(),
    };

    // Interpolate payload fields into command template
    let command = if let Ok(payload_value) = serde_json::from_str::<serde_json::Value>(&payload_str)
    {
        interpolate_command(&command, &payload_value).into_owned()
    } else {
        command
    };

    let env = hook.env.clone();
    let cwd = hook.cwd.clone();
    let logs_dir = config.logs.dir.clone();

    let retry_config = retry::resolve_retry_config(hook, &config.defaults.retries);

    // Pre-generate the execution ID so we can set the correct log_path
    let exec_id = crate::id::new_id();
    let log_path = format!("{logs_dir}/{exec_id}");

    let exec = execution::create(
        pool,
        &execution::NewExecution {
            id: Some(&exec_id),
            hook_slug: &slug,
            log_path: &log_path,
            trigger_source: &source_ip,
            request_payload: &payload_str,
            retry_of: None,
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;

    // Log successful trigger attempt
    let _ = trigger_attempt::insert(
        pool,
        &NewTriggerAttempt {
            hook_slug: &slug,
            source_ip: &source_ip,
            status: TriggerAttemptStatus::Fired,
            reason: "ok",
            execution_id: Some(&exec.id),
        },
    )
    .await;

    let ctx = crate::executor::ExecutionContext {
        execution_id: exec.id.clone(),
        hook_slug: slug,
        command,
        env,
        cwd,
        timeout,
        logs_dir,
        payload_json: payload_str,
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
    auth: AuthUser,
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

    // Check if the command references a script in the managed scripts directory.
    // If so, provide a link to the script editor.
    let script_edit_url = {
        let scripts_dir = &config.scripts.dir;
        let cmd_path = std::path::Path::new(executor_command);
        // Match commands like "data/scripts/deploy.sh" against the scripts dir
        if let Ok(stripped) = cmd_path.strip_prefix(scripts_dir) {
            stripped
                .to_str()
                .filter(|name| !name.contains('/') && !name.is_empty())
                .map(|name| format!("/scripts/{name}"))
        } else {
            None
        }
    };

    let timeout_display = hook
        .timeout
        .unwrap_or(config.defaults.timeout)
        .as_secs();

    let env_vars: Vec<_> = hook.env.keys().collect();

    let execution_rows = build_execution_rows(&executions);

    let payload_fields: Vec<serde_json::Value> = hook
        .payload
        .as_ref()
        .map(|schema| {
            schema
                .fields
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "name": f.name,
                        "field_type": f.field_type.to_string(),
                        "type": f.field_type.to_string(),
                        "required": f.required,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let (auth_mode, auth_header, auth_algorithm) = match &hook.auth {
        Some(HookAuthConfig::Bearer { .. }) => ("bearer", "", ""),
        Some(HookAuthConfig::Hmac { header, algorithm, .. }) => {
            let algo = match algorithm {
                HmacAlgorithm::Sha256 => "sha256",
            };
            ("hmac", header.as_str(), algo)
        }
        _ => ("none", "", ""),
    };

    let html = state.templates.render(
        "hook_detail.html",
        context! {
            name => hook.name,
            slug => hook.slug,
            description => hook.description,
            enabled => hook.enabled,
            executor_type => executor_type,
            executor_command => executor_command,
            script_edit_url => script_edit_url,
            cwd => hook.cwd,
            timeout_secs => timeout_display,
            env_vars => env_vars,
            auth_mode => auth_mode,
            auth_header => auth_header,
            auth_algorithm => auth_algorithm,
            payload_fields => payload_fields,
            executions => execution_rows,
            total => total,
            page => 1,
            total_pages => total_pages,
            has_more => has_more,
            username => auth.username,
            nav_active => "hooks",
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
/// Environment variables arrive as a single textarea (`env_text`) with one
/// `KEY=value` pair per line, since `serde_urlencoded` does not support
/// repeated keys deserialised into `Vec`.
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
    /// One `KEY=value` pair per line.
    #[serde(default)]
    env_text: String,
    #[serde(default)]
    retry_count: Option<String>,
    #[serde(default)]
    retry_backoff: Option<String>,
    #[serde(default)]
    retry_initial_delay: Option<String>,
    #[serde(default)]
    retry_max_delay: Option<String>,
    // Auth fields
    #[serde(default)]
    auth_mode: Option<String>,
    #[serde(default)]
    auth_token: Option<String>,
    #[serde(default)]
    auth_header: Option<String>,
    #[serde(default)]
    auth_algorithm: Option<String>,
    #[serde(default)]
    auth_secret: Option<String>,
    /// One field per line: name:type[:required]
    #[serde(default)]
    payload_text: String,
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

/// Parse payload field definitions from textarea.
///
/// Each non-empty line is `name:type` or `name:type:required`.
/// The `:required` suffix marks the field as required; otherwise optional.
/// Returns `None` if the text is empty (no schema defined).
fn parse_payload_text(text: &str) -> Result<Option<PayloadSchema>, String> {
    let mut fields = Vec::new();

    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() < 2 {
            return Err(format!(
                "payload line {}: expected 'name:type' or 'name:type:required', got '{line}'",
                i + 1,
            ));
        }

        let name = parts[0].trim();
        if name.is_empty() {
            return Err(format!("payload line {}: field name cannot be empty", i + 1));
        }

        let type_str = parts[1].trim();
        let field_type = match type_str {
            "string" => FieldType::String,
            "number" => FieldType::Number,
            "boolean" => FieldType::Boolean,
            "object" => FieldType::Object,
            "array" => FieldType::Array,
            other => {
                return Err(format!(
                    "payload line {}: unknown type '{other}' (expected string, number, boolean, object, or array)",
                    i + 1,
                ));
            }
        };

        let required = if parts.len() == 3 {
            match parts[2].trim() {
                "required" => true,
                "optional" | "" => false,
                other => {
                    return Err(format!(
                        "payload line {}: expected 'required' or 'optional', got '{other}'",
                        i + 1,
                    ));
                }
            }
        } else {
            false
        };

        fields.push(PayloadField {
            name: name.to_owned(),
            field_type,
            required,
        });
    }

    if fields.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PayloadSchema { fields }))
    }
}

/// Convert raw form data into `HookFormData` used by ConfigWriter.
fn parse_hook_form(form: &HookForm) -> Result<HookFormData, String> {
    let timeout = parse_duration_field(&form.timeout)?;

    // Build env map from textarea lines (KEY=value format)
    let mut env = HashMap::new();
    for line in form.env_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if !key.is_empty() {
                env.insert(key.to_owned(), value.to_owned());
            }
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

    let auth = match form.auth_mode.as_deref().unwrap_or("none") {
        "bearer" => {
            let token = form.auth_token.as_deref().unwrap_or("").trim().to_owned();
            if token.is_empty() {
                return Err("auth token must be non-empty for bearer mode".into());
            }
            Some(HookAuthConfig::Bearer { token })
        }
        "hmac" => {
            let header = form.auth_header.as_deref().unwrap_or("").trim().to_owned();
            let secret = form.auth_secret.as_deref().unwrap_or("").trim().to_owned();
            if header.is_empty() {
                return Err("auth header must be non-empty for HMAC mode".into());
            }
            if secret.is_empty() {
                return Err("auth secret must be non-empty for HMAC mode".into());
            }
            let algorithm = match form.auth_algorithm.as_deref().unwrap_or("sha256") {
                "sha256" => HmacAlgorithm::Sha256,
                other => return Err(format!("unsupported HMAC algorithm: {other}")),
            };
            Some(HookAuthConfig::Hmac { header, algorithm, secret })
        }
        _ => None,
    };

    let payload = parse_payload_text(&form.payload_text)?;

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
        auth,
        payload,
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
    auth: AuthUser,
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
            form_env_text => "",
            form_retry_count => 0,
            form_retry_backoff => "exponential",
            form_retry_initial_delay => "",
            form_retry_max_delay => "",
            form_auth_mode => "none",
            form_auth_token => "",
            form_auth_header => "X-Hub-Signature-256",
            form_auth_algorithm => "sha256",
            form_auth_secret => "",
            form_payload_text => "",
            success => flash.success,
            error => flash.error,
            username => auth.username,
            nav_active => "hooks",
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
    auth: AuthUser,
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

    let env_text = {
        let mut pairs: Vec<_> = hook.env.iter().collect();
        pairs.sort_by_key(|(k, _)| k.as_str());
        pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n")
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

    let (auth_mode, auth_token, auth_header, auth_algorithm, auth_secret) = match &hook.auth {
        Some(HookAuthConfig::Bearer { token }) => {
            ("bearer", token.as_str(), "", "", "")
        }
        Some(HookAuthConfig::Hmac { header, algorithm, secret }) => {
            let algo = match algorithm {
                HmacAlgorithm::Sha256 => "sha256",
            };
            ("hmac", "", header.as_str(), algo, secret.as_str())
        }
        Some(HookAuthConfig::None) | None => ("none", "", "", "sha256", ""),
    };

    let payload_text = hook
        .payload
        .as_ref()
        .map(|schema| {
            schema
                .fields
                .iter()
                .map(|f| {
                    if f.required {
                        format!("{}:{}:required", f.name, f.field_type)
                    } else {
                        format!("{}:{}", f.name, f.field_type)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

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
            form_env_text => env_text,
            form_retry_count => retry_count,
            form_retry_backoff => retry_backoff,
            form_retry_initial_delay => retry_initial_delay,
            form_retry_max_delay => retry_max_delay,
            form_auth_mode => auth_mode,
            form_auth_token => auth_token,
            form_auth_header => auth_header,
            form_auth_algorithm => auth_algorithm,
            form_auth_secret => auth_secret,
            form_payload_text => payload_text,
            success => flash.success,
            error => flash.error,
            username => auth.username,
            nav_active => "hooks",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::db::Db;
    use crate::models::user;
    use crate::templates::Templates;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// Create a test AppState backed by a temporary directory for the TOML config.
    /// Returns (state, temp_dir) -- the temp_dir must be kept alive for the test duration.
    async fn test_state_with_config(
        toml_content: &str,
    ) -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("tmp dir");
        let config_path = dir.path().join("sendword.toml");
        std::fs::write(&config_path, toml_content).expect("write config");

        let config =
            AppConfig::load_from(config_path.to_str().unwrap_or(""), "nonexistent.json")
                .expect("load config");

        let db = Db::new_in_memory().await.expect("in-memory db");
        db.migrate().await.expect("migration");
        let templates = Templates::new(Templates::default_dir());
        let state = AppState::new(config, &config_path, db, templates);
        (state, dir)
    }

    /// Create a test user and return a session cookie value.
    async fn create_test_session(state: &Arc<AppState>) -> String {
        let pool = state.db.pool();
        let u = user::create(pool, "admin", "password123").await.unwrap();
        let session_lifetime = state.config.load().auth.session_lifetime;
        let sess = crate::models::session::create(pool, &u.id, session_lifetime)
            .await
            .unwrap();
        format!("sendword_session={}", sess.id)
    }

    /// Build the test app with a ConnectInfo layer so trigger_hook can extract
    /// the peer address even when using `oneshot()`.
    fn app(state: Arc<AppState>) -> Router {
        use std::net::{Ipv4Addr, SocketAddr};
        let peer = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        crate::server::router(state).layer(axum::middleware::from_fn(
            move |mut req: axum::http::Request<Body>, next: axum::middleware::Next| {
                req.extensions_mut()
                    .insert(ConnectInfo(peer));
                async move { next.run(req).await }
            },
        ))
    }

    // --- New hook form ---

    #[tokio::test]
    async fn new_hook_form_requires_auth() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .uri("/hooks/new")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn new_hook_form_renders() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .uri("/hooks/new")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("New hook"));
        assert!(html.contains("Create hook"));
    }

    // --- Create hook ---

    #[tokio::test]
    async fn create_hook_redirects_to_detail() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;

        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/new")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Deploy&slug=deploy&command=echo+deploy&enabled=true",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(location, "/hooks/deploy");

        // Verify config was hot-reloaded
        let config = state.config.load();
        assert_eq!(config.hooks.len(), 1);
        assert_eq!(config.hooks[0].slug, "deploy");
        assert_eq!(config.hooks[0].name, "Deploy");
    }

    #[tokio::test]
    async fn create_hook_with_all_fields() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;

        let body = "name=Full+Hook&slug=full-hook&description=A+full+hook\
            &enabled=true&command=make+deploy&cwd=%2Fopt%2Fapp\
            &timeout=2m&env_text=APP_ENV%3Dproduction%0ADEBUG%3Dfalse\
            &retry_count=3&retry_backoff=exponential\
            &retry_initial_delay=2s&retry_max_delay=30s";

        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/new")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);

        let config = state.config.load();
        let hook = &config.hooks[0];
        assert_eq!(hook.name, "Full Hook");
        assert_eq!(hook.description, "A full hook");
        assert!(hook.enabled);
        assert_eq!(hook.cwd.as_deref(), Some("/opt/app"));
        assert_eq!(hook.timeout, Some(Duration::from_secs(120)));
        assert_eq!(hook.env.get("APP_ENV").map(String::as_str), Some("production"));
        assert_eq!(hook.env.get("DEBUG").map(String::as_str), Some("false"));
        let retries = hook.retries.as_ref().expect("retries should be set");
        assert_eq!(retries.count, 3);
        assert_eq!(retries.backoff, BackoffStrategy::Exponential);
    }

    #[tokio::test]
    async fn create_hook_duplicate_slug_shows_error() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Existing"
slug = "deploy"
[hooks.executor]
type = "shell"
command = "echo existing"
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let cookie = create_test_session(&state).await;

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/new")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Another&slug=deploy&command=echo+deploy&enabled=true",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.starts_with("/hooks/new?error="));
        assert!(location.contains("already+exists") || location.contains("already%20exists"));
    }

    // --- Edit hook form ---

    #[tokio::test]
    async fn edit_hook_form_renders_with_existing_data() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Deploy"
slug = "deploy"
description = "Deploy the app"
enabled = true
cwd = "/opt/app"
timeout = "2m"
[hooks.executor]
type = "shell"
command = "make deploy"
[hooks.env]
APP_ENV = "production"
[hooks.retries]
count = 3
backoff = "exponential"
initial_delay = "2s"
max_delay = "30s"
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let cookie = create_test_session(&state).await;

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .uri("/hooks/deploy/edit")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Edit hook"));
        assert!(html.contains("Deploy"));
        assert!(html.contains("make deploy"));
        assert!(html.contains("/opt/app"));
        assert!(html.contains("APP_ENV"));
        assert!(html.contains("production"));
    }

    #[tokio::test]
    async fn edit_hook_form_not_found() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .uri("/hooks/nonexistent/edit")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // --- Update hook ---

    #[tokio::test]
    async fn update_hook_changes_config() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Old Name"
slug = "my-hook"
[hooks.executor]
type = "shell"
command = "echo old"
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let cookie = create_test_session(&state).await;

        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/my-hook/edit")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=New+Name&slug=my-hook&command=echo+new&enabled=true&description=updated",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(location, "/hooks/my-hook");

        // Verify config was updated and reloaded
        let config = state.config.load();
        assert_eq!(config.hooks[0].name, "New Name");
        assert_eq!(config.hooks[0].description, "updated");
        let ExecutorConfig::Shell { command } = &config.hooks[0].executor;
        assert_eq!(command, "echo new");
    }

    #[tokio::test]
    async fn update_nonexistent_hook_shows_error() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/nonexistent/edit")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Test&slug=nonexistent&command=echo+test&enabled=true",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("error="));
        assert!(location.contains("not+found") || location.contains("not%20found"));
    }

    // --- Delete hook ---

    #[tokio::test]
    async fn delete_hook_removes_from_config() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "To Delete"
slug = "delete-me"
[hooks.executor]
type = "shell"
command = "echo delete"
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let cookie = create_test_session(&state).await;

        // Verify hook exists before deletion
        assert_eq!(state.config.load().hooks.len(), 1);

        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/delete-me/delete")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(location, "/");

        // Verify config was updated
        assert!(state.config.load().hooks.is_empty());
    }

    #[tokio::test]
    async fn delete_nonexistent_hook_shows_error() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/nonexistent/delete")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("error="));
    }

    // --- Checkbox behavior ---

    #[tokio::test]
    async fn create_hook_without_enabled_checkbox_creates_disabled() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;

        // Note: no "enabled" field in the form body — checkbox unchecked
        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/new")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=Disabled+Hook&slug=disabled&command=echo+off"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let config = state.config.load();
        assert!(!config.hooks[0].enabled);
    }

    // --- Duration parsing ---

    #[tokio::test]
    async fn create_hook_with_invalid_timeout_shows_error() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/new")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Bad&slug=bad&command=echo+bad&timeout=notaduration",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("error="));
    }

    // --- Trigger hook with payload validation ---

    #[tokio::test]
    async fn trigger_with_valid_payload_succeeds() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
[[hooks.payload.fields]]
name = "action"
type = "string"
required = true
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"action":"deploy"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn trigger_missing_required_field_returns_422() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
[[hooks.payload.fields]]
name = "action"
type = "string"
required = true
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "payload validation failed");
        assert_eq!(json["details"][0]["field"].as_str().unwrap(), "action");
    }

    #[tokio::test]
    async fn trigger_type_mismatch_returns_422() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
[[hooks.payload.fields]]
name = "count"
type = "number"
required = true
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"count":"not-a-number"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["details"][0]["message"]
            .as_str()
            .unwrap()
            .contains("expected type number"));
    }

    #[tokio::test]
    async fn trigger_invalid_json_returns_400() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
[[hooks.payload.fields]]
name = "action"
type = "string"
required = true
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .header("Content-Type", "application/json")
                    .body(Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn trigger_no_schema_accepts_any_body() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"anything":"goes"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn trigger_no_schema_accepts_empty_body() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn trigger_no_schema_accepts_non_json_body() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .body(Body::from("not json at all"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn trigger_stores_payload_in_execution() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"key":"value"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let exec_id = json["execution_id"].as_str().unwrap();

        let pool = state.db.pool();
        let exec = crate::models::execution::get_by_id(pool, exec_id)
            .await
            .unwrap();
        let stored: serde_json::Value =
            serde_json::from_str(&exec.request_payload).unwrap();
        assert_eq!(stored["key"], "value");
    }


    // --- parse_payload_text ---

    #[test]
    fn parse_payload_text_valid_lines() {
        let text = "action:string:required\ntag:string\ncount:number:required";
        let schema = parse_payload_text(text).unwrap().unwrap();
        assert_eq!(schema.fields.len(), 3);
        assert_eq!(schema.fields[0].name, "action");
        assert!(schema.fields[0].required);
        assert_eq!(schema.fields[1].name, "tag");
        assert!(!schema.fields[1].required);
        assert_eq!(schema.fields[2].name, "count");
        assert!(schema.fields[2].required);
    }

    #[test]
    fn parse_payload_text_empty_returns_none() {
        assert!(parse_payload_text("").unwrap().is_none());
        assert!(parse_payload_text("  \n  \n  ").unwrap().is_none());
    }

    #[test]
    fn parse_payload_text_invalid_type() {
        let result = parse_payload_text("action:integer:required");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown type"));
    }

    #[test]
    fn parse_payload_text_missing_type() {
        let result = parse_payload_text("action");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected"));
    }

    #[test]
    fn parse_payload_text_blank_lines_skipped() {
        let text = "action:string:required\n\n\ntag:string";
        let schema = parse_payload_text(text).unwrap().unwrap();
        assert_eq!(schema.fields.len(), 2);
    }

    #[test]
    fn parse_payload_text_explicit_optional() {
        let text = "tag:string:optional";
        let schema = parse_payload_text(text).unwrap().unwrap();
        assert!(!schema.fields[0].required);
    }

    #[test]
    fn parse_payload_text_invalid_required_flag() {
        let result = parse_payload_text("action:string:mandatory");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected 'required' or 'optional'"));
    }


    // --- Integration tests: payload schema end-to-end ---

    #[tokio::test]
    async fn create_hook_with_payload_schema_and_trigger() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;

        // Create hook with payload schema via form
        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/new")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Webhook&slug=webhook&command=echo+ok&enabled=true&payload_text=action%3Astring%3Arequired%0Atag%3Astring",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);

        // Verify schema was persisted
        let config = state.config.load();
        let hook = config.hooks.iter().find(|h| h.slug == "webhook").unwrap();
        let schema = hook.payload.as_ref().expect("schema should be set");
        assert_eq!(schema.fields.len(), 2);

        // Trigger with valid payload
        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/webhook")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"action":"deploy","tag":"v1.0"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_hook_with_payload_and_trigger_missing_required() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;

        // Create hook with required field
        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/new")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Webhook&slug=webhook&command=echo+ok&enabled=true&payload_text=action%3Astring%3Arequired",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);

        // Trigger without required field
        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/webhook")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn edit_hook_preserves_payload_schema() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
[[hooks.payload.fields]]
name = "action"
type = "string"
required = true
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let cookie = create_test_session(&state).await;

        // Load edit form -- verify it renders without error
        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/hooks/test/edit")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("action:string:required"));

        // Submit edit with modified schema (add a field)
        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/test/edit")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Test&slug=test&command=echo+ok&enabled=true&payload_text=action%3Astring%3Arequired%0Atag%3Astring",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);

        // Verify updated schema
        let config = state.config.load();
        let hook = &config.hooks[0];
        let schema = hook.payload.as_ref().unwrap();
        assert_eq!(schema.fields.len(), 2);
    }

    #[tokio::test]
    async fn edit_hook_clearing_payload_removes_schema() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
[[hooks.payload.fields]]
name = "action"
type = "string"
required = true
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let cookie = create_test_session(&state).await;

        // Submit edit with empty payload text
        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/test/edit")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Test&slug=test&command=echo+ok&enabled=true&payload_text=",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);

        // Verify schema removed
        let config = state.config.load();
        assert!(config.hooks[0].payload.is_none());

        // Trigger now accepts any body
        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"anything":"goes"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_hook_with_invalid_payload_text_shows_error() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;

        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/new")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Bad&slug=bad&command=echo+bad&enabled=true&payload_text=action%3Ainteger%3Arequired",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Should redirect back with error (existing pattern: flash message)
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("error="));
    }

    #[tokio::test]
    async fn trigger_empty_body_with_schema_returns_422() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
[[hooks.payload.fields]]
name = "action"
type = "string"
required = true
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "payload validation failed");
        assert!(json["details"][0]["message"]
            .as_str()
            .unwrap()
            .contains("missing"));
    }

    #[tokio::test]
    async fn trigger_multiple_errors_returns_all_in_422() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
[[hooks.payload.fields]]
name = "action"
type = "string"
required = true
[[hooks.payload.fields]]
name = "count"
type = "number"
required = true
"#;
        let (state, _dir) = test_state_with_config(toml).await;
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"action": 42, "count": "not-a-number"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let details = json["details"].as_array().unwrap();
        assert_eq!(details.len(), 2, "should accumulate both type-mismatch errors");
    }

    #[tokio::test]
    async fn trigger_dot_notation_field_validation() {
        let toml = r#"[server]
port = 8080

[[hooks]]
name = "Test"
slug = "test"
[hooks.executor]
type = "shell"
command = "echo ok"
[[hooks.payload.fields]]
name = "repo.name"
type = "string"
required = true
"#;
        let (state, _dir) = test_state_with_config(toml).await;

        // Valid nested payload
        let resp = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"repo": {"name": "myapp"}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Missing nested field
        let resp = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hook/test")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"repo": {}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

}
