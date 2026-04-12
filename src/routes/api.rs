use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;

use crate::config::AppConfig;
use crate::server::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/config/export", get(export_config))
        .route("/api/config/import", post(import_config))
}

/// GET /api/config/export
///
/// Returns the current loaded config as a JSON object.
async fn export_config(State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.config.load();
    let value = serde_json::to_value(&*config).unwrap_or(Value::Null);
    Json(value)
}

/// POST /api/config/import
///
/// Accepts a JSON config object. Validates, writes to disk as TOML, and reloads.
/// Returns 422 if validation fails, 500 on write/reload failure.
async fn import_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    // 1. Deserialize into AppConfig
    let config: AppConfig = match serde_json::from_value(body) {
        Ok(c) => c,
        Err(e) => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("invalid config: {e}") })),
            ));
        }
    };

    // 2. Validate
    if let Err(e) = config.validate() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": e.to_string() })),
        ));
    }

    // 3. Serialize to TOML and write to disk
    let toml_str = match toml_edit::ser::to_string_pretty(&config) {
        Ok(s) => s,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("failed to serialize config: {e}") })),
            ));
        }
    };

    let config_path = state.config_writer.path().to_owned();
    if let Err(e) = std::fs::write(&config_path, toml_str.as_bytes()) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("failed to write config: {e}") })),
        ));
    }

    // 4. Reload from disk
    if let Err(e) = state.reload_config() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("failed to reload config: {e}") })),
        ));
    }

    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use axum::Router;
    use tower::ServiceExt;

    use crate::config::AppConfig;
    use crate::db::Db;
    use crate::server::AppState;

    async fn test_state_with_config(toml_content: &str) -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let config_path = dir.path().join("sendword.toml");
        std::fs::write(&config_path, toml_content).expect("write config");

        let config = AppConfig::load_from(
            config_path.to_str().unwrap(),
            "nonexistent_overlay.json",
        )
        .expect("load config");

        let db = Db::new_in_memory().await.expect("db");
        db.migrate().await.expect("migrate");

        let templates = crate::templates::Templates::new(crate::templates::Templates::default_dir());
        let state = AppState::new(config, &config_path, db, templates);
        (state, dir)
    }

    fn app(state: Arc<AppState>) -> Router {
        Router::new().merge(router()).with_state(state)
    }

    #[tokio::test]
    async fn export_roundtrip() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let app = app(Arc::clone(&state));

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/config/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();

        // Verify key fields round-trip correctly
        assert_eq!(value["server"]["port"], 8080);
        let reparsed: AppConfig = serde_json::from_value(value).unwrap();
        assert_eq!(reparsed.server.port, 8080);
    }

    #[tokio::test]
    async fn import_invalid_rejects() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let original_port = state.config.load().server.port;
        let app = app(Arc::clone(&state));

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/config/import")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"server": {"port": 0}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        // Config should be unchanged
        assert_eq!(state.config.load().server.port, original_port);
    }

    #[tokio::test]
    async fn import_valid_updates_and_reloads() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let app = app(Arc::clone(&state));

        // Export current config, change the port, re-import
        let export_resp = Router::new()
            .merge(router())
            .with_state(Arc::clone(&state))
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/config/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(export_resp.into_body(), usize::MAX).await.unwrap();
        let mut config: Value = serde_json::from_slice(&body).unwrap();
        config["server"]["port"] = Value::from(9999u64);

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/config/import")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&config).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(state.config.load().server.port, 9999);
    }
}
