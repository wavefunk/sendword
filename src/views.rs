use askama::Template;
use axum::response::Html;
use wavefunk_ui::components::{
    Alert, Avatar, BreadcrumbItem, Breadcrumbs, Button, ButtonSize, ButtonVariant, EmptyState,
    FeedbackKind, NavItem, NavSection, Tag, TrustedHtml,
};
use wavefunk_ui::layouts::{AppShell, SidebarProfile};

use crate::error::AppError;

pub mod dashboard;

const APP_NAME: &str = "SENDWORD";
const APP_STATUS_VERSION: &str = "V1.0";
const APP_HEAD_HTML: &str = r##"<link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect x='2' y='6' width='28' height='20' rx='3' fill='%230c0a09' stroke='%23f59e0b' stroke-width='2'/%3E%3Cpath d='M2 8l14 10 14-10' fill='none' stroke='%23f59e0b' stroke-width='2' stroke-linecap='round'/%3E%3Ccircle cx='26' cy='8' r='5' fill='%23ef4444'/%3E%3C/svg%3E">
<style>
  :root { --accent: #e06c75; --accent-ink: #000000; }
  [data-mode="light"] { --accent: #d20f39; --accent-ink: #ffffff; }
</style>"##;

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
    /// Already-rendered breadcrumb markup from Askama or wavefunk-ui components.
    pub breadcrumbs_html: TrustedHtml<'a>,
    /// Already-rendered action markup from Askama or wavefunk-ui components.
    pub actions_html: Option<TrustedHtml<'a>>,
    /// Already-rendered page body markup from Askama templates.
    pub content_html: TrustedHtml<'a>,
    pub flash: FlashMessages<'a>,
    pub include_htmx_sse: bool,
    /// App-specific script tags only; generic wavefunk.js behavior is loaded by AppShell.
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusKind {
    Neutral,
    Ok,
    Info,
    Warn,
    Error,
}

impl StatusKind {
    const fn feedback(self) -> Option<FeedbackKind> {
        match self {
            Self::Neutral => None,
            Self::Ok => Some(FeedbackKind::Ok),
            Self::Info => Some(FeedbackKind::Info),
            Self::Warn => Some(FeedbackKind::Warn),
            Self::Error => Some(FeedbackKind::Error),
        }
    }
}

pub fn render_status_tag(label: &str, kind: StatusKind) -> Result<String, AppError> {
    let tag = match kind.feedback() {
        Some(feedback) => Tag::status(feedback, label),
        None => Tag::new(label).with_dot(),
    };
    render_template(&tag)
}

#[derive(Debug, Template)]
#[template(
    source = r#"<span data-ts="{{ value }}">{{ value }}</span>"#,
    ext = "html"
)]
struct DateSpan<'a> {
    value: &'a str,
}

pub fn render_date_span(value: &str) -> Result<String, AppError> {
    render_template(&DateSpan { value })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionKind {
    Default,
    Primary,
    Danger,
}

impl ActionKind {
    const fn variant(self) -> ButtonVariant {
        match self {
            Self::Default => ButtonVariant::Default,
            Self::Primary => ButtonVariant::Primary,
            Self::Danger => ButtonVariant::Danger,
        }
    }
}

pub fn render_action_link(label: &str, href: &str, kind: ActionKind) -> Result<String, AppError> {
    render_template(
        &Button::link(label, href)
            .with_variant(kind.variant())
            .with_size(ButtonSize::Small),
    )
}

pub fn render_empty_state(
    title: &str,
    body: &str,
    actions_html: Option<TrustedHtml<'_>>,
) -> Result<String, AppError> {
    let empty = EmptyState::new(title, body);
    let empty = if let Some(actions_html) = actions_html {
        empty.with_actions(actions_html)
    } else {
        empty
    };
    render_template(&empty)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaginationState {
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
    pub item_count: i64,
}

impl PaginationState {
    pub fn new(page: i64, per_page: i64, total: i64, item_count: i64) -> Self {
        let per_page = per_page.max(1);
        let total = total.max(0);
        let total_pages = if total == 0 {
            1
        } else {
            (total + per_page - 1) / per_page
        };

        Self {
            page: page.clamp(1, total_pages),
            per_page,
            total,
            item_count: item_count.clamp(0, per_page),
        }
    }

    pub fn total_pages(&self) -> i64 {
        if self.total == 0 {
            1
        } else {
            (self.total + self.per_page - 1) / self.per_page
        }
    }

    pub fn start_index(&self) -> i64 {
        if self.total == 0 || self.item_count == 0 {
            0
        } else {
            ((self.page - 1) * self.per_page) + 1
        }
    }

    pub fn end_index(&self) -> i64 {
        if self.total == 0 || self.item_count == 0 {
            0
        } else {
            (self.start_index() + self.item_count - 1).min(self.total)
        }
    }

    pub fn has_previous(&self) -> bool {
        self.page > 1
    }

    pub fn has_next(&self) -> bool {
        self.page < self.total_pages()
    }

    pub fn summary_label(&self) -> String {
        format!(
            "{}-{} OF {}",
            self.start_index(),
            self.end_index(),
            self.total
        )
    }
}

pub fn render_shell(shell: &PageShell<'_>) -> Result<Html<String>, AppError> {
    let nav_html = render_nav(shell.active_nav)?;
    let content_html = render_content(shell)?;
    let avatar_initial = avatar_initial(shell.username);
    let profile = SidebarProfile::new()
        .with_name(shell.username)
        .with_avatar(Avatar::new(&avatar_initial).accent());

    let mut app_shell = AppShell::new(shell.title, APP_NAME, &content_html)
        .with_head(TrustedHtml::new(APP_HEAD_HTML))
        .with_nav(&nav_html)
        .with_breadcrumbs(shell.breadcrumbs_html)
        .with_profile(profile)
        .with_status(APP_NAME, APP_STATUS_VERSION);

    if let Some(actions_html) = shell.actions_html {
        app_shell = app_shell.with_actions(actions_html.as_str());
    }

    if let Some(scripts_html) = shell.scripts_html {
        app_shell = app_shell.with_scripts(scripts_html);
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
        assert!(!html.contains(r#"/static/js/sendword.js"#));
        assert!(!html.contains(r#"/static/css/wavefunk.css"#));
        assert!(!html.contains("unpkg.com/htmx"));
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

    #[test]
    fn render_shell_escapes_user_visible_profile_text() {
        let Html(html) = super::render_shell(&super::PageShell::new(
            "sendword - users",
            r#"<admin@example.com>"#,
            super::NavActive::Admin,
            TrustedHtml::new(r#"<span aria-current="page">USERS</span>"#),
            TrustedHtml::new("<h1>Users</h1>"),
        ))
        .unwrap();

        assert!(!html.contains(r#"<span class="wf-user-name"><admin@example.com></span>"#));
        assert!(html.contains("&#60;admin@example.com&#62;"));
    }

    #[test]
    fn shared_status_and_date_helpers_escape_and_classify() {
        let success = super::render_status_tag("SUCCESS", super::StatusKind::Ok).unwrap();
        let neutral = super::render_status_tag("DISABLED", super::StatusKind::Neutral).unwrap();
        let date = super::render_date_span(r#"2026-05-17T10:00:00Z" onclick="bad"#).unwrap();

        assert!(success.contains(r#"class="wf-tag ok""#));
        assert!(success.contains("SUCCESS"));
        assert!(neutral.contains(r#"class="wf-tag""#));
        assert!(neutral.contains("DISABLED"));
        assert!(date.contains(r#"data-ts="2026-05-17T10:00:00Z&#34; onclick=&#34;bad""#));
        assert!(date.contains("2026-05-17T10:00:00Z&#34; onclick=&#34;bad"));
    }

    #[test]
    fn shared_action_empty_state_and_pagination_helpers_render() {
        let action =
            super::render_action_link("NEW HOOK", "/hooks/new", super::ActionKind::Primary)
                .unwrap();
        let empty = super::render_empty_state(
            "NO HOOKS YET",
            "Create your first hook.",
            Some(TrustedHtml::new(&action)),
        )
        .unwrap();
        let pagination = super::PaginationState::new(2, 20, 45, 20);

        assert!(action.contains(r#"class="wf-btn primary sm" href="/hooks/new""#));
        assert!(empty.contains(r#"class="wf-empty""#));
        assert!(empty.contains("NO HOOKS YET"));
        assert_eq!(pagination.start_index(), 21);
        assert_eq!(pagination.end_index(), 40);
        assert_eq!(pagination.total_pages(), 3);
        assert!(pagination.has_previous());
        assert!(pagination.has_next());
        assert_eq!(pagination.summary_label(), "21-40 OF 45");

        let out_of_range = super::PaginationState::new(99, 20, 45, 20);
        assert_eq!(out_of_range.page, 3);
        assert_eq!(out_of_range.summary_label(), "41-45 OF 45");
    }
}
