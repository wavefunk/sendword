use askama::Template;
use axum::response::Html;
use wavefunk_ui::components::{BreadcrumbItem, PageHeader, TrustedHtml};

use crate::error::AppError;

use super::{
    FlashMessages, NavActive, PageShell, SENDWORD_APP_SCRIPT_TAG, render_breadcrumbs, render_shell,
    render_template,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookFormPage {
    pub is_new: bool,
    pub slug: String,
    pub page_title: String,
    pub form_action: String,
    pub cancel_href: String,
    pub submit_label: &'static str,
    pub form_name: String,
    pub form_slug: String,
    pub form_description: String,
    pub form_enabled: bool,
    pub form_executor_type: String,
    pub form_command: String,
    pub form_cwd: String,
    pub form_timeout: String,
    pub form_env_text: String,
    pub form_retry_count: u32,
    pub form_retry_backoff: String,
    pub form_retry_initial_delay: String,
    pub form_retry_max_delay: String,
    pub form_auth_mode: String,
    pub form_auth_token: String,
    pub form_auth_header: String,
    pub form_auth_algorithm: String,
    pub form_auth_secret: String,
    pub form_payload_text: String,
    pub form_trigger_filters_text: String,
    pub form_trigger_windows_text: String,
    pub form_trigger_cooldown: String,
    pub form_trigger_rate_max: String,
    pub form_trigger_rate_window: String,
}

impl HookFormPage {
    pub fn new_hook() -> Self {
        Self {
            is_new: true,
            slug: String::new(),
            page_title: "New hook".to_owned(),
            form_action: "/hooks/new".to_owned(),
            cancel_href: "/".to_owned(),
            submit_label: "CREATE HOOK",
            form_name: String::new(),
            form_slug: String::new(),
            form_description: String::new(),
            form_enabled: true,
            form_executor_type: "shell".to_owned(),
            form_command: String::new(),
            form_cwd: String::new(),
            form_timeout: String::new(),
            form_env_text: String::new(),
            form_retry_count: 0,
            form_retry_backoff: "exponential".to_owned(),
            form_retry_initial_delay: String::new(),
            form_retry_max_delay: String::new(),
            form_auth_mode: "none".to_owned(),
            form_auth_token: String::new(),
            form_auth_header: "X-Hub-Signature-256".to_owned(),
            form_auth_algorithm: "sha256".to_owned(),
            form_auth_secret: String::new(),
            form_payload_text: String::new(),
            form_trigger_filters_text: String::new(),
            form_trigger_windows_text: String::new(),
            form_trigger_cooldown: String::new(),
            form_trigger_rate_max: String::new(),
            form_trigger_rate_window: String::new(),
        }
    }

    pub fn edit(slug: impl Into<String>, name: impl Into<String>) -> Self {
        let slug = slug.into();
        let name = name.into();
        Self {
            is_new: false,
            page_title: format!("Edit {name}"),
            form_action: format!("/hooks/{slug}/edit"),
            cancel_href: format!("/hooks/{slug}"),
            submit_label: "SAVE CHANGES",
            form_name: name,
            form_slug: slug.clone(),
            slug,
            ..Self::new_hook()
        }
    }

    pub fn option_selected(&self, current: &str, expected: &str) -> bool {
        current == expected
    }

    pub fn slug_hint(&self) -> &'static str {
        if self.is_new {
            "Lowercase letters, numbers, and hyphens. Used in the webhook URL."
        } else {
            "Slug cannot be changed after creation."
        }
    }
}

pub fn render_hook_form_page(
    username: &str,
    page: &HookFormPage,
    flash: FlashMessages<'_>,
) -> Result<Html<String>, AppError> {
    let breadcrumbs = render_form_breadcrumbs(page)?;
    let content = render_form_content(page)?;

    render_shell(
        &PageShell::new(
            "sendword - hook form",
            username,
            NavActive::Hooks,
            TrustedHtml::new(&breadcrumbs),
            TrustedHtml::new(&content),
        )
        .with_flash(flash.success, flash.error)
        .with_scripts(TrustedHtml::new(SENDWORD_APP_SCRIPT_TAG)),
    )
}

fn render_form_breadcrumbs(page: &HookFormPage) -> Result<String, AppError> {
    if page.is_new {
        render_breadcrumbs(&[
            BreadcrumbItem::link("HOOKS", "/"),
            BreadcrumbItem::current("NEW"),
        ])
    } else {
        let slug_label = page.slug.to_uppercase();
        let hook_href = format!("/hooks/{}", page.slug);
        render_breadcrumbs(&[
            BreadcrumbItem::link("HOOKS", "/"),
            BreadcrumbItem::link(&slug_label, &hook_href),
            BreadcrumbItem::current("EDIT"),
        ])
    }
}

fn render_form_content(page: &HookFormPage) -> Result<String, AppError> {
    let header = render_template(&PageHeader::new(&page.page_title).with_subtitle(
        if page.is_new {
            "Create a webhook endpoint and command runner."
        } else {
            "Update this hook without changing its slug."
        },
    ))?;
    let form = render_template(&HookFormTemplate { page })?;

    render_template(&HookFormContentTemplate {
        header_html: TrustedHtml::new(&header),
        form_html: TrustedHtml::new(&form),
    })
}

#[derive(Debug, Template)]
#[template(
    source = r##"
{{ header_html }}
{{ form_html }}
"##,
    ext = "html"
)]
struct HookFormContentTemplate<'a> {
    header_html: TrustedHtml<'a>,
    form_html: TrustedHtml<'a>,
}

#[derive(Debug, Template)]
#[template(
    source = r##"
<form method="post" action="{{ page.form_action }}">
  <section class="wf-panel wf-mb-4">
    <div class="wf-panel-head"><span class="wf-panel-title">BASIC INFO</span></div>
    <div class="wf-panel-body">
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="name">Name</label>
        <input type="text" id="name" name="name" value="{{ page.form_name }}" required placeholder="Deploy App" class="wf-input">
      </div>
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="slug">Slug</label>
        <input type="text" id="slug" name="slug" value="{{ page.form_slug }}"{% if !page.is_new %} readonly{% endif %} required pattern="[a-z0-9]+(-[a-z0-9]+)*" maxlength="64" placeholder="deploy-app" class="wf-input">
        <span class="wf-field-hint">{{ page.slug_hint() }}</span>
      </div>
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="description">Description</label>
        <textarea id="description" name="description" rows="2" placeholder="Optional description" class="wf-textarea">{{ page.form_description }}</textarea>
      </div>
      <label class="wf-check-row">
        <input type="checkbox" name="enabled" value="true"{% if page.form_enabled %} checked{% endif %} class="wf-switch">
        <span>Enabled</span>
      </label>
    </div>
  </section>

  <section class="wf-panel wf-mb-4">
    <div class="wf-panel-head"><span class="wf-panel-title">AUTHENTICATION</span></div>
    <div class="wf-panel-body">
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="auth_mode">Auth mode</label>
        <select id="auth_mode" name="auth_mode" class="wf-select">
          <option value="none"{% if page.option_selected(page.form_auth_mode.as_str(), "none") %} selected{% endif %}>None (public)</option>
          <option value="bearer"{% if page.option_selected(page.form_auth_mode.as_str(), "bearer") %} selected{% endif %}>Bearer token</option>
          <option value="hmac"{% if page.option_selected(page.form_auth_mode.as_str(), "hmac") %} selected{% endif %}>HMAC signature</option>
        </select>
        <span class="wf-field-hint">How callers authenticate when triggering this hook.</span>
      </div>

      <div id="bearer-fields">
        <div class="wf-field wf-mb-4">
          <label class="wf-label" for="auth_token">Bearer token</label>
          <input type="text" id="auth_token" name="auth_token" value="{{ page.form_auth_token }}" placeholder="${HOOK_TOKEN} or literal value" class="wf-input">
          <span class="wf-field-hint">Use ${ENV_VAR} syntax to reference an environment variable.</span>
        </div>
      </div>

      <div id="hmac-fields">
        <div class="wf-field wf-mb-4">
          <label class="wf-label" for="auth_header">Signature header</label>
          <input type="text" id="auth_header" name="auth_header" value="{{ page.form_auth_header }}" placeholder="X-Hub-Signature-256" class="wf-input">
          <span class="wf-field-hint">HTTP header containing the HMAC signature.</span>
        </div>
        <div class="wf-field wf-mb-4">
          <label class="wf-label" for="auth_algorithm">Algorithm</label>
          <select id="auth_algorithm" name="auth_algorithm" class="wf-select">
            <option value="sha256"{% if page.option_selected(page.form_auth_algorithm.as_str(), "sha256") %} selected{% endif %}>SHA-256</option>
          </select>
        </div>
        <div class="wf-field">
          <label class="wf-label" for="auth_secret">Shared secret</label>
          <input type="text" id="auth_secret" name="auth_secret" value="{{ page.form_auth_secret }}" placeholder="${WEBHOOK_SECRET} or literal value" class="wf-input">
          <span class="wf-field-hint">Use ${ENV_VAR} syntax to reference an environment variable.</span>
        </div>
      </div>
    </div>
  </section>

  <section class="wf-panel wf-mb-4">
    <div class="wf-panel-head"><span class="wf-panel-title">EXECUTOR</span></div>
    <div class="wf-panel-body">
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="executor_type">Executor type</label>
        <select id="executor_type" name="executor_type" class="wf-select">
          <option value="shell"{% if page.option_selected(page.form_executor_type.as_str(), "shell") %} selected{% endif %}>Shell command</option>
          <option value="script"{% if page.option_selected(page.form_executor_type.as_str(), "script") %} selected{% endif %}>Executable script</option>
          <option value="javascript"{% if page.option_selected(page.form_executor_type.as_str(), "javascript") %} selected{% endif %}>JavaScript script</option>
          <option value="python"{% if page.option_selected(page.form_executor_type.as_str(), "python") %} selected{% endif %}>Python script</option>
          <option value="http"{% if page.option_selected(page.form_executor_type.as_str(), "http") %} selected{% endif %}>HTTP request</option>
        </select>
        <span class="wf-field-hint">Choose how sendword should run this hook.</span>
      </div>
      <div class="wf-field wf-mb-4">
        <label class="wf-label" id="command_label" for="command">Shell command</label>
        <input type="text" id="command" name="command" value="{{ page.form_command }}" required placeholder="make deploy" class="wf-input">
        <span class="wf-field-hint" id="command_hint">Shell commands may use payload interpolation like &#123;&#123; action &#125;&#125;.</span>
      </div>
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="cwd">Working directory</label>
        <input type="text" id="cwd" name="cwd" value="{{ page.form_cwd }}" placeholder="/opt/app (optional)" class="wf-input">
      </div>
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="timeout">Timeout</label>
        <input type="text" id="timeout" name="timeout" value="{{ page.form_timeout }}" placeholder="30s" class="wf-input">
        <span class="wf-field-hint">Duration with unit, e.g. 30s, 2m, 1h. Leave blank for default.</span>
      </div>
      <div class="wf-field">
        <label class="wf-label" for="env_text">Environment variables</label>
        <textarea id="env_text" name="env_text" rows="4" placeholder="KEY=VALUE" class="wf-textarea" spellcheck="false">{{ page.form_env_text }}</textarea>
        <span class="wf-field-hint">One variable per line, in KEY=VALUE format.</span>
      </div>
    </div>
  </section>

  <section class="wf-panel wf-mb-4">
    <div class="wf-panel-head"><span class="wf-panel-title">PAYLOAD SCHEMA</span></div>
    <div class="wf-panel-body">
      <div class="wf-field">
        <label class="wf-label" for="payload_text">Field definitions</label>
        <textarea id="payload_text" name="payload_text" rows="4" placeholder="action:string:required&#10;tag:string&#10;count:number:required" class="wf-textarea" spellcheck="false">{{ page.form_payload_text }}</textarea>
        <span class="wf-field-hint">One field per line: name:type or name:type:required. Types: string, number, boolean, object, array.</span>
      </div>
    </div>
  </section>

  <section class="wf-panel wf-mb-4">
    <div class="wf-panel-head"><span class="wf-panel-title">TRIGGER RULES</span></div>
    <div class="wf-panel-body">
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="trigger_filters_text">Payload filters</label>
        <textarea id="trigger_filters_text" name="trigger_filters_text" rows="3" placeholder="action:equals:deploy&#10;env:contains:prod" class="wf-textarea" spellcheck="false">{{ page.form_trigger_filters_text }}</textarea>
        <span class="wf-field-hint">One filter per line: field:operator:value. Operators: equals, not_equals, contains, regex, gt, lt, gte, lte, exists.</span>
      </div>
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="trigger_windows_text">Time windows (UTC)</label>
        <textarea id="trigger_windows_text" name="trigger_windows_text" rows="2" placeholder="Mon,Tue,Wed,Thu,Fri:09:00-17:00" class="wf-textarea" spellcheck="false">{{ page.form_trigger_windows_text }}</textarea>
        <span class="wf-field-hint">One window per line: Days:HH:MM-HH:MM. Leave blank to allow at any time.</span>
      </div>
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="trigger_cooldown">Cooldown</label>
        <input type="text" id="trigger_cooldown" name="trigger_cooldown" value="{{ page.form_trigger_cooldown }}" placeholder="5m" class="wf-input">
        <span class="wf-field-hint">Duration with unit, e.g. 30s, 5m, 1h.</span>
      </div>
      <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 12px;">
        <div class="wf-field">
          <label class="wf-label" for="trigger_rate_max">Rate limit max</label>
          <input type="number" id="trigger_rate_max" name="trigger_rate_max" value="{{ page.form_trigger_rate_max }}" min="1" placeholder="10" class="wf-input">
        </div>
        <div class="wf-field">
          <label class="wf-label" for="trigger_rate_window">Rate limit window</label>
          <input type="text" id="trigger_rate_window" name="trigger_rate_window" value="{{ page.form_trigger_rate_window }}" placeholder="1h" class="wf-input">
        </div>
      </div>
    </div>
  </section>

  <section class="wf-panel wf-mb-4">
    <div class="wf-panel-head"><span class="wf-panel-title">RETRY</span></div>
    <div class="wf-panel-body">
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="retry_count">Retry count</label>
        <input type="number" id="retry_count" name="retry_count" value="{{ page.form_retry_count }}" min="0" placeholder="0" class="wf-input">
        <span class="wf-field-hint">0 = no retries.</span>
      </div>
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="retry_backoff">Backoff strategy</label>
        <select id="retry_backoff" name="retry_backoff" class="wf-select">
          <option value="none"{% if page.option_selected(page.form_retry_backoff.as_str(), "none") %} selected{% endif %}>None</option>
          <option value="linear"{% if page.option_selected(page.form_retry_backoff.as_str(), "linear") %} selected{% endif %}>Linear</option>
          <option value="exponential"{% if page.option_selected(page.form_retry_backoff.as_str(), "exponential") %} selected{% endif %}>Exponential</option>
        </select>
      </div>
      <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 12px;">
        <div class="wf-field">
          <label class="wf-label" for="retry_initial_delay">Initial delay</label>
          <input type="text" id="retry_initial_delay" name="retry_initial_delay" value="{{ page.form_retry_initial_delay }}" placeholder="1s" class="wf-input">
        </div>
        <div class="wf-field">
          <label class="wf-label" for="retry_max_delay">Max delay</label>
          <input type="text" id="retry_max_delay" name="retry_max_delay" value="{{ page.form_retry_max_delay }}" placeholder="60s" class="wf-input">
        </div>
      </div>
    </div>
  </section>

  <div class="wf-f wf-gap-3 wf-jc-e wf-mt-4">
    <a class="wf-btn" href="{{ page.cancel_href }}">CANCEL</a>
    <button type="submit" class="wf-btn primary">{{ page.submit_label }}</button>
  </div>
</form>

"##,
    ext = "html"
)]
struct HookFormTemplate<'a> {
    page: &'a HookFormPage,
}

#[cfg(test)]
mod tests {
    use axum::response::Html;

    use super::*;

    #[test]
    fn new_hook_form_renders_defaults_and_escapes_flash() {
        let page = HookFormPage::new_hook();
        let Html(html) = render_hook_form_page(
            "admin@example.com",
            &page,
            FlashMessages {
                success: None,
                error: Some("Bad <hook>"),
            },
        )
        .unwrap();

        assert!(html.contains(r#"action="/hooks/new""#));
        assert!(html.contains(r#"name="slug""#));
        assert!(!html.contains("readonly"));
        assert!(html.contains(r#"<option value="shell" selected>Shell command</option>"#));
        assert!(html.contains(r#"<option value="none" selected>None (public)</option>"#));
        assert!(html.contains(r#"<option value="exponential" selected>Exponential</option>"#));
        assert!(html.contains("CREATE HOOK"));
        assert!(html.contains("Bad &#60;hook&#62;"));
        assert!(html.contains(r#"/static/js/sendword.js"#));
        assert!(!html.contains(r#"function toggleAuthFields()"#));
        assert!(!html.contains(r#"onchange="toggleAuthFields()""#));
    }

    #[test]
    fn edit_hook_form_renders_values_and_readonly_slug() {
        let mut page = HookFormPage::edit("deploy-hook", "Deploy <App>");
        page.form_description = "Ship <prod>".to_owned();
        page.form_executor_type = "python".to_owned();
        page.form_command = "data/scripts/deploy.py".to_owned();
        page.form_cwd = "/srv/app".to_owned();
        page.form_timeout = "2m".to_owned();
        page.form_env_text = "APP_ENV=prod".to_owned();
        page.form_retry_count = 2;
        page.form_retry_backoff = "linear".to_owned();
        page.form_retry_initial_delay = "1s".to_owned();
        page.form_retry_max_delay = "30s".to_owned();
        page.form_auth_mode = "hmac".to_owned();
        page.form_auth_header = "X-Hub-Signature-256".to_owned();
        page.form_auth_secret = "${WEBHOOK_SECRET}".to_owned();
        page.form_payload_text = "action:string:required".to_owned();
        page.form_trigger_filters_text = "tag:contains:release".to_owned();
        page.form_trigger_windows_text = "Mon,Fri:09:00-17:00".to_owned();
        page.form_trigger_cooldown = "5m".to_owned();
        page.form_trigger_rate_max = "10".to_owned();
        page.form_trigger_rate_window = "1h".to_owned();

        let Html(html) = render_hook_form_page(
            "admin@example.com",
            &page,
            FlashMessages {
                success: Some("Saved"),
                error: None,
            },
        )
        .unwrap();

        assert!(html.contains("Deploy &#60;App&#62;"));
        assert!(!html.contains("Deploy <App>"));
        assert!(html.contains(r#"action="/hooks/deploy-hook/edit""#));
        assert!(html.contains(r#"value="deploy-hook" readonly required"#));
        assert!(html.contains(r#"<option value="python" selected>Python script</option>"#));
        assert!(html.contains(r#"<option value="hmac" selected>HMAC signature</option>"#));
        assert!(html.contains(r#"<option value="linear" selected>Linear</option>"#));
        assert!(html.contains("tag:contains:release"));
        assert!(html.contains("SAVE CHANGES"));
        assert!(html.contains(r#"href="/hooks/deploy-hook""#));
    }
}
