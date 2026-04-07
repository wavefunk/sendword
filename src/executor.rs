use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::fs;

use crate::models::ExecutionStatus;

/// Everything the executor needs to run a command.
pub struct ExecutionContext {
    /// The execution record ID (UUIDv7 string).
    pub execution_id: String,
    /// The hook slug, passed as SENDWORD_HOOK_SLUG env var.
    pub hook_slug: String,
    /// Shell command to run via `sh -c`.
    pub command: String,
    /// Additional environment variables for the process.
    pub env: HashMap<String, String>,
    /// Working directory. If None, inherits from the server process.
    pub cwd: Option<String>,
    /// Maximum execution time. Process is killed on expiry.
    pub timeout: Duration,
    /// Base directory for log files (e.g., "data/logs").
    pub logs_dir: String,
}

/// The outcome of an execution attempt.
pub struct ExecutionResult {
    /// Terminal status: Success, Failed, or TimedOut.
    pub status: ExecutionStatus,
    /// Process exit code. None if the process was killed or failed to spawn.
    pub exit_code: Option<i32>,
    /// Path to the log directory (data/logs/{execution_id}).
    pub log_dir: String,
}

/// Create the log directory and open stdout/stderr files for writing.
/// Returns (log_dir_path, stdout_file, stderr_file).
async fn prepare_log_files(
    logs_dir: &str,
    execution_id: &str,
) -> std::io::Result<(PathBuf, fs::File, fs::File)> {
    let log_dir = Path::new(logs_dir).join(execution_id);
    fs::create_dir_all(&log_dir).await?;

    let stdout_file = fs::File::create(log_dir.join("stdout.log")).await?;
    let stderr_file = fs::File::create(log_dir.join("stderr.log")).await?;

    Ok((log_dir, stdout_file, stderr_file))
}

/// Collect system environment variables that should be passed to child processes.
/// Returns only the vars that are actually set in the current process.
fn system_env_vars() -> HashMap<String, String> {
    const INHERIT_VARS: &[&str] = &["PATH", "HOME", "USER", "LANG"];

    let mut vars = HashMap::with_capacity(INHERIT_VARS.len());
    for &name in INHERIT_VARS {
        if let Ok(val) = std::env::var(name) {
            vars.insert(name.into(), val);
        }
    }
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn prepare_log_files_creates_directory_and_files() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let logs_dir = tmp.path().to_str().expect("utf-8 path");
        let exec_id = "test-exec-001";

        let (log_dir, _stdout, _stderr) = prepare_log_files(logs_dir, exec_id)
            .await
            .expect("prepare_log_files");

        assert!(log_dir.exists());
        assert!(log_dir.join("stdout.log").exists());
        assert!(log_dir.join("stderr.log").exists());
    }

    #[test]
    fn system_env_vars_includes_path() {
        let vars = system_env_vars();
        assert!(
            vars.contains_key("PATH"),
            "PATH should be present in system env vars"
        );
    }

    #[test]
    fn system_env_vars_excludes_arbitrary_vars() {
        // Safety: test-only env var, unique name avoids collisions
        unsafe { std::env::set_var("SENDWORD_TEST_ARBITRARY_XYZ_999", "leaked") };
        let vars = system_env_vars();
        assert!(
            !vars.contains_key("SENDWORD_TEST_ARBITRARY_XYZ_999"),
            "arbitrary env vars should not be inherited"
        );
        unsafe { std::env::remove_var("SENDWORD_TEST_ARBITRARY_XYZ_999") };
    }

    #[tokio::test]
    async fn prepare_log_files_creates_nested_parents() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        // logs_dir itself does not exist yet
        let nested = tmp.path().join("a").join("b").join("logs");
        let logs_dir = nested.to_str().expect("utf-8 path");
        let exec_id = "test-exec-002";

        let (log_dir, _stdout, _stderr) = prepare_log_files(logs_dir, exec_id)
            .await
            .expect("prepare_log_files");

        assert!(log_dir.exists());
        assert!(log_dir.join("stdout.log").exists());
        assert!(log_dir.join("stderr.log").exists());
    }
}
