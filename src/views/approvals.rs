use askama::Template;
use axum::response::Html;
use wavefunk_ui::components::{BreadcrumbItem, PageHeader, TrustedHtml};

use crate::error::AppError;
use crate::models::Execution;

use super::{
    NavActive, PageShell, render_breadcrumbs, render_empty_state, render_shell, render_template,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalRow {
    pub id: String,
    pub hook_slug: String,
    pub triggered_at: String,
    pub trigger_source: String,
}

impl ApprovalRow {
    pub fn from_execution(execution: Execution) -> Self {
        Self {
            id: execution.id,
            hook_slug: execution.hook_slug,
            triggered_at: execution.triggered_at,
            trigger_source: execution.trigger_source,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalsPage {
    pub rows: Vec<ApprovalRow>,
}

impl ApprovalsPage {
    pub fn new(rows: Vec<ApprovalRow>) -> Self {
        Self { rows }
    }

    pub fn count_label(&self) -> String {
        format!("{} pending", self.rows.len())
    }
}

pub fn render_approvals_page(
    username: &str,
    page: &ApprovalsPage,
) -> Result<Html<String>, AppError> {
    let breadcrumbs = render_breadcrumbs(&[BreadcrumbItem::current("APPROVALS")])?;
    let content = render_approvals_content(page)?;

    render_shell(&PageShell::new(
        "sendword - approvals",
        username,
        NavActive::Approvals,
        TrustedHtml::new(&breadcrumbs),
        TrustedHtml::new(&content),
    ))
}

fn render_approvals_content(page: &ApprovalsPage) -> Result<String, AppError> {
    let header = render_template(&PageHeader::new("Approvals").with_subtitle(&page.count_label()))?;
    let body = if page.rows.is_empty() {
        render_empty_state("ALL CLEAR", "No executions pending approval.", None)?
    } else {
        render_template(&ApprovalsTableTemplate { rows: &page.rows })?
    };

    render_template(&ApprovalsContentTemplate {
        header_html: TrustedHtml::new(&header),
        body_html: TrustedHtml::new(&body),
    })
}

#[derive(Debug, Template)]
#[template(
    source = r#"
{{ header_html }}
{{ body_html }}
"#,
    ext = "html"
)]
struct ApprovalsContentTemplate<'a> {
    header_html: TrustedHtml<'a>,
    body_html: TrustedHtml<'a>,
}

#[derive(Debug, Template)]
#[template(
    source = r#"
<section class="wf-panel">
  <div class="wf-tablescroll">
    <table class="wf-table">
      <thead>
        <tr>
          <th>HOOK</th>
          <th>TRIGGERED</th>
          <th>SOURCE</th>
          <th>ACTIONS</th>
        </tr>
      </thead>
      <tbody>
        {%- for row in rows %}
        <tr>
          <td class="strong"><a href="/hooks/{{ row.hook_slug }}">{{ row.hook_slug }}</a></td>
          <td><span data-ts="{{ row.triggered_at }}">{{ row.triggered_at }}</span></td>
          <td>{{ row.trigger_source }}</td>
          <td>
            <div class="wf-btn-group">
              <form method="post" action="/executions/{{ row.id }}/approve" class="wf-ib">
                <button type="submit" class="wf-btn sm">APPROVE</button>
              </form>
              <form method="post" action="/executions/{{ row.id }}/reject" class="wf-ib">
                <button type="submit" class="wf-btn sm danger">REJECT</button>
              </form>
            </div>
          </td>
        </tr>
        {%- endfor %}
      </tbody>
    </table>
  </div>
</section>
"#,
    ext = "html"
)]
struct ApprovalsTableTemplate<'a> {
    rows: &'a [ApprovalRow],
}

#[cfg(test)]
mod tests {
    use axum::response::Html;

    use super::*;

    #[test]
    fn approvals_page_renders_rows_and_escapes_fields() {
        let page = ApprovalsPage::new(vec![ApprovalRow {
            id: "exec123".to_owned(),
            hook_slug: "deploy-hook".to_owned(),
            triggered_at: r#"2026-05-17T10:00:00Z" onclick="bad"#.to_owned(),
            trigger_source: "<source>".to_owned(),
        }]);

        let Html(html) = render_approvals_page("admin@example.com", &page).unwrap();

        assert!(html.contains("1 pending"));
        assert!(html.contains(r#"href="/hooks/deploy-hook""#));
        assert!(html.contains(r#"action="/executions/exec123/approve""#));
        assert!(html.contains(r#"action="/executions/exec123/reject""#));
        assert!(html.contains("&#60;source&#62;"));
        assert!(!html.contains("<source>"));
        assert!(html.contains(r#"data-ts="2026-05-17T10:00:00Z&#34; onclick=&#34;bad""#));
    }

    #[test]
    fn approvals_page_renders_empty_state() {
        let page = ApprovalsPage::new(Vec::new());

        let Html(html) = render_approvals_page("admin@example.com", &page).unwrap();

        assert!(html.contains("0 pending"));
        assert!(html.contains("ALL CLEAR"));
        assert!(html.contains("No executions pending approval."));
        assert!(!html.contains("<table"));
    }
}
