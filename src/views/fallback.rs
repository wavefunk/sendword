use axum::response::Html;
use wavefunk_ui::components::{BreadcrumbItem, TrustedHtml};
use wavefunk_ui::layouts::AppShell;

use crate::error::AppError;

use super::{
    APP_HEAD_HTML, APP_NAME, APP_STATUS_VERSION, ActionKind, render_action_link,
    render_breadcrumbs, render_empty_state, render_page,
};

pub fn render_not_found_page() -> Result<Html<String>, AppError> {
    let action = render_action_link("BACK TO DASHBOARD", "/", ActionKind::Default)?;
    let content = render_empty_state(
        "PAGE NOT FOUND",
        "The page you're looking for doesn't exist.",
        Some(TrustedHtml::new(&action)),
    )?;
    let breadcrumbs = render_breadcrumbs(&[BreadcrumbItem::current("NOT FOUND")])?;

    render_page(
        &AppShell::new("sendword - 404", APP_NAME, &content)
            .with_head(TrustedHtml::new(APP_HEAD_HTML))
            .with_breadcrumbs(TrustedHtml::new(&breadcrumbs))
            .with_status(APP_NAME, APP_STATUS_VERSION),
    )
}

#[cfg(test)]
mod tests {
    use axum::response::Html;

    use super::*;

    #[test]
    fn not_found_page_uses_minimal_wavefunk_shell() {
        let Html(html) = render_not_found_page().unwrap();

        assert!(html.contains("<title>sendword - 404</title>"));
        assert!(html.contains("PAGE NOT FOUND"));
        assert!(html.contains("BACK TO DASHBOARD"));
        assert!(html.contains(r#"/static/wavefunk/css/wavefunk.css"#));
        assert!(html.contains(r#"/static/wavefunk/js/wavefunk.js"#));
        assert!(html.contains(r#"<span class="wf-brand-name">SENDWORD</span>"#));
        assert!(!html.contains(r#"hx-post="/logout""#));
    }
}
