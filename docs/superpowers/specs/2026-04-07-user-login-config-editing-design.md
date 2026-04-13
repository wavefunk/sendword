# User Login & Config/Script Editing Design Spec

Adds user authentication for the web UI, form-based hook config editing with TOML write-back, and a managed script file editor. Inserted as the new Milestone 2, shifting existing M2-M5 to M3-M6.

## User Management

### Data model

SQLite `users` table:
- `id` (TEXT PK, UUIDv7)
- `username` (TEXT UNIQUE NOT NULL, 3-32 chars, alphanumeric + hyphens)
- `password_hash` (TEXT NOT NULL, argon2id)
- `created_at` (TEXT, ISO8601)

### Bootstrap

First user created via CLI:

```
sendword user create --username admin
```

Prompts for password interactively. After bootstrap, users are managed through the web UI.

### UI user management

Routes:
- `GET /settings/users` — list existing users, create new user form (username + password), delete button (cannot delete yourself)
- `POST /settings/users` — create a new user
- `POST /settings/users/:id/delete` — delete a user
- `GET /settings/password` — change own password form (current password + new password)
- `POST /settings/password` — validate current password, update hash

All authenticated users are equal — no roles.

## Sessions & Auth Middleware

### Data model

SQLite `sessions` table:
- `id` (TEXT PK, random 32-byte token, base64url-encoded)
- `user_id` (TEXT NOT NULL, FK to users)
- `created_at` (TEXT, ISO8601)
- `expires_at` (TEXT, ISO8601)

### Configuration

```toml
[auth]
session_lifetime = "24h"
secure_cookie = false
```

Session lifetime configurable, defaults to 24 hours. Expired sessions cleaned up lazily (checked on each session lookup) and via a Tokio background task that sweeps every hour.

### Cookie

Name: `sendword_session`. HTTP-only, SameSite=Lax. Secure flag controlled by config (`auth.secure_cookie`, defaults to `false` — set to `true` when behind a TLS-terminating reverse proxy). No JS access.

### Middleware

Axum extractor (`AuthUser`) checks cookie, looks up session, rejects with redirect to `/login` if invalid.

Protected: all routes except:
- `POST /hook/:slug` (webhook triggers — per-hook auth, not user sessions)
- `GET /healthz`
- `GET /login`, `POST /login`, `GET /logout`
- `/static/*`

### Login flow

- `GET /login` — login form
- `POST /login` — validate credentials, create session, set cookie, redirect to `/`
- `GET /logout` — delete session, clear cookie, redirect to `/login`

## Config Editor

Form-based editing of hook configuration. Edits write back to `sendword.toml` and trigger hot-reload.

### Routes

- `GET /hooks/new` — blank form for creating a new hook
- `POST /hooks/new` — validate, append to TOML, hot-reload, redirect to hook detail
- `GET /hooks/:slug/edit` — form pre-filled with current hook config
- `POST /hooks/:slug/edit` — validate, write to TOML, hot-reload, redirect to hook detail
- `POST /hooks/:slug/delete` — remove from TOML, hot-reload, redirect to dashboard

### Editable fields (M2 scope)

- Name, slug (immutable after creation), description, enabled toggle
- Executor type (shell only in M2), command, working directory
- Env vars (key-value pairs, add/remove rows)
- Timeout, retry config (count, backoff strategy, initial delay, max delay)

Later milestones add form sections for auth, payload schema, trigger rules, rate limits, concurrency, and approval config as those features are built.

### TOML write-back

- Use `toml_edit` crate to preserve formatting and comments
- Read current `sendword.toml`, deserialize, apply changes, re-serialize
- Atomic write: write to temp file, rename into place
- After write, trigger config hot-reload via `ArcSwap` (swap `AppConfig` in shared state)

### Validation

Same validation from `config.rs` runs on form submission. Errors displayed inline on the form.

## Script Editor

### Managed scripts directory

Configured in `sendword.toml`:

```toml
[scripts]
dir = "data/scripts"
```

Defaults to `data/scripts/`. Created on startup if it doesn't exist.

### Routes

- `GET /scripts` — file listing of managed directory (flat, no subdirectories)
- `GET /scripts/new` — blank editor
- `POST /scripts/new` — write file, set executable bit, redirect to editor
- `GET /scripts/:filename` — editor page with file contents in monospace textarea
- `POST /scripts/:filename` — validate and save, set executable bit
- `POST /scripts/:filename/delete` — delete file, redirect to listing

### Constraints

- Filenames: alphanumeric, hyphens, underscores, dots only. No path traversal.
- Server validates resolved path stays inside managed directory.
- Max file size: 1MB.

### External script references

Hooks referencing scripts outside the managed directory show the path as read-only text on the hook detail page. Scripts inside the managed directory get an "Edit script" link.

## Milestone Restructuring

| Before | After |
|--------|-------|
| M1: Foundation | M1: Foundation (unchanged) |
| — | **M2: User Login & Config/Script Editing (new)** |
| M2: Auth & Payload Validation | M3: Auth & Payload Validation |
| M3: Trigger Rules & Rate Limiting | M4: Trigger Rules & Rate Limiting |
| M4: Execution Barriers | M5: Execution Barriers |
| M5: Executors, Backups & Polish | M6: Executors, Backups & Polish |

All existing bd tasks bump their milestone label by one.

### New M2 deliverables

- `users` and `sessions` SQLite migrations
- User model with argon2id hashing
- CLI `sendword user create` command
- Session middleware / `AuthUser` extractor
- Login/logout pages and routes
- Config editor: hook create/edit/delete forms, TOML write-back with `toml_edit`, config hot-reload via `ArcSwap`
- Script editor: managed directory, file list/create/edit/delete, executable bit
- User management page (list, create, delete, password change)
- UI nav links for Scripts, Settings; "Edit" button on hook detail

### Non-goals update

Remove "Multi-user auth for the web UI" from non-goals — now in scope as M2.
