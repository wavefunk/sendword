pub mod s3;
pub mod scheduler;
pub mod tarball;

pub use s3::{BackupEntry, S3Client};

use std::path::Path;

use sqlx::SqlitePool;

use crate::config::BackupConfig;
use crate::timestamp;

/// Error type for backup operations.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("S3 error: {0}")]
    S3(#[from] s3::S3Error),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// Create a backup: snapshot DB via VACUUM INTO, bundle with config, upload to S3.
/// Returns the S3 object key of the created backup.
pub async fn create_backup(
    pool: &SqlitePool,
    config: &BackupConfig,
    config_path: &Path,
) -> Result<String, BackupError> {
    let client = S3Client::new(config)?;

    // Create a temporary directory for the backup files
    let tmp = tempfile::TempDir::new()?;

    // Snapshot the database via VACUUM INTO.
    // VACUUM INTO takes a literal string path — we construct the SQL with the
    // path embedded. The path comes from tempfile (not user input), so this is safe.
    let snapshot_path = tmp.path().join("sendword.db");
    let snapshot_str = snapshot_path.to_string_lossy().into_owned();
    // Escape any single quotes in the path (standard SQL quoting).
    let escaped = snapshot_str.replace('\'', "''");
    {
        let vacuum_sql = format!("VACUUM INTO '{escaped}'");
        // AssertSqlSafe is needed because this query string is dynamically constructed.
        // The path comes from tempfile (controlled by us), not user input.
        sqlx::query(sqlx::AssertSqlSafe(vacuum_sql))
            .execute(pool)
            .await?;
    }

    // Create tarball
    let tarball_name = format!("backup-{}.tar.gz", timestamp::now_utc_filename());
    let tarball_path = tmp.path().join(&tarball_name);
    tarball::create_tarball(config_path, &snapshot_path, &tarball_path)?;

    // Upload to S3
    let tarball_bytes = std::fs::read(&tarball_path)?;
    client.put(&tarball_name, &tarball_bytes).await?;

    tracing::info!(key = %tarball_name, "backup created");
    Ok(tarball_name)
}

/// Restore from a backup: download from S3, extract, return paths for manual replacement.
///
/// The caller is responsible for replacing the live config and DB files atomically.
/// This function extracts to a temporary directory and returns its path.
pub async fn restore_backup(
    config: &BackupConfig,
    key: &str,
    output_dir: &Path,
) -> Result<(), BackupError> {
    let client = S3Client::new(config)?;

    let data = client.get(key).await?;
    let tmp = tempfile::TempDir::new()?;
    let tarball_path = tmp.path().join("download.tar.gz");
    std::fs::write(&tarball_path, &data)?;

    tarball::extract_tarball(&tarball_path, output_dir)?;
    tracing::info!(key = %key, dir = %output_dir.display(), "backup extracted");
    Ok(())
}

/// Apply retention policy: delete backups exceeding max_count or max_age.
pub async fn apply_retention(config: &BackupConfig) -> Result<(), BackupError> {
    let retention = &config.retention;
    if retention.max_count.is_none() && retention.max_age.is_none() {
        return Ok(());
    }

    let client = S3Client::new(config)?;
    let mut entries = client.list().await?;

    // Sort by last_modified ascending (oldest first)
    entries.sort_by(|a, b| a.last_modified.cmp(&b.last_modified));

    let mut to_delete: Vec<String> = Vec::new();

    // Apply max_count: keep only the newest N
    if let Some(max_count) = retention.max_count {
        let max_count = max_count as usize;
        if entries.len() > max_count {
            let excess = entries.len() - max_count;
            for entry in entries.iter().take(excess) {
                to_delete.push(entry.key.clone());
            }
        }
    }

    // Apply max_age: delete entries older than the cutoff
    if let Some(max_age) = retention.max_age {
        let cutoff = chrono::Utc::now() - chrono::Duration::from_std(max_age).unwrap_or_default();
        let cutoff_str = cutoff.to_rfc3339();
        for entry in &entries {
            if entry.last_modified < cutoff_str && !to_delete.contains(&entry.key) {
                to_delete.push(entry.key.clone());
            }
        }
    }

    for key in &to_delete {
        client.delete(key).await?;
        tracing::info!(key = %key, "deleted old backup (retention policy)");
    }

    Ok(())
}

/// List available backups.
pub async fn list_backups(config: &BackupConfig) -> Result<Vec<BackupEntry>, BackupError> {
    let client = S3Client::new(config)?;
    let mut entries = client.list().await?;
    // Sort newest first
    entries.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
    Ok(entries)
}
