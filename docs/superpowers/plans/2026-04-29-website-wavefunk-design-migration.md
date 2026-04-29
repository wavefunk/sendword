# Website Wave Funk Design Migration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate sendword's Eigen website from custom CSS to the Wave Funk design system, matching substrukt's pattern.

**Architecture:** Copy the Wave Funk CSS bundle into `website/static/css/wavefunk/`, create a 10-line `sendword.css` accent override, rewrite all templates to use `mk-*` (marketing) and `wf-*` (docs) design system classes, delete old `style.css`. Content and data files stay unchanged.

**Tech Stack:** Eigen static site generator, Wave Funk CSS design system, MiniJinja templates, HTMX

---

## File Structure

| Action | Path | Responsibility |
|--------|------|---------------|
| Copy | `website/static/css/wavefunk/` | Full design system from `../design/css/` (all 6 CSS files + fonts + wavefunk.css entry) |
| Create | `website/static/css/sendword.css` | Accent color overrides (~10 lines) |
| Rewrite | `website/templates/_base.html` | HTML shell, load wavefunk.css + sendword.css, HTMX |
| Create | `website/templates/_marketing.html` | mk-wrap wrapper with nav + footer |
| Rewrite | `website/templates/index.html` | Landing page with mk-hero, mk-sect, mk-feat, mk-step |
| Rewrite | `website/templates/_partials/nav.html` | wf-mnav navigation |
| Rewrite | `website/templates/_partials/footer.html` | mk-foot 4-column footer |
| Rewrite | `website/templates/_docs.html` | wf-docs-shell layout with sidebar + content + toc |
| Rewrite | `website/templates/_partials/sidebar.html` | wf-docs-side sidebar |
| Create | `website/templates/_partials/docs-toc.html` | wf-toc table of contents |
| Rewrite | `website/templates/docs/index.html` | Docs landing with category cards |
| Rewrite | `website/templates/docs/[doc].html` | Doc page with wf-prose |
| Rewrite | `website/templates/404.html` | Error page with design system classes |
| Delete | `website/static/css/style.css` | Old custom CSS (912 lines) |

---

### Task 1: Copy design system CSS and create sendword.css

**Files:**
- Copy: `website/static/css/wavefunk/` (from `../design/css/`)
- Create: `website/static/css/sendword.css`

- [ ] **Step 1: Copy the Wave Funk design system**

```bash
cp -r /home/nambiar/projects/wavefunk/design/css/ /home/nambiar/projects/wavefunk/sendword/website/static/css/wavefunk/
```

Verify the copy includes: `wavefunk.css`, `01-tokens.css` through `06-marketing.css`, and `fonts/` directory with Martian woff2 files.

```bash
ls website/static/css/wavefunk/
ls website/static/css/wavefunk/fonts/
```

- [ ] **Step 2: Create sendword.css**

Create `website/static/css/sendword.css`:

```css
:root {
  --accent: #cba6f7;
  --accent-ink: #000000;
}

[data-mode="light"] {
  --accent: #8839ef;
  --accent-ink: #ffffff;
}
```

- [ ] **Step 3: Commit**

```bash
cd /home/nambiar/projects/wavefunk/sendword
git add website/static/css/wavefunk/ website/static/css/sendword.css
git commit -m "feat(website): add Wave Funk design system CSS and sendword accent overrides"
```

---

### Task 2: Rewrite _base.html and create _marketing.html

**Files:**
- Rewrite: `website/templates/_base.html`
- Create: `website/templates/_marketing.html`

- [ ] **Step 1: Rewrite _base.html**

Replace the entire contents of `website/templates/_base.html` with:

```html
<!doctype html>
<html lang="en" data-mode="dark">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{% block title %}{{ site.name }}{% endblock %}</title>
<link rel="stylesheet" href="/css/wavefunk/wavefunk.css">
<link rel="stylesheet" href="/css/sendword.css">
<script src="https://unpkg.com/htmx.org@2.0.4" defer></script>
{% block head %}{% endblock %}
</head>
<body>
{% block body %}{% endblock %}
</body>
</html>
```

- [ ] **Step 2: Create _marketing.html**

Create `website/templates/_marketing.html`:

```html
{% extends "_base.html" %}

{% block head %}{% endblock %}

{% block body %}
<div class="mk-wrap">
  {% include "_partials/nav.html" %}
  {% block content %}{% endblock %}
  {% include "_partials/footer.html" %}
</div>
{% endblock %}
```

- [ ] **Step 3: Commit**

```bash
git add website/templates/_base.html website/templates/_marketing.html
git commit -m "feat(website): rewrite base template for wavefunk design system"
```

---

### Task 3: Rewrite nav.html and footer.html

**Files:**
- Rewrite: `website/templates/_partials/nav.html`
- Rewrite: `website/templates/_partials/footer.html`

- [ ] **Step 1: Rewrite nav.html**

Replace `website/templates/_partials/nav.html` with:

```html
<div class="wf-mnav">
  <div class="wf-wordmark">
    <div style="width: 22px; height: 22px; background: var(--accent); color: var(--accent-ink); display: inline-flex; align-items: center; justify-content: center; font-family: var(--font-mono); font-weight: 800; font-size: 13px;">S</div>
    <span class="wf-wordmark-name">SENDWORD</span>
  </div>
  {% for item in nav %}
    {% if item.external %}
      <a href="{{ item.url }}" target="_blank" rel="noopener">{{ item.label }}</a>
    {% else %}
      <a href="{{ item.url }}">{{ item.label }}</a>
    {% endif %}
  {% endfor %}
  <div class="wf-mnav-spacer"></div>
</div>
```

- [ ] **Step 2: Rewrite footer.html**

Replace `website/templates/_partials/footer.html` with:

```html
<footer>
  <div class="mk-foot">
    <div>
      <div class="wf-wordmark" style="margin-bottom: 14px;">
        <div style="width: 20px; height: 20px; background: var(--accent); color: var(--accent-ink); display: inline-flex; align-items: center; justify-content: center; font-family: var(--font-mono); font-weight: 800; font-size: 12px;">S</div>
        <span class="wf-wordmark-name">SENDWORD</span>
      </div>
      <p style="font-size: 12px; color: var(--fg-muted); max-width: 32ch; line-height: 1.55; font-family: var(--font-mono);">Simple HTTP webhook to command runner sidecar. Receive, validate, execute — with auth, filtering, retries, and a web UI.</p>
    </div>
    <div>
      <div class="mk-foot-h">RESOURCES</div>
      <a href="/docs/quickstart">Quickstart</a>
      <a href="/docs/hooks">Hooks</a>
      <a href="/docs/executors">Executors</a>
      <a href="/docs/api-overview">API Reference</a>
    </div>
    <div>
      <div class="mk-foot-h">PROJECT</div>
      <a href="https://github.com/wavefunk/sendword" target="_blank" rel="noopener">GitHub</a>
      <a href="https://github.com/wavefunk/sendword/blob/main/LICENSE" target="_blank" rel="noopener">License</a>
      <a href="https://github.com/wavefunk/sendword/issues" target="_blank" rel="noopener">Issues</a>
    </div>
    <div>
      <div class="mk-foot-h">OPERATIONS</div>
      <a href="/docs/backup-restore">Backup &amp; Restore</a>
      <a href="/docs/log-masking">Log Masking</a>
      <a href="/docs/trigger-rules">Trigger Rules</a>
    </div>
  </div>
  <div class="mk-colophon">
    <span>&copy; {{ current_year() }} SENDWORD</span>
    <a href="https://wavefunk.io" target="_blank" rel="noopener" class="mk-colophon-badge">
      <svg width="14" height="12" viewBox="0 0 64 42" fill="currentColor" xmlns="http://www.w3.org/2000/svg"><rect width="20" height="20" x="22" y="0" rx="4"/><rect width="20" height="20" x="44" y="0" rx="4"/><rect width="20" height="20" x="22" y="22" rx="4"/><rect width="20" height="20" x="0" y="22" rx="4"/></svg>
      BUILT BY WAVEFUNK
    </a>
    <span>OPEN SOURCE · MIT</span>
  </div>
</footer>
```

- [ ] **Step 3: Commit**

```bash
git add website/templates/_partials/nav.html website/templates/_partials/footer.html
git commit -m "feat(website): rewrite nav and footer with wavefunk design system"
```

---

### Task 4: Rewrite index.html landing page

**Files:**
- Rewrite: `website/templates/index.html`

- [ ] **Step 1: Rewrite index.html**

Replace the entire contents of `website/templates/index.html` with:

```html
---
data:
  nav:
    file: "nav.yaml"
  features:
    file: "features.yaml"
seo:
  title: "sendword — webhooks that run commands"
  description: "A lightweight HTTP webhook-to-command runner sidecar. Receive webhooks, validate payloads, execute commands — with auth, filtering, retries, and a web UI."
schema:
  type: WebSite
---
{% extends "_marketing.html" %}

{% block title %}sendword — webhooks that run commands{% endblock %}

{% block content %}
<!-- HERO -->
<section class="mk-hero">
  <div class="mk-hero-grid" aria-hidden="true"></div>
  <div class="mk-hero-inner">
    <div class="mk-hero-eyebrow">SENDWORD · OPEN SOURCE · SINGLE BINARY</div>
    <h1>Webhooks that run <em>commands</em>.</h1>
    <p>Receive HTTP webhooks, validate payloads, execute shell commands — with authentication, filtering, retries, and a web dashboard. One binary, zero dependencies.</p>
    <div class="mk-hero-cta">
      <a href="/docs/quickstart" class="wf-btn lg primary">Get Started</a>
      <a href="https://github.com/wavefunk/sendword" class="wf-btn lg" target="_blank" rel="noopener">GitHub</a>
      <span class="sep"></span>
      <code class="shell-line"><span class="prompt">$</span>cargo install sendword</code>
    </div>
  </div>
  <div class="mk-hero-stats">
    <div><div class="l">RUNTIME</div><div class="v">SINGLE BINARY</div></div>
    <div><div class="l">CONFIG</div><div class="v">TOML + JSON</div></div>
    <div><div class="l">DATABASE</div><div class="v">SQLITE</div></div>
    <div><div class="l">FRONTEND</div><div class="v">WEB DASHBOARD</div></div>
  </div>
</section>

<!-- FEATURES -->
<section class="mk-sect">
  <div class="mk-sect-head">
    <div>
      <div class="mk-sect-kicker">— 01 / FEATURES</div>
      <h2 class="mk-sect-title">What it does</h2>
    </div>
    <p class="mk-sect-sub">Everything you need to turn incoming webhooks into running commands — authentication, validation, concurrency control, and full execution history.</p>
  </div>
  <div class="mk-features">
    {% for feat in features %}
    <div class="mk-feat">
      <div class="mk-feat-num">— 0{{ loop.index }}</div>
      <h3 class="mk-feat-t">{{ feat.title }}</h3>
      <p class="mk-feat-b">{{ feat.description }}</p>
    </div>
    {% endfor %}
  </div>
</section>

<!-- HOW IT WORKS -->
<section class="mk-sect">
  <div class="mk-sect-head">
    <div>
      <div class="mk-sect-kicker">— 02 / HOW IT WORKS</div>
      <h2 class="mk-sect-title">Three steps to running.</h2>
    </div>
    <p class="mk-sect-sub">Define a hook in TOML, add rules and guards, start the server.</p>
  </div>
  <div class="mk-steps">
    <div class="mk-step">
      <div class="mk-step-num">— 01</div>
      <h3 class="mk-step-t">Configure a hook</h3>
      <p class="mk-step-b">Define the hook name, slug, executor command, and authentication in sendword.toml.</p>
    </div>
    <div class="mk-step">
      <div class="mk-step-num">— 02</div>
      <h3 class="mk-step-t">Add rules &amp; guards</h3>
      <p class="mk-step-b">Filter by payload fields, enforce cooldowns, set rate limits, and configure concurrency barriers.</p>
    </div>
    <div class="mk-step">
      <div class="mk-step-num">— 03</div>
      <h3 class="mk-step-t">Run sendword</h3>
      <p class="mk-step-b">Start the server. Hooks are available as POST endpoints. The dashboard shows execution history.</p>
    </div>
  </div>
</section>

<!-- QUICK START -->
<section class="mk-sect mk-code">
  <div class="mk-sect-head">
    <div>
      <div class="mk-sect-kicker">— 03 / CONFIG</div>
      <h2 class="mk-sect-title">One file. Full control.</h2>
    </div>
    <p class="mk-sect-sub">Everything lives in sendword.toml. Export as JSON, import on another machine, version-control the whole thing.</p>
  </div>
  <pre><code><span class="comment"># sendword.toml</span>
[[hooks]]
name = "Deploy"
slug = "deploy"

[hooks.executor]
type = "shell"
command = "bash deploy.sh { branch }"

[hooks.auth]
mode = "bearer"
token = "${DEPLOY_TOKEN}"

[hooks.trigger_rules]
cooldown = "5m"

[[hooks.trigger_rules.payload_filters]]
field = "branch"
operator = "regex"
value = "^(main|release/.*)"</code></pre>
</section>
{% endblock %}
```

- [ ] **Step 2: Commit**

```bash
git add website/templates/index.html
git commit -m "feat(website): rewrite landing page with wavefunk design system"
```

---

### Task 5: Rewrite docs templates (layout, sidebar, toc)

**Files:**
- Rewrite: `website/templates/_docs.html`
- Rewrite: `website/templates/_partials/sidebar.html`
- Create: `website/templates/_partials/docs-toc.html`

- [ ] **Step 1: Rewrite _docs.html**

Replace `website/templates/_docs.html` with:

```html
{% extends "_base.html" %}

{% block title %}{{ doc.title }} — sendword docs{% endblock %}

{% block body %}
  <div class="wf-docs-shell">
    {% block sidebar %}
      {% include "_partials/sidebar.html" %}
    {% endblock %}

    {% block doc_content %}
      <div id="doc-content" class="wf-docs-content">
        <article class="wf-prose">
          <div class="wf-crumbs">
            <a href="/">HOME</a><span class="sep">/</span>
            <a href="/docs/index.html">DOCS</a><span class="sep">/</span>
            <span aria-current="page">{{ doc.title | upper }}</span>
          </div>

          <h1>{{ doc.title }}</h1>
          {% if doc.description %}
            <p class="wf-lead">{{ doc.description }}</p>
          {% endif %}

          <div id="docs-content">
            {{ doc.content | markdown }}
          </div>
        </article>
        <div class="wf-docs-pager">
          <span></span>
          <span></span>
        </div>
      </div>
    {% endblock %}
    {% include "_partials/docs-toc.html" %}
  </div>

<script>
function buildToc() {
  var content = document.getElementById('docs-content');
  var toc = document.getElementById('docs-toc-links');
  if (!content || !toc) return;
  while (toc.firstChild) toc.removeChild(toc.firstChild);
  var headings = content.querySelectorAll('h2');
  headings.forEach(function(h) {
    var id = h.textContent.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/(^-|-$)/g, '');
    h.id = id;
    var a = document.createElement('a');
    a.href = '#' + id;
    a.textContent = h.textContent;
    toc.appendChild(a);
  });
}
buildToc();
document.body.addEventListener('htmx:afterSettle', function() {
  buildToc();
  var main = document.getElementById('doc-content');
  if (main) main.scrollTop = 0;
  window.scrollTo(0, 0);
});
document.addEventListener('click', function(e) {
  var a = e.target.closest('a[href]');
  if (!a) return;
  var href = a.getAttribute('href').replace(/\/+$/, '');
  var path = window.location.pathname.replace(/\/+$/, '');
  if (href === path) e.preventDefault();
});
</script>
{% endblock %}
```

- [ ] **Step 2: Rewrite sidebar.html**

Replace `website/templates/_partials/sidebar.html` with:

```html
<aside class="wf-docs-side" id="sidebar">
  <a href="/" class="wf-brand" style="cursor: pointer;">
    <div style="width: 24px; height: 24px; background: var(--accent); color: var(--accent-ink); display: inline-flex; align-items: center; justify-content: center; font-family: var(--font-mono); font-weight: 800; font-size: 14px;">S</div>
    <div>
      <div class="wf-brand-name">sendword</div>
      <div class="wf-caption">Docs</div>
    </div>
  </a>

  <div class="wf-docs-side-nav">
    {% for cat in categories %}
    <div class="wf-docs-side-section">{{ cat.name }}</div>
    {% for d in docs_list %}
      {% if d.category == cat.name %}
      <a href="/docs/{{ d.slug }}"
         hx-get="/_fragments/docs/{{ d.slug }}/doc_content.html"
         hx-target="#doc-content"
         hx-swap="outerHTML"
         hx-push-url="/docs/{{ d.slug }}"
         {% if d.slug == doc.slug %} class="is-active"{% endif %}>{{ d.title }}</a>
      {% endif %}
    {% endfor %}
    {% endfor %}
  </div>

  <a href="https://wavefunk.io" target="_blank" rel="noopener" class="wf-docs-side-badge">
    <svg width="12" height="10" viewBox="0 0 64 42" fill="currentColor" xmlns="http://www.w3.org/2000/svg"><rect width="20" height="20" x="22" y="0" rx="4"/><rect width="20" height="20" x="44" y="0" rx="4"/><rect width="20" height="20" x="22" y="22" rx="4"/><rect width="20" height="20" x="0" y="22" rx="4"/></svg>
    Built by Wavefunk
  </a>
</aside>
```

- [ ] **Step 3: Create docs-toc.html**

Create `website/templates/_partials/docs-toc.html`:

```html
<aside class="wf-toc">
  <h4>ON THIS PAGE</h4>
  <div id="docs-toc-links"></div>

  <h4 class="wf-mt-6">ACTIONS</h4>
  <a href="https://github.com/wavefunk/sendword" target="_blank" rel="noopener">View on GitHub ↗</a>
</aside>
```

- [ ] **Step 4: Commit**

```bash
git add website/templates/_docs.html website/templates/_partials/sidebar.html website/templates/_partials/docs-toc.html
git commit -m "feat(website): rewrite docs layout with wavefunk design system"
```

---

### Task 6: Rewrite docs/index.html, docs/[doc].html, and 404.html

**Files:**
- Rewrite: `website/templates/docs/index.html`
- Rewrite: `website/templates/docs/[doc].html`
- Rewrite: `website/templates/404.html`

- [ ] **Step 1: Rewrite docs/index.html**

Replace `website/templates/docs/index.html` with:

```html
---
data:
  nav:
    file: "nav.yaml"
  docs_list:
    file: "docs.yaml"
  categories:
    file: "categories.yaml"
seo:
  title: "Documentation — sendword"
  description: "Comprehensive documentation for sendword — hooks, executors, authentication, trigger rules, retries, and more."
schema:
  type: WebSite
  breadcrumb_names:
    docs: "Documentation"
---
{% extends "_marketing.html" %}

{% block title %}Documentation — {{ site.name }}{% endblock %}

{% block content %}
<section class="mk-sect">
  <div class="mk-sect-head">
    <div>
      <div class="mk-sect-kicker">— DOCUMENTATION</div>
      <h2 class="mk-sect-title">Everything you need.</h2>
    </div>
    <p class="mk-sect-sub">Start with the quickstart or jump to a specific topic. Everything you need to turn webhooks into running commands.</p>
  </div>
  <div class="mk-features">
    {% for cat in categories %}
    <div class="mk-feat">
      <div class="mk-feat-num">{{ cat.name | upper }}</div>
      <h3 class="mk-feat-t">{{ cat.name }}</h3>
      <p class="mk-feat-b">{{ cat.desc }}</p>
      <div style="margin-top: 12px;">
        {% for d in docs_list %}
          {% if d.category == cat.name %}
          <a href="/docs/{{ d.slug }}" style="display: block; font-size: 13px; margin-top: 4px; color: var(--fg-dim);">{{ d.title }}</a>
          {% endif %}
        {% endfor %}
      </div>
    </div>
    {% endfor %}
  </div>
</section>
{% endblock %}
```

- [ ] **Step 2: Rewrite docs/[doc].html**

Replace `website/templates/docs/[doc].html` with:

```html
---
collection:
  file: "docs.yaml"
slug_field: slug
item_as: doc
fragment_blocks:
  - doc_content
  - sidebar
data:
  nav:
    file: "nav.yaml"
  docs_list:
    file: "docs.yaml"
  categories:
    file: "categories.yaml"
seo:
  title: "{{ doc.title }} — sendword docs"
  description: "{{ doc.description }}"
schema:
  type: Article
  breadcrumb_names:
    docs: "Documentation"
---
{% extends "_docs.html" %}

{% block title %}{{ doc.title }} — {{ site.name }} docs{% endblock %}

{% block doc_content %}
  <div id="doc-content" class="wf-docs-content">
    <article class="wf-prose">
      <div class="wf-crumbs">
        <a href="/">HOME</a><span class="sep">/</span>
        <a href="/docs/index.html">DOCS</a><span class="sep">/</span>
        <span aria-current="page">{{ doc.title | upper }}</span>
      </div>

      <h1>{{ doc.title }}</h1>
      {% if doc.description %}
        <p class="wf-lead">{{ doc.description }}</p>
      {% endif %}

      <div id="docs-content">
        {{ doc.content | markdown }}
      </div>
    </article>
    <div class="wf-docs-pager">
      <span></span>
      <span></span>
    </div>
  </div>
{% endblock %}
```

- [ ] **Step 3: Rewrite 404.html**

Replace `website/templates/404.html` with:

```html
---
data:
  nav:
    file: "nav.yaml"
---
{% extends "_marketing.html" %}

{% block title %}404 — {{ site.name }}{% endblock %}

{% block content %}
<section class="mk-sect" style="text-align: center; padding: 160px 32px;">
  <h1 style="font-size: clamp(64px, 10vw, 112px); font-family: var(--font-mono); font-weight: 800; color: var(--fg-muted);">404</h1>
  <p style="color: var(--fg-dim); margin-top: 16px;">Message not delivered. This page doesn't exist.</p>
  <a href="/" class="wf-btn lg primary" style="margin-top: 32px;">Back to home</a>
</section>
{% endblock %}
```

- [ ] **Step 4: Commit**

```bash
git add website/templates/docs/index.html website/templates/docs/\[doc\].html website/templates/404.html
git commit -m "feat(website): rewrite docs index, doc pages, and 404 with wavefunk design"
```

---

### Task 7: Delete old CSS and build

**Files:**
- Delete: `website/static/css/style.css`

- [ ] **Step 1: Delete old CSS**

```bash
rm /home/nambiar/projects/wavefunk/sendword/website/static/css/style.css
```

- [ ] **Step 2: Build the site**

```bash
cd /home/nambiar/projects/wavefunk/sendword/website && eigen build
```

Expected: `Built N page(s) in dist/` with no errors.

- [ ] **Step 3: Verify key pages exist**

```bash
ls website/dist/index.html
ls website/dist/docs/index.html
ls website/dist/docs/quickstart.html
ls website/dist/404.html
```

- [ ] **Step 4: Start dev server and visually verify**

```bash
cd /home/nambiar/projects/wavefunk/sendword/website && eigen dev --port 4000
```

Check in browser:
1. `http://localhost:4000/` — landing page with purple accent, mk-hero grid, features, steps
2. `http://localhost:4000/docs/quickstart` — docs page with sidebar, prose, toc
3. `http://localhost:4000/docs/index.html` — docs index with category cards
4. Navigation between docs pages works (HTMX fragments)
5. Dark mode by default, Martian fonts loaded

- [ ] **Step 5: Commit**

```bash
cd /home/nambiar/projects/wavefunk/sendword
git add -u website/static/css/style.css
git commit -m "chore(website): remove old custom CSS"
```
