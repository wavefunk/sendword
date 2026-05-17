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
use crate::views::FlashMessages;
use crate::views::dashboard::{DashboardHookRow, DashboardStatusDot, render_dashboard_page};

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

        let recent_statuses = recent
            .iter()
            .rev()
            .map(|e| DashboardStatusDot::from_execution_status(&e.status))
            .collect();

        hooks.push(DashboardHookRow::new(
            &h.name,
            &h.slug,
            h.enabled,
            last.map(|e| e.triggered_at.clone()),
            recent_statuses,
        ));
    }

    render_dashboard_page(
        auth.email.as_str(),
        &hooks,
        FlashMessages {
            success: flash.success.as_deref(),
            error: flash.error.as_deref(),
        },
    )
}
