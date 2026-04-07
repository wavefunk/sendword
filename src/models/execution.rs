use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Pending,
    PendingApproval,
    Approved,
    Rejected,
    Expired,
    Running,
    Success,
    Failed,
    TimedOut,
}

impl ExecutionStatus {
    /// Returns true if this status represents a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Success | Self::Failed | Self::TimedOut | Self::Rejected | Self::Expired
        )
    }
}

impl std::fmt::Display for ExecutionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::PendingApproval => "pending_approval",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Running => "running",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Execution {
    pub id: String,
    pub hook_slug: String,
    pub triggered_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub status: ExecutionStatus,
    pub exit_code: Option<i32>,
    pub log_path: String,
    pub trigger_source: String,
    pub request_payload: String,
    pub retry_count: i32,
    pub retry_of: Option<String>,
    pub approved_at: Option<String>,
    pub approved_by: Option<String>,
}
