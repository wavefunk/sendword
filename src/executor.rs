use std::collections::HashMap;
use std::time::Duration;

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
