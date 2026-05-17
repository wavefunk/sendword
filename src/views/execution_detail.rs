use askama::Template;
use axum::response::Html;
use wavefunk_ui::components::{BreadcrumbItem, PageHeader, TrustedHtml};

use crate::error::AppError;
use crate::models::ExecutionStatus;

use super::{
    NavActive, PageShell, SENDWORD_APP_SCRIPT_TAG, StatusKind, render_breadcrumbs, render_shell,
    render_status_tag, render_template,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionDetailPage {
    pub id: String,
    pub short_id: String,
    pub hook_slug: String,
    pub status_label: String,
    pub status_kind: StatusKind,
    pub is_running: bool,
    pub exit_code: Option<i32>,
    pub triggered_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub duration: Option<String>,
    pub trigger_source: String,
    pub retry_count: i32,
    pub retry_of: Option<String>,
    pub retry_short_id: Option<String>,
    pub stdout: String,
    pub stderr: String,
}

impl ExecutionDetailPage {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        hook_slug: impl Into<String>,
        status: &ExecutionStatus,
        exit_code: Option<i32>,
        triggered_at: impl Into<String>,
        started_at: Option<String>,
        completed_at: Option<String>,
        duration: Option<String>,
        trigger_source: impl Into<String>,
        retry_count: i32,
        retry_of: Option<String>,
        stdout: impl Into<String>,
        stderr: impl Into<String>,
    ) -> Self {
        let id = id.into();
        let retry_short_id = retry_of.as_ref().map(|value| short_id(value));
        Self {
            short_id: short_id(&id),
            id,
            hook_slug: hook_slug.into(),
            status_label: status.to_string().to_uppercase(),
            status_kind: status_kind(status),
            is_running: matches!(status, ExecutionStatus::Running),
            exit_code,
            triggered_at: triggered_at.into(),
            started_at,
            completed_at,
            duration,
            trigger_source: trigger_source.into(),
            retry_count,
            retry_of,
            retry_short_id,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    pub fn has_stderr(&self) -> bool {
        !self.stderr.is_empty()
    }

    pub fn has_retry(&self) -> bool {
        self.retry_count > 0
    }
}

pub fn render_execution_detail_page(
    username: &str,
    page: &ExecutionDetailPage,
) -> Result<Html<String>, AppError> {
    let breadcrumbs = render_execution_breadcrumbs(page)?;
    let actions = render_template(&ExecutionActionsTemplate { page })?;
    let content = render_execution_content(page)?;

    let shell = PageShell::new(
        "sendword - execution",
        username,
        NavActive::Hooks,
        TrustedHtml::new(&breadcrumbs),
        TrustedHtml::new(&content),
    )
    .with_actions(TrustedHtml::new(&actions))
    .with_flash(None, None)
    .with_scripts(TrustedHtml::new(SENDWORD_APP_SCRIPT_TAG));

    let shell = if page.is_running {
        shell.with_htmx_sse()
    } else {
        shell
    };

    render_shell(&shell)
}

fn render_execution_breadcrumbs(page: &ExecutionDetailPage) -> Result<String, AppError> {
    let hook_label = page.hook_slug.to_uppercase();
    let hook_href = format!("/hooks/{}", page.hook_slug);
    let exec_label = page.short_id.to_uppercase();
    render_breadcrumbs(&[
        BreadcrumbItem::link("HOOKS", "/"),
        BreadcrumbItem::link(&hook_label, &hook_href),
        BreadcrumbItem::current(&exec_label),
    ])
}

fn render_execution_content(page: &ExecutionDetailPage) -> Result<String, AppError> {
    let status_tag = render_status_tag(&page.status_label, page.status_kind)?;
    let title = format!("Execution {}", page.short_id);
    let header = render_template(
        &PageHeader::new(&title)
            .with_meta(TrustedHtml::new(&status_tag))
            .with_subtitle(&page.id),
    )?;

    render_template(&ExecutionDetailContentTemplate {
        header_html: TrustedHtml::new(&header),
        page,
    })
}

fn status_kind(status: &ExecutionStatus) -> StatusKind {
    match status {
        ExecutionStatus::Success => StatusKind::Ok,
        ExecutionStatus::Failed
        | ExecutionStatus::TimedOut
        | ExecutionStatus::Rejected
        | ExecutionStatus::Expired => StatusKind::Error,
        ExecutionStatus::Running => StatusKind::Warn,
        ExecutionStatus::Pending | ExecutionStatus::PendingApproval => StatusKind::Info,
        ExecutionStatus::Approved => StatusKind::Neutral,
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

#[derive(Debug, Template)]
#[template(
    source = r##"
<form method="post" class="wf-ib" hx-post="/executions/{{ page.id }}/replay" hx-swap="none" data-sendword-replay>
  <button type="submit" class="wf-btn sm">REPLAY</button>
</form>
"##,
    ext = "html"
)]
struct ExecutionActionsTemplate<'a> {
    page: &'a ExecutionDetailPage,
}

#[derive(Debug, Template)]
#[template(
    source = r##"
{{ header_html }}

<section class="wf-panel wf-mb-4">
  <div class="wf-panel-head"><span class="wf-panel-title">DETAILS</span></div>
  <dl class="wf-dl flush">
    <div class="wf-dl-row"><dt>ID</dt><dd><code>{{ page.id }}</code></dd></div>
    <div class="wf-dl-row"><dt>HOOK</dt><dd><a href="/hooks/{{ page.hook_slug }}">{{ page.hook_slug }}</a></dd></div>
    {%- match page.exit_code -%}
    {%- when Some with (exit_code) -%}
    <div class="wf-dl-row"><dt>EXIT CODE</dt><dd>{{ exit_code }}</dd></div>
    {%- when None -%}
    {%- endmatch -%}
    <div class="wf-dl-row"><dt>SOURCE</dt><dd>{{ page.trigger_source }}</dd></div>
    <div class="wf-dl-row"><dt>TRIGGERED</dt><dd><span data-ts="{{ page.triggered_at }}">{{ page.triggered_at }}</span></dd></div>
    {%- match page.started_at -%}
    {%- when Some with (started_at) -%}
    <div class="wf-dl-row"><dt>STARTED</dt><dd><span data-ts="{{ started_at }}">{{ started_at }}</span></dd></div>
    {%- when None -%}
    {%- endmatch -%}
    {%- match page.completed_at -%}
    {%- when Some with (completed_at) -%}
    <div class="wf-dl-row"><dt>COMPLETED</dt><dd><span data-ts="{{ completed_at }}">{{ completed_at }}</span></dd></div>
    {%- when None -%}
    {%- endmatch -%}
    {%- match page.duration -%}
    {%- when Some with (duration) -%}
    <div class="wf-dl-row"><dt>DURATION</dt><dd>{{ duration }}</dd></div>
    {%- when None -%}
    {%- endmatch -%}
    {%- if page.has_retry() %}
    <div class="wf-dl-row">
      <dt>RETRY</dt>
      <dd>#{{ page.retry_count }}{%- match page.retry_of -%}{%- when Some with (retry_of) %} of <a href="/executions/{{ retry_of }}">{%- match page.retry_short_id -%}{%- when Some with (short_id) -%}{{ short_id }}{%- when None -%}{{ retry_of }}{%- endmatch -%}</a>{%- when None -%}{%- endmatch -%}</dd>
    </div>
    {%- endif %}
  </dl>
</section>

{%- if page.is_running %}
<div hx-ext="sse" sse-connect="/executions/{{ page.id }}/logs/stream">
  <section class="wf-panel wf-mb-4">
    <div class="wf-panel-head"><span class="wf-panel-title">STDOUT</span></div>
    <div class="wf-panel-body">
      <pre id="log-stdout" sse-swap="stdout" hx-swap="beforeend" style="min-height: 80px; max-height: 600px; overflow: auto; margin: 0;"></pre>
    </div>
  </section>
  <section class="wf-panel wf-mb-4">
    <div class="wf-panel-head"><span class="wf-panel-title">STDERR</span></div>
    <div class="wf-panel-body">
      <pre id="log-stderr" sse-swap="stderr" hx-swap="beforeend" style="min-height: 40px; max-height: 400px; overflow: auto; margin: 0;"></pre>
    </div>
  </section>
  <div sse-swap="done" hx-swap="innerHTML" style="display: none;" data-sendword-reload-after-settle="500"></div>
</div>
{%- else %}
<section class="wf-panel wf-mb-4">
  <div class="wf-panel-head"><span class="wf-panel-title">STDOUT</span></div>
  <div class="wf-panel-body">
    <pre style="min-height: 80px; max-height: 600px; overflow: auto; margin: 0;">{{ page.stdout }}</pre>
  </div>
</section>
{%- if page.has_stderr() %}
<section class="wf-panel wf-mb-4">
  <div class="wf-panel-head"><span class="wf-panel-title">STDERR</span></div>
  <div class="wf-panel-body">
    <pre style="min-height: 40px; max-height: 400px; overflow: auto; margin: 0;">{{ page.stderr }}</pre>
  </div>
</section>
{%- endif %}
{%- endif %}
"##,
    ext = "html"
)]
struct ExecutionDetailContentTemplate<'a> {
    header_html: TrustedHtml<'a>,
    page: &'a ExecutionDetailPage,
}

#[cfg(test)]
mod tests {
    use axum::response::Html;

    use super::*;

    #[test]
    fn execution_detail_page_renders_metadata_logs_and_escapes_output() {
        let page = ExecutionDetailPage::new(
            "abcdef123456",
            "deploy-hook",
            &ExecutionStatus::Failed,
            Some(1),
            r#"2026-05-17T10:00:00Z" onclick="bad"#,
            Some("2026-05-17T10:00:01Z".to_owned()),
            Some("2026-05-17T10:00:02Z".to_owned()),
            Some("1.000s".to_owned()),
            "127.0.0.1",
            2,
            Some("retryabcdef123456".to_owned()),
            "<stdout>",
            "<stderr>",
        );

        let Html(html) = render_execution_detail_page("admin@example.com", &page).unwrap();

        assert!(html.contains("Execution abcdef12"));
        assert!(html.contains(r#"href="/hooks/deploy-hook""#));
        assert!(html.contains(r#"hx-post="/executions/abcdef123456/replay""#));
        assert!(html.contains("FAILED"));
        assert!(html.contains("EXIT CODE"));
        assert!(html.contains("RETRY"));
        assert!(html.contains(r#"href="/executions/retryabcdef123456""#));
        assert!(html.contains("&#60;stdout&#62;"));
        assert!(html.contains("&#60;stderr&#62;"));
        assert!(!html.contains("<stdout>"));
        assert!(html.contains(r#"data-ts="2026-05-17T10:00:00Z&#34; onclick=&#34;bad""#));
        assert!(html.contains(r#"/static/js/sendword.js"#));
    }

    #[test]
    fn running_execution_detail_uses_sse_log_panels() {
        let page = ExecutionDetailPage::new(
            "abcdef123456",
            "deploy-hook",
            &ExecutionStatus::Running,
            None,
            "2026-05-17T10:00:00Z",
            Some("2026-05-17T10:00:01Z".to_owned()),
            None,
            None,
            "127.0.0.1",
            0,
            None,
            "ignored while running",
            "",
        );

        let Html(html) = render_execution_detail_page("admin@example.com", &page).unwrap();

        assert!(html.contains(r#"hx-ext="sse""#));
        assert!(html.contains(r#"sse-connect="/executions/abcdef123456/logs/stream""#));
        assert!(html.contains(r#"data-sendword-reload-after-settle="500""#));
        assert!(html.contains(r#"/static/wavefunk/js/htmx-sse.js"#));
        assert!(!html.contains("ignored while running"));
    }
}
