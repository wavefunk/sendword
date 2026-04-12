use crate::config::ApprovalConfig;

/// Returns true if the hook config requires human approval before execution.
pub fn requires_approval(approval: Option<&ApprovalConfig>) -> bool {
    approval.map_or(false, |a| a.required)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn approval_required() -> ApprovalConfig {
        ApprovalConfig {
            required: true,
            timeout: None,
        }
    }

    fn approval_not_required() -> ApprovalConfig {
        ApprovalConfig {
            required: false,
            timeout: Some(Duration::from_secs(300)),
        }
    }

    #[test]
    fn requires_approval_true_when_required() {
        assert!(requires_approval(Some(&approval_required())));
    }

    #[test]
    fn requires_approval_false_when_not_required() {
        assert!(!requires_approval(Some(&approval_not_required())));
    }

    #[test]
    fn requires_approval_false_when_none() {
        assert!(!requires_approval(None));
    }
}
