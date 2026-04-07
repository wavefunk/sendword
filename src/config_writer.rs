//! TOML write-back for `sendword.toml`.
//!
//! Uses `toml_edit` to parse and modify the config file while preserving
//! comments, formatting, and key ordering. Writes are atomic: content goes
//! to a temporary file first, then is renamed into place.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use toml_edit::{DocumentMut, Formatted, Item, Table, Value};

use crate::config::{AppConfig, BackoffStrategy, ConfigError, HmacAlgorithm, HookAuthConfig};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml parse error: {0}")]
    Parse(#[from] toml_edit::TomlError),

    #[error("hook not found: {0}")]
    HookNotFound(String),

    #[error("slug already exists: {0}")]
    SlugConflict(String),

    #[error("config validation failed: {0}")]
    Validation(#[from] ConfigError),
}

// ---------------------------------------------------------------------------
// Public data types for form submissions
// ---------------------------------------------------------------------------

/// Fields submitted when creating or editing a hook via the web form.
#[derive(Debug, Clone)]
pub struct HookFormData {
    pub name: String,
    pub slug: String,
    pub description: String,
    pub enabled: bool,
    pub command: String,
    pub cwd: Option<String>,
    pub env: HashMap<String, String>,
    pub timeout: Option<Duration>,
    pub retries: Option<RetryFormData>,
    pub auth: Option<HookAuthConfig>,
}

#[derive(Debug, Clone)]
pub struct RetryFormData {
    pub count: u32,
    pub backoff: BackoffStrategy,
    pub initial_delay: Duration,
    pub max_delay: Duration,
}

// ---------------------------------------------------------------------------
// ConfigWriter
// ---------------------------------------------------------------------------

/// Reads, modifies, and atomically writes `sendword.toml`.
pub struct ConfigWriter {
    path: PathBuf,
}

impl ConfigWriter {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Path to the config file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    // -- public mutations ----------------------------------------------------

    /// Append a new hook to the `[[hooks]]` array.
    pub fn add_hook(&self, data: &HookFormData) -> Result<(), WriteError> {
        let mut doc = self.read_document()?;

        // Check for slug conflicts
        if let Some(hooks) = doc.get("hooks").and_then(|v| v.as_array_of_tables()) {
            for table in hooks.iter() {
                if table.get("slug").and_then(|v| v.as_str()) == Some(&data.slug) {
                    return Err(WriteError::SlugConflict(data.slug.clone()));
                }
            }
        }

        let hook_table = build_hook_table(data);

        let hooks = doc
            .entry("hooks")
            .or_insert_with(|| Item::ArrayOfTables(Default::default()));

        if let Some(arr) = hooks.as_array_of_tables_mut() {
            arr.push(hook_table);
        }

        self.validate_and_write(&doc)?;
        Ok(())
    }

    /// Update an existing hook identified by `slug`.
    ///
    /// The slug itself is immutable after creation. All other fields in
    /// `data` overwrite the existing values (except `data.slug` which is
    /// used only for lookup).
    pub fn update_hook(&self, slug: &str, data: &HookFormData) -> Result<(), WriteError> {
        let mut doc = self.read_document()?;

        let idx = self.find_hook_index(&doc, slug)?;

        let hooks = doc["hooks"]
            .as_array_of_tables_mut()
            .expect("hooks is array of tables");

        let table = hooks.get_mut(idx).expect("index validated by find_hook_index");
        apply_hook_fields(table, data);

        self.validate_and_write(&doc)?;
        Ok(())
    }

    /// Remove a hook by slug.
    pub fn remove_hook(&self, slug: &str) -> Result<(), WriteError> {
        let mut doc = self.read_document()?;

        let idx = self.find_hook_index(&doc, slug)?;

        let hooks = doc["hooks"]
            .as_array_of_tables_mut()
            .expect("hooks is array of tables");

        hooks.remove(idx);

        // If no hooks remain, remove the key entirely to keep the file clean
        if hooks.is_empty() {
            doc.remove("hooks");
        }

        self.validate_and_write(&doc)?;
        Ok(())
    }

    // -- internal helpers ----------------------------------------------------

    /// Read and parse the TOML document, preserving formatting.
    fn read_document(&self) -> Result<DocumentMut, WriteError> {
        let content = std::fs::read_to_string(&self.path).unwrap_or_default();
        let doc: DocumentMut = content.parse()?;
        Ok(doc)
    }

    /// Find the index of a hook in the `[[hooks]]` array by slug.
    fn find_hook_index(&self, doc: &DocumentMut, slug: &str) -> Result<usize, WriteError> {
        let hooks = doc
            .get("hooks")
            .and_then(|v| v.as_array_of_tables())
            .ok_or_else(|| WriteError::HookNotFound(slug.to_owned()))?;

        hooks
            .iter()
            .position(|t| t.get("slug").and_then(|v| v.as_str()) == Some(slug))
            .ok_or_else(|| WriteError::HookNotFound(slug.to_owned()))
    }

    /// Validate the document by re-parsing it through `AppConfig::load_from`,
    /// then atomically write it to disk.
    fn validate_and_write(&self, doc: &DocumentMut) -> Result<(), WriteError> {
        let serialized = doc.to_string();

        // Validate by parsing through the normal config pipeline.
        // We write to a temp file first, then load from it.
        let dir = self.path.parent().unwrap_or(Path::new("."));
        let tmp = tempfile_in(dir, &serialized)?;
        let tmp_path_str = tmp.to_str().unwrap_or("");

        let validation_result = AppConfig::load_from(tmp_path_str, "nonexistent.json");
        if let Err(e) = validation_result {
            // Clean up temp file on validation failure
            let _ = std::fs::remove_file(&tmp);
            return Err(WriteError::Validation(e));
        }

        // Atomic rename into place
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// TOML table construction helpers
// ---------------------------------------------------------------------------

/// Build a complete `[[hooks]]` table from form data.
fn build_hook_table(data: &HookFormData) -> Table {
    let mut table = Table::new();
    apply_hook_fields(&mut table, data);
    // Slug is set during creation but not changed by apply_hook_fields
    table.insert("slug", toml_string(&data.slug));
    table
}

/// Apply (overwrite) all mutable hook fields on an existing table.
/// Does NOT touch the `slug` key.
fn apply_hook_fields(table: &mut Table, data: &HookFormData) {
    table.insert("name", toml_string(&data.name));
    table.insert("description", toml_string(&data.description));
    table.insert("enabled", toml_bool(data.enabled));

    // executor sub-table
    let mut executor = Table::new();
    executor.insert("type", toml_string("shell"));
    executor.insert("command", toml_string(&data.command));
    table.insert("executor", Item::Table(executor));

    // cwd — set or remove
    match &data.cwd {
        Some(cwd) if !cwd.is_empty() => {
            table.insert("cwd", toml_string(cwd));
        }
        _ => {
            table.remove("cwd");
        }
    }

    // env — inline table or remove
    if data.env.is_empty() {
        table.remove("env");
    } else {
        let mut env_table = Table::new();
        let mut keys: Vec<&String> = data.env.keys().collect();
        keys.sort();
        for key in keys {
            env_table.insert(key, toml_string(&data.env[key]));
        }
        table.insert("env", Item::Table(env_table));
    }

    // timeout — humantime string or remove
    match data.timeout {
        Some(t) => {
            table.insert("timeout", toml_string(&format_duration(t)));
        }
        None => {
            table.remove("timeout");
        }
    }

    // retries sub-table or remove
    match &data.retries {
        Some(r) if r.count > 0 => {
            let mut retries = Table::new();
            retries.insert("count", toml_int(r.count));
            retries.insert("backoff", toml_string(backoff_str(r.backoff)));
            retries.insert(
                "initial_delay",
                toml_string(&format_duration(r.initial_delay)),
            );
            retries.insert("max_delay", toml_string(&format_duration(r.max_delay)));
            table.insert("retries", Item::Table(retries));
        }
        _ => {
            table.remove("retries");
        }
    }

    // auth sub-table or remove
    match &data.auth {
        Some(HookAuthConfig::Bearer { token }) => {
            let mut auth_table = Table::new();
            auth_table.insert("mode", toml_string("bearer"));
            auth_table.insert("token", toml_string(token));
            table.insert("auth", Item::Table(auth_table));
        }
        Some(HookAuthConfig::Hmac { header, algorithm, secret }) => {
            let mut auth_table = Table::new();
            auth_table.insert("mode", toml_string("hmac"));
            auth_table.insert("header", toml_string(header));
            let algo_str = match algorithm {
                HmacAlgorithm::Sha256 => "sha256",
            };
            auth_table.insert("algorithm", toml_string(algo_str));
            auth_table.insert("secret", toml_string(secret));
            table.insert("auth", Item::Table(auth_table));
        }
        Some(HookAuthConfig::None) | None => {
            table.remove("auth");
        }
    }
}

fn toml_string(s: &str) -> Item {
    Item::Value(Value::String(Formatted::new(s.to_owned())))
}

fn toml_bool(b: bool) -> Item {
    Item::Value(Value::Boolean(Formatted::new(b)))
}

fn toml_int(n: u32) -> Item {
    Item::Value(Value::Integer(Formatted::new(i64::from(n))))
}

pub fn backoff_str(b: BackoffStrategy) -> &'static str {
    match b {
        BackoffStrategy::None => "none",
        BackoffStrategy::Linear => "linear",
        BackoffStrategy::Exponential => "exponential",
    }
}

/// Format a duration as a human-readable string compatible with `humantime`.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let millis = d.subsec_millis();

    if secs == 0 && millis > 0 {
        return format!("{millis}ms");
    }

    if secs < 60 {
        return format!("{secs}s");
    }

    if secs < 3600 && secs.is_multiple_of(60) {
        return format!("{}m", secs / 60);
    }

    if secs.is_multiple_of(3600) {
        return format!("{}h", secs / 3600);
    }

    // Fall back to seconds for non-round durations
    format!("{secs}s")
}

// ---------------------------------------------------------------------------
// Atomic file write
// ---------------------------------------------------------------------------

/// Write content to a temporary file in `dir` and return its path.
/// The caller is responsible for renaming or removing it.
fn tempfile_in(dir: &Path, content: &str) -> Result<PathBuf, std::io::Error> {
    use std::io::Write;

    let tmp_name = format!(".sendword-{}.tmp", std::process::id());
    let tmp_path = dir.join(tmp_name);

    let mut file = std::fs::File::create(&tmp_path)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;

    Ok(tmp_path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_config(content: &str) -> (tempfile::TempDir, ConfigWriter) {
        let dir = tempfile::TempDir::new().expect("tmp dir");
        let path = dir.path().join("sendword.toml");
        fs::write(&path, content).expect("write initial config");
        let writer = ConfigWriter::new(path);
        (dir, writer)
    }

    fn minimal_hook() -> HookFormData {
        HookFormData {
            name: "Test Hook".into(),
            slug: "test-hook".into(),
            description: String::new(),
            enabled: true,
            command: "echo hello".into(),
            cwd: None,
            env: HashMap::new(),
            timeout: None,
            retries: None,
            auth: None,
        }
    }

    fn read_config(writer: &ConfigWriter) -> String {
        fs::read_to_string(writer.path()).expect("read config")
    }

    #[test]
    fn add_hook_to_empty_config() {
        let (_dir, writer) = tmp_config(
            r#"[server]
port = 8080
"#,
        );

        writer.add_hook(&minimal_hook()).expect("add hook");

        let content = read_config(&writer);
        assert!(content.contains("[[hooks]]"));
        assert!(content.contains("test-hook"));
        assert!(content.contains("echo hello"));
        // Original content preserved
        assert!(content.contains("[server]"));
        assert!(content.contains("port = 8080"));
    }

    #[test]
    fn add_hook_preserves_comments() {
        let (_dir, writer) = tmp_config(
            r#"# Main config
[server]
# The port
port = 8080
"#,
        );

        writer.add_hook(&minimal_hook()).expect("add hook");

        let content = read_config(&writer);
        assert!(content.contains("# Main config"));
        assert!(content.contains("# The port"));
    }

    #[test]
    fn add_hook_rejects_duplicate_slug() {
        let (_dir, writer) = tmp_config(
            r#"[[hooks]]
name = "Existing"
slug = "test-hook"
[hooks.executor]
type = "shell"
command = "echo existing"
"#,
        );

        let err = writer.add_hook(&minimal_hook()).expect_err("should fail");
        assert!(matches!(err, WriteError::SlugConflict(_)));
    }

    #[test]
    fn update_hook_changes_fields() {
        let (_dir, writer) = tmp_config(
            r#"[[hooks]]
name = "Old Name"
slug = "my-hook"
description = "old desc"
enabled = true
[hooks.executor]
type = "shell"
command = "echo old"
"#,
        );

        let data = HookFormData {
            name: "New Name".into(),
            slug: "my-hook".into(),
            description: "new desc".into(),
            enabled: false,
            command: "echo new".into(),
            cwd: Some("/tmp".into()),
            env: HashMap::from([("KEY".into(), "val".into())]),
            timeout: Some(Duration::from_secs(60)),
            retries: None,
            auth: None,
        };

        writer.update_hook("my-hook", &data).expect("update hook");

        let content = read_config(&writer);
        assert!(content.contains("New Name"));
        assert!(content.contains("new desc"));
        assert!(content.contains("echo new"));
        assert!(content.contains("enabled = false"));
        assert!(content.contains(r#"timeout = "1m""#));
        assert!(content.contains("KEY"));
    }

    #[test]
    fn update_hook_not_found_returns_error() {
        let (_dir, writer) = tmp_config("[server]\nport = 8080\n");

        let err = writer
            .update_hook("nonexistent", &minimal_hook())
            .expect_err("should fail");
        assert!(matches!(err, WriteError::HookNotFound(_)));
    }

    #[test]
    fn remove_hook_by_slug() {
        let (_dir, writer) = tmp_config(
            r#"[[hooks]]
name = "Keep"
slug = "keep"
[hooks.executor]
type = "shell"
command = "echo keep"

[[hooks]]
name = "Remove"
slug = "remove-me"
[hooks.executor]
type = "shell"
command = "echo remove"
"#,
        );

        writer.remove_hook("remove-me").expect("remove hook");

        let content = read_config(&writer);
        assert!(content.contains("keep"));
        assert!(!content.contains("remove-me"));
    }

    #[test]
    fn remove_last_hook_removes_hooks_key() {
        let (_dir, writer) = tmp_config(
            r#"[server]
port = 8080

[[hooks]]
name = "Only"
slug = "only"
[hooks.executor]
type = "shell"
command = "echo only"
"#,
        );

        writer.remove_hook("only").expect("remove hook");

        let content = read_config(&writer);
        assert!(!content.contains("[[hooks]]"));
        assert!(content.contains("[server]"));
    }

    #[test]
    fn add_hook_with_retries() {
        let (_dir, writer) = tmp_config("[server]\nport = 8080\n");

        let mut data = minimal_hook();
        data.retries = Some(RetryFormData {
            count: 3,
            backoff: BackoffStrategy::Exponential,
            initial_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(60),
        });

        writer.add_hook(&data).expect("add hook");

        let content = read_config(&writer);
        assert!(content.contains("count = 3"));
        assert!(content.contains(r#"backoff = "exponential""#));
        assert!(content.contains(r#"initial_delay = "2s""#));
        assert!(content.contains(r#"max_delay = "1m""#));
    }

    #[test]
    fn add_hook_with_env_vars() {
        let (_dir, writer) = tmp_config("[server]\nport = 8080\n");

        let mut data = minimal_hook();
        data.env = HashMap::from([
            ("APP_ENV".into(), "production".into()),
            ("DEBUG".into(), "false".into()),
        ]);

        writer.add_hook(&data).expect("add hook");

        let content = read_config(&writer);
        assert!(content.contains("APP_ENV"));
        assert!(content.contains("production"));
        assert!(content.contains("DEBUG"));
    }

    #[test]
    fn format_duration_produces_human_readable() {
        assert_eq!(format_duration(Duration::from_millis(500)), "500ms");
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(Duration::from_secs(60)), "1m");
        assert_eq!(format_duration(Duration::from_secs(300)), "5m");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h");
        assert_eq!(format_duration(Duration::from_secs(7200)), "2h");
        assert_eq!(format_duration(Duration::from_secs(90)), "90s");
    }

    #[test]
    fn validation_rejects_invalid_hook_on_add() {
        let (_dir, writer) = tmp_config("[server]\nport = 8080\n");

        let mut data = minimal_hook();
        data.name = String::new(); // invalid: empty name
        data.command = "echo ok".into();

        let err = writer.add_hook(&data).expect_err("should fail");
        assert!(matches!(err, WriteError::Validation(_)));
    }

    #[test]
    fn atomic_write_preserves_original_on_validation_failure() {
        let original = r#"[server]
port = 8080
"#;
        let (_dir, writer) = tmp_config(original);

        let mut data = minimal_hook();
        data.name = String::new(); // invalid

        let _ = writer.add_hook(&data);

        let content = read_config(&writer);
        assert_eq!(content, original, "original file should be unchanged");
    }
}
