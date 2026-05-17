# Project Overview : sendword
Simple HTTP webhook to command runner sidecar. Frontend for managing hooks, JSON state for config portability, SQLite for execution history and logs. Supports authed hooks, trigger rules, custom payload definitions, configurable rate limiting, and command execution barriers.

## Tech Stack
Async runtime = Tokio
Web framework = Axum
Database = SQLite via SQLx
Templating = Askama
UI components/layout = wavefunk-ui
Frontend = HTMX + Sendword-specific JavaScript

## Local development
Nix, direnv and flake to manage local dev environment
just to run often used commands
Cargo.toml depends on the published `wavefunk-ui` crate. During local UI
co-development, the gitignored `.cargo/config.toml` can use a path override to
`../ui` so local crate changes are exercised without changing committed
dependency metadata.

## Architecture Overview

### Request Flow
Axum handler → core logic → SQLx → typed Askama view

### Frontend Architecture
HTML pages live in `src/views/` as typed Askama templates that compose
`wavefunk-ui` components and layouts. Shared Wave Funk CSS, fonts, HTMX, SSE,
and JavaScript are served by the `wavefunk-ui` asset router under
`/static/wavefunk`. Sendword-owned behavior lives in `static/js/sendword.js`.

## Work Structure
Always create a plan, then review, then implement.
Always create a git branch for the work.
Create atomic commits for coherent work done.

## Planning Style
- Small milestones - never more than 5-10 tasks per milestone.
- Each task single actionable item, not a group of outputs
- use `br` for task tracking

## Code Style
- Idiomatic rust code
- Optimized for readability first
- Avoid long format!() chains and other allocations. Be memory efficient.
- Write tests immediately after a feature.
- Do not write "ceremony" tests that actually just test the library being used.
- Do not use unwrap or expect unless its an invariant.

## Available commands
The just file has available commands. Be mindful of commands that you run often, add it to the justfile.
