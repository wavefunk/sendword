use std::fs;
use std::io;
use std::path::Path;

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use tar::{Archive, Builder};

/// Create a `.tar.gz` archive containing the config file, optional JSON overlay,
/// and the SQLite DB snapshot. Returns the number of bytes written.
///
/// File layout inside the archive:
/// - `sendword.toml` (from `config_path`)
/// - `sendword.db`   (from `db_snapshot_path`)
pub fn create_tarball(
    config_path: &Path,
    db_snapshot_path: &Path,
    output_path: &Path,
) -> io::Result<()> {
    let output_file = fs::File::create(output_path)?;
    let gz = GzEncoder::new(output_file, Compression::default());
    let mut archive = Builder::new(gz);

    // Add config file as sendword.toml
    let config_name = config_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("sendword.toml");
    archive.append_path_with_name(config_path, config_name)?;

    // Add DB snapshot as sendword.db
    archive.append_path_with_name(db_snapshot_path, "sendword.db")?;

    archive.into_inner()?.finish()?;
    Ok(())
}

/// Extract a `.tar.gz` archive to the given output directory.
pub fn extract_tarball(tarball_path: &Path, output_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(output_dir)?;
    let file = fs::File::open(tarball_path)?;
    let gz = GzDecoder::new(file);
    let mut archive = Archive::new(gz);
    archive.unpack(output_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_extract_roundtrip() {
        let tmp = tempfile::TempDir::new().expect("temp dir");

        // Create dummy config and DB files
        let config_path = tmp.path().join("sendword.toml");
        let db_path = tmp.path().join("sendword.db");
        fs::write(&config_path, b"[server]\nport = 8080\n").expect("write config");
        fs::write(&db_path, b"SQLITE_FAKE_DB").expect("write db");

        let tarball_path = tmp.path().join("backup.tar.gz");
        create_tarball(&config_path, &db_path, &tarball_path).expect("create tarball");

        assert!(tarball_path.exists(), "tarball should be created");

        let extract_dir = tmp.path().join("extracted");
        extract_tarball(&tarball_path, &extract_dir).expect("extract tarball");

        let extracted_config = fs::read(extract_dir.join("sendword.toml")).expect("read config");
        assert_eq!(extracted_config, b"[server]\nport = 8080\n");

        let extracted_db = fs::read(extract_dir.join("sendword.db")).expect("read db");
        assert_eq!(extracted_db, b"SQLITE_FAKE_DB");
    }

    /// A tarball that doesn't include sendword.json (no JSON config overlay) should
    /// extract without error. The caller is responsible for deciding if the absence
    /// of config.json is acceptable — the extraction step itself must not fail.
    #[test]
    fn extract_handles_missing_json_config() {
        let tmp = tempfile::TempDir::new().expect("temp dir");

        // Build a tarball that contains only the TOML config and the DB snapshot.
        // There is no sendword.json in this archive.
        let config_path = tmp.path().join("sendword.toml");
        let db_path = tmp.path().join("sendword.db");
        fs::write(&config_path, b"[server]\nport = 9090\n").expect("write config");
        fs::write(&db_path, b"DB_BYTES").expect("write db");

        let tarball_path = tmp.path().join("no-json-backup.tar.gz");
        create_tarball(&config_path, &db_path, &tarball_path).expect("create tarball");

        // Extracting the tarball must succeed even though sendword.json is absent.
        let extract_dir = tmp.path().join("extracted");
        let result = extract_tarball(&tarball_path, &extract_dir);
        assert!(result.is_ok(), "extraction should succeed without sendword.json: {result:?}");

        // The TOML config and DB snapshot are present.
        assert!(
            extract_dir.join("sendword.toml").exists(),
            "sendword.toml should be extracted"
        );
        assert!(
            extract_dir.join("sendword.db").exists(),
            "sendword.db should be extracted"
        );

        // The JSON overlay is absent — that's the point of this test.
        assert!(
            !extract_dir.join("sendword.json").exists(),
            "sendword.json should not exist in this archive"
        );
    }
}
