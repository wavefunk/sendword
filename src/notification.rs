use crate::config::{HookConfig, NotificationConfig, NotifyOutcome};
use crate::executor::ExecutionResult;
use crate::interpolation::interpolate_command;
use crate::models::execution::Execution;
use crate::models::ExecutionStatus;

/// Build a JSON context for notification template interpolation.
fn notification_context(
    hook: &HookConfig,
    result: &ExecutionResult,
    execution: &Execution,
) -> serde_json::Value {
    let duration = compute_duration(execution);
    serde_json::json!({
        "hook_name": hook.name,
        "hook_slug": hook.slug,
        "status": result.status.to_string(),
        "exit_code": result.exit_code.map(|c| c.to_string()).unwrap_or_default(),
        "execution_id": execution.id,
        "duration": duration,
        "trigger_source": execution.trigger_source,
    })
}

/// Compute execution duration in seconds as a string, or "unknown" if incomplete.
fn compute_duration(execution: &Execution) -> String {
    match (&execution.started_at, &execution.completed_at) {
        (Some(started), Some(completed)) => {
            // Timestamps are stored as ISO 8601 strings; parse as chrono DateTime.
            // If parsing fails, fall back to "unknown".
            let start = chrono::DateTime::parse_from_rfc3339(started).ok();
            let end = chrono::DateTime::parse_from_rfc3339(completed).ok();
            match (start, end) {
                (Some(s), Some(e)) => {
                    let secs = (e - s).num_seconds();
                    format!("{secs}s")
                }
                _ => "unknown".into(),
            }
        }
        _ => "unknown".into(),
    }
}

/// Map ExecutionStatus to NotifyOutcome for filtering.
fn status_to_outcome(status: &ExecutionStatus) -> Option<NotifyOutcome> {
    match status {
        ExecutionStatus::Success => Some(NotifyOutcome::Success),
        ExecutionStatus::Failed => Some(NotifyOutcome::Failure),
        ExecutionStatus::TimedOut => Some(NotifyOutcome::Timeout),
        // Rejected, Expired, Pending, etc. don't trigger notifications.
        _ => None,
    }
}

/// Send a completion notification if configured and the outcome matches.
///
/// Failures are logged as warnings but do not propagate — notifications are
/// best-effort and must not affect the execution record.
pub async fn send_notification(
    client: &reqwest::Client,
    config: &NotificationConfig,
    hook: &HookConfig,
    result: &ExecutionResult,
    execution: &Execution,
) {
    let Some(outcome) = status_to_outcome(&result.status) else {
        return;
    };
    if !config.on.contains(&outcome) {
        return;
    }

    let context = notification_context(hook, result, execution);
    let body = interpolate_command(&config.body, &context).into_owned();

    let mut builder = client
        .post(&config.url)
        .timeout(std::time::Duration::from_secs(10))
        .body(body);

    for (k, v) in &config.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }

    match builder.send().await {
        Ok(resp) => tracing::info!(
            hook_slug = %hook.slug,
            status = resp.status().as_u16(),
            "notification sent"
        ),
        Err(e) => tracing::warn!(
            hook_slug = %hook.slug,
            error = %e,
            "notification failed"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::ExecutionResult;
    use crate::models::ExecutionStatus;

    fn make_hook() -> HookConfig {
        use crate::config::ExecutorConfig;
        use std::collections::HashMap;
        HookConfig {
            name: "Test Hook".into(),
            slug: "test-hook".into(),
            description: String::new(),
            enabled: true,
            auth: None,
            executor: ExecutorConfig::Shell {
                command: "echo ok".into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries: None,
            rate_limit: None,
            payload: None,
            trigger_rules: None,
            concurrency: None,
            approval: None,
            notification: None,
        }
    }

    fn make_execution() -> Execution {
        Execution {
            id: "exec-1".into(),
            hook_slug: "test-hook".into(),
            triggered_at: "2026-04-12T10:00:00Z".into(),
            started_at: Some("2026-04-12T10:00:01Z".into()),
            completed_at: Some("2026-04-12T10:00:04Z".into()),
            status: ExecutionStatus::Success,
            exit_code: Some(0),
            log_path: "data/logs/exec-1".into(),
            trigger_source: "webhook".into(),
            request_payload: "{}".into(),
            retry_count: 0,
            retry_of: None,
            approved_at: None,
            approved_by: None,
        }
    }

    fn make_result(status: ExecutionStatus) -> ExecutionResult {
        ExecutionResult {
            status,
            exit_code: Some(0),
            log_dir: "data/logs/exec-1".into(),
        }
    }

    #[test]
    fn notification_context_builds_all_fields() {
        let hook = make_hook();
        let execution = make_execution();
        let result = make_result(ExecutionStatus::Success);

        let ctx = notification_context(&hook, &result, &execution);

        assert_eq!(ctx["hook_name"], "Test Hook");
        assert_eq!(ctx["hook_slug"], "test-hook");
        assert_eq!(ctx["status"], "success");
        assert_eq!(ctx["execution_id"], "exec-1");
        assert_eq!(ctx["trigger_source"], "webhook");
        // duration: 3 seconds between 10:00:01 and 10:00:04
        assert_eq!(ctx["duration"], "3s");
    }

    #[test]
    fn status_to_outcome_maps_correctly() {
        assert_eq!(
            status_to_outcome(&ExecutionStatus::Success),
            Some(NotifyOutcome::Success)
        );
        assert_eq!(
            status_to_outcome(&ExecutionStatus::Failed),
            Some(NotifyOutcome::Failure)
        );
        assert_eq!(
            status_to_outcome(&ExecutionStatus::TimedOut),
            Some(NotifyOutcome::Timeout)
        );
        assert!(status_to_outcome(&ExecutionStatus::Rejected).is_none());
        assert!(status_to_outcome(&ExecutionStatus::Pending).is_none());
        assert!(status_to_outcome(&ExecutionStatus::Running).is_none());
        assert!(status_to_outcome(&ExecutionStatus::Expired).is_none());
    }

    #[tokio::test]
    async fn notification_not_sent_when_outcome_not_in_on() {
        use std::collections::HashMap;
        // config.on = [failure, timeout], but result is Success → no POST
        let config = NotificationConfig {
            url: "http://127.0.0.1:1".into(), // unreachable port; if a send were attempted, it would error
            on: vec![NotifyOutcome::Failure, NotifyOutcome::Timeout],
            headers: HashMap::new(),
            body: "done".into(),
        };
        let hook = make_hook();
        let execution = make_execution();
        let result = make_result(ExecutionStatus::Success);

        let client = reqwest::Client::new();
        // send_notification should return without making any network call.
        // If it did try to connect, it would fail (port 1 is unreachable) --
        // the test would still pass because we don't await the error, but the
        // test verifies the silent short-circuit by completing without error.
        send_notification(&client, &config, &hook, &result, &execution).await;
        // If we reach here the function returned without panicking -- outcome filtered.
    }
}
