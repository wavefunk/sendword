use askama::Template;
use axum::response::Html;

use crate::error::AppError;
use crate::models::{Execution, ExecutionStatus, TriggerAttempt, TriggerAttemptStatus};

use super::{PaginationState, render_partial};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionListRow {
    pub id: String,
    pub short_id: String,
    pub triggered_at: String,
    pub status_label: String,
    pub status_class: &'static str,
    pub exit_code: Option<i32>,
    pub duration: Option<String>,
}

impl ExecutionListRow {
    pub fn from_execution(execution: &Execution) -> Self {
        let status = execution.status.to_string();
        Self {
            id: execution.id.clone(),
            short_id: short_id(&execution.id),
            triggered_at: execution.triggered_at.clone(),
            status_label: status.to_uppercase(),
            status_class: execution_status_class(&execution.status),
            exit_code: execution.exit_code,
            duration: compute_duration(&execution.started_at, &execution.completed_at),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionListView {
    pub slug: String,
    pub rows: Vec<ExecutionListRow>,
    pub pagination: PaginationState,
    pub active_status: String,
    pub active_from: String,
    pub active_to: String,
}

impl ExecutionListView {
    pub fn new(
        slug: impl Into<String>,
        rows: Vec<ExecutionListRow>,
        page: i64,
        per_page: i64,
        total: i64,
        active_status: impl Into<String>,
        active_from: impl Into<String>,
        active_to: impl Into<String>,
    ) -> Self {
        let item_count = rows.len() as i64;
        Self {
            slug: slug.into(),
            rows,
            pagination: PaginationState::new(page, per_page, total, item_count),
            active_status: active_status.into(),
            active_from: active_from.into(),
            active_to: active_to.into(),
        }
    }

    pub fn has_pages(&self) -> bool {
        self.pagination.total_pages() > 1
    }

    pub fn previous_page(&self) -> i64 {
        self.pagination.page - 1
    }

    pub fn next_page(&self) -> i64 {
        self.pagination.page + 1
    }
}

pub fn render_execution_list(view: &ExecutionListView) -> Result<Html<String>, AppError> {
    render_partial(&ExecutionListTemplate { view })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptListRow {
    pub attempted_at: String,
    pub status_label: String,
    pub status_class: &'static str,
    pub source_ip: String,
    pub reason: String,
    pub execution_id: Option<String>,
    pub execution_short_id: Option<String>,
}

impl AttemptListRow {
    pub fn from_attempt(attempt: &TriggerAttempt) -> Self {
        let status = attempt.status.to_string();
        Self {
            attempted_at: attempt.attempted_at.clone(),
            status_label: status.to_uppercase(),
            status_class: attempt_status_class(&attempt.status),
            source_ip: attempt.source_ip.clone(),
            reason: attempt.reason.clone(),
            execution_id: attempt.execution_id.clone(),
            execution_short_id: attempt.execution_id.as_ref().map(|id| short_id(id)),
        }
    }

    pub fn has_reason(&self) -> bool {
        !self.reason.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptListView {
    pub slug: String,
    pub rows: Vec<AttemptListRow>,
    pub pagination: PaginationState,
    pub active_status: String,
}

impl AttemptListView {
    pub fn new(
        slug: impl Into<String>,
        rows: Vec<AttemptListRow>,
        page: i64,
        per_page: i64,
        total: i64,
        active_status: impl Into<String>,
    ) -> Self {
        let item_count = rows.len() as i64;
        Self {
            slug: slug.into(),
            rows,
            pagination: PaginationState::new(page, per_page, total, item_count),
            active_status: active_status.into(),
        }
    }

    pub fn has_pages(&self) -> bool {
        self.pagination.total_pages() > 1
    }

    pub fn previous_page(&self) -> i64 {
        self.pagination.page - 1
    }

    pub fn next_page(&self) -> i64 {
        self.pagination.page + 1
    }
}

pub fn render_attempt_list(view: &AttemptListView) -> Result<Html<String>, AppError> {
    render_partial(&AttemptListTemplate { view })
}

fn execution_status_class(status: &ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Success => "wf-tag ok",
        ExecutionStatus::Failed
        | ExecutionStatus::TimedOut
        | ExecutionStatus::Rejected
        | ExecutionStatus::Expired => "wf-tag err",
        ExecutionStatus::Running => "wf-tag warn",
        ExecutionStatus::Pending | ExecutionStatus::PendingApproval => "wf-tag info",
        ExecutionStatus::Approved => "wf-tag",
    }
}

fn attempt_status_class(status: &TriggerAttemptStatus) -> &'static str {
    match status {
        TriggerAttemptStatus::Fired => "wf-tag ok",
        TriggerAttemptStatus::AuthFailed | TriggerAttemptStatus::ValidationFailed => "wf-tag err",
        TriggerAttemptStatus::RateLimited | TriggerAttemptStatus::ConcurrencyRejected => {
            "wf-tag warn"
        }
        TriggerAttemptStatus::Filtered
        | TriggerAttemptStatus::ScheduleSkipped
        | TriggerAttemptStatus::CooldownSkipped
        | TriggerAttemptStatus::PendingApproval => "wf-tag info",
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Compute duration string from ISO8601 timestamps.
fn compute_duration(started_at: &Option<String>, completed_at: &Option<String>) -> Option<String> {
    let started = started_at.as_ref()?;
    let completed = completed_at.as_ref()?;

    let start = chrono::DateTime::parse_from_rfc3339(started).ok()?;
    let end = chrono::DateTime::parse_from_rfc3339(completed).ok()?;
    let duration = end.signed_duration_since(start);

    let secs = duration.num_seconds();
    if secs < 0 {
        return None;
    }

    if secs < 60 {
        let ms = duration.num_milliseconds() % 1000;
        Some(format!("{secs}.{ms:03}s"))
    } else if secs < 3600 {
        Some(format!("{}m {}s", secs / 60, secs % 60))
    } else {
        Some(format!("{}h {}m", secs / 3600, (secs % 3600) / 60))
    }
}

#[derive(Debug, Template)]
#[template(
    source = r##"
<div class="wf-filterbar">
  <select class="wf-select sm" name="status" hx-get="/hooks/{{ view.slug }}/executions" hx-target="#activity-tab-executions" hx-swap="innerHTML" hx-include="[name='from_date'],[name='to_date']">
    <option value=""{% if view.active_status.is_empty() %} selected{% endif %}>ALL STATUS</option>
    <option value="success"{% if view.active_status == "success" %} selected{% endif %}>SUCCESS</option>
    <option value="failed"{% if view.active_status == "failed" %} selected{% endif %}>FAILED</option>
    <option value="timed_out"{% if view.active_status == "timed_out" %} selected{% endif %}>TIMED OUT</option>
    <option value="running"{% if view.active_status == "running" %} selected{% endif %}>RUNNING</option>
    <option value="pending"{% if view.active_status == "pending" %} selected{% endif %}>PENDING</option>
    <option value="pending_approval"{% if view.active_status == "pending_approval" %} selected{% endif %}>PENDING APPROVAL</option>
    <option value="rejected"{% if view.active_status == "rejected" %} selected{% endif %}>REJECTED</option>
    <option value="expired"{% if view.active_status == "expired" %} selected{% endif %}>EXPIRED</option>
  </select>
  <input class="wf-input sm" type="date" name="from_date" value="{{ view.active_from }}" hx-get="/hooks/{{ view.slug }}/executions" hx-target="#activity-tab-executions" hx-swap="innerHTML" hx-include="[name='status'],[name='to_date']" hx-trigger="change">
  <input class="wf-input sm" type="date" name="to_date" value="{{ view.active_to }}" hx-get="/hooks/{{ view.slug }}/executions" hx-target="#activity-tab-executions" hx-swap="innerHTML" hx-include="[name='status'],[name='from_date']" hx-trigger="change">
</div>

{% if view.rows.is_empty() %}
<div class="wf-empty">
  <div class="wf-empty-msg">No executions{% if !view.active_status.is_empty() %} matching "{{ view.active_status }}"{% endif %}.</div>
</div>
{% else %}
<div class="wf-tablescroll wf-grow">
  <table class="wf-table sticky is-interactive">
    <thead>
      <tr><th>ID</th><th>TRIGGERED</th><th>STATUS</th><th class="num">EXIT</th><th class="num">DURATION</th></tr>
    </thead>
    <tbody>
      {%- for row in view.rows %}
      <tr>
        <td class="strong"><a href="/executions/{{ row.id }}"><code>{{ row.short_id }}</code></a></td>
        <td><span data-ts="{{ row.triggered_at }}">{{ row.triggered_at }}</span></td>
        <td><span class="{{ row.status_class }}"><span class="dot"></span>{{ row.status_label }}</span></td>
        <td class="num">{%- match row.exit_code -%}{%- when Some with (code) -%}{{ code }}{%- when None -%}&mdash;{%- endmatch -%}</td>
        <td class="num">{%- match row.duration -%}{%- when Some with (duration) -%}{{ duration }}{%- when None -%}&mdash;{%- endmatch -%}</td>
      </tr>
      {%- endfor %}
    </tbody>
  </table>
</div>
<div class="wf-tablefoot">
  <span>{{ view.pagination.summary_label() }}</span>
  <span class="wf-grow"></span>
  {% if view.has_pages() %}
  <div class="wf-pagination">
    {% if view.pagination.has_previous() %}
      <a hx-get="/hooks/{{ view.slug }}/executions?page={{ view.previous_page() }}&status={{ view.active_status }}&from_date={{ view.active_from }}&to_date={{ view.active_to }}" hx-target="#activity-tab-executions" hx-swap="innerHTML">&larr;</a>
    {% else %}
      <button disabled>&larr;</button>
    {% endif %}
    <a class="is-active">{{ view.pagination.page }}</a>
    {% if view.pagination.has_next() %}
      <a hx-get="/hooks/{{ view.slug }}/executions?page={{ view.next_page() }}&status={{ view.active_status }}&from_date={{ view.active_from }}&to_date={{ view.active_to }}" hx-target="#activity-tab-executions" hx-swap="innerHTML">&rarr;</a>
    {% else %}
      <button disabled>&rarr;</button>
    {% endif %}
  </div>
  {% endif %}
</div>
{% endif %}
"##,
    ext = "html"
)]
struct ExecutionListTemplate<'a> {
    view: &'a ExecutionListView,
}

#[derive(Debug, Template)]
#[template(
    source = r##"
<div class="wf-filterbar">
  <div class="wf-seg sm grid-4 wf-w-full" id="attempt-filters">
    <button class="wf-seg-opt{% if view.active_status.is_empty() %} is-active{% endif %}" hx-get="/hooks/{{ view.slug }}/attempts" hx-target="#activity-tab-attempts" hx-swap="innerHTML">ALL</button>
    <button class="wf-seg-opt{% if view.active_status == "fired" %} is-active{% endif %}" hx-get="/hooks/{{ view.slug }}/attempts?status=fired" hx-target="#activity-tab-attempts" hx-swap="innerHTML">FIRED</button>
    <button class="wf-seg-opt{% if view.active_status == "auth_failed" %} is-active{% endif %}" hx-get="/hooks/{{ view.slug }}/attempts?status=auth_failed" hx-target="#activity-tab-attempts" hx-swap="innerHTML">AUTH FAILED</button>
    <button class="wf-seg-opt{% if view.active_status == "validation_failed" %} is-active{% endif %}" hx-get="/hooks/{{ view.slug }}/attempts?status=validation_failed" hx-target="#activity-tab-attempts" hx-swap="innerHTML">VALIDATION FAILED</button>
    <button class="wf-seg-opt{% if view.active_status == "filtered" %} is-active{% endif %}" hx-get="/hooks/{{ view.slug }}/attempts?status=filtered" hx-target="#activity-tab-attempts" hx-swap="innerHTML">FILTERED</button>
    <button class="wf-seg-opt{% if view.active_status == "rate_limited" %} is-active{% endif %}" hx-get="/hooks/{{ view.slug }}/attempts?status=rate_limited" hx-target="#activity-tab-attempts" hx-swap="innerHTML">RATE LIMITED</button>
    <button class="wf-seg-opt{% if view.active_status == "schedule_skipped" %} is-active{% endif %}" hx-get="/hooks/{{ view.slug }}/attempts?status=schedule_skipped" hx-target="#activity-tab-attempts" hx-swap="innerHTML">SCHEDULE SKIPPED</button>
    <button class="wf-seg-opt{% if view.active_status == "cooldown_skipped" %} is-active{% endif %}" hx-get="/hooks/{{ view.slug }}/attempts?status=cooldown_skipped" hx-target="#activity-tab-attempts" hx-swap="innerHTML">COOLDOWN SKIPPED</button>
  </div>
</div>

{% if view.rows.is_empty() %}
<div class="wf-empty">
  <div class="wf-empty-msg">No trigger attempts{% if !view.active_status.is_empty() %} matching "{{ view.active_status }}"{% endif %}.</div>
</div>
{% else %}
<div class="wf-tablescroll wf-grow">
  <table class="wf-table sticky">
    <thead>
      <tr><th>TIME</th><th>STATUS</th><th>SOURCE IP</th><th>REASON</th><th>EXECUTION</th></tr>
    </thead>
    <tbody>
      {%- for row in view.rows %}
      <tr>
        <td><span data-ts="{{ row.attempted_at }}">{{ row.attempted_at }}</span></td>
        <td><span class="{{ row.status_class }}"><span class="dot"></span>{{ row.status_label }}</span></td>
        <td><code>{{ row.source_ip }}</code></td>
        <td>{% if row.has_reason() %}{{ row.reason }}{% else %}&mdash;{% endif %}</td>
        <td>
          {%- match row.execution_id -%}
          {%- when Some with (execution_id) -%}
          <a href="/executions/{{ execution_id }}"><code>{%- match row.execution_short_id -%}{%- when Some with (short_id) -%}{{ short_id }}{%- when None -%}{{ execution_id }}{%- endmatch -%}</code></a>
          {%- when None -%}
          &mdash;
          {%- endmatch -%}
        </td>
      </tr>
      {%- endfor %}
    </tbody>
  </table>
</div>
<div class="wf-tablefoot">
  <span>{{ view.pagination.summary_label() }}</span>
  <span class="wf-grow"></span>
  {% if view.has_pages() %}
  <div class="wf-pagination">
    {% if view.pagination.has_previous() %}
      <a hx-get="/hooks/{{ view.slug }}/attempts?page={{ view.previous_page() }}&status={{ view.active_status }}" hx-target="#activity-tab-attempts" hx-swap="innerHTML">&larr;</a>
    {% else %}
      <button disabled>&larr;</button>
    {% endif %}
    <a class="is-active">{{ view.pagination.page }}</a>
    {% if view.pagination.has_next() %}
      <a hx-get="/hooks/{{ view.slug }}/attempts?page={{ view.next_page() }}&status={{ view.active_status }}" hx-target="#activity-tab-attempts" hx-swap="innerHTML">&rarr;</a>
    {% else %}
      <button disabled>&rarr;</button>
    {% endif %}
  </div>
  {% endif %}
</div>
{% endif %}
"##,
    ext = "html"
)]
struct AttemptListTemplate<'a> {
    view: &'a AttemptListView,
}

#[cfg(test)]
mod tests {
    use axum::response::Html;

    use super::*;

    #[test]
    fn execution_list_renders_rows_filters_and_pagination() {
        let rows = vec![ExecutionListRow {
            id: "abcdef123456".to_owned(),
            short_id: "abcdef12".to_owned(),
            triggered_at: r#"2026-05-17T10:00:00Z" onclick="bad"#.to_owned(),
            status_label: "SUCCESS".to_owned(),
            status_class: "wf-tag ok",
            exit_code: Some(0),
            duration: Some("1.234s".to_owned()),
        }];
        let view = ExecutionListView::new(
            "deploy-hook",
            rows,
            2,
            20,
            45,
            "success",
            "2026-05-01",
            "2026-05-17",
        );

        let Html(html) = render_execution_list(&view).unwrap();

        assert!(html.contains(r#"<option value="success" selected>SUCCESS</option>"#));
        assert!(html.contains(r#"hx-get="/hooks/deploy-hook/executions"#));
        assert!(html.contains(r#"/executions/abcdef123456"#));
        assert!(html.contains(r#"<code>abcdef12</code>"#));
        assert!(html.contains(r#"class="wf-tag ok""#));
        assert!(html.contains("21-21 OF 45"));
        assert!(html.contains(r#"data-ts="2026-05-17T10:00:00Z&#34; onclick=&#34;bad""#));
    }

    #[test]
    fn execution_list_renders_empty_state_with_active_status() {
        let view = ExecutionListView::new("deploy-hook", Vec::new(), 1, 20, 0, "failed", "", "");
        let Html(html) = render_execution_list(&view).unwrap();

        assert!(html.contains(r#"No executions matching "failed"."#));
        assert!(!html.contains("<table"));
    }

    #[test]
    fn attempt_list_renders_rows_filters_and_pagination() {
        let rows = vec![AttemptListRow {
            attempted_at: "2026-05-17T10:00:00Z".to_owned(),
            status_label: "AUTH_FAILED".to_owned(),
            status_class: "wf-tag err",
            source_ip: "127.0.0.1".to_owned(),
            reason: "<bad token>".to_owned(),
            execution_id: Some("exec123456789".to_owned()),
            execution_short_id: Some("exec1234".to_owned()),
        }];
        let view = AttemptListView::new("auth-hook", rows, 1, 20, 21, "auth_failed");

        let Html(html) = render_attempt_list(&view).unwrap();

        assert!(html.contains(r#"id="attempt-filters""#));
        assert!(html.contains(r#"status=auth_failed""#));
        assert!(html.contains(r#"class="wf-tag err""#));
        assert!(html.contains("AUTH_FAILED"));
        assert!(html.contains("&#60;bad token&#62;"));
        assert!(html.contains(r#"/executions/exec123456789"#));
        assert!(html.contains("1-1 OF 21"));
        assert!(html.contains(r#"hx-get="/hooks/auth-hook/attempts?page=2"#));
        assert!(html.contains("status=auth_failed"));
    }

    #[test]
    fn attempt_list_renders_empty_state_with_active_status() {
        let view = AttemptListView::new("auth-hook", Vec::new(), 1, 20, 0, "filtered");
        let Html(html) = render_attempt_list(&view).unwrap();

        assert!(html.contains(r#"No trigger attempts matching "filtered"."#));
        assert!(!html.contains("<table"));
    }
}
