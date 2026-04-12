pub mod cooldown;
pub mod payload_filter;
pub mod rate_limit;
pub mod time_window;

use crate::models::trigger_attempt::TriggerAttemptStatus;

/// Outcome of a trigger rule evaluation.
pub enum EvalOutcome {
    /// Request passes this evaluator.
    Allow,
    /// Request is rejected by this evaluator.
    Reject {
        status: TriggerAttemptStatus,
        reason: String,
    },
}
