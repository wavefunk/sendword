use askama::Template;
use axum::response::Html;
use wavefunk_ui::components::{
    BreadcrumbItem, ControlSize, HtmlAttr, Input, PageHeader, Panel, TrustedHtml,
};

use crate::error::AppError;
use crate::models::ExecutionStatus;

use super::{
    ActionKind, FlashMessages, NavActive, PageShell, SENDWORD_APP_SCRIPT_TAG, render_action_link,
    render_breadcrumbs, render_empty_state, render_shell, render_template,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DashboardStatusDot {
    color_var: &'static str,
}

impl DashboardStatusDot {
    pub const fn success() -> Self {
        Self { color_var: "--ok" }
    }

    pub const fn failed() -> Self {
        Self { color_var: "--err" }
    }

    pub const fn running() -> Self {
        Self {
            color_var: "--warn",
        }
    }

    pub const fn muted() -> Self {
        Self {
            color_var: "--fg-muted",
        }
    }

    pub fn from_execution_status(status: &ExecutionStatus) -> Self {
        match status {
            ExecutionStatus::Success => Self::success(),
            ExecutionStatus::Failed => Self::failed(),
            ExecutionStatus::Running => Self::running(),
            _ => Self::muted(),
        }
    }

    pub const fn color_var(&self) -> &'static str {
        self.color_var
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardHookRow {
    pub name: String,
    pub slug: String,
    pub filter_name: String,
    pub enabled: bool,
    pub last_triggered_at: Option<String>,
    pub recent_statuses: Vec<DashboardStatusDot>,
}

impl DashboardHookRow {
    pub fn new(
        name: impl Into<String>,
        slug: impl Into<String>,
        enabled: bool,
        last_triggered_at: Option<String>,
        recent_statuses: Vec<DashboardStatusDot>,
    ) -> Self {
        let name = name.into();
        Self {
            filter_name: name.to_lowercase(),
            name,
            slug: slug.into(),
            enabled,
            last_triggered_at,
            recent_statuses,
        }
    }
}

pub fn render_dashboard_page(
    username: &str,
    hooks: &[DashboardHookRow],
    flash: FlashMessages<'_>,
) -> Result<Html<String>, AppError> {
    let breadcrumbs = render_breadcrumbs(&[BreadcrumbItem::current("HOOKS")])?;
    let new_hook_action = render_action_link("+ NEW HOOK", "/hooks/new", ActionKind::Primary)?;
    let content = render_dashboard_content(hooks)?;

    render_shell(
        &PageShell::new(
            "sendword - hooks",
            username,
            NavActive::Hooks,
            TrustedHtml::new(&breadcrumbs),
            TrustedHtml::new(&content),
        )
        .with_actions(TrustedHtml::new(&new_hook_action))
        .with_flash(flash.success, flash.error)
        .with_scripts(TrustedHtml::new(SENDWORD_APP_SCRIPT_TAG)),
    )
}

fn render_dashboard_content(hooks: &[DashboardHookRow]) -> Result<String, AppError> {
    let subtitle = hook_count_label(hooks.len());
    let header = render_template(&PageHeader::new("Hooks").with_subtitle(&subtitle))?;
    let body = if hooks.is_empty() {
        render_empty_dashboard()?
    } else {
        render_hooks_panel(hooks)?
    };

    render_template(&DashboardContentTemplate {
        header_html: TrustedHtml::new(&header),
        body_html: TrustedHtml::new(&body),
    })
}

fn render_empty_dashboard() -> Result<String, AppError> {
    let action = render_action_link("+ NEW HOOK", "/hooks/new", ActionKind::Primary)?;
    render_empty_state(
        "NO HOOKS YET",
        "Create your first webhook hook to get started.",
        Some(TrustedHtml::new(&action)),
    )
}

fn render_hooks_panel(hooks: &[DashboardHookRow]) -> Result<String, AppError> {
    let search = render_search_control()?;
    let table = render_template(&DashboardHookTableTemplate { hooks })?;
    render_template(
        &Panel::new("ALL HOOKS", TrustedHtml::new(&table)).with_action(TrustedHtml::new(&search)),
    )
}

fn render_search_control() -> Result<String, AppError> {
    let attrs = [
        HtmlAttr::new("id", "hook-filter"),
        HtmlAttr::new("data-sendword-hook-filter", "true"),
    ];
    let input = render_template(
        &Input::search("hook-filter")
            .with_placeholder("Filter hooks...")
            .with_size(ControlSize::Small)
            .with_attrs(&attrs),
    )?;

    render_template(&SearchControlTemplate {
        input_html: TrustedHtml::new(&input),
    })
}

fn hook_count_label(count: usize) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} hook{plural} configured")
}

#[derive(Debug, Template)]
#[template(
    source = r#"
{{ header_html }}
{{ body_html }}
"#,
    ext = "html"
)]
struct DashboardContentTemplate<'a> {
    header_html: TrustedHtml<'a>,
    body_html: TrustedHtml<'a>,
}

#[derive(Debug, Template)]
#[template(
    source = r#"
<div class="wf-tablewrap">
  <div class="wf-tablescroll">
    <table class="wf-table is-interactive">
      <thead>
        <tr>
          <th>NAME</th>
          <th>SLUG</th>
          <th>STATUS</th>
          <th>RECENT</th>
          <th>LAST TRIGGERED</th>
        </tr>
      </thead>
      <tbody id="hook-list">
        {%- for hook in hooks %}
        <tr data-hook-name="{{ hook.filter_name }}" data-hook-slug="{{ hook.slug }}">
          <td class="strong"><a href="/hooks/{{ hook.slug }}">{{ hook.name }}</a></td>
          <td><code>{{ hook.slug }}</code></td>
          <td>
            {%- if hook.enabled -%}
            <span class="wf-tag ok"><span class="dot"></span>ENABLED</span>
            {%- else -%}
            <span class="wf-tag"><span class="dot"></span>DISABLED</span>
            {%- endif -%}
          </td>
          <td>
            {%- for status in hook.recent_statuses -%}
            <span class="wf-dot" style="color: var({{ status.color_var() }});"></span>
            {%- endfor -%}
          </td>
          <td>
            {%- match hook.last_triggered_at -%}
            {%- when Some with (triggered_at) -%}
            <span data-ts="{{ triggered_at }}">{{ triggered_at }}</span>
            {%- when None -%}
            <span class="muted">&mdash;</span>
            {%- endmatch -%}
          </td>
        </tr>
        {%- endfor %}
      </tbody>
    </table>
  </div>
</div>
"#,
    ext = "html"
)]
struct DashboardHookTableTemplate<'a> {
    hooks: &'a [DashboardHookRow],
}

#[derive(Debug, Template)]
#[template(
    source = r#"<div class="wf-input-group" style="max-width: 240px;"><span class="wf-input-addon">&#x2315;</span>{{ input_html }}</div>"#,
    ext = "html"
)]
struct SearchControlTemplate<'a> {
    input_html: TrustedHtml<'a>,
}

#[cfg(test)]
mod tests {
    use axum::response::Html;

    use super::*;

    #[test]
    fn dashboard_page_renders_shell_table_and_escapes_hook_data() {
        let hooks = vec![DashboardHookRow::new(
            "Deploy <App>",
            "deploy-app",
            true,
            Some(r#"2026-05-17T10:00:00Z" onclick="bad"#.to_owned()),
            vec![DashboardStatusDot::success(), DashboardStatusDot::failed()],
        )];

        let Html(html) = render_dashboard_page(
            "admin@example.com",
            &hooks,
            FlashMessages {
                success: Some("Created <hook>"),
                error: None,
            },
        )
        .unwrap();

        assert!(html.contains(r#"<title>sendword - hooks</title>"#));
        assert!(html.contains(r#"<span aria-current="page">HOOKS</span>"#));
        assert!(html.contains("1 hook configured"));
        assert!(html.contains("Deploy &#60;App&#62;"));
        assert!(!html.contains("Deploy <App>"));
        assert!(html.contains(r#"data-hook-name="deploy &#60;app&#62;""#));
        assert!(html.contains(r#"<a href="/hooks/deploy-app">"#));
        assert!(html.contains(r#"class="wf-tag ok""#));
        assert!(html.contains("var(--ok)"));
        assert!(html.contains("var(--err)"));
        assert!(html.contains(r#"id="hook-filter""#));
        assert!(html.contains(r#"/static/js/sendword.js"#));
        assert!(!html.contains("function filterHooks(query)"));
        assert!(html.contains("Created &#60;hook&#62;"));
        assert!(html.contains(r#"data-ts="2026-05-17T10:00:00Z&#34; onclick=&#34;bad""#));
        assert!(html.contains(r#"/static/wavefunk/js/wavefunk.js"#));
    }

    #[test]
    fn dashboard_page_renders_empty_state_without_filter_script() {
        let Html(html) = render_dashboard_page(
            "admin@example.com",
            &[],
            FlashMessages {
                success: None,
                error: None,
            },
        )
        .unwrap();

        assert!(html.contains("0 hooks configured"));
        assert!(html.contains("NO HOOKS YET"));
        assert!(html.contains(r#"href="/hooks/new""#));
        assert!(!html.contains(r#"id="hook-filter""#));
        assert!(html.contains(r#"/static/js/sendword.js"#));
        assert!(!html.contains("function filterHooks(query)"));
    }
}
