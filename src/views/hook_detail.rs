use askama::Template;
use axum::response::Html;
use wavefunk_ui::components::{
    BreadcrumbItem, Button, ButtonSize, ButtonVariant, PageHeader, TrustedHtml,
};

use crate::error::AppError;

use super::{
    ActionKind, FlashMessages, NavActive, PageShell, StatusKind, render_action_link,
    render_breadcrumbs, render_shell, render_status_tag, render_template,
};

const ACTIVITY_TAB_SCRIPT: &str = r#"<script>
function switchActivityTab(tab, el) {
  var tabs = document.querySelectorAll('#activity-tabs button');
  for (var i = 0; i < tabs.length; i++) tabs[i].classList.remove('is-active');
  el.classList.add('is-active');

  var exec = document.getElementById('activity-tab-executions');
  var att = document.getElementById('activity-tab-attempts');
  if (tab === 'executions') {
    exec.classList.remove('wf-hidden');
    exec.classList.add('wf-f');
    att.classList.add('wf-hidden');
    att.classList.remove('wf-f');
  } else {
    exec.classList.add('wf-hidden');
    exec.classList.remove('wf-f');
    att.classList.remove('wf-hidden');
    att.classList.add('wf-f');
  }
}
</script>"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthModeView {
    None,
    Bearer,
    Hmac,
}

impl AuthModeView {
    pub const fn value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Bearer => "bearer",
            Self::Hmac => "hmac",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Bearer => "BEARER",
            Self::Hmac => "HMAC",
        }
    }

    pub const fn tag_class(self) -> &'static str {
        match self {
            Self::Bearer => "wf-tag info",
            Self::Hmac => "wf-tag accent",
            Self::None => "wf-tag",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayloadFieldRow {
    pub name: String,
    pub field_type: String,
    pub required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TriggerFilterRow {
    pub field: String,
    pub operator: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TriggerWindowRow {
    pub days: String,
    pub start_time: String,
    pub end_time: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TriggerRateRow {
    pub max_requests: String,
    pub window: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookDetailPage {
    pub name: String,
    pub slug: String,
    pub slug_label: String,
    pub description: String,
    pub enabled: bool,
    pub executor_type: String,
    pub executor_value_label: String,
    pub executor_command: String,
    pub script_edit_url: Option<String>,
    pub cwd: Option<String>,
    pub timeout_secs: u64,
    pub env_vars: Vec<String>,
    pub auth_mode: AuthModeView,
    pub auth_header: Option<String>,
    pub auth_algorithm: Option<String>,
    pub payload_fields: Vec<PayloadFieldRow>,
    pub trigger_filter_rows: Vec<TriggerFilterRow>,
    pub trigger_window_rows: Vec<TriggerWindowRow>,
    pub trigger_cooldown: Option<String>,
    pub trigger_rate: Option<TriggerRateRow>,
    pub hook_rate_limit_max_per_minute: Option<u32>,
}

impl HookDetailPage {
    pub fn new(name: impl Into<String>, slug: impl Into<String>) -> Self {
        let slug = slug.into();
        Self {
            name: name.into(),
            slug_label: slug.to_uppercase(),
            slug,
            description: String::new(),
            enabled: true,
            executor_type: String::new(),
            executor_value_label: String::new(),
            executor_command: String::new(),
            script_edit_url: None,
            cwd: None,
            timeout_secs: 0,
            env_vars: Vec::new(),
            auth_mode: AuthModeView::None,
            auth_header: None,
            auth_algorithm: None,
            payload_fields: Vec::new(),
            trigger_filter_rows: Vec::new(),
            trigger_window_rows: Vec::new(),
            trigger_cooldown: None,
            trigger_rate: None,
            hook_rate_limit_max_per_minute: None,
        }
    }

    pub fn has_description(&self) -> bool {
        !self.description.is_empty()
    }

    pub fn has_auth(&self) -> bool {
        self.auth_mode != AuthModeView::None
    }

    pub fn has_trigger_rules(&self) -> bool {
        !self.trigger_filter_rows.is_empty()
            || !self.trigger_window_rows.is_empty()
            || self.trigger_cooldown.is_some()
            || self.trigger_rate.is_some()
    }
}

pub fn render_hook_detail_page(
    username: &str,
    hook: &HookDetailPage,
    flash: FlashMessages<'_>,
) -> Result<Html<String>, AppError> {
    let breadcrumbs = render_breadcrumbs(&[
        BreadcrumbItem::link("HOOKS", "/"),
        BreadcrumbItem::current(&hook.slug_label),
    ])?;
    let actions = render_hook_actions(hook)?;
    let content = render_hook_detail_content(hook)?;

    render_shell(
        &PageShell::new(
            "sendword - hook",
            username,
            NavActive::Hooks,
            TrustedHtml::new(&breadcrumbs),
            TrustedHtml::new(&content),
        )
        .with_actions(TrustedHtml::new(&actions))
        .with_flash(flash.success, flash.error),
    )
}

fn render_hook_actions(hook: &HookDetailPage) -> Result<String, AppError> {
    let edit_href = format!("/hooks/{}/edit", hook.slug);
    let edit = render_action_link("EDIT", &edit_href, ActionKind::Default)?;
    let delete = render_template(
        &Button::new("DELETE")
            .with_variant(ButtonVariant::Danger)
            .with_size(ButtonSize::Small)
            .with_button_type("submit"),
    )?;

    render_template(&HookActionsTemplate {
        edit_html: TrustedHtml::new(&edit),
        delete_button_html: TrustedHtml::new(&delete),
        hook,
    })
}

fn render_hook_detail_content(hook: &HookDetailPage) -> Result<String, AppError> {
    let status = render_status_tag(
        if hook.enabled { "ENABLED" } else { "DISABLED" },
        if hook.enabled {
            StatusKind::Ok
        } else {
            StatusKind::Neutral
        },
    )?;
    let mut page_header = PageHeader::new(&hook.name).with_meta(TrustedHtml::new(status.as_str()));
    if hook.has_description() {
        page_header = page_header.with_subtitle(&hook.description);
    }
    let header = render_template(&page_header)?;
    let overview = render_template(&HookOverviewTemplate { hook })?;
    let activity = render_template(&HookActivityTemplate { hook })?;

    render_template(&HookDetailContentTemplate {
        header_html: TrustedHtml::new(&header),
        overview_html: TrustedHtml::new(&overview),
        activity_html: TrustedHtml::new(&activity),
        script_html: TrustedHtml::new(ACTIVITY_TAB_SCRIPT),
    })
}

#[derive(Debug, Template)]
#[template(
    source = r#"
{{ edit_html }}
<form method="post" action="/hooks/{{ hook.slug }}/delete" data-confirm="Delete hook {{ hook.name }}?" onsubmit="return confirm(this.dataset.confirm)" class="wf-ib">
  {{ delete_button_html }}
</form>
"#,
    ext = "html"
)]
struct HookActionsTemplate<'a> {
    edit_html: TrustedHtml<'a>,
    delete_button_html: TrustedHtml<'a>,
    hook: &'a HookDetailPage,
}

#[derive(Debug, Template)]
#[template(
    source = r#"
{{ header_html }}
<div class="wf-split wf-grow">
  {{ overview_html }}
  {{ activity_html }}
</div>
{{ script_html }}
"#,
    ext = "html"
)]
struct HookDetailContentTemplate<'a> {
    header_html: TrustedHtml<'a>,
    overview_html: TrustedHtml<'a>,
    activity_html: TrustedHtml<'a>,
    script_html: TrustedHtml<'a>,
}

#[derive(Debug, Template)]
#[template(
    source = r#"
<section class="wf-panel wf-p-0 wf-grow">
  <div class="wf-panel-head"><span class="wf-panel-title">OVERVIEW</span></div>
  <dl class="wf-dl flush">
    <div class="wf-dl-row"><dt>Slug</dt><dd><code>{{ hook.slug }}</code></dd></div>
    {%- if hook.has_description() %}
    <div class="wf-dl-row"><dt>Description</dt><dd>{{ hook.description }}</dd></div>
    {%- endif %}
    <div class="wf-dl-row"><dt>Endpoint</dt><dd><code>/hook/{{ hook.slug }}</code></dd></div>
    <div class="wf-dl-row">
      <dt>Status</dt>
      <dd>
        {%- if hook.enabled -%}
        <span class="wf-tag ok"><span class="dot"></span>ENABLED</span>
        {%- else -%}
        <span class="wf-tag"><span class="dot"></span>DISABLED</span>
        {%- endif -%}
      </dd>
    </div>
  </dl>

  <div class="wf-panel-head wf-dim"><span class="wf-panel-title">EXECUTOR</span></div>
  <dl class="wf-dl flush">
    <div class="wf-dl-row"><dt>Type</dt><dd>{{ hook.executor_type }}</dd></div>
    <div class="wf-dl-row">
      <dt>{{ hook.executor_value_label }}</dt>
      <dd class="wf-f wf-ai-c wf-gap-2">
        <code>{{ hook.executor_command }}</code>
        {%- match hook.script_edit_url -%}
        {%- when Some with (url) -%}
        <a class="wf-btn sm" href="{{ url }}">EDIT SCRIPT</a>
        {%- when None -%}
        {%- endmatch -%}
      </dd>
    </div>
    {%- match hook.cwd -%}
    {%- when Some with (cwd) -%}
    <div class="wf-dl-row"><dt>Working dir</dt><dd><code>{{ cwd }}</code></dd></div>
    {%- when None -%}
    {%- endmatch -%}
    <div class="wf-dl-row"><dt>Timeout</dt><dd>{{ hook.timeout_secs }}s</dd></div>
    {%- if !hook.env_vars.is_empty() %}
    <div class="wf-dl-row">
      <dt>Environment</dt>
      <dd class="wf-f wf-wrap wf-gap-1">
        {%- for var in hook.env_vars %}
        <span class="wf-tag">{{ var }}</span>
        {%- endfor %}
      </dd>
    </div>
    {%- endif %}
  </dl>

  {%- if hook.has_auth() %}
  <div class="wf-panel-head wf-dim"><span class="wf-panel-title">AUTHENTICATION</span></div>
  <dl class="wf-dl flush">
    <div class="wf-dl-row">
      <dt>Mode</dt>
      <dd><span class="{{ hook.auth_mode.tag_class() }}" data-auth-mode="{{ hook.auth_mode.value() }}">{{ hook.auth_mode.label() }}</span></dd>
    </div>
    {%- match hook.auth_header -%}
    {%- when Some with (header) -%}
    <div class="wf-dl-row"><dt>Header</dt><dd><code>{{ header }}</code></dd></div>
    {%- when None -%}
    {%- endmatch -%}
    {%- match hook.auth_algorithm -%}
    {%- when Some with (algorithm) -%}
    <div class="wf-dl-row"><dt>Algorithm</dt><dd>{{ algorithm }}</dd></div>
    {%- when None -%}
    {%- endmatch -%}
  </dl>
  {%- endif %}

  {%- if !hook.payload_fields.is_empty() %}
  <div class="wf-panel-head wf-dim"><span class="wf-panel-title">PAYLOAD SCHEMA</span></div>
  <table class="wf-table">
    <thead><tr><th>NAME</th><th>TYPE</th><th>REQUIRED</th></tr></thead>
    <tbody>
      {%- for field in hook.payload_fields %}
      <tr>
        <td class="strong"><code>{{ field.name }}</code></td>
        <td><code>{{ field.field_type }}</code></td>
        <td>
          {%- if field.required -%}
          <span class="wf-tag warn">REQUIRED</span>
          {%- else -%}
          <span class="muted">optional</span>
          {%- endif -%}
        </td>
      </tr>
      {%- endfor %}
    </tbody>
  </table>
  {%- endif %}

  {%- if hook.has_trigger_rules() %}
  <div class="wf-panel-head wf-dim"><span class="wf-panel-title">TRIGGER RULES</span></div>
  {%- if hook.trigger_cooldown.is_some() || hook.trigger_rate.is_some() %}
  <dl class="wf-dl flush">
    {%- match hook.trigger_cooldown -%}
    {%- when Some with (cooldown) -%}
    <div class="wf-dl-row"><dt>Cooldown</dt><dd>{{ cooldown }}</dd></div>
    {%- when None -%}
    {%- endmatch -%}
    {%- match hook.trigger_rate -%}
    {%- when Some with (rate) -%}
    <div class="wf-dl-row"><dt>Rate limit</dt><dd>{{ rate.max_requests }} per {{ rate.window }}</dd></div>
    {%- when None -%}
    {%- endmatch -%}
  </dl>
  {%- endif %}
  {%- if !hook.trigger_filter_rows.is_empty() %}
  <table class="wf-table">
    <thead><tr><th>FIELD</th><th>OPERATOR</th><th>VALUE</th></tr></thead>
    <tbody>
      {%- for row in hook.trigger_filter_rows %}
      <tr>
        <td class="strong"><code>{{ row.field }}</code></td>
        <td><span class="wf-tag info">{{ row.operator }}</span></td>
        <td>
          {%- match row.value -%}
          {%- when Some with (value) -%}
          {{ value }}
          {%- when None -%}
          <span class="muted">&mdash;</span>
          {%- endmatch -%}
        </td>
      </tr>
      {%- endfor %}
    </tbody>
  </table>
  {%- endif %}
  {%- if !hook.trigger_window_rows.is_empty() %}
  <table class="wf-table">
    <thead><tr><th>DAYS</th><th>START</th><th>END</th></tr></thead>
    <tbody>
      {%- for row in hook.trigger_window_rows %}
      <tr>
        <td>{{ row.days }}</td>
        <td><code>{{ row.start_time }}</code></td>
        <td><code>{{ row.end_time }}</code></td>
      </tr>
      {%- endfor %}
    </tbody>
  </table>
  {%- endif %}
  {%- endif %}

  {%- match hook.hook_rate_limit_max_per_minute -%}
  {%- when Some with (max_per_minute) -%}
  <div class="wf-panel-head wf-dim"><span class="wf-panel-title">RATE LIMIT</span></div>
  <dl class="wf-dl flush">
    <div class="wf-dl-row"><dt>Max per minute</dt><dd>{{ max_per_minute }}</dd></div>
  </dl>
  {%- when None -%}
  {%- endmatch -%}
</section>
"#,
    ext = "html"
)]
struct HookOverviewTemplate<'a> {
    hook: &'a HookDetailPage,
}

#[derive(Debug, Template)]
#[template(
    source = r#"
<section class="wf-panel wf-p-0 wf-grow">
  <div class="wf-tabs" id="activity-tabs">
    <button class="is-active" onclick="switchActivityTab('executions', this)">Executions</button>
    <button onclick="switchActivityTab('attempts', this)">Trigger Attempts</button>
  </div>

  <div id="activity-tab-executions" class="wf-f wf-col wf-grow wf-min-w-0" hx-get="/hooks/{{ hook.slug }}/executions?page=1" hx-trigger="load" hx-swap="innerHTML"></div>
  <div id="activity-tab-attempts" class="wf-col wf-grow wf-min-w-0 wf-hidden" hx-get="/hooks/{{ hook.slug }}/attempts" hx-trigger="revealed" hx-swap="innerHTML"></div>
</section>
"#,
    ext = "html"
)]
struct HookActivityTemplate<'a> {
    hook: &'a HookDetailPage,
}

#[cfg(test)]
mod tests {
    use axum::response::Html;

    use super::*;

    #[test]
    fn hook_detail_page_renders_config_and_escapes_actions() {
        let mut hook = HookDetailPage::new("Deploy 'App' <script>", "deploy-app");
        hook.description = "<b>Deploys app</b>".to_owned();
        hook.executor_type = "Shell".to_owned();
        hook.executor_value_label = "Command".to_owned();
        hook.executor_command = "make deploy".to_owned();
        hook.cwd = Some("/tmp/sendword".to_owned());
        hook.timeout_secs = 30;
        hook.env_vars = vec!["APP_ENV".to_owned()];
        hook.auth_mode = AuthModeView::Hmac;
        hook.auth_header = Some("X-Hub-Signature-256".to_owned());
        hook.auth_algorithm = Some("sha256".to_owned());
        hook.payload_fields = vec![PayloadFieldRow {
            name: "action".to_owned(),
            field_type: "string".to_owned(),
            required: true,
        }];
        hook.trigger_filter_rows = vec![TriggerFilterRow {
            field: "action".to_owned(),
            operator: "equals".to_owned(),
            value: Some("deploy".to_owned()),
        }];
        hook.trigger_window_rows = vec![TriggerWindowRow {
            days: "Mon, Fri".to_owned(),
            start_time: "09:00".to_owned(),
            end_time: "17:00".to_owned(),
        }];
        hook.trigger_cooldown = Some("5m".to_owned());
        hook.trigger_rate = Some(TriggerRateRow {
            max_requests: "10".to_owned(),
            window: "1h".to_owned(),
        });
        hook.hook_rate_limit_max_per_minute = Some(60);

        let Html(html) = render_hook_detail_page(
            "admin@example.com",
            &hook,
            FlashMessages {
                success: Some("Updated <hook>"),
                error: None,
            },
        )
        .unwrap();

        assert!(html.contains("Deploy &#39;App&#39; &#60;script&#62;"));
        assert!(!html.contains("Deploy 'App' <script>"));
        assert!(
            html.contains(r#"data-confirm="Delete hook Deploy &#39;App&#39; &#60;script&#62;?""#)
        );
        assert!(html.contains(r#"onsubmit="return confirm(this.dataset.confirm)""#));
        assert!(html.contains(r#"/hooks/deploy-app/edit"#));
        assert!(html.contains(r#"/hooks/deploy-app/delete"#));
        assert!(html.contains(r#"data-auth-mode="hmac""#));
        assert!(html.contains("X-Hub-Signature-256"));
        assert!(html.contains("PAYLOAD SCHEMA"));
        assert!(html.contains("TRIGGER RULES"));
        assert!(html.contains(r#"hx-get="/hooks/deploy-app/executions?page=1""#));
        assert!(html.contains(r#"hx-get="/hooks/deploy-app/attempts""#));
        assert!(html.contains("function switchActivityTab(tab, el)"));
        assert!(html.contains("Updated &#60;hook&#62;"));
    }

    #[test]
    fn hook_detail_page_omits_optional_sections_for_public_hook() {
        let mut hook = HookDetailPage::new("Plain Hook", "plain-hook");
        hook.enabled = false;
        hook.executor_type = "Shell".to_owned();
        hook.executor_value_label = "Command".to_owned();
        hook.executor_command = "echo ok".to_owned();
        hook.timeout_secs = 30;

        let Html(html) = render_hook_detail_page(
            "admin@example.com",
            &hook,
            FlashMessages {
                success: None,
                error: None,
            },
        )
        .unwrap();

        assert!(html.contains("Plain Hook"));
        assert!(html.contains("DISABLED"));
        assert!(!html.contains("AUTHENTICATION"));
        assert!(!html.contains("PAYLOAD SCHEMA"));
        assert!(!html.contains("TRIGGER RULES"));
        assert!(!html.contains("RATE LIMIT"));
    }
}
