use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::server::AppState;
use crate::templates::context;

/// Maximum script file size: 1 MB.
const MAX_SCRIPT_SIZE: usize = 1024 * 1024;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/scripts", get(list_scripts))
        .route("/scripts/new", get(new_script).post(create_script))
        .route(
            "/scripts/{filename}",
            get(edit_script).post(save_script),
        )
        .route("/scripts/{filename}/delete", post(delete_script))
}

// --- Filename validation ---

/// Validate a script filename.
///
/// Allowed characters: `[a-zA-Z0-9._-]`. The filename must not start with
/// a dot (hidden files), must not contain path separators or `..`, and the
/// resolved path must remain inside `scripts_dir`.
fn validate_filename(filename: &str, scripts_dir: &Path) -> Result<PathBuf, &'static str> {
    if filename.is_empty() {
        return Err("Filename cannot be empty");
    }

    if filename.starts_with('.') {
        return Err("Filename cannot start with a dot");
    }

    // Reject any character outside the allowed set
    if !filename
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
    {
        return Err("Filename may only contain letters, numbers, hyphens, underscores, and dots");
    }

    // Reject path traversal components
    if filename.contains("..") {
        return Err("Filename cannot contain '..'");
    }

    let candidate = scripts_dir.join(filename);

    // Canonicalize the scripts dir for comparison. If the scripts dir
    // doesn't exist yet, use the joined path directly — the directory
    // will be created on write.
    let canon_dir = scripts_dir.canonicalize().unwrap_or_else(|_| scripts_dir.to_path_buf());

    // Canonicalize the candidate. If the file doesn't exist yet, canonicalize
    // the parent and append the filename so we can still verify containment.
    let canon_candidate = if candidate.exists() {
        candidate
            .canonicalize()
            .map_err(|_| "Failed to resolve file path")?
    } else {
        let parent = candidate.parent().ok_or("Invalid path")?;
        let canon_parent = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
        canon_parent.join(filename)
    };

    if !canon_candidate.starts_with(&canon_dir) {
        return Err("File path escapes the managed scripts directory");
    }

    Ok(candidate)
}

// --- Helpers ---

struct ScriptEntry {
    name: String,
    size: String,
    modified: String,
}

/// Format byte count into a human-readable string (B, KB, MB).
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        let kb = bytes as f64 / 1024.0;
        if kb < 10.0 {
            format!("{kb:.1} KB")
        } else {
            format!("{:.0} KB", kb)
        }
    } else {
        let mb = bytes as f64 / (1024.0 * 1024.0);
        format!("{mb:.1} MB")
    }
}

/// Format a system time into a human-readable relative/absolute string.
fn format_modified(modified: std::time::SystemTime) -> String {
    let dt: DateTime<Utc> = modified.into();
    dt.format("%Y-%m-%d %H:%M").to_string()
}

/// Ensure the scripts directory exists and return its path.
async fn ensure_scripts_dir(state: &AppState) -> PathBuf {
    let config = state.config.load();
    let scripts_dir = PathBuf::from(&config.scripts.dir);
    if let Err(e) = tokio::fs::create_dir_all(&scripts_dir).await {
        tracing::warn!(dir = %scripts_dir.display(), error = %e, "failed to create scripts directory");
    }
    scripts_dir
}

/// Set the executable bit on a file (Unix only).
#[cfg(unix)]
async fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = tokio::fs::metadata(path).await?;
    let mut perms = metadata.permissions();
    // Add owner+group+other execute bits (preserving existing mode)
    let mode = perms.mode() | 0o111;
    perms.set_mode(mode);
    tokio::fs::set_permissions(path, perms).await
}

#[cfg(not(unix))]
async fn set_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

// --- Query params for flash messages ---

#[derive(Deserialize)]
struct FlashParams {
    success: Option<String>,
    error: Option<String>,
}

// --- Handlers ---

async fn list_scripts(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(flash): Query<FlashParams>,
) -> Result<Html<String>, AppError> {
    let scripts_dir = ensure_scripts_dir(&state).await;

    let mut entries = Vec::new();
    let mut read_dir = match tokio::fs::read_dir(&scripts_dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let html = state.templates.render(
                "scripts.html",
                context! {
                    scripts => Vec::<()>::new(),
                    success => flash.success,
                    error => flash.error,
                    username => auth.username,
                    nav_active => "scripts",
                },
            )?;
            return Ok(Html(html));
        }
        Err(e) => return Err(e.into()),
    };

    while let Some(entry) = read_dir.next_entry().await? {
        let metadata = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };

        if metadata.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        let size = format_size(metadata.len());
        let modified = metadata
            .modified()
            .map(format_modified)
            .unwrap_or_default();

        entries.push(ScriptEntry {
            name,
            size,
            modified,
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));

    let scripts: Vec<_> = entries
        .iter()
        .map(|e| {
            context! {
                name => e.name,
                size => e.size,
                modified => e.modified,
            }
        })
        .collect();

    let html = state.templates.render(
        "scripts.html",
        context! {
            scripts => scripts,
            success => flash.success,
            error => flash.error,
            username => auth.username,
            nav_active => "scripts",
        },
    )?;
    Ok(Html(html))
}

// --- GET /scripts/new ---

async fn new_script(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(flash): Query<FlashParams>,
) -> Result<Html<String>, AppError> {
    let html = state.templates.render(
        "script_editor.html",
        context! {
            is_new => true,
            filename => "",
            content => "",
            success => flash.success,
            error => flash.error,
            username => auth.username,
            nav_active => "scripts",
        },
    )?;
    Ok(Html(html))
}

// --- POST /scripts/new ---

#[derive(Deserialize)]
struct NewScriptForm {
    filename: String,
    content: String,
}

async fn create_script(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<NewScriptForm>,
) -> Response {
    let scripts_dir = ensure_scripts_dir(&state).await;

    // Validate filename
    let path = match validate_filename(&form.filename, &scripts_dir) {
        Ok(p) => p,
        Err(msg) => {
            let encoded = urlencoding::encode(msg);
            return Redirect::to(&format!("/scripts/new?error={encoded}")).into_response();
        }
    };

    // Check if file already exists
    if path.exists() {
        let msg = urlencoding::encode("A script with that filename already exists");
        return Redirect::to(&format!("/scripts/new?error={msg}")).into_response();
    }

    // Check content size
    if form.content.len() > MAX_SCRIPT_SIZE {
        let msg = urlencoding::encode("Script content exceeds 1 MB limit");
        return Redirect::to(&format!("/scripts/new?error={msg}")).into_response();
    }

    // Write file
    if let Err(e) = tokio::fs::write(&path, &form.content).await {
        tracing::error!(path = %path.display(), error = %e, "failed to write script");
        let msg = urlencoding::encode("Failed to write script file");
        return Redirect::to(&format!("/scripts/new?error={msg}")).into_response();
    }

    // Set executable bit
    if let Err(e) = set_executable(&path).await {
        tracing::warn!(path = %path.display(), error = %e, "failed to set executable bit");
    }

    let encoded_name = urlencoding::encode(&form.filename);
    Redirect::to(&format!("/scripts/{encoded_name}?success=Script+created")).into_response()
}

// --- GET /scripts/:filename ---

async fn edit_script(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    AxumPath(filename): AxumPath<String>,
    Query(flash): Query<FlashParams>,
) -> Result<Html<String>, AppError> {
    let scripts_dir = ensure_scripts_dir(&state).await;

    let path = validate_filename(&filename, &scripts_dir)
        .map_err(|msg| eyre::eyre!(msg))?;

    if !path.is_file() {
        return Err(AppError::not_found("script"));
    }

    let content = tokio::fs::read_to_string(&path).await?;

    let html = state.templates.render(
        "script_editor.html",
        context! {
            is_new => false,
            filename => filename,
            content => content,
            success => flash.success,
            error => flash.error,
            username => auth.username,
            nav_active => "scripts",
        },
    )?;
    Ok(Html(html))
}

// --- POST /scripts/:filename ---

#[derive(Deserialize)]
struct SaveScriptForm {
    content: String,
}

async fn save_script(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    AxumPath(filename): AxumPath<String>,
    Form(form): Form<SaveScriptForm>,
) -> Response {
    let scripts_dir = ensure_scripts_dir(&state).await;

    let path = match validate_filename(&filename, &scripts_dir) {
        Ok(p) => p,
        Err(msg) => {
            let encoded = urlencoding::encode(msg);
            let encoded_name = urlencoding::encode(&filename);
            return Redirect::to(&format!("/scripts/{encoded_name}?error={encoded}"))
                .into_response();
        }
    };

    if !path.is_file() {
        return Redirect::to("/scripts?error=Script+not+found").into_response();
    }

    // Check content size
    if form.content.len() > MAX_SCRIPT_SIZE {
        let encoded_name = urlencoding::encode(&filename);
        let msg = urlencoding::encode("Script content exceeds 1 MB limit");
        return Redirect::to(&format!("/scripts/{encoded_name}?error={msg}")).into_response();
    }

    // Write file
    if let Err(e) = tokio::fs::write(&path, &form.content).await {
        tracing::error!(path = %path.display(), error = %e, "failed to write script");
        let encoded_name = urlencoding::encode(&filename);
        let msg = urlencoding::encode("Failed to save script");
        return Redirect::to(&format!("/scripts/{encoded_name}?error={msg}")).into_response();
    }

    // Set executable bit
    if let Err(e) = set_executable(&path).await {
        tracing::warn!(path = %path.display(), error = %e, "failed to set executable bit");
    }

    let encoded_name = urlencoding::encode(&filename);
    Redirect::to(&format!("/scripts/{encoded_name}?success=Script+saved")).into_response()
}

// --- POST /scripts/:filename/delete ---

async fn delete_script(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    AxumPath(filename): AxumPath<String>,
) -> Response {
    let scripts_dir = ensure_scripts_dir(&state).await;

    let path = match validate_filename(&filename, &scripts_dir) {
        Ok(p) => p,
        Err(msg) => {
            let encoded = urlencoding::encode(msg);
            return Redirect::to(&format!("/scripts?error={encoded}")).into_response();
        }
    };

    if !path.is_file() {
        return Redirect::to("/scripts?error=Script+not+found").into_response();
    }

    if let Err(e) = tokio::fs::remove_file(&path).await {
        tracing::error!(path = %path.display(), error = %e, "failed to delete script");
        let msg = urlencoding::encode("Failed to delete script");
        return Redirect::to(&format!("/scripts?error={msg}")).into_response();
    }

    Redirect::to("/scripts?success=Script+deleted").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // --- validate_filename unit tests ---

    fn test_dir() -> PathBuf {
        PathBuf::from("/tmp/sendword-test-scripts")
    }

    #[test]
    fn valid_filenames_accepted() {
        let dir = test_dir();
        assert!(validate_filename("deploy.sh", &dir).is_ok());
        assert!(validate_filename("my-script", &dir).is_ok());
        assert!(validate_filename("backup_db.py", &dir).is_ok());
        assert!(validate_filename("run123", &dir).is_ok());
        assert!(validate_filename("a.b.c", &dir).is_ok());
    }

    #[test]
    fn rejects_empty_filename() {
        assert!(validate_filename("", &test_dir()).is_err());
    }

    #[test]
    fn rejects_leading_dot() {
        assert!(validate_filename(".hidden", &test_dir()).is_err());
        assert!(validate_filename(".env", &test_dir()).is_err());
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(validate_filename("../etc/passwd", &test_dir()).is_err());
        assert!(validate_filename("..sneaky", &test_dir()).is_err());
    }

    #[test]
    fn rejects_slashes() {
        assert!(validate_filename("sub/script.sh", &test_dir()).is_err());
        assert!(validate_filename("a\\b", &test_dir()).is_err());
    }

    #[test]
    fn rejects_spaces_and_special_chars() {
        assert!(validate_filename("my script.sh", &test_dir()).is_err());
        assert!(validate_filename("script;rm -rf", &test_dir()).is_err());
        assert!(validate_filename("file\0name", &test_dir()).is_err());
    }

    // --- format_size ---

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn format_size_kilobytes() {
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(10240), "10 KB");
        assert_eq!(format_size(512 * 1024), "512 KB");
    }

    #[test]
    fn format_size_megabytes() {
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(2 * 1024 * 1024 + 512 * 1024), "2.5 MB");
    }
}
