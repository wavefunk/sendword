use askama::Template;
use axum::response::Html;

use crate::error::AppError;

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

#[cfg(test)]
mod tests {
    use askama::Template;
    use axum::response::Html;

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
}
