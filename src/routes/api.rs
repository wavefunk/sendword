use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;

use crate::backup::BackupEntry;
use crate::config::AppConfig;
use crate::extractors::AuthUser;
use crate::server::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/config/export", get(export_config))
        .route("/api/config/import", post(import_config))
        .route("/api/backup/list", get(list_backups))
        .route("/api/backup/create", post(create_backup))
        .route("/api/backup/restore", post(restore_backup))
}

/// GET /api/config/export
///
/// Returns the current loaded config as a JSON object.
async fn export_config(_auth: AuthUser, State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.config.load();
    let value = serde_json::to_value(&*config).unwrap_or(Value::Null);
    Json(value)
}

/// POST /api/config/import
///
/// Accepts a JSON config object. Validates, writes to disk as TOML, and reloads.
/// Returns 422 if validation fails, 500 on write/reload failure.
async fn import_config(
    _auth: AuthUser,
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

// --- Backup endpoints ---

#[derive(serde::Serialize)]
struct BackupListItem {
    key: String,
    size: u64,
    last_modified: String,
}

impl From<BackupEntry> for BackupListItem {
    fn from(e: BackupEntry) -> Self {
        Self {
            key: e.key,
            size: e.size,
            last_modified: e.last_modified,
        }
    }
}

/// GET /api/backup/list
///
/// Lists available backups from S3. Returns 503 if backup is not configured.
async fn list_backups(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<BackupListItem>>, (StatusCode, Json<Value>)> {
    let config = state.config.load();
    let backup_config = config.backup.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "backup is not configured" })),
        )
    })?;

    match crate::backup::list_backups(backup_config).await {
        Ok(entries) => Ok(Json(
            entries.into_iter().map(BackupListItem::from).collect(),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )),
    }
}

/// POST /api/backup/create
///
/// Triggers an immediate backup. Returns the S3 key of the created backup.
async fn create_backup(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let config = state.config.load();
    let backup_config = config.backup.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "backup is not configured" })),
        )
    })?;

    let config_path = state.config_writer.path().to_owned();
    let pool = state.db.pool().clone();

    match crate::backup::create_backup(&pool, backup_config, &config_path).await {
        Ok(key) => Ok(Json(serde_json::json!({ "key": key }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )),
    }
}

#[derive(Deserialize)]
struct RestoreRequest {
    key: String,
    /// Must be `true` to proceed with restore (safety gate).
    #[serde(default)]
    confirm: bool,
}

/// POST /api/backup/restore
///
/// Downloads a backup and extracts it to a temporary directory. Returns the
/// extracted file paths for manual application. Requires `{"key": "...", "confirm": true}`.
async fn restore_backup(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<RestoreRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !body.confirm {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "restore requires confirm: true in request body"
            })),
        ));
    }

    let config = state.config.load();
    let backup_config = config.backup.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "backup is not configured" })),
        )
    })?;

    let tmp = tempfile::TempDir::new().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
    })?;

    let output_dir = tmp.path().to_path_buf();
    if let Err(e) = crate::backup::restore_backup(backup_config, &body.key, &output_dir).await {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ));
    }

    // List extracted files
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&output_dir) {
        for entry in entries.flatten() {
            files.push(entry.file_name().to_string_lossy().into_owned());
        }
    }

    Ok(Json(serde_json::json!({
        "key": body.key,
        "files": files,
        "message": "backup extracted. Apply manually from the extraction directory.",
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use tower::ServiceExt;

    use allowthem_core::{AllowThemBuilder, Email, EmbeddedAuthClient, generate_token, hash_token};
    use chrono::{Duration, Utc};

    use crate::config::AppConfig;
    use crate::db::Db;
    use crate::server::AppState;

    async fn test_state_with_config(toml_content: &str) -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let config_path = dir.path().join("sendword.toml");
        std::fs::write(&config_path, toml_content).expect("write config");

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

    fn app(state: Arc<AppState>) -> Router {
        Router::new().merge(router()).with_state(state)
    }

    async fn create_test_session(state: &Arc<AppState>) -> String {
        let email = Email::new("admin@example.com".into()).unwrap();
        let user = state
            .ath
            .db()
            .create_user(email, "password123", None, None)
            .await
            .unwrap();
        let token = generate_token();
        let token_hash = hash_token(&token);
        let expires = Utc::now() + Duration::hours(24);
        state
            .ath
            .db()
            .create_session(user.id, token_hash, None, None, expires)
            .await
            .unwrap();
        let cookie = state.ath.session_cookie(&token);
        cookie.split(';').next().unwrap().to_string()
    }

    #[tokio::test]
    async fn export_roundtrip() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;
        let app = app(Arc::clone(&state));

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/config/export")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();

        // Verify key fields round-trip correctly
        assert_eq!(value["server"]["port"], 8080);
        let reparsed: AppConfig = serde_json::from_value(value).unwrap();
        assert_eq!(reparsed.server.port, 8080);
    }

    #[tokio::test]
    async fn import_invalid_rejects() {
        let (state, _dir) = test_state_with_config("[server]\nport = 8080\n").await;
        let cookie = create_test_session(&state).await;
        let original_port = state.config.load().server.port;
        let app = app(Arc::clone(&state));

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/config/import")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
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
        let cookie = create_test_session(&state).await;
        let app = app(Arc::clone(&state));

        // Export current config, change the port, re-import
        let export_resp = Router::new()
            .merge(router())
            .with_state(Arc::clone(&state))
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/config/export")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(export_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let mut config: Value = serde_json::from_slice(&body).unwrap();
        config["server"]["port"] = Value::from(9999u64);

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/config/import")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(serde_json::to_string(&config).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(state.config.load().server.port, 9999);
    }
}
