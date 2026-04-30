# sendword

HTTP webhook receiver that runs commands. Define hooks in TOML, trigger them with HTTP requests, see results in a web dashboard.

[Documentation](https://sendword.online) | [v0.0.2](https://github.com/wavefunk/sendword/releases)

## What it does

sendword sits next to your application as a sidecar. It listens for incoming webhooks and executes shell commands, scripts, or HTTP calls in response. Configuration lives in a single TOML file. Execution history and logs are stored in SQLite.

```
GitHub/CI/Monitoring ──HTTP POST──▶ sendword ──▶ shell command
                                                  script
                                                  HTTP request
```

## Features

- **Webhook authentication** --- Bearer tokens, HMAC-SHA256 signature verification, or open access. Constant-time comparison prevents timing attacks.
- **Payload validation** --- JSON schema per hook. Malformed payloads are rejected before your command runs.
- **Trigger rules** --- Filter by payload fields, restrict to time windows, enforce cooldowns, and apply rate limits before execution.
- **Three executor types** --- Shell commands with payload interpolation, managed scripts, or HTTP forwarding. Per-hook timeouts and working directories.
- **Retries with backoff** --- None, linear, or exponential. Configurable per hook or globally, with max delay caps.
- **Execution barriers** --- Mutex or queue-based concurrency control. Approval workflows gate hooks behind human review with optional timeouts.
- **Secret masking** --- Redact env var values and regex patterns from dashboard output and log files.
- **Backup and restore** --- Snapshot database and config to S3-compatible storage on a cron schedule. Restore with one command. Retention policies handle cleanup.
- **Web dashboard** --- View hooks, executions, trigger attempts, and stream logs in real time via SSE.
- **Config portability** --- Export/import configuration as JSON. Environment variable overrides for every setting.

## Quick start

### Prerequisites

- Rust nightly (the project pins `nightly-2026-01-05` via `rust-toolchain.toml`)
- SQLite 3

With [Nix](https://nixos.org/) and [direnv](https://direnv.net/), dependencies are managed automatically:

```sh
direnv allow
```

### Build and run

```sh
# Build
cargo build --release

# Run (creates data/ directory and SQLite database on first start)
./target/release/sendword serve
```

Or during development:

```sh
cargo run
```

sendword starts on `127.0.0.1:8080` by default. Open it in a browser to access the dashboard.

### Create a user

The dashboard requires authentication. Create your first user:

```sh
sendword user create --email you@example.com
```

You'll be prompted for a password.

## Configuration

sendword loads config from `sendword.toml`, then `sendword.json`, then environment variables (in that priority order). Environment variables use the `SENDWORD_<SECTION>__<KEY>` format.

### Minimal example

```toml
[server]
bind = "127.0.0.1"
port = 8080

[[hooks]]
name = "Deploy App"
slug = "deploy-app"

[hooks.executor]
type = "shell"
command = "cd /opt/app && git pull && make deploy"
```

This creates a hook at `POST /hook/deploy-app` that runs the deploy command.

### Full example

```toml
[server]
bind = "127.0.0.1"
port = 9090

[database]
path = "data/sendword.db"

[logs]
dir = "data/logs"

[auth]
session_lifetime = "24h"
secure_cookie = false

[auth.smtp]
host = "smtp.example.com"
port = 587
username = "sendword@example.com"
password = "your-smtp-password"
from = "sendword@example.com"
starttls = true

[scripts]
dir = "data/scripts"

[defaults]
timeout = "30s"

[defaults.rate_limit]
max_per_minute = 60

[defaults.retries]
count = 0
backoff = "exponential"
initial_delay = "1s"
max_delay = "60s"

[masking]
env_vars = ["DATABASE_URL", "API_KEY", "AWS_SECRET_ACCESS_KEY"]
patterns = ["Bearer [A-Za-z0-9._~+/=-]+", "ghp_[A-Za-z0-9]{36}"]

[[hooks]]
name = "Deploy App"
slug = "deploy-app"
description = "Triggers app deployment"
enabled = true
cwd = "/opt/app"
timeout = "120s"

[hooks.auth]
mode = "bearer"
token = "secret-deploy-token"

[hooks.executor]
type = "shell"
command = "echo 'deploying $APP_ENV'"

[hooks.env]
APP_ENV = "production"

[hooks.retries]
count = 2
backoff = "exponential"
initial_delay = "2s"
max_delay = "30s"

[hooks.rate_limit]
max_per_minute = 5

[hooks.trigger_rules]
cooldown = "30s"
payload_filters = [{ field = "action", operator = "equals", value = "deploy" }]

[hooks.trigger_rules.rate_limit]
max_requests = 10
window = "1h"

[[hooks.trigger_rules.time_windows]]
days = ["Mon", "Tue", "Wed", "Thu", "Fri"]
start_time = "09:00"
end_time = "17:00"

[hooks.concurrency]
mode = "mutex"

[hooks.approval]
required = true
timeout = "30m"

[hooks.notification]
url = "https://hooks.slack.com/services/T00/B00/xxx"
on = ["failure", "timeout"]
body = '{"text": "Hook {{hook_name}} {{outcome}}"}'

[backup]
endpoint = "https://s3.amazonaws.com"
bucket = "sendword-backups"
access_key = "AKIA..."
secret_key = "..."
region = "us-east-1"
prefix = "prod/"
schedule = "0 0 3 * * *"

[backup.retention]
max_count = 30
max_age = "90d"
```

### Executor types

**Shell** --- runs a command in a shell process:

```toml
[hooks.executor]
type = "shell"
command = "deploy.sh --env production"
```

**Script** --- runs a managed script from the scripts directory:

```toml
[hooks.executor]
type = "script"
path = "deploy.sh"
```

**HTTP** --- forwards to an endpoint:

```toml
[hooks.executor]
type = "http"
method = "POST"
url = "https://api.example.com/deploy"
headers = { Authorization = "Bearer token" }
body = '{"ref": "main"}'
follow_redirects = true
```

### Webhook authentication

**Bearer token:**

```toml
[hooks.auth]
mode = "bearer"
token = "your-secret-token"
```

Send as: `Authorization: Bearer your-secret-token`

**HMAC-SHA256:**

```toml
[hooks.auth]
mode = "hmac"
header = "X-Hub-Signature-256"
algorithm = "sha256"
secret = "your-hmac-secret"
```

Compatible with GitHub webhook signatures.

### Trigger rules

Control when a hook fires:

```toml
[hooks.trigger_rules]
cooldown = "60s"  # minimum time between executions

# Only fire when payload matches
payload_filters = [
  { field = "action", operator = "equals", value = "deploy" },
  { field = "environment", operator = "contains", value = "prod" },
  { field = "tag", operator = "regex", value = "^v\\d+\\.\\d+\\.\\d+$" },
  { field = "metadata.priority", operator = "gte", value = "5" },
]

# Rate limit triggers
[hooks.trigger_rules.rate_limit]
max_requests = 10
window = "1h"

# Only allow during business hours
[[hooks.trigger_rules.time_windows]]
days = ["Mon", "Tue", "Wed", "Thu", "Fri"]
start_time = "09:00"
end_time = "17:00"
```

Filter operators: `equals`, `not_equals`, `contains`, `regex`, `exists`, `gt`, `lt`, `gte`, `lte`.

### Execution barriers

Prevent conflicting concurrent executions:

```toml
# Mutex: only one execution at a time, others are rejected
[hooks.concurrency]
mode = "mutex"

# Queue: executions wait in line
[hooks.concurrency]
mode = "queue"
queue_depth = 10
```

Gate hooks behind human approval:

```toml
[hooks.approval]
required = true
timeout = "30m"  # optional, auto-reject after timeout
```

### Environment variable overrides

Every config field can be set via environment variables:

```sh
SENDWORD_SERVER__PORT=9090
SENDWORD_DATABASE__PATH=/var/lib/sendword/db.sqlite
SENDWORD_AUTH__SESSION_LIFETIME=48h
SENDWORD_DEFAULTS__TIMEOUT=60s
```

## CLI

```
sendword [COMMAND]

Commands:
  serve     Start the web server (default)
  export    Export current config as JSON to stdout
  import    Import config from a JSON file
  user      User management
  backup    Backup management
  restore   Restore from a backup
```

### Examples

```sh
# Start the server
sendword serve

# Export config for version control or migration
sendword export > config-backup.json

# Import config from JSON
sendword import config.json

# Create a user
sendword user create --email admin@example.com

# Create a backup
sendword backup create

# List backups
sendword backup list

# Restore from backup
sendword restore --from backups/2026-04-30.tar.gz --output restored/
```

## Triggering hooks

Send an HTTP POST to `/hook/<slug>`:

```sh
# Simple trigger
curl -X POST http://localhost:8080/hook/deploy-app

# With payload
curl -X POST http://localhost:8080/hook/deploy-app \
  -H "Content-Type: application/json" \
  -d '{"action": "deploy", "environment": "production"}'

# With bearer auth
curl -X POST http://localhost:8080/hook/deploy-app \
  -H "Authorization: Bearer secret-deploy-token" \
  -d '{"action": "deploy"}'
```

## Development

sendword uses [Nix flakes](https://nixos.org/manual/nix/stable/command-ref/new-cli/nix3-flake.html) for development environment management and [just](https://just.systems/) as a command runner.

```sh
just          # list available commands
just run      # cargo run
just check    # cargo check
just test     # cargo test
just clippy   # cargo clippy -- -D warnings
just fmt      # cargo fmt
just watch    # bacon (file watcher)
just build    # cargo build --release
```

### Database

```sh
just migrate          # run pending migrations
just migrate-new NAME # create a new migration
just sqlx-prepare     # prepare sqlx offline queries
just sqlx-reset       # reset database and re-run migrations
```

### Tech stack

| Layer | Choice |
|-------|--------|
| Language | Rust (nightly) |
| Async runtime | Tokio |
| Web framework | Axum |
| Database | SQLite via SQLx |
| Templating | MiniJinja |
| Frontend | HTMX + Tailwind CSS |
| Config | Figment (TOML + JSON + env) |

## License

See [LICENSE](LICENSE) for details.
