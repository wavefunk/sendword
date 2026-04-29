# Sendword UI Migration to Wave Funk Design System

## Summary

Full migration of sendword's UI from Tailwind CSS + custom `.sw-*` components to the Wave Funk design system (pure CSS, `.wf-*` classes). Big-bang rewrite of all ~12 templates. Removes Tailwind, TypeScript, and Google Fonts. Adopts the app-shell scaffold (sidebar + topbar + statusbar), self-hosted Martian fonts, and plain JS.

## Approach

Big-bang rewrite. All templates rewritten in one pass against the `.wf-*` vocabulary. No hybrid/incremental migration — the project has ~12 templates, making a clean break practical and preferable.

## Build System & Asset Changes

### Remove

- `tailwind.config.js`
- `static/css/src/app.css` (Tailwind source)
- `static/dist/app.css` (compiled Tailwind output)
- `static/ts/` directory (TypeScript source)
- `static/dist/main.js` + source map (compiled TS output)
- Google Fonts CDN links (Outfit, Space Mono)
- esbuild / TypeScript build step

### Add

- `static/css/wavefunk.css` + layer files (`01-tokens.css` through `06-marketing.css`) — vendored from `../design/css/`
- `static/css/fonts/MartianGrotesk-VF.woff2` + `MartianMono-VF.woff2` — vendored from `../design/css/fonts/`
- `static/js/echo.js` — vendored from `../design/js/echo.js` (3KB minibuffer runtime)
- `static/js/sendword.js` — new plain JS file replacing `main.ts`

### Keep

- `build.rs` (MiniJinja template embedding — unchanged)
- `package.json` — retains Playwright deps for QA testing (remove tailwind/esbuild if listed)
- HTMX from CDN (upgrade to pinned version 2.0.4 to match design system)
- Add HTMX SSE extension: `https://unpkg.com/htmx-ext-sse@2.2.2/sse.js`
- Existing favicon (inline SVG data URI) — preserve as-is

### Justfile Updates

Remove recipes: `build-css`, `watch-css`, `build-ts`, `build-ts-dev`, `watch-ts`, `npm-install`.

Simplify `dev` to just `cargo run` (no CSS/TS watch processes needed).

Simplify `build` to just `cargo build --release` (no CSS/TS compilation steps).

Add `vendor-design` recipe to re-vendor CSS + fonts + echo.js from `../design/` for future updates.

## Base Template & App Shell

### New `base.html` structure

The current flat layout (top nav + centered container) is replaced by the Wave Funk app-shell grid:

```
<html data-mode="dark">
  <div class="app"> (grid: 240px sidebar | 1fr content, 56px topbar | 1fr main | 28px statusbar)
    <aside class="wf-sidebar">
      .wf-brand: "SENDWORD"
      .wf-nav-list:
        Section "HOOKS":   Hooks → /, Approvals → /approvals
        Section "TOOLS":   Scripts → /scripts
        Section "ADMIN":   Users → /settings/users
      .wf-user (bottom): avatar + username, popover with Change Password + Log Out
    </aside>
    <header class="wf-topbar">
      {% block crumbs %} — .wf-crumbs breadcrumbs
      spacer
      {% block actions %} — page-specific topbar buttons
    </header>
    <main>
      <div class="page">
        {% block content %}
      </div>
    </main>
    <div class="wf-statusbar">SENDWORD + version</div>
  </div>
  <div class="wf-toast-host" id="toast-host"></div>
```

### Template blocks

- `{% block title %}` — HTML page title
- `{% block crumbs %}` — topbar breadcrumbs
- `{% block actions %}` — topbar right-side buttons
- `{% block content %}` — main content area

### Login exception

`login.html` extends a separate `base_auth.html` with no app shell — uses the design system's auth layout components (`.wf-auth-top` for centered form area, `.wf-field` for inputs, `.wf-alert err` for errors).

### Nav active state

Sidebar nav items get `.is-active` based on the existing `nav_active` template variable.

## Page-by-Page Template Mapping

### Dashboard (`dashboard.html`)

- Page title: "Hooks"
- Topbar action: `+ New Hook` (`.wf-btn sm primary`)
- Hook list as `.wf-table` with `.is-interactive` rows
- Status via `.wf-tag` + `.wf-dot` (ok/err/warn)
- Filtering via `.wf-filterbar` with `.wf-seg` or `.wf-select`
- Pagination via `.wf-tablefoot` + `.wf-pagination`

### Hook Detail (`hook_detail.html`)

- Breadcrumbs: HOOKS / hook-slug
- Config displayed as `.wf-dl` sections inside `.wf-panel`s (basic info, auth, executor, payload schema, trigger rules, retry)
- Edit button in topbar actions
- Execution history as `.wf-table` in its own `.wf-panel` (reuses partial)

### Hook Form (`hook_form.html`)

- Breadcrumbs: HOOKS / hook-slug / EDIT (or HOOKS / NEW)
- Form sections as `.wf-panel`s with `.wf-panel-head` titles
- Inputs use `.wf-field` > `.wf-input` / `.wf-select` / `.wf-check` / `.wf-switch`
- Conditional show/hide JS stays as inline `<script>`, targeting new class names
- Submit as `.wf-btn primary`, cancel as `.wf-btn`

### Execution Detail (`execution_detail.html`)

- Breadcrumbs: HOOKS / hook-slug / EXECUTIONS / exec-id
- Status/metadata as `.wf-dl` in a `.wf-panel`
- Log output in a `.wf-panel` with `<pre>`
- SSE log streaming via `hx-ext="sse"` declaratively (replaces custom EventSource JS)
- Replay button in topbar actions

### Execution List Partial (`execution_list.html`)

- `.wf-table` with status `.wf-tag`s and `.wf-dot`s
- Filters as `.wf-filterbar` with `.wf-select` dropdowns
- Pagination in `.wf-tablefoot`
- HTMX attributes preserved, new markup only

### Trigger Attempt List Partial (`trigger_attempt_list.html`)

- Same treatment as execution list — `.wf-table` with status indicators
- HTMX attributes preserved

### Approvals (`approvals.html`)

- Page title: "Approvals"
- Pending approvals as `.wf-table` with approve/reject `.wf-btn` pairs
- Empty state via `.wf-empty`

### Users (`users.html`)

- Page title: "Users"
- Topbar action: `+ Add User`
- User list as `.wf-table`, delete as `.wf-btn sm danger`

### Scripts (`scripts.html`)

- Page title: "Scripts"
- Script list as `.wf-table` with `.is-interactive` rows

### Script Editor (`script_editor.html`)

- Breadcrumbs: SCRIPTS / filename
- Editor area in `.wf-panel`
- Save/cancel buttons

### Password (`password.html`)

- Form in `.wf-panel` with `.wf-field` inputs

### 404 (`404.html`)

- `.wf-empty` component with message and back link

## JavaScript Layer

### `static/js/sendword.js` (plain JS, no build step)

**Popover toggles:** Click `[data-popover-toggle]` toggles `.is-open` on nearest `.wf-popover`. Click outside closes all.

**Toast listener:** Listens for `wfToast` custom event on `document.body`. Creates `.wf-toast` element, appends to `#toast-host`, auto-dismisses after ~2.5s.

**Relative timestamps:** On `DOMContentLoaded` and `htmx:afterSettle`, finds `[data-ts]` elements and formats as relative time.

### SSE log streaming — removed from JS

Handled declaratively by HTMX SSE extension in `execution_detail.html`:
- `hx-ext="sse"` on container
- `sse-connect="/executions/{id}/logs/stream"` 
- `sse-swap="stdout"` / `sse-swap="stderr"` / `sse-swap="done"` on target elements

### Echo minibuffer

`echo.js` loaded from design system. Available for minibuffer messaging if `.wf-modeline` / `.wf-minibuffer` is added to statusbar later. Optional for initial migration.

### Form interactivity

Stays as inline `<script>` blocks in `hook_form.html`. Only change: target new `.wf-*` class names. Simplification deferred to separate work.

## Server-Side Changes

### SSE endpoint

HTMX SSE extension expects named events with HTML fragment data. Current endpoint already sends `stdout`, `stderr`, `done` event types. Adjust data payloads to emit HTML fragments (e.g., `<div>line</div>`) so HTMX can swap them directly. The `done` event returns a status badge fragment.

### Toast triggers

Update `HX-Trigger` headers to use `wfToast` event name with `{"kind": "ok|err|info", "msg": "..."}` payload. Map from current trigger format.

### Template context

- `nav_active` — stays, drives sidebar `.is-active`
- Add `breadcrumbs` — list of `{label, url}` for `.wf-crumbs`
- Add `page_title` — for `.page-title` heading
- `username` — stays for sidebar user section

### Scope of backend changes

Routes, handlers, database, and core logic are untouched. The Rust changes are limited to:
- SSE endpoint: change data payloads from plain text to HTML fragments
- Response builders: change `HX-Trigger` toast headers to `wfToast` format
- Template context: add `breadcrumbs` and `page_title` variables to handler responses

If anything consumes the SSE stream externally (curl scripts, monitoring), the format change will break it. Verify no external consumers exist before migrating.

## Design Decisions

- **Density:** Use `density-dense` on `<body>` — sendword is a technical admin tool where compact layout is appropriate.
- **Statusbar:** Single statusbar at grid bottom only (not duplicated in sidebar bottom, unlike the design system reference template which shows it in both locations).

## Out of Scope

- Simplifying hook form JS — separate follow-up
- Adding modeline/minibuffer to statusbar — future enhancement
- Dark/light mode toggle — system defaults to dark, toggle can be added later
- Responsive/mobile sidebar collapse — future enhancement
