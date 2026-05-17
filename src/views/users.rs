use askama::Template;
use axum::response::Html;
use wavefunk_ui::components::{BreadcrumbItem, PageHeader, TrustedHtml};

use crate::error::AppError;

use super::{
    FlashMessages, NavActive, PageShell, SENDWORD_APP_SCRIPT_TAG, render_breadcrumbs, render_shell,
    render_template,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserRow {
    pub id: String,
    pub email: String,
    pub created_at: String,
    pub is_self: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsersPage {
    pub rows: Vec<UserRow>,
}

impl UsersPage {
    pub fn new(rows: Vec<UserRow>) -> Self {
        Self { rows }
    }
}

pub fn render_users_page(
    username: &str,
    page: &UsersPage,
    flash: FlashMessages<'_>,
) -> Result<Html<String>, AppError> {
    let breadcrumbs = render_breadcrumbs(&[BreadcrumbItem::current("USERS")])?;
    let content = render_users_content(page)?;

    render_shell(
        &PageShell::new(
            "sendword - users",
            username,
            NavActive::Admin,
            TrustedHtml::new(&breadcrumbs),
            TrustedHtml::new(&content),
        )
        .with_flash(flash.success, flash.error)
        .with_scripts(TrustedHtml::new(SENDWORD_APP_SCRIPT_TAG)),
    )
}

fn render_users_content(page: &UsersPage) -> Result<String, AppError> {
    let header = render_template(&PageHeader::new("Users"))?;
    let add_form = render_template(&AddUserFormTemplate)?;
    let table = render_template(&UsersTableTemplate { rows: &page.rows })?;

    render_template(&UsersContentTemplate {
        header_html: TrustedHtml::new(&header),
        add_form_html: TrustedHtml::new(&add_form),
        table_html: TrustedHtml::new(&table),
    })
}

#[derive(Debug, Template)]
#[template(
    source = r#"
{{ header_html }}
{{ add_form_html }}
{{ table_html }}
"#,
    ext = "html"
)]
struct UsersContentTemplate<'a> {
    header_html: TrustedHtml<'a>,
    add_form_html: TrustedHtml<'a>,
    table_html: TrustedHtml<'a>,
}

#[derive(Debug, Template)]
#[template(
    source = r#"
<section class="wf-panel wf-mb-4">
  <div class="wf-panel-head"><span class="wf-panel-title">ADD USER</span></div>
  <div class="wf-panel-body">
    <form method="post" action="/admin/users" class="wf-f wf-ai-e wf-gap-3 wf-wrap">
      <div class="wf-field wf-grow">
        <label class="wf-label" for="new-email">EMAIL</label>
        <input class="wf-input" type="email" id="new-email" name="email" required>
      </div>
      <div class="wf-field wf-grow">
        <label class="wf-label" for="new-password">PASSWORD</label>
        <input class="wf-input" type="password" id="new-password" name="password" required>
      </div>
      <button type="submit" class="wf-btn primary sm">ADD USER</button>
    </form>
  </div>
</section>
"#,
    ext = "html"
)]
struct AddUserFormTemplate;

#[derive(Debug, Template)]
#[template(
    source = r#"
<section class="wf-panel">
  <div class="wf-panel-head"><span class="wf-panel-title">ALL USERS</span></div>
  <div class="wf-tablescroll">
    <table class="wf-table">
      <thead>
        <tr>
          <th>EMAIL</th>
          <th>CREATED</th>
          <th></th>
        </tr>
      </thead>
      <tbody>
        {%- for row in rows %}
        <tr>
          <td class="strong">{{ row.email }}{% if row.is_self %} <span class="wf-tag accent">YOU</span>{% endif %}</td>
          <td><span data-ts="{{ row.created_at }}">{{ row.created_at }}</span></td>
          <td class="num">
            {%- if !row.is_self %}
            <form method="post" action="/admin/users/{{ row.id }}/delete" class="wf-ib" data-sendword-confirm="Delete user {{ row.email }}?">
              <button type="submit" class="wf-btn sm danger">DELETE</button>
            </form>
            {%- endif %}
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
struct UsersTableTemplate<'a> {
    rows: &'a [UserRow],
}

#[cfg(test)]
mod tests {
    use axum::response::Html;

    use super::*;

    #[test]
    fn users_page_renders_add_form_current_user_and_delete_action() {
        let page = UsersPage::new(vec![
            UserRow {
                id: "current-user".to_owned(),
                email: "admin@example.com".to_owned(),
                created_at: "2026-05-17T10:00:00Z".to_owned(),
                is_self: true,
            },
            UserRow {
                id: "other-user".to_owned(),
                email: "other <user>@example.com".to_owned(),
                created_at: r#"2026-05-17T10:00:00Z" onclick="bad"#.to_owned(),
                is_self: false,
            },
        ]);

        let Html(html) = render_users_page(
            "admin@example.com",
            &page,
            FlashMessages {
                success: Some("Saved <user>"),
                error: None,
            },
        )
        .unwrap();

        assert!(html.contains(r#"name="email""#));
        assert!(!html.contains(r#"name="username""#));
        assert!(html.contains("YOU"));
        assert!(html.contains("other &#60;user&#62;@example.com"));
        assert!(html.contains(r#"action="/admin/users/other-user/delete""#));
        assert!(
            html.contains(
                r#"data-sendword-confirm="Delete user other &#60;user&#62;@example.com?""#
            )
        );
        assert!(!html.contains("onsubmit="));
        assert!(html.contains("Saved &#60;user&#62;"));
        assert!(html.contains(r#"/static/js/sendword.js"#));
        assert!(html.contains(r#"data-ts="2026-05-17T10:00:00Z&#34; onclick=&#34;bad""#));
    }
}
