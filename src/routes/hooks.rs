use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::config::ExecutorConfig;
use crate::models::execution;
use crate::server::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/hook/{slug}", post(trigger_hook))
        .route("/hooks/{slug}", get(hook_detail))
}

#[derive(Serialize)]
struct TriggerResponse {
    execution_id: String,
}

async fn trigger_hook(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<TriggerResponse>, StatusCode> {
    let hook = state
        .config
        .hooks
        .iter()
        .find(|h| h.slug == slug && h.enabled)
        .ok_or(StatusCode::NOT_FOUND)?;

    let timeout = hook.timeout.unwrap_or(state.config.defaults.timeout);

    let command = match &hook.executor {
        ExecutorConfig::Shell { command } => command.clone(),
    };

    let env = hook.env.clone();
    let cwd = hook.cwd.clone();
    let logs_dir = state.config.logs.dir.clone();

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
        let result = crate::executor::run(&pool, ctx).await;
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

async fn hook_detail(Path(_slug): Path<String>) -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}
