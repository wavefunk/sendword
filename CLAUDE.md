# Project Overview : sendword
Simple HTTP webhook to command runner sidecar. Frontend for managing hooks, JSON state for config portability, SQLite for execution history and logs. Supports authed hooks, trigger rules, custom payload definitions, configurable rate limiting, and command execution barriers.

## Tech Stack
Async runtime = Tokio
Web framework = Axum
Database = SQLite via SQLx
Templating = MiniJinja
Frontend = HTMX + Tailwind

## Local development
Nix, direnv and flake to manage local dev environment
just to run often used commands

## Context Loading
Before exploring the codebase (reading files, checking structure, dispatching exploration agents):
1. Read `.claude/summaries/project-summary.md` — full directory/module map
2. Read the specific `.claude/summaries/<area>.md` for the area you're working in
3. Only explore files directly if the summaries don't answer your question

## Architecture Overview

### Request Flow
Axum handler → core logic → SQLx → MiniJinja template

### Frontend Architecture
HTMX + Tailwind for HTML pages. Templates in templates/. TypeScript bundled via esbuild.

## Work Structure
Always create a plan, then review, then implement.
Always create a git branch for the work.
Create atomic commits for coherent work done.

## Planning Style
- Small milestones - never more than 5-10 tasks per milestone.
- use `bd` for task tracking

## Code Style
- Idiomatic rust code
- Optimized for readability first
- Avoid long format!() chains and other allocations. Be memory efficient.
- Write tests immediately after a feature.
- Do not write "ceremony" tests that actually just test the library being used.
- Do not use unwrap or expect unless its an invariant.

## Repository Structure
sendword/
├── Cargo.toml
├── CLAUDE.md
├── rust-toolchain.toml
├── flake.nix
├── .envrc
├── .gitignore
├── justfile
├── sqlx.toml
├── package.json
├── tailwind.config.js
├── build.rs
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── db.rs
│   ├── error.rs
│   ├── id.rs
│   ├── timestamp.rs
│   └── templates.rs
├── data/
├── migrations/
├── static/
│   ├── css/src/app.css
│   └── ts/
│       ├── main.ts
│       └── tsconfig.json
└── templates/
    └── base.html

## Available commands
The just file has available commands. Be mindful of commands that you run often, add it to the justfile.

### Extras
Read to AGENTS.md for additional instructions.
