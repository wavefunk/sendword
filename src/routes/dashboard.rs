use std::sync::Arc;

use axum::Router;
use axum::extract::{Query, State};
use axum::response::Html;
use axum::routing::get;
use serde::Deserialize;

use crate::error::AppError;
use crate::extractors::AuthUser;
use crate::models::execution;
use crate::server::AppState;
use crate::templates::context;

#[derive(Deserialize)]
struct FlashParams {
    success: Option<String>,
    error: Option<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/", get(dashboard))
}

async fn dashboard(
    AuthUser(auth): AuthUser,
    State(state): State<Arc<AppState>>,
    Query(flash): Query<FlashParams>,
) -> Result<Html<String>, AppError> {
    let config = state.config.load();
    let pool = state.db.pool();

    let mut hooks = Vec::with_capacity(config.hooks.len());
    for h in &config.hooks {
        let recent = match execution::list_recent_by_hook(pool, &h.slug, 5).await {
            Ok(execs) => execs,
            Err(e) => {
                tracing::warn!(hook = %h.slug, error = %e, "failed to fetch recent executions");
                Vec::new()
            }
        };

        let last = recent.first();

        // Build a list of status strings for the last 5 executions (oldest last,
        // displayed as dots left-to-right from oldest to newest).
        let recent_statuses: Vec<String> =
            recent.iter().rev().map(|e| e.status.to_string()).collect();

        hooks.push(context! {
            name => h.name,
            slug => h.slug,
            description => h.description,
            enabled => h.enabled,
            last_status => last.map(|e| e.status.to_string()),
            last_triggered_at => last.map(|e| &e.triggered_at),
            last_execution_id => last.map(|e| &e.id),
            recent_statuses => recent_statuses,
        });
    }

    let html = state.templates.render(
        "dashboard.html",
        context! {
            hooks => hooks,
            success => flash.success,
            error => flash.error,
            username => auth.email.as_str(),
            nav_active => "dashboard",
        },
    )?;
    Ok(Html(html))
}
