use askama::Template;
use axum::response::Html;
use wavefunk_ui::components::{
    Alert, Avatar, BreadcrumbItem, Breadcrumbs, FeedbackKind, NavItem, NavSection, TrustedHtml,
};
use wavefunk_ui::layouts::{AppShell, SidebarProfile};

use crate::error::AppError;

const APP_NAME: &str = "SENDWORD";
const APP_STATUS_VERSION: &str = "V1.0";
const APP_HEAD_HTML: &str = r##"<link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect x='2' y='6' width='28' height='20' rx='3' fill='%230c0a09' stroke='%23f59e0b' stroke-width='2'/%3E%3Cpath d='M2 8l14 10 14-10' fill='none' stroke='%23f59e0b' stroke-width='2' stroke-linecap='round'/%3E%3Ccircle cx='26' cy='8' r='5' fill='%23ef4444'/%3E%3C/svg%3E">
<style>
  :root { --accent: #e06c75; --accent-ink: #000000; }
  [data-mode="light"] { --accent: #d20f39; --accent-ink: #ffffff; }
</style>"##;
const SENDWORD_SCRIPT_TAG: &str = r#"<script src="/static/js/sendword.js" defer></script>"#;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NavActive {
    #[default]
    None,
    Hooks,
    Approvals,
    Scripts,
    Admin,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FlashMessages<'a> {
    pub success: Option<&'a str>,
    pub error: Option<&'a str>,
}

#[derive(Clone, Copy, Debug)]
pub struct PageShell<'a> {
    pub title: &'a str,
    pub username: &'a str,
    pub active_nav: NavActive,
    pub breadcrumbs_html: TrustedHtml<'a>,
    pub actions_html: Option<TrustedHtml<'a>>,
    pub content_html: TrustedHtml<'a>,
    pub flash: FlashMessages<'a>,
    pub include_htmx_sse: bool,
    pub scripts_html: Option<TrustedHtml<'a>>,
}

impl<'a> PageShell<'a> {
    pub const fn new(
        title: &'a str,
        username: &'a str,
        active_nav: NavActive,
        breadcrumbs_html: TrustedHtml<'a>,
        content_html: TrustedHtml<'a>,
    ) -> Self {
        Self {
            title,
            username,
            active_nav,
            breadcrumbs_html,
            actions_html: None,
            content_html,
            flash: FlashMessages {
                success: None,
                error: None,
            },
            include_htmx_sse: false,
            scripts_html: None,
        }
    }

    pub const fn with_actions(mut self, actions_html: TrustedHtml<'a>) -> Self {
        self.actions_html = Some(actions_html);
        self
    }

    pub const fn with_flash(mut self, success: Option<&'a str>, error: Option<&'a str>) -> Self {
        self.flash = FlashMessages { success, error };
        self
    }

    pub const fn with_htmx_sse(mut self) -> Self {
        self.include_htmx_sse = true;
        self
    }

    pub const fn with_scripts(mut self, scripts_html: TrustedHtml<'a>) -> Self {
        self.scripts_html = Some(scripts_html);
        self
    }
}

pub fn render_template(template: &impl Template) -> Result<String, AppError> {
    template.render().map_err(|err| {
        AppError::from(eyre::eyre!(
            "failed to render Askama template {}: {err}",
            std::any::type_name_of_val(template)
        ))
    })
}

pub fn render_page(template: &impl Template) -> Result<Html<String>, AppError> {
    render_template(template).map(Html)
}

pub fn render_partial(template: &impl Template) -> Result<Html<String>, AppError> {
    render_template(template).map(Html)
}

pub fn render_breadcrumbs(items: &[BreadcrumbItem<'_>]) -> Result<String, AppError> {
    render_template(&Breadcrumbs::new(items))
}

pub fn render_shell(shell: &PageShell<'_>) -> Result<Html<String>, AppError> {
    let nav_html = render_nav(shell.active_nav)?;
    let content_html = render_content(shell)?;
    let scripts_html = render_scripts(shell.scripts_html);
    let avatar_initial = avatar_initial(shell.username);
    let profile = SidebarProfile::new()
        .with_name(shell.username)
        .with_avatar(Avatar::new(&avatar_initial).accent());

    let mut app_shell = AppShell::new(shell.title, APP_NAME, &content_html)
        .with_head(TrustedHtml::new(APP_HEAD_HTML))
        .with_nav(&nav_html)
        .with_breadcrumbs(shell.breadcrumbs_html)
        .with_profile(profile)
        .with_status(APP_NAME, APP_STATUS_VERSION)
        .with_scripts(TrustedHtml::new(&scripts_html));

    if let Some(actions_html) = shell.actions_html {
        app_shell = app_shell.with_actions(actions_html.as_str());
    }

    if shell.include_htmx_sse {
        app_shell = app_shell.with_htmx_sse();
    }

    render_page(&app_shell)
}

fn render_nav(active: NavActive) -> Result<String, AppError> {
    let mut html = String::new();

    html.push_str(&render_template(&NavSection::new("HOOKS"))?);
    html.push_str(&render_template(&nav_item(
        "Hooks",
        "/",
        active == NavActive::Hooks,
    ))?);
    html.push_str(&render_template(&nav_item(
        "Approvals",
        "/approvals",
        active == NavActive::Approvals,
    ))?);

    html.push_str(&render_template(&NavSection::new("TOOLS"))?);
    html.push_str(&render_template(&nav_item(
        "Scripts",
        "/scripts",
        active == NavActive::Scripts,
    ))?);

    html.push_str(&render_template(&NavSection::new("ADMIN"))?);
    html.push_str(&render_template(&nav_item(
        "Users",
        "/admin/users",
        active == NavActive::Admin,
    ))?);

    Ok(html)
}

fn nav_item<'a>(label: &'a str, href: &'a str, active: bool) -> NavItem<'a> {
    let item = NavItem::new(label, href);
    if active { item.active() } else { item }
}

fn render_content(shell: &PageShell<'_>) -> Result<String, AppError> {
    let mut html = String::new();

    if let Some(message) = shell.flash.success {
        html.push_str(&render_template(&Alert::new(FeedbackKind::Ok, message))?);
    }

    if let Some(message) = shell.flash.error {
        html.push_str(&render_template(&Alert::new(FeedbackKind::Error, message))?);
    }

    html.push_str(shell.content_html.as_str());
    Ok(html)
}

fn render_scripts(extra: Option<TrustedHtml<'_>>) -> String {
    let mut html = String::from(SENDWORD_SCRIPT_TAG);
    if let Some(extra) = extra {
        html.push_str(extra.as_str());
    }
    html
}

fn avatar_initial(username: &str) -> String {
    username
        .chars()
        .next()
        .map(|ch| ch.to_uppercase().collect())
        .unwrap_or_else(|| "S".to_owned())
}

#[cfg(test)]
mod tests {
    use askama::Template;
    use axum::response::Html;
    use wavefunk_ui::components::TrustedHtml;

    #[derive(Template)]
    #[template(source = "<p>{{ message }}</p>", ext = "html")]
    struct MessageTemplate<'a> {
        message: &'a str,
    }

    #[test]
    fn render_page_returns_html_and_keeps_askama_escaping() {
        let Html(html) = super::render_page(&MessageTemplate {
            message: "<script>",
        })
        .unwrap();

        assert_eq!(html, "<p>&#60;script&#62;</p>");
    }

    #[test]
    fn render_partial_returns_html_and_keeps_askama_escaping() {
        let Html(html) = super::render_partial(&MessageTemplate {
            message: "hooks & scripts",
        })
        .unwrap();

        assert_eq!(html, "<p>hooks &#38; scripts</p>");
    }

    #[test]
    fn render_shell_adds_sendword_chrome_and_profile() {
        let Html(html) = super::render_shell(
            &super::PageShell::new(
                "sendword - hooks",
                "admin@example.com",
                super::NavActive::Hooks,
                TrustedHtml::new(r#"<span aria-current="page">HOOKS</span>"#),
                TrustedHtml::new("<h1>Hooks</h1>"),
            )
            .with_actions(TrustedHtml::new(
                r#"<a class="wf-btn sm primary" href="/hooks/new">+ NEW HOOK</a>"#,
            ))
            .with_flash(Some("Hook created"), Some("Bad <input>")),
        )
        .unwrap();

        assert!(html.contains(r#"<title>sendword - hooks</title>"#));
        assert!(html.contains(r#"<span class="wf-brand-name">SENDWORD</span>"#));
        assert!(html.contains(r#"/static/wavefunk/css/wavefunk.css"#));
        assert!(html.contains(r#"/static/wavefunk/js/wavefunk.js"#));
        assert!(html.contains(r#"/static/js/sendword.js"#));
        assert!(html.contains(r#"<a class="wf-nav-item is-active" href="/" aria-current="page">"#));
        assert!(html.contains(r#"<span class="wf-user-name">admin@example.com</span>"#));
        assert!(html.contains(r#"hx-post="/logout""#));
        assert!(html.contains("Hook created"));
        assert!(html.contains("Bad &#60;input&#62;"));
        assert!(html.contains(r#"<h1>Hooks</h1>"#));
    }

    #[test]
    fn render_shell_can_include_htmx_sse_and_custom_scripts() {
        let Html(html) = super::render_shell(
            &super::PageShell::new(
                "sendword - execution",
                "admin@example.com",
                super::NavActive::Hooks,
                TrustedHtml::new(r#"<a href="/">HOOKS</a><span>EXEC</span>"#),
                TrustedHtml::new("<section>Logs</section>"),
            )
            .with_htmx_sse()
            .with_scripts(TrustedHtml::new(
                r#"<script id="execution-hooks">window.executionHooks = true;</script>"#,
            )),
        )
        .unwrap();

        assert!(html.contains(r#"/static/wavefunk/js/htmx-sse.js"#));
        assert!(html.contains(r#"<script id="execution-hooks">"#));
        assert!(html.contains(r#"<section>Logs</section>"#));
    }
}
