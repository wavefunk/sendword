use askama::Template;
use axum::response::Html;
use wavefunk_ui::components::{BreadcrumbItem, PageHeader, TrustedHtml};

use crate::error::AppError;

use super::{
    ActionKind, FlashMessages, NavActive, PageShell, SENDWORD_APP_SCRIPT_TAG, render_action_link,
    render_breadcrumbs, render_empty_state, render_shell, render_template,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptListRow {
    pub name: String,
    pub href: String,
    pub size: String,
    pub modified: String,
}

impl ScriptListRow {
    pub fn new(
        name: impl Into<String>,
        size: impl Into<String>,
        modified: impl Into<String>,
    ) -> Self {
        let name = name.into();
        let href = format!("/scripts/{}", urlencoding::encode(&name));
        Self {
            name,
            href,
            size: size.into(),
            modified: modified.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptsPage {
    pub rows: Vec<ScriptListRow>,
}

impl ScriptsPage {
    pub fn new(rows: Vec<ScriptListRow>) -> Self {
        Self { rows }
    }

    pub fn count_label(&self) -> String {
        let plural = if self.rows.len() == 1 { "" } else { "s" };
        format!("{} file{plural}", self.rows.len())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptEditorPage {
    pub is_new: bool,
    pub filename: String,
    pub encoded_filename: String,
    pub content: String,
}

impl ScriptEditorPage {
    pub fn new_script() -> Self {
        Self {
            is_new: true,
            filename: String::new(),
            encoded_filename: String::new(),
            content: String::new(),
        }
    }

    pub fn edit(filename: impl Into<String>, content: impl Into<String>) -> Self {
        let filename = filename.into();
        Self {
            is_new: false,
            encoded_filename: urlencoding::encode(&filename).into_owned(),
            filename,
            content: content.into(),
        }
    }

    pub fn title(&self) -> &str {
        if self.is_new {
            "New Script"
        } else {
            &self.filename
        }
    }

    pub fn panel_title(&self) -> &'static str {
        if self.is_new {
            "CREATE SCRIPT"
        } else {
            "EDIT SCRIPT"
        }
    }

    pub fn submit_label(&self) -> &'static str {
        if self.is_new { "CREATE" } else { "SAVE" }
    }

    pub fn form_action(&self) -> String {
        if self.is_new {
            "/scripts/new".to_owned()
        } else {
            format!("/scripts/{}", self.encoded_filename)
        }
    }

    pub fn delete_action(&self) -> String {
        format!("/scripts/{}/delete", self.encoded_filename)
    }
}

pub fn render_scripts_page(
    username: &str,
    page: &ScriptsPage,
    flash: FlashMessages<'_>,
) -> Result<Html<String>, AppError> {
    let breadcrumbs = render_breadcrumbs(&[BreadcrumbItem::current("SCRIPTS")])?;
    let action = render_action_link("+ NEW SCRIPT", "/scripts/new", ActionKind::Primary)?;
    let content = render_scripts_content(page)?;

    render_shell(
        &PageShell::new(
            "sendword - scripts",
            username,
            NavActive::Scripts,
            TrustedHtml::new(&breadcrumbs),
            TrustedHtml::new(&content),
        )
        .with_actions(TrustedHtml::new(&action))
        .with_flash(flash.success, flash.error),
    )
}

pub fn render_script_editor_page(
    username: &str,
    page: &ScriptEditorPage,
    flash: FlashMessages<'_>,
) -> Result<Html<String>, AppError> {
    let breadcrumbs = render_editor_breadcrumbs(page)?;
    let content = render_script_editor_content(page)?;

    render_shell(
        &PageShell::new(
            "sendword - script editor",
            username,
            NavActive::Scripts,
            TrustedHtml::new(&breadcrumbs),
            TrustedHtml::new(&content),
        )
        .with_flash(flash.success, flash.error)
        .with_scripts(TrustedHtml::new(SENDWORD_APP_SCRIPT_TAG)),
    )
}

fn render_scripts_content(page: &ScriptsPage) -> Result<String, AppError> {
    let header = render_template(&PageHeader::new("Scripts").with_subtitle(&page.count_label()))?;
    let body = if page.rows.is_empty() {
        let action = render_action_link("+ NEW SCRIPT", "/scripts/new", ActionKind::Primary)?;
        render_empty_state(
            "NO SCRIPTS",
            "Upload a script to reference in hook executors.",
            Some(TrustedHtml::new(&action)),
        )?
    } else {
        render_template(&ScriptsTableTemplate { rows: &page.rows })?
    };

    render_template(&ScriptsContentTemplate {
        header_html: TrustedHtml::new(&header),
        body_html: TrustedHtml::new(&body),
    })
}

fn render_editor_breadcrumbs(page: &ScriptEditorPage) -> Result<String, AppError> {
    if page.is_new {
        render_breadcrumbs(&[
            BreadcrumbItem::link("SCRIPTS", "/scripts"),
            BreadcrumbItem::current("NEW"),
        ])
    } else {
        let filename_label = page.filename.to_uppercase();
        render_breadcrumbs(&[
            BreadcrumbItem::link("SCRIPTS", "/scripts"),
            BreadcrumbItem::current(&filename_label),
        ])
    }
}

fn render_script_editor_content(page: &ScriptEditorPage) -> Result<String, AppError> {
    let header = render_template(&PageHeader::new(page.title()))?;
    let form = render_template(&ScriptEditorFormTemplate { page })?;

    render_template(&ScriptEditorContentTemplate {
        header_html: TrustedHtml::new(&header),
        form_html: TrustedHtml::new(&form),
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
struct ScriptsContentTemplate<'a> {
    header_html: TrustedHtml<'a>,
    body_html: TrustedHtml<'a>,
}

#[derive(Debug, Template)]
#[template(
    source = r#"
<section class="wf-panel">
  <div class="wf-tablescroll">
    <table class="wf-table is-interactive">
      <thead>
        <tr>
          <th>FILENAME</th>
          <th class="num">SIZE</th>
          <th>MODIFIED</th>
        </tr>
      </thead>
      <tbody>
        {%- for row in rows %}
        <tr>
          <td class="strong"><a href="{{ row.href }}"><code>{{ row.name }}</code></a></td>
          <td class="num">{{ row.size }}</td>
          <td><span data-ts="{{ row.modified }}">{{ row.modified }}</span></td>
        </tr>
        {%- endfor %}
      </tbody>
    </table>
  </div>
</section>
"#,
    ext = "html"
)]
struct ScriptsTableTemplate<'a> {
    rows: &'a [ScriptListRow],
}

#[derive(Debug, Template)]
#[template(
    source = r#"
{{ header_html }}
{{ form_html }}
"#,
    ext = "html"
)]
struct ScriptEditorContentTemplate<'a> {
    header_html: TrustedHtml<'a>,
    form_html: TrustedHtml<'a>,
}

#[derive(Debug, Template)]
#[template(
    source = r##"
<form method="post" action="{{ page.form_action() }}">
  <section class="wf-panel wf-mb-4">
    <div class="wf-panel-head"><span class="wf-panel-title">{{ page.panel_title() }}</span></div>
    <div class="wf-panel-body">
      <div class="wf-field wf-mb-4">
        <label class="wf-label" for="filename">FILENAME</label>
        <input class="wf-input" type="text" id="filename" name="filename" value="{{ page.filename }}"{% if !page.is_new %} readonly{% endif %}{% if page.is_new %} required pattern="[a-zA-Z0-9_\-]+(\.[a-zA-Z0-9]+)?"{% endif %}>
      </div>
      <div class="wf-field">
        <label class="wf-label" for="content">CONTENT</label>
        <textarea class="wf-textarea" id="content" name="content" rows="24" style="font-family: var(--font-mono); font-size: 13px;" maxlength="1048576">{{ page.content }}</textarea>
      </div>
    </div>
  </section>

  <div class="wf-f wf-gap-3 wf-jc-e">
    <a class="wf-btn" href="/scripts">CANCEL</a>
    <button type="submit" class="wf-btn primary">{{ page.submit_label() }}</button>
  </div>
</form>

{%- if !page.is_new %}
<form method="post" action="{{ page.delete_action() }}" class="wf-mt-4" data-sendword-confirm="Delete {{ page.filename }}?">
  <section class="wf-panel">
    <div class="wf-panel-body wf-f wf-ai-c wf-jc-b wf-gap-3">
      <span class="wf-fg-muted wf-t-sm">Permanently delete this script.</span>
      <button type="submit" class="wf-btn sm danger">DELETE SCRIPT</button>
    </div>
  </section>
</form>
{%- endif %}
"##,
    ext = "html"
)]
struct ScriptEditorFormTemplate<'a> {
    page: &'a ScriptEditorPage,
}

#[cfg(test)]
mod tests {
    use axum::response::Html;

    use super::*;

    #[test]
    fn scripts_page_renders_rows_and_encodes_links() {
        let page = ScriptsPage::new(vec![ScriptListRow::new(
            "deploy <app>.sh",
            "42 B",
            r#"2026-05-17T10:00:00Z" onclick="bad"#,
        )]);

        let Html(html) = render_scripts_page(
            "admin@example.com",
            &page,
            FlashMessages {
                success: None,
                error: Some("Bad <script>"),
            },
        )
        .unwrap();

        assert!(html.contains("1 file"));
        assert!(html.contains("deploy &#60;app&#62;.sh"));
        assert!(html.contains(r#"href="/scripts/deploy%20%3Capp%3E.sh""#));
        assert!(html.contains("Bad &#60;script&#62;"));
        assert!(html.contains(r#"data-ts="2026-05-17T10:00:00Z&#34; onclick=&#34;bad""#));
    }

    #[test]
    fn scripts_page_renders_empty_state() {
        let page = ScriptsPage::new(Vec::new());

        let Html(html) =
            render_scripts_page("admin@example.com", &page, FlashMessages::default()).unwrap();

        assert!(html.contains("0 files"));
        assert!(html.contains("NO SCRIPTS"));
        assert!(html.contains(r#"href="/scripts/new""#));
        assert!(!html.contains("<table"));
    }

    #[test]
    fn script_editor_renders_new_and_edit_modes() {
        let new_page = ScriptEditorPage::new_script();
        let Html(new_html) =
            render_script_editor_page("admin@example.com", &new_page, FlashMessages::default())
                .unwrap();
        assert!(new_html.contains("New Script"));
        assert!(new_html.contains(r#"action="/scripts/new""#));
        assert!(new_html.contains("CREATE"));
        assert!(new_html.contains(r#"name="filename" value="" required"#));

        let edit_page = ScriptEditorPage::edit("deploy <app>.sh", "echo <ok>");
        let Html(edit_html) =
            render_script_editor_page("admin@example.com", &edit_page, FlashMessages::default())
                .unwrap();
        assert!(edit_html.contains("deploy &#60;app&#62;.sh"));
        assert!(edit_html.contains(r#"action="/scripts/deploy%20%3Capp%3E.sh""#));
        assert!(edit_html.contains(r#"action="/scripts/deploy%20%3Capp%3E.sh/delete""#));
        assert!(edit_html.contains("echo &#60;ok&#62;"));
        assert!(edit_html.contains("DELETE SCRIPT"));
        assert!(!edit_html.contains("onsubmit="));
    }
}
