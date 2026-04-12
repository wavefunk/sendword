pub fn now_utc() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// Returns a timestamp string suitable for use in filenames (no colons or dots).
/// Format: `YYYYMMDD-HHMMSS`
pub fn now_utc_filename() -> String {
    chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string()
}
