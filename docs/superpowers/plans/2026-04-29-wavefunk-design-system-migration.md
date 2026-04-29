# Wave Funk Design System Migration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace sendword's Tailwind + custom `.sw-*` CSS with the Wave Funk design system — full app-shell scaffold, vendored CSS/fonts, plain JS, no build step.

**Architecture:** Big-bang rewrite of all ~14 templates against the `.wf-*` component vocabulary. Vendor the design system's CSS + fonts into `static/css/`. Replace TypeScript with a single plain JS file. Adopt the sidebar + topbar + statusbar app shell. Convert SSE log streaming from custom JS to HTMX SSE extension.

**Tech Stack:** Wave Funk CSS design system, HTMX 2.0.4 + SSE extension, MiniJinja templates, plain JS, Axum (Rust)

**Design system reference:** `../design/` — use `partials/*.html` as rendered examples for each component, `docs/components/*.md` for API details, `docs/COMPOSITION.md` for layout rules.

---

## File Structure

### Create
- `static/css/wavefunk.css` — vendored entry point (imports layer files)
- `static/css/01-tokens.css` through `static/css/06-marketing.css` — vendored layers
- `static/css/fonts/MartianGrotesk-VF.woff2` — vendored font
- `static/css/fonts/MartianMono-VF.woff2` — vendored font
- `static/js/echo.js` — vendored minibuffer runtime
- `static/js/sendword.js` — replaces `static/ts/main.ts`
- `templates/base_auth.html` — minimal layout for login + 404

### Modify
- `templates/base.html` — rewrite to app-shell scaffold
- `templates/login.html` — extend `base_auth.html`, use `.wf-*` classes
- `templates/dashboard.html` — rewrite with `.wf-table`, `.wf-tag`, `.wf-filterbar`
- `templates/hook_detail.html` — rewrite with `.wf-panel`, `.wf-dl`, `.wf-table`
- `templates/hook_form.html` — rewrite with `.wf-panel`, `.wf-field`, `.wf-input`
- `templates/execution_detail.html` — rewrite with `.wf-dl`, HTMX SSE for logs
- `templates/approvals.html` — rewrite with `.wf-table`, `.wf-empty`
- `templates/users.html` — rewrite with `.wf-table`, `.wf-field`
- `templates/scripts.html` — rewrite with `.wf-table`
- `templates/script_editor.html` — rewrite with `.wf-panel`, `.wf-field`
- `templates/password.html` — rewrite with `.wf-panel`, `.wf-field`
- `templates/404.html` — extend `base_auth.html`, use `.wf-empty`
- `templates/partials/execution_list.html` — rewrite with `.wf-table`, `.wf-filterbar`
- `templates/partials/trigger_attempt_list.html` — rewrite with `.wf-table`
- `justfile` — remove CSS/TS recipes, simplify dev/build, add vendor-design
- `src/routes/executions.rs` — HTML-escape SSE data, emit HTML badge on done event

### Delete
- `tailwind.config.js`
- `static/css/src/app.css`
- `static/dist/app.css`
- `static/dist/main.js`
- `static/dist/main.js.map`
- `static/ts/main.ts`
- `static/ts/tsconfig.json`

---

## Milestone 1: Infrastructure

### Task 1: Vendor design system assets

**Files:**
- Create: `static/css/wavefunk.css`, `static/css/01-tokens.css` through `06-marketing.css`, `static/css/fonts/*.woff2`
- Create: `static/js/echo.js`

- [ ] **Step 1: Copy CSS and fonts from design system**

```bash
cp ../design/css/wavefunk.css static/css/wavefunk.css
cp ../design/css/01-tokens.css static/css/01-tokens.css
cp ../design/css/02-base.css static/css/02-base.css
cp ../design/css/03-layout.css static/css/03-layout.css
cp ../design/css/04-components.css static/css/04-components.css
cp ../design/css/05-utilities.css static/css/05-utilities.css
cp ../design/css/06-marketing.css static/css/06-marketing.css
mkdir -p static/css/fonts
cp ../design/css/fonts/MartianGrotesk-VF.woff2 static/css/fonts/
cp ../design/css/fonts/MartianMono-VF.woff2 static/css/fonts/
```

- [ ] **Step 2: Copy echo.js**

```bash
mkdir -p static/js
cp ../design/js/echo.js static/js/echo.js
```

- [ ] **Step 3: Verify files are in place**

```bash
ls -la static/css/wavefunk.css static/css/01-tokens.css static/css/fonts/*.woff2 static/js/echo.js
```

Expected: all files present with non-zero sizes.

- [ ] **Step 4: Commit**

```bash
git add static/css/ static/js/echo.js
git commit -m "feat: vendor Wave Funk design system CSS, fonts, and echo.js"
```

---

### Task 2: Write sendword.js

**Files:**
- Create: `static/js/sendword.js`

- [ ] **Step 1: Create sendword.js**

```js
// Popover toggles — click trigger to open, click outside to close.
document.addEventListener('click', e => {
  const trigger = e.target.closest('[data-popover-toggle]');
  if (trigger) {
    const anchor = trigger.closest('.wf-pop-anchor');
    const pop = anchor && anchor.querySelector('.wf-popover');
    if (pop) {
      const wasOpen = pop.classList.contains('is-open');
      document.querySelectorAll('.wf-popover.is-open').forEach(p => p.classList.remove('is-open'));
      if (!wasOpen) pop.classList.add('is-open');
    }
    return;
  }
  if (!e.target.closest('.wf-popover')) {
    document.querySelectorAll('.wf-popover.is-open').forEach(p => p.classList.remove('is-open'));
  }
});

// Toast listener — wfToast custom events from htmx HX-Trigger headers.
// Uses textContent for the message to avoid XSS from server-provided strings.
document.body.addEventListener('wfToast', e => {
  const { kind = '', msg = '' } = e.detail || {};
  const host = document.getElementById('toast-host');
  if (!host) return;
  const t = document.createElement('div');
  t.className = 'wf-toast' + (kind ? ' ' + kind : '');
  const dot = document.createElement('span');
  dot.className = 'wf-dot';
  const span = document.createElement('span');
  span.textContent = msg;
  t.appendChild(dot);
  t.appendChild(span);
  host.appendChild(t);
  setTimeout(() => {
    t.style.opacity = '0';
    t.style.transition = 'opacity 200ms';
    setTimeout(() => t.remove(), 220);
  }, 2600);
});

// Relative timestamps — format [data-ts] elements as "3m ago", "2h ago", etc.
function formatTimestamps() {
  document.querySelectorAll('[data-ts]').forEach(el => {
    const iso = el.getAttribute('data-ts');
    if (!iso) return;
    const diff = Date.now() - new Date(iso).getTime();
    const secs = Math.floor(diff / 1000);
    let text;
    if (secs < 60) text = 'just now';
    else if (secs < 3600) text = Math.floor(secs / 60) + 'm ago';
    else if (secs < 86400) text = Math.floor(secs / 3600) + 'h ago';
    else if (secs < 604800) text = Math.floor(secs / 86400) + 'd ago';
    else text = new Date(iso).toLocaleDateString();
    el.textContent = text;
  });
}

document.addEventListener('DOMContentLoaded', formatTimestamps);
document.body.addEventListener('htmx:afterSettle', formatTimestamps);
```

- [ ] **Step 2: Verify file exists**

```bash
wc -l static/js/sendword.js
```

Expected: ~55 lines.

- [ ] **Step 3: Commit**

```bash
git add static/js/sendword.js
git commit -m "feat: add sendword.js — popovers, toasts, timestamps"
```

---

### Task 3: Update justfile

**Files:**
- Modify: `justfile`

- [ ] **Step 1: Rewrite justfile**

Keep: `default`, `run`, `check`, `test`, `clippy`, `fmt`, `watch`, `migrate`, `migrate-new`, `sqlx-prepare`, `sqlx-reset`.

Remove: `npm-install`, `build-css`, `watch-css`, `build-ts`, `build-ts-dev`, `watch-ts`.

Replace `dev` with:

```just
dev:
    cargo run
```

Replace `build` with:

```just
build:
    cargo build --release
```

Add `vendor-design` recipe:

```just
vendor-design:
    cp ../design/css/wavefunk.css static/css/wavefunk.css
    cp ../design/css/01-tokens.css static/css/01-tokens.css
    cp ../design/css/02-base.css static/css/02-base.css
    cp ../design/css/03-layout.css static/css/03-layout.css
    cp ../design/css/04-components.css static/css/04-components.css
    cp ../design/css/05-utilities.css static/css/05-utilities.css
    cp ../design/css/06-marketing.css static/css/06-marketing.css
    cp ../design/css/fonts/MartianGrotesk-VF.woff2 static/css/fonts/
    cp ../design/css/fonts/MartianMono-VF.woff2 static/css/fonts/
    cp ../design/js/echo.js static/js/echo.js
```

- [ ] **Step 2: Verify justfile parses**

```bash
just --list
```

Expected: lists all recipes without errors. No `build-css`, `watch-css`, `build-ts`, `watch-ts`, `npm-install` in output.

- [ ] **Step 3: Commit**

```bash
git add justfile
git commit -m "chore: simplify justfile — remove Tailwind/TS build recipes, add vendor-design"
```

---

## Milestone 2: Base Templates

### Task 4: Rewrite base.html (app shell)

**Files:**
- Modify: `templates/base.html`

**Reference:** `../design/templates/app-shell.html` for the grid layout and sidebar/topbar structure.

- [ ] **Step 1: Rewrite base.html**

```html
<!doctype html>
<html lang="en" data-mode="dark">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{% block title %}sendword{% endblock %}</title>
  <link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect x='2' y='6' width='28' height='20' rx='3' fill='%230c0a09' stroke='%23f59e0b' stroke-width='2'/%3E%3Cpath d='M2 8l14 10 14-10' fill='none' stroke='%23f59e0b' stroke-width='2' stroke-linecap='round'/%3E%3Ccircle cx='26' cy='8' r='5' fill='%23ef4444'/%3E%3C/svg%3E">
  <link rel="stylesheet" href="/static/css/wavefunk.css">
  <script src="https://unpkg.com/htmx.org@2.0.4" defer></script>
  <script src="https://unpkg.com/htmx-ext-sse@2.2.2/sse.js" defer></script>
  <script src="/static/js/sendword.js" defer></script>
  <style>
    body { overflow: hidden; height: 100vh; }
    .app {
      display: grid;
      grid-template-columns: 240px 1fr;
      grid-template-rows: 56px 1fr 28px;
      grid-template-areas:
        "side   top"
        "side   main"
        "status status";
      height: 100vh;
    }
    .app > .wf-sidebar   { grid-area: side; }
    .app > .wf-topbar    { grid-area: top; border-bottom: 1px solid var(--hairline); }
    .app > main          { grid-area: main; overflow: auto; background: var(--bg); }
    .app > .wf-statusbar { grid-area: status; }
    .page     { padding: 28px 32px 96px; max-width: 1200px; }
    .page-head { display: flex; align-items: baseline; gap: 16px; margin-bottom: 4px; }
    .page-title {
      font-family: var(--font-mono); font-size: 28px; font-weight: 800;
      letter-spacing: -0.02em; text-transform: uppercase;
      color: var(--fg-strong); margin: 0;
    }
    .page-sub { color: var(--fg-muted); font-size: 13px; margin: 0 0 28px; }
  </style>
</head>
<body class="density-dense" hx-boost="true">

<div class="app">

  <aside class="wf-sidebar">
    <div class="wf-brand">
      <div style="width: 22px; height: 22px; background: var(--accent); color: var(--accent-ink); display: inline-flex; align-items: center; justify-content: center; font-family: var(--font-mono); font-weight: 800; font-size: 13px;">S</div>
      <span class="wf-brand-name">SENDWORD</span>
    </div>
    <nav class="wf-nav-list">
      <div class="wf-nav-section">HOOKS</div>
      <a class="wf-nav-item{{ ' is-active' if nav_active == 'dashboard' }}" href="/">▸ Hooks</a>
      <a class="wf-nav-item{{ ' is-active' if nav_active == 'approvals' }}" href="/approvals">▸ Approvals</a>
      <div class="wf-nav-section">TOOLS</div>
      <a class="wf-nav-item{{ ' is-active' if nav_active == 'scripts' }}" href="/scripts">▸ Scripts</a>
      <div class="wf-nav-section">ADMIN</div>
      <a class="wf-nav-item{{ ' is-active' if nav_active == 'settings' }}" href="/settings/users">▸ Users</a>
    </nav>
    <div class="wf-pop-anchor" style="display: block;">
      <button class="wf-user" data-popover-toggle>
        <div class="wf-avatar accent">{{ username | first | upper }}</div>
        <div class="wf-user-id">
          <div class="wf-user-name">{{ username }}</div>
        </div>
        <span class="wf-user-caret">▴</span>
      </button>
      <div class="wf-popover" data-side="top" style="left: 8px; right: 8px;">
        <div class="wf-popover-head">{{ username | upper }}</div>
        <div class="wf-menu">
          <a class="wf-menu-item" href="/settings/password">Change password</a>
          <div class="wf-menu-sep"></div>
          <a class="wf-menu-item danger" href="/logout">Log out</a>
        </div>
      </div>
    </div>
  </aside>

  <header class="wf-topbar">
    {% block crumbs %}
    <div class="wf-crumbs">
      <span aria-current="page">SENDWORD</span>
    </div>
    {% endblock %}
    <div style="flex: 1;"></div>
    {% block actions %}{% endblock %}
  </header>

  <main>
    <div class="page">
      {% if success %}<div class="wf-alert ok" style="margin-bottom: 20px;">{{ success }}</div>{% endif %}
      {% if error %}<div class="wf-alert err" style="margin-bottom: 20px;">{{ error }}</div>{% endif %}
      {% block content %}{% endblock %}
    </div>
  </main>

  <div class="wf-statusbar" style="border-top: 1px solid var(--hairline);">
    <span>SENDWORD</span>
    <span style="flex: 1;"></span>
    <span>V1.0</span>
  </div>

</div>

<div class="wf-toast-host" id="toast-host"></div>

</body>
</html>
```

- [ ] **Step 2: Verify template compiles**

```bash
cargo check
```

Expected: no errors (MiniJinja embed picks up the new template).

- [ ] **Step 3: Commit**

```bash
git add templates/base.html
git commit -m "feat: rewrite base.html to Wave Funk app-shell scaffold"
```

---

### Task 5: Create base_auth.html + rewrite login.html + rewrite 404.html

**Files:**
- Create: `templates/base_auth.html`
- Modify: `templates/login.html`
- Modify: `templates/404.html`

**Reference:** `../design/docs/components/layout.md` for `.wf-auth-top`.

- [ ] **Step 1: Create base_auth.html**

Minimal document for unauthenticated pages — no app shell, just centered content.

```html
<!doctype html>
<html lang="en" data-mode="dark">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{% block title %}sendword{% endblock %}</title>
  <link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect x='2' y='6' width='28' height='20' rx='3' fill='%230c0a09' stroke='%23f59e0b' stroke-width='2'/%3E%3Cpath d='M2 8l14 10 14-10' fill='none' stroke='%23f59e0b' stroke-width='2' stroke-linecap='round'/%3E%3Ccircle cx='26' cy='8' r='5' fill='%23ef4444'/%3E%3C/svg%3E">
  <link rel="stylesheet" href="/static/css/wavefunk.css">
  <style>
    body { min-height: 100vh; display: flex; align-items: center; justify-content: center; background: var(--bg); }
  </style>
</head>
<body class="density-dense">
  {% block content %}{% endblock %}
</body>
</html>
```

- [ ] **Step 2: Rewrite login.html**

```html
{% extends "base_auth.html" %}

{% block title %}sendword — login{% endblock %}

{% block content %}
<div style="width: 100%; max-width: 380px; padding: 0 16px;">
  <div style="text-align: center; margin-bottom: 32px;">
    <div style="display: inline-flex; align-items: center; gap: 10px;">
      <div style="width: 28px; height: 28px; background: var(--accent); color: var(--accent-ink); display: inline-flex; align-items: center; justify-content: center; font-family: var(--font-mono); font-weight: 800; font-size: 16px;">S</div>
      <span style="font-family: var(--font-mono); font-size: 18px; font-weight: 800; text-transform: uppercase; letter-spacing: 0.04em; color: var(--fg-strong);">SENDWORD</span>
    </div>
  </div>

  {% if error %}
  <div class="wf-alert err" style="margin-bottom: 20px;">{{ error }}</div>
  {% endif %}

  <form method="post" action="/login">
    <div class="wf-field" style="margin-bottom: 16px;">
      <label for="email">EMAIL</label>
      <input class="wf-input" type="email" id="email" name="email" autocomplete="email" required>
    </div>
    <div class="wf-field" style="margin-bottom: 24px;">
      <label for="password">PASSWORD</label>
      <input class="wf-input" type="password" id="password" name="password" autocomplete="current-password" required>
    </div>
    <button type="submit" class="wf-btn primary" style="width: 100%; justify-content: center;">SIGN IN</button>
  </form>
</div>
{% endblock %}
```

- [ ] **Step 3: Rewrite 404.html**

`404.html` extends `base_auth.html` because the fallback handler has no auth context.

```html
{% extends "base_auth.html" %}

{% block title %}sendword — 404{% endblock %}

{% block content %}
<div class="wf-empty">
  <div class="wf-empty-title">PAGE NOT FOUND</div>
  <div class="wf-empty-msg">The page you're looking for doesn't exist.</div>
  <a class="wf-btn sm" href="/">BACK TO DASHBOARD</a>
</div>
{% endblock %}
```

- [ ] **Step 4: Verify templates compile**

```bash
cargo check
```

- [ ] **Step 5: Start dev server and verify login page and 404**

```bash
just dev
```

Open `http://localhost:<port>/login` — verify centered form with Martian fonts, dark background, `.wf-input` styling.
Open `http://localhost:<port>/nonexistent` — verify centered empty state.

- [ ] **Step 6: Commit**

```bash
git add templates/base_auth.html templates/login.html templates/404.html
git commit -m "feat: create base_auth.html, rewrite login and 404 with Wave Funk"
```

---

## Milestone 3: Page Templates

For each template below, reference `../design/partials/*.html` for rendered component examples. The class mappings follow this table:

| Current `.sw-*` | Wave Funk `.wf-*` | Notes |
|---|---|---|
| `.sw-card` | `.wf-panel` | Use `.wf-panel-head` + `.wf-panel-title` for headers |
| `.sw-btn .sw-btn-primary` | `.wf-btn primary` | Add `sm` for small |
| `.sw-btn .sw-btn-secondary` | `.wf-btn` | Default is secondary/ghost |
| `.sw-btn .sw-btn-danger` | `.wf-btn danger` | |
| `.sw-badge` | `.wf-tag` | Add `ok`/`err`/`warn`/`info`/`accent` |
| `.sw-label` | Wave Funk labels are uppercase by default in `.wf-field > label` | |
| `.sw-input` | `.wf-input` | Wrap in `.wf-field` with `<label>` |
| `.sw-alert .sw-alert-error` | `.wf-alert err` | Also: `ok`, `warn`, `info` |
| `.sw-table` | `.wf-table` | Add `.is-interactive` for hover rows |
| `.sw-dot` | `.wf-dot` | Set color via `style="color: var(--ok)"` etc. |
| `.sw-filter-btn` | `.wf-seg-opt` inside `.wf-seg` | |
| `.sw-pre` | `<pre>` (base styles handle it) | |
| Tailwind `text-sw-amber` | `var(--accent)` | |
| Tailwind `text-sw-fg` | `var(--fg)` | |
| Tailwind status colors | `var(--ok)`, `var(--err)`, `var(--warn)`, `var(--info)` | |

### Task 6: Rewrite dashboard.html

**Files:**
- Modify: `templates/dashboard.html`

**Reference:** `../design/partials/table.html` for `.wf-table` patterns. `../design/partials/htmx.html` for integration examples.

**Context variables available:** `hooks` (array: `name`, `slug`, `description`, `enabled`, `last_status`, `last_triggered_at`, `last_execution_id`, `recent_statuses`), `success`, `error`, `username`, `nav_active`.

- [ ] **Step 1: Rewrite dashboard.html**

```html
{% extends "base.html" %}

{% block title %}sendword — hooks{% endblock %}

{% block crumbs %}
<div class="wf-crumbs">
  <span aria-current="page">HOOKS</span>
</div>
{% endblock %}

{% block actions %}
<a class="wf-btn sm primary" href="/hooks/new">+ NEW HOOK</a>
{% endblock %}

{% block content %}
<div class="page-head">
  <h1 class="page-title">Hooks</h1>
</div>
<p class="page-sub">{{ hooks | length }} hook{{ 's' if hooks | length != 1 }} configured</p>

{% if hooks | length == 0 %}
<div class="wf-empty">
  <div class="wf-empty-title">NO HOOKS YET</div>
  <div class="wf-empty-msg">Create your first webhook hook to get started.</div>
  <a class="wf-btn sm primary" href="/hooks/new">+ NEW HOOK</a>
</div>
{% else %}
<div class="wf-panel">
  <div class="wf-panel-head">
    <span class="wf-panel-title">ALL HOOKS</span>
    <div class="wf-input-group" style="max-width: 240px;">
      <span class="wf-input-addon">&#x2315;</span>
      <input class="wf-input sm" placeholder="Filter hooks…" id="hook-filter" oninput="filterHooks(this.value)">
    </div>
  </div>
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
      {% for hook in hooks %}
      <tr data-hook-name="{{ hook.name | lower }}" data-hook-slug="{{ hook.slug }}" onclick="location.href='/hooks/{{ hook.slug }}'">
        <td class="strong">{{ hook.name }}</td>
        <td><code>{{ hook.slug }}</code></td>
        <td>
          {% if hook.enabled %}
            <span class="wf-tag ok"><span class="dot"></span>ENABLED</span>
          {% else %}
            <span class="wf-tag"><span class="dot"></span>DISABLED</span>
          {% endif %}
        </td>
        <td>
          {% for status in hook.recent_statuses %}
            <span class="wf-dot" style="color: var({% if status == 'success' %}--ok{% elif status == 'failed' %}--err{% elif status == 'running' %}--warn{% else %}--fg-muted{% endif %});"></span>
          {% endfor %}
        </td>
        <td>
          {% if hook.last_triggered_at %}
            <span data-ts="{{ hook.last_triggered_at }}">{{ hook.last_triggered_at }}</span>
          {% else %}
            <span class="muted">—</span>
          {% endif %}
        </td>
      </tr>
      {% endfor %}
    </tbody>
  </table>
</div>
{% endif %}

<script>
function filterHooks(query) {
  var q = query.toLowerCase();
  document.querySelectorAll('#hook-list tr').forEach(function(row) {
    var name = row.getAttribute('data-hook-name') || '';
    var slug = row.getAttribute('data-hook-slug') || '';
    row.style.display = (name.includes(q) || slug.includes(q)) ? '' : 'none';
  });
}
</script>
{% endblock %}
```

- [ ] **Step 2: Verify**

```bash
cargo check
```

Then start the dev server and open the dashboard. Verify hooks table renders with Wave Funk styling, filter works, status tags show correct colors.

- [ ] **Step 3: Commit**

```bash
git add templates/dashboard.html
git commit -m "feat: rewrite dashboard.html with Wave Funk table and tag components"
```

---

### Task 7: Rewrite hook_detail.html + execution_list partial + trigger_attempt_list partial

**Files:**
- Modify: `templates/hook_detail.html`
- Modify: `templates/partials/execution_list.html`
- Modify: `templates/partials/trigger_attempt_list.html`

**Reference:** `../design/partials/table.html`, `../design/docs/components/dl.md` for definition lists, `../design/partials/htmx.html`.

**Context variables (hook_detail):** `name`, `slug`, `enabled`, `description`, `executor_type`, `executor_command`, `script_edit_url`, `cwd`, `timeout_secs`, `env_vars`, `auth_mode`, `auth_algorithm`, `auth_header`, `payload_fields`, `trigger_filter_rows`, `trigger_window_rows`, `trigger_cooldown`, `trigger_rate_max`, `trigger_rate_window`, `total`, `success`, `error`, `username`, `nav_active`.

**Context variables (execution_list):** `slug`, `executions`, `active_status`, `active_from`, `active_to`, `page`, `total_pages`, `has_more`. Each execution: `id`, `triggered_at`, `status`, `exit_code`, `duration`.

**Context variables (trigger_attempt_list):** `slug`, `attempts`, `active_status`. Each attempt: `attempted_at`, `status`, `source_ip`, `reason`, `execution_id`.

- [ ] **Step 1: Rewrite hook_detail.html**

Structure: breadcrumbs (HOOKS / slug), page title with enable/edit/delete actions. Then panels for config sections, followed by execution and trigger attempt lists loaded via HTMX.

Key layout:
```html
{% extends "base.html" %}
{% block title %}sendword — {{ name }}{% endblock %}

{% block crumbs %}
<div class="wf-crumbs">
  <a href="/">HOOKS</a><span class="sep">/</span>
  <span aria-current="page">{{ slug | upper }}</span>
</div>
{% endblock %}

{% block actions %}
<a class="wf-btn sm" href="/hooks/{{ slug }}/edit">EDIT</a>
<form method="post" action="/hooks/{{ slug }}/delete" style="display: inline;" onsubmit="return confirm('Delete hook {{ name }}?');">
  <button type="submit" class="wf-btn sm danger">DELETE</button>
</form>
{% endblock %}
```

Content area: `.page-head` with title + enabled/disabled tag. Then use `.wf-panel` blocks with `.wf-dl` inside for each config section:

- **Overview panel:** `.wf-dl` rows for slug, description, endpoint URL, status
- **Executor panel:** `.wf-dl` rows for type, command (with link to script editor if `script_edit_url`), cwd, timeout, env vars
- **Authentication panel** (if `auth_mode != "none"`): `.wf-dl` rows for mode, header, algorithm
- **Payload schema panel** (if `payload_fields`): `.wf-table` with name/type/required columns
- **Trigger rules panel** (if any trigger config): `.wf-dl` rows for cooldown, rate limits; `.wf-table` for filter rows and window rows
- **Executions panel:** `<div id="execution-list" hx-get="/hooks/{{ slug }}/executions" hx-trigger="load" hx-swap="innerHTML"></div>`
- **Trigger attempts panel:** `<div id="attempt-list" hx-get="/hooks/{{ slug }}/trigger-attempts" hx-trigger="load" hx-swap="innerHTML"></div>` with filter buttons

For the trigger attempt filter buttons, use `.wf-seg` with `.wf-seg-opt` buttons that have `hx-get` attributes to reload the attempt list filtered by status.

Preserve all existing conditionals and variable references from the current template. Map every Tailwind class and `.sw-*` class to its `.wf-*` equivalent per the mapping table above. Reference `../design/partials/sidebar.html` for `.wf-dl` row patterns.

- [ ] **Step 2: Rewrite execution_list.html partial**

Structure: filter bar with status select and date inputs, then table, then pagination.

```html
<div class="wf-filterbar" style="margin-bottom: 12px;">
  <select class="wf-select sm" name="status"
    hx-get="/hooks/{{ slug }}/executions" hx-target="#execution-list" hx-swap="innerHTML"
    hx-include="[name='from'],[name='to']">
    <option value="">ALL STATUS</option>
    <option value="success" {{ 'selected' if active_status == 'success' }}>SUCCESS</option>
    <option value="failed" {{ 'selected' if active_status == 'failed' }}>FAILED</option>
    <option value="running" {{ 'selected' if active_status == 'running' }}>RUNNING</option>
    <option value="pending" {{ 'selected' if active_status == 'pending' }}>PENDING</option>
    <option value="pending_approval" {{ 'selected' if active_status == 'pending_approval' }}>PENDING APPROVAL</option>
  </select>
  <input class="wf-input sm" type="date" name="from" value="{{ active_from }}"
    hx-get="/hooks/{{ slug }}/executions" hx-target="#execution-list" hx-swap="innerHTML"
    hx-include="[name='status'],[name='to']" hx-trigger="change">
  <input class="wf-input sm" type="date" name="to" value="{{ active_to }}"
    hx-get="/hooks/{{ slug }}/executions" hx-target="#execution-list" hx-swap="innerHTML"
    hx-include="[name='status'],[name='from']" hx-trigger="change">
</div>

{% if executions | length == 0 %}
<div class="wf-empty">
  <div class="wf-empty-msg">No executions{% if active_status %} matching "{{ active_status }}"{% endif %}.</div>
</div>
{% else %}
<table class="wf-table is-interactive">
  <thead>
    <tr>
      <th>ID</th>
      <th>TRIGGERED</th>
      <th>STATUS</th>
      <th class="num">EXIT</th>
      <th class="num">DURATION</th>
    </tr>
  </thead>
  <tbody>
    {% for exec in executions %}
    <tr onclick="location.href='/executions/{{ exec.id }}'">
      <td class="strong"><code>{{ exec.id[:8] }}</code></td>
      <td><span data-ts="{{ exec.triggered_at }}">{{ exec.triggered_at }}</span></td>
      <td>
        <span class="wf-tag {% if exec.status == 'success' %}ok{% elif exec.status == 'failed' %}err{% elif exec.status == 'running' %}warn{% endif %}">
          <span class="dot"></span>{{ exec.status | upper }}
        </span>
      </td>
      <td class="num">{{ exec.exit_code if exec.exit_code is defined else '—' }}</td>
      <td class="num">{{ exec.duration if exec.duration else '—' }}</td>
    </tr>
    {% endfor %}
  </tbody>
</table>
{% if has_more %}
<div class="wf-tablefoot">
  <span>PAGE {{ page }} OF {{ total_pages }}</span>
  <span style="flex: 1;"></span>
  <button class="wf-btn sm"
    hx-get="/hooks/{{ slug }}/executions?page={{ page + 1 }}&status={{ active_status }}&from={{ active_from }}&to={{ active_to }}"
    hx-target="#execution-list" hx-swap="innerHTML">
    LOAD MORE
  </button>
</div>
{% endif %}
{% endif %}
```

- [ ] **Step 3: Rewrite trigger_attempt_list.html partial**

Same pattern — `.wf-table` with status tags. Map attempt statuses to tag colors:
- `fired` → `.wf-tag ok`
- `auth_failed` → `.wf-tag err`
- `validation_failed` → `.wf-tag err`
- `filtered` → `.wf-tag`
- `rate_limited` → `.wf-tag warn`
- `schedule_skipped` → `.wf-tag`
- `cooldown_skipped` → `.wf-tag`

```html
{% if attempts | length == 0 %}
<div class="wf-empty">
  <div class="wf-empty-msg">No trigger attempts{% if active_status %} matching "{{ active_status }}"{% endif %}.</div>
</div>
{% else %}
<table class="wf-table">
  <thead>
    <tr>
      <th>TIME</th>
      <th>STATUS</th>
      <th>SOURCE IP</th>
      <th>REASON</th>
      <th>EXECUTION</th>
    </tr>
  </thead>
  <tbody>
    {% for attempt in attempts %}
    <tr>
      <td><span data-ts="{{ attempt.attempted_at }}">{{ attempt.attempted_at }}</span></td>
      <td>
        <span class="wf-tag {% if attempt.status == 'fired' %}ok{% elif attempt.status == 'auth_failed' or attempt.status == 'validation_failed' %}err{% elif attempt.status == 'rate_limited' %}warn{% endif %}">
          <span class="dot"></span>{{ attempt.status | upper }}
        </span>
      </td>
      <td><code>{{ attempt.source_ip }}</code></td>
      <td>{{ attempt.reason if attempt.reason else '—' }}</td>
      <td>
        {% if attempt.execution_id %}
          <a href="/executions/{{ attempt.execution_id }}"><code>{{ attempt.execution_id[:8] }}</code></a>
        {% else %}
          —
        {% endif %}
      </td>
    </tr>
    {% endfor %}
  </tbody>
</table>
{% endif %}
```

- [ ] **Step 4: Verify and test**

```bash
cargo check
```

Start dev server, navigate to a hook detail page. Verify: panels render, DL rows display config, execution list loads via HTMX, trigger attempts load, filter buttons work.

- [ ] **Step 5: Commit**

```bash
git add templates/hook_detail.html templates/partials/execution_list.html templates/partials/trigger_attempt_list.html
git commit -m "feat: rewrite hook detail and list partials with Wave Funk panels and tables"
```

---

### Task 8: Rewrite hook_form.html

**Files:**
- Modify: `templates/hook_form.html`

**Reference:** `../design/docs/form-layouts.md` for the section-grouped form pattern. `../design/partials/forms.html` for input examples.

**Context variables:** `is_new`, `form_name`, `form_slug`, `form_description`, `form_enabled`, `form_auth_mode`, `form_auth_token`, `form_auth_header`, `form_auth_algorithm`, `form_auth_secret`, `form_command`, `form_cwd`, `form_timeout`, `form_env_text`, `form_payload_text`, `form_trigger_filters_text`, `form_trigger_windows_text`, `form_trigger_cooldown`, `form_trigger_rate_max`, `form_trigger_rate_window`, `form_retry_count`, `form_retry_backoff`, `form_retry_initial_delay`, `form_retry_max_delay`, `success`, `error`, `username`, `nav_active`, `slug` (edit only).

- [ ] **Step 1: Rewrite hook_form.html**

Structure: breadcrumbs, page title, form with `.wf-panel` sections. Use the design system's section-grouped form layout (sections separated by panel boundaries).

```html
{% extends "base.html" %}

{% block title %}sendword — {% if is_new %}new hook{% else %}edit {{ form_name }}{% endif %}{% endblock %}

{% block crumbs %}
<div class="wf-crumbs">
  <a href="/">HOOKS</a><span class="sep">/</span>
  {% if is_new %}
  <span aria-current="page">NEW</span>
  {% else %}
  <a href="/hooks/{{ slug }}">{{ slug | upper }}</a><span class="sep">/</span>
  <span aria-current="page">EDIT</span>
  {% endif %}
</div>
{% endblock %}
```

Form body: `<form method="post" action="{% if is_new %}/hooks/new{% else %}/hooks/{{ slug }}/edit{% endif %}">` containing `.wf-panel` sections:

**Section 1 — Basic Info panel:**
- `.wf-field` for name (`.wf-input`, required)
- `.wf-field` for slug (`.wf-input`, required, `{% if not is_new %}readonly{% endif %}`)
- `.wf-field` for description (`.wf-textarea`)
- `.wf-check-row` for enabled (`.wf-switch`)

**Section 2 — Authentication panel:**
- `.wf-field` for auth_mode (`.wf-select` with options: none, bearer, hmac)
- Conditional `.wf-field`s wrapped in `<div id="bearer-fields">` and `<div id="hmac-fields">` for bearer token, HMAC header/algorithm/secret (shown/hidden by JS)

**Section 3 — Executor panel:**
- `.wf-field` for command (`.wf-input`, required)
- `.wf-field` for cwd (`.wf-input`)
- `.wf-field` for timeout (`.wf-input`, type=number)
- `.wf-field` for environment (`.wf-textarea`, placeholder: `KEY=VALUE\nKEY2=VALUE2`)

**Section 4 — Payload Schema panel:**
- `.wf-field` for payload_text (`.wf-textarea`, placeholder: `name:type:required`)

**Section 5 — Trigger Rules panel:**
- `.wf-field` for filters_text (`.wf-textarea`)
- `.wf-field` for windows_text (`.wf-textarea`)
- `.wf-field` for cooldown (`.wf-input`, type=number)
- Two `.wf-field`s side by side for rate_max and rate_window

**Section 6 — Retry panel:**
- `.wf-field` for retry_count (`.wf-input`, type=number)
- `.wf-field` for retry_backoff (`.wf-select`: none, linear, exponential)
- Two `.wf-field`s for initial_delay and max_delay

**Bottom buttons:**
```html
<div style="display: flex; gap: 12px; justify-content: flex-end; margin-top: 24px;">
  <a class="wf-btn" href="{% if is_new %}/{% else %}/hooks/{{ slug }}{% endif %}">CANCEL</a>
  <button type="submit" class="wf-btn primary">{% if is_new %}CREATE HOOK{% else %}SAVE CHANGES{% endif %}</button>
</div>
```

**Inline JS for auth field toggling:**
```html
<script>
function toggleAuthFields() {
  var mode = document.getElementById('auth_mode').value;
  document.getElementById('bearer-fields').style.display = mode === 'bearer' ? '' : 'none';
  document.getElementById('hmac-fields').style.display = mode === 'hmac' ? '' : 'none';
}
document.addEventListener('DOMContentLoaded', toggleAuthFields);
</script>
```

Preserve ALL existing form field `name` attributes, values, and conditionals from the current template. Only change the markup and CSS classes.

- [ ] **Step 2: Verify**

```bash
cargo check
```

Start dev server, navigate to new hook form and edit hook form. Verify: all sections render, auth field visibility toggles, form submits correctly.

- [ ] **Step 3: Commit**

```bash
git add templates/hook_form.html
git commit -m "feat: rewrite hook form with Wave Funk panels and form fields"
```

---

### Task 9: Rewrite execution_detail.html

**Files:**
- Modify: `templates/execution_detail.html`

**Reference:** `../design/partials/htmx.html` for HTMX integration patterns.

**Context variables:** `id`, `status`, `hook_slug`, `exit_code`, `trigger_source`, `triggered_at`, `started_at`, `completed_at`, `duration`, `retry_count`, `retry_of`, `stdout`, `stderr`, `username`, `nav_active`.

- [ ] **Step 1: Rewrite execution_detail.html**

Key change: replace custom EventSource JS with HTMX SSE extension for log streaming.

```html
{% extends "base.html" %}

{% block title %}sendword — execution {{ id[:8] }}{% endblock %}

{% block crumbs %}
<div class="wf-crumbs">
  <a href="/">HOOKS</a><span class="sep">/</span>
  <a href="/hooks/{{ hook_slug }}">{{ hook_slug | upper }}</a><span class="sep">/</span>
  <span aria-current="page">{{ id[:8] | upper }}</span>
</div>
{% endblock %}

{% block actions %}
<form method="post" style="display: inline;"
  hx-post="/executions/{{ id }}/replay" hx-swap="none"
  hx-on::after-request="if(event.detail.successful){var d=JSON.parse(event.detail.xhr.responseText);location.href='/executions/'+d.id;}">
  <button type="submit" class="wf-btn sm">REPLAY</button>
</form>
{% endblock %}

{% block content %}
<div class="page-head">
  <h1 class="page-title">Execution {{ id[:8] }}</h1>
  <span id="exec-status" class="wf-tag {% if status == 'success' %}ok{% elif status == 'failed' %}err{% elif status == 'running' %}warn{% endif %}">
    <span class="dot"></span>{{ status | upper }}
  </span>
</div>

<div class="wf-panel" style="margin-bottom: 20px;">
  <div class="wf-panel-head"><span class="wf-panel-title">DETAILS</span></div>
  <div class="wf-panel-body">
    <dl class="wf-dl flush">
      <div class="wf-dl-row"><dt>ID</dt><dd><code>{{ id }}</code></dd></div>
      <div class="wf-dl-row"><dt>HOOK</dt><dd><a href="/hooks/{{ hook_slug }}">{{ hook_slug }}</a></dd></div>
      {% if exit_code is defined %}<div class="wf-dl-row"><dt>EXIT CODE</dt><dd>{{ exit_code }}</dd></div>{% endif %}
      <div class="wf-dl-row"><dt>SOURCE</dt><dd>{{ trigger_source }}</dd></div>
      <div class="wf-dl-row"><dt>TRIGGERED</dt><dd><span data-ts="{{ triggered_at }}">{{ triggered_at }}</span></dd></div>
      {% if started_at %}<div class="wf-dl-row"><dt>STARTED</dt><dd><span data-ts="{{ started_at }}">{{ started_at }}</span></dd></div>{% endif %}
      {% if completed_at %}<div class="wf-dl-row"><dt>COMPLETED</dt><dd><span data-ts="{{ completed_at }}">{{ completed_at }}</span></dd></div>{% endif %}
      {% if duration %}<div class="wf-dl-row"><dt>DURATION</dt><dd>{{ duration }}</dd></div>{% endif %}
      {% if retry_count and retry_count > 0 %}
      <div class="wf-dl-row"><dt>RETRY</dt><dd>#{{ retry_count }}{% if retry_of %} of <a href="/executions/{{ retry_of }}">{{ retry_of[:8] }}</a>{% endif %}</dd></div>
      {% endif %}
    </dl>
  </div>
</div>

{% if status == 'running' %}
<div hx-ext="sse" sse-connect="/executions/{{ id }}/logs/stream">
  <div class="wf-panel" style="margin-bottom: 20px;">
    <div class="wf-panel-head"><span class="wf-panel-title">STDOUT</span></div>
    <div class="wf-panel-body">
      <pre id="log-stdout" sse-swap="stdout" hx-swap="beforeend" style="min-height: 80px; max-height: 600px; overflow: auto; margin: 0;"></pre>
    </div>
  </div>
  <div class="wf-panel" style="margin-bottom: 20px;">
    <div class="wf-panel-head"><span class="wf-panel-title">STDERR</span></div>
    <div class="wf-panel-body">
      <pre id="log-stderr" sse-swap="stderr" hx-swap="beforeend" style="min-height: 40px; max-height: 400px; overflow: auto; margin: 0;"></pre>
    </div>
  </div>
  <div sse-swap="done" hx-swap="innerHTML" style="display: none;"
    hx-on::after-settle="setTimeout(function(){location.reload()},500)"></div>
</div>
{% else %}
<div class="wf-panel" style="margin-bottom: 20px;">
  <div class="wf-panel-head"><span class="wf-panel-title">STDOUT</span></div>
  <div class="wf-panel-body">
    <pre style="min-height: 80px; max-height: 600px; overflow: auto; margin: 0;">{{ stdout }}</pre>
  </div>
</div>
{% if stderr %}
<div class="wf-panel" style="margin-bottom: 20px;">
  <div class="wf-panel-head"><span class="wf-panel-title">STDERR</span></div>
  <div class="wf-panel-body">
    <pre style="min-height: 40px; max-height: 400px; overflow: auto; margin: 0;">{{ stderr }}</pre>
  </div>
</div>
{% endif %}
{% endif %}
{% endblock %}
```

**Note on SSE done handling:** When the `done` event arrives, the page reloads to show the final static view with complete logs and updated status badge.

**Note on HTML escaping:** MiniJinja auto-escapes `{{ stdout }}` and `{{ stderr }}` in the static display. For SSE streaming, the Rust endpoint HTML-escapes data before sending (Task 13).

- [ ] **Step 2: Verify**

```bash
cargo check
```

Start dev server, view a completed execution — verify static logs display. If possible, trigger a new execution and view it while running to test SSE streaming.

- [ ] **Step 3: Commit**

```bash
git add templates/execution_detail.html
git commit -m "feat: rewrite execution detail with Wave Funk panels and HTMX SSE"
```

---

### Task 10: Rewrite approvals.html + users.html

**Files:**
- Modify: `templates/approvals.html`
- Modify: `templates/users.html`

**Context variables (approvals):** `executions` (array: `id`, `hook_slug`, `triggered_at`, `trigger_source`), `username`, `nav_active`.

**Context variables (users):** `users` (array: `id`, `username`, `created_at`, `is_self`), `success`, `error`, `username`, `nav_active`.

- [ ] **Step 1: Rewrite approvals.html**

```html
{% extends "base.html" %}

{% block title %}sendword — approvals{% endblock %}

{% block crumbs %}
<div class="wf-crumbs">
  <span aria-current="page">APPROVALS</span>
</div>
{% endblock %}

{% block content %}
<div class="page-head">
  <h1 class="page-title">Approvals</h1>
</div>
<p class="page-sub">{{ executions | length }} pending</p>

{% if executions | length == 0 %}
<div class="wf-empty">
  <div class="wf-empty-title">ALL CLEAR</div>
  <div class="wf-empty-msg">No executions pending approval.</div>
</div>
{% else %}
<div class="wf-panel">
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
      {% for exec in executions %}
      <tr>
        <td class="strong"><a href="/hooks/{{ exec.hook_slug }}">{{ exec.hook_slug }}</a></td>
        <td><span data-ts="{{ exec.triggered_at }}">{{ exec.triggered_at }}</span></td>
        <td>{{ exec.trigger_source }}</td>
        <td>
          <div class="wf-btn-group">
            <form method="post" action="/executions/{{ exec.id }}/approve" style="display: inline;">
              <button type="submit" class="wf-btn sm">APPROVE</button>
            </form>
            <form method="post" action="/executions/{{ exec.id }}/reject" style="display: inline;">
              <button type="submit" class="wf-btn sm danger">REJECT</button>
            </form>
          </div>
        </td>
      </tr>
      {% endfor %}
    </tbody>
  </table>
</div>
{% endif %}
{% endblock %}
```

- [ ] **Step 2: Rewrite users.html**

```html
{% extends "base.html" %}

{% block title %}sendword — users{% endblock %}

{% block crumbs %}
<div class="wf-crumbs">
  <span aria-current="page">USERS</span>
</div>
{% endblock %}

{% block content %}
<div class="page-head">
  <h1 class="page-title">Users</h1>
</div>

<div class="wf-panel" style="margin-bottom: 20px;">
  <div class="wf-panel-head"><span class="wf-panel-title">ADD USER</span></div>
  <div class="wf-panel-body">
    <form method="post" action="/settings/users" style="display: flex; gap: 12px; align-items: flex-end;">
      <div class="wf-field" style="flex: 1;">
        <label for="new-email">EMAIL</label>
        <input class="wf-input" type="email" id="new-email" name="username" required>
      </div>
      <div class="wf-field" style="flex: 1;">
        <label for="new-password">PASSWORD</label>
        <input class="wf-input" type="password" id="new-password" name="password" required>
      </div>
      <button type="submit" class="wf-btn primary sm">ADD USER</button>
    </form>
  </div>
</div>

<div class="wf-panel">
  <div class="wf-panel-head"><span class="wf-panel-title">ALL USERS</span></div>
  <table class="wf-table">
    <thead>
      <tr>
        <th>EMAIL</th>
        <th>CREATED</th>
        <th></th>
      </tr>
    </thead>
    <tbody>
      {% for user in users %}
      <tr>
        <td class="strong">{{ user.username }}{% if user.is_self %} <span class="wf-tag accent">YOU</span>{% endif %}</td>
        <td><span data-ts="{{ user.created_at }}">{{ user.created_at }}</span></td>
        <td class="num">
          {% if not user.is_self %}
          <form method="post" action="/settings/users/{{ user.id }}/delete" style="display: inline;" onsubmit="return confirm('Delete user {{ user.username }}?');">
            <button type="submit" class="wf-btn sm danger">DELETE</button>
          </form>
          {% endif %}
        </td>
      </tr>
      {% endfor %}
    </tbody>
  </table>
</div>
{% endblock %}
```

- [ ] **Step 3: Verify**

```bash
cargo check
```

Verify both pages render correctly in the dev server.

- [ ] **Step 4: Commit**

```bash
git add templates/approvals.html templates/users.html
git commit -m "feat: rewrite approvals and users pages with Wave Funk components"
```

---

### Task 11: Rewrite scripts.html + script_editor.html

**Files:**
- Modify: `templates/scripts.html`
- Modify: `templates/script_editor.html`

**Context variables (scripts):** `scripts` (array: `name`, `size`, `modified`), `success`, `error`, `username`, `nav_active`.

**Context variables (script_editor):** `is_new`, `filename`, `content`, `success`, `error`, `username`, `nav_active`.

- [ ] **Step 1: Rewrite scripts.html**

```html
{% extends "base.html" %}

{% block title %}sendword — scripts{% endblock %}

{% block crumbs %}
<div class="wf-crumbs">
  <span aria-current="page">SCRIPTS</span>
</div>
{% endblock %}

{% block actions %}
<a class="wf-btn sm primary" href="/scripts/new">+ NEW SCRIPT</a>
{% endblock %}

{% block content %}
<div class="page-head">
  <h1 class="page-title">Scripts</h1>
</div>
<p class="page-sub">{{ scripts | length }} file{{ 's' if scripts | length != 1 }}</p>

{% if scripts | length == 0 %}
<div class="wf-empty">
  <div class="wf-empty-title">NO SCRIPTS</div>
  <div class="wf-empty-msg">Upload a script to reference in hook executors.</div>
  <a class="wf-btn sm primary" href="/scripts/new">+ NEW SCRIPT</a>
</div>
{% else %}
<div class="wf-panel">
  <table class="wf-table is-interactive">
    <thead>
      <tr>
        <th>FILENAME</th>
        <th class="num">SIZE</th>
        <th>MODIFIED</th>
      </tr>
    </thead>
    <tbody>
      {% for script in scripts %}
      <tr onclick="location.href='/scripts/{{ script.name }}'">
        <td class="strong"><code>{{ script.name }}</code></td>
        <td class="num">{{ script.size }}</td>
        <td><span data-ts="{{ script.modified }}">{{ script.modified }}</span></td>
      </tr>
      {% endfor %}
    </tbody>
  </table>
</div>
{% endif %}
{% endblock %}
```

- [ ] **Step 2: Rewrite script_editor.html**

```html
{% extends "base.html" %}

{% block title %}sendword — {% if is_new %}new script{% else %}{{ filename }}{% endif %}{% endblock %}

{% block crumbs %}
<div class="wf-crumbs">
  <a href="/scripts">SCRIPTS</a><span class="sep">/</span>
  <span aria-current="page">{% if is_new %}NEW{% else %}{{ filename | upper }}{% endif %}</span>
</div>
{% endblock %}

{% block content %}
<div class="page-head">
  <h1 class="page-title">{% if is_new %}New Script{% else %}{{ filename }}{% endif %}</h1>
</div>

<form method="post" action="{% if is_new %}/scripts/new{% else %}/scripts/{{ filename }}{% endif %}">
  <div class="wf-panel" style="margin-bottom: 20px;">
    <div class="wf-panel-head"><span class="wf-panel-title">{% if is_new %}CREATE SCRIPT{% else %}EDIT SCRIPT{% endif %}</span></div>
    <div class="wf-panel-body">
      <div class="wf-field" style="margin-bottom: 16px;">
        <label for="filename">FILENAME</label>
        <input class="wf-input" type="text" id="filename" name="filename" value="{{ filename }}"
          {% if not is_new %}readonly{% endif %}
          {% if is_new %}required pattern="[a-zA-Z0-9_\-]+(\.[a-zA-Z0-9]+)?"{% endif %}>
      </div>
      <div class="wf-field">
        <label for="content">CONTENT</label>
        <textarea class="wf-textarea" id="content" name="content" rows="24" style="font-family: var(--font-mono); font-size: 13px;" maxlength="1048576">{{ content }}</textarea>
      </div>
    </div>
  </div>

  <div style="display: flex; gap: 12px; justify-content: flex-end;">
    <a class="wf-btn" href="/scripts">CANCEL</a>
    <button type="submit" class="wf-btn primary">{% if is_new %}CREATE{% else %}SAVE{% endif %}</button>
  </div>
</form>

{% if not is_new %}
<form method="post" action="/scripts/{{ filename }}/delete" style="margin-top: 32px; padding-top: 20px; border-top: 1px solid var(--hairline);" onsubmit="return confirm('Delete {{ filename }}?');">
  <div style="display: flex; align-items: center; justify-content: space-between;">
    <span style="color: var(--fg-muted); font-size: 13px;">Permanently delete this script.</span>
    <button type="submit" class="wf-btn sm danger">DELETE SCRIPT</button>
  </div>
</form>
{% endif %}
{% endblock %}
```

- [ ] **Step 3: Verify**

```bash
cargo check
```

Verify both pages in the dev server.

- [ ] **Step 4: Commit**

```bash
git add templates/scripts.html templates/script_editor.html
git commit -m "feat: rewrite scripts and script editor with Wave Funk components"
```

---

### Task 12: Rewrite password.html

**Files:**
- Modify: `templates/password.html`

**Context variables:** `username`, `success`, `error`, `nav_active`.

- [ ] **Step 1: Rewrite password.html**

```html
{% extends "base.html" %}

{% block title %}sendword — change password{% endblock %}

{% block crumbs %}
<div class="wf-crumbs">
  <a href="/settings/users">USERS</a><span class="sep">/</span>
  <span aria-current="page">PASSWORD</span>
</div>
{% endblock %}

{% block content %}
<div class="page-head">
  <h1 class="page-title">Change Password</h1>
</div>

<form method="post" action="/settings/password" style="max-width: 480px;">
  <input type="hidden" name="username_hint" value="{{ username }}" autocomplete="username">
  <div class="wf-panel">
    <div class="wf-panel-body">
      <div class="wf-field" style="margin-bottom: 16px;">
        <label for="current_password">CURRENT PASSWORD</label>
        <input class="wf-input" type="password" id="current_password" name="current_password" autocomplete="current-password" required>
      </div>
      <div class="wf-field" style="margin-bottom: 16px;">
        <label for="new_password">NEW PASSWORD</label>
        <input class="wf-input" type="password" id="new_password" name="new_password" autocomplete="new-password" required>
      </div>
      <div class="wf-field" style="margin-bottom: 16px;">
        <label for="confirm_password">CONFIRM PASSWORD</label>
        <input class="wf-input" type="password" id="confirm_password" name="confirm_password" autocomplete="new-password" required>
      </div>
    </div>
  </div>
  <div style="display: flex; gap: 12px; justify-content: flex-end; margin-top: 16px;">
    <a class="wf-btn" href="/settings/users">CANCEL</a>
    <button type="submit" class="wf-btn primary">CHANGE PASSWORD</button>
  </div>
</form>
{% endblock %}
```

- [ ] **Step 2: Verify**

```bash
cargo check
```

- [ ] **Step 3: Commit**

```bash
git add templates/password.html
git commit -m "feat: rewrite password page with Wave Funk form components"
```

---

## Milestone 4: Backend Adjustments

### Task 13: HTML-escape SSE log data for HTMX SSE extension

**Files:**
- Modify: `src/routes/executions.rs`

The HTMX SSE extension swaps event data as inner content. Raw log output may contain `<`, `>`, `&` which would be interpreted as HTML. The SSE endpoint must HTML-escape stdout/stderr data. The `done` event should emit an HTML status badge fragment.

- [ ] **Step 1: Add a helper function for HTML escaping**

At the top of `src/routes/executions.rs` (near the other helper functions), add:

```rust
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}
```

- [ ] **Step 2: Update the SSE stream to use HTML escaping**

In the `log_stream` function, change all `.data(...)` calls for stdout/stderr to escape the content.

For terminal executions (full log dump), change:
```rust
yield Ok::<Event, Infallible>(Event::default().event("stdout").data(stdout));
```
to:
```rust
yield Ok::<Event, Infallible>(Event::default().event("stdout").data(html_escape(&stdout)));
```

Apply the same change to the stderr line and both stdout/stderr in the tailing loop.

- [ ] **Step 3: Update the `done` event to emit an HTML badge**

Replace the JSON `done` data with an HTML fragment. In both the terminal and tailing branches, change:
```rust
yield Ok(Event::default().event("done").data(
    serde_json::json!({ "status": status }).to_string()
));
```
to:
```rust
let tag_class = match status.as_str() {
    "success" => "ok",
    "failed" => "err",
    _ => "",
};
yield Ok(Event::default().event("done").data(
    format!(r#"<span class="wf-tag {tag_class}"><span class="dot"></span>{}</span>"#, status.to_uppercase())
));
```

For the tailing branch, the status comes from `e.status.to_string()` — adjust accordingly:
```rust
Ok(e) if e.status.is_terminal() => {
    let status = e.status.to_string();
    let tag_class = match status.as_str() {
        "success" => "ok",
        "failed" => "err",
        _ => "",
    };
    yield Ok(Event::default().event("done").data(
        format!(r#"<span class="wf-tag {tag_class}"><span class="dot"></span>{}</span>"#, status.to_uppercase())
    ));
    break;
}
```

- [ ] **Step 4: Run existing tests**

```bash
cargo test
```

Expected: all tests pass. The SSE test (`sse_route_requires_auth`) only checks auth, not response format.

- [ ] **Step 5: Verify manually**

Start dev server. View a completed execution's detail page — verify it renders correctly. If possible, trigger a new execution and watch the SSE stream.

- [ ] **Step 6: Commit**

```bash
git add src/routes/executions.rs
git commit -m "fix: HTML-escape SSE log data for HTMX SSE extension compatibility"
```

---

## Milestone 5: Cleanup

### Task 14: Remove old Tailwind/TypeScript files

**Files:**
- Delete: `tailwind.config.js`, `static/css/src/app.css`, `static/dist/app.css`, `static/dist/main.js`, `static/dist/main.js.map`, `static/ts/main.ts`, `static/ts/tsconfig.json`

- [ ] **Step 1: Remove files**

```bash
rm -f tailwind.config.js
rm -f static/css/src/app.css
rm -rf static/dist/
rm -rf static/ts/
```

- [ ] **Step 2: Verify the app still builds and runs**

```bash
cargo check
just dev
```

Navigate through all pages to confirm nothing references the old files.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "chore: remove Tailwind, TypeScript, and compiled dist artifacts"
```

---

## Final Verification

After all tasks are complete:

1. `cargo check` — no compilation errors
2. `cargo test` — all tests pass
3. `cargo clippy` — no warnings
4. Manual walkthrough of every page:
   - Login → Dashboard → Hook detail → Edit hook → New hook
   - Execution detail (completed) → Execution detail (running, if possible)
   - Approvals → Scripts → Script editor → Users → Password → 404
5. Verify: sidebar navigation highlights correctly, breadcrumbs are accurate, popovers open/close, timestamps format as relative, toast host is present
