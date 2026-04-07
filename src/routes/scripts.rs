use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::server::AppState;
use crate::templates::context;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/scripts", get(list_scripts))
}

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

async fn list_scripts(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let config = state.config.load();
    let scripts_dir = &config.scripts.dir;

    // Ensure the scripts directory exists
    if let Err(e) = tokio::fs::create_dir_all(scripts_dir).await {
        tracing::warn!(dir = %scripts_dir, error = %e, "failed to create scripts directory");
    }

    let mut entries = Vec::new();
    let mut read_dir = match tokio::fs::read_dir(scripts_dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Directory doesn't exist even after create attempt; show empty list
            let html = state
                .templates
                .render("scripts.html", context! { scripts => Vec::<()>::new() })?;
            return Ok(Html(html));
        }
        Err(e) => return Err(e.into()),
    };

    while let Some(entry) = read_dir.next_entry().await? {
        let metadata = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };

        // Skip directories — flat listing only
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

    let html = state
        .templates
        .render("scripts.html", context! { scripts => scripts })?;
    Ok(Html(html))
}

#[cfg(test)]
mod tests {
    use super::*;

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
