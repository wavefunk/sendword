# JavaScript and Python Script Executors Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add first-class JavaScript and Python hook executors with explicit runtime behavior, UI support, validation, and tests.

**Architecture:** Add explicit `javascript` and `python` config variants, resolve all config executors through one shared resolver, and generalize the existing script runner with a small runtime enum. The hook form continues posting one executor value field named `command` for compatibility, but adds an executor type selector.

**Tech Stack:** Rust, Tokio, Axum, SQLx, MiniJinja, TOML via `toml_edit`, existing cargo test suite.

---

## Files

- Modify: `src/config.rs`
- Modify: `src/executor/mod.rs`
- Modify: `src/executor/script.rs`
- Modify: `src/routes/hooks.rs`
- Modify: `src/routes/executions.rs`
- Modify: `src/barriers/mod.rs`
- Modify: `src/config_writer.rs`
- Modify: `templates/hook_form.html`
- Modify: `templates/hook_detail.html`
- Modify: `sendword.toml`
- Modify: `README.md`
- Test: existing unit tests in the modified Rust modules
- Test: `tests/server_integration.rs`

---

## Task 1: Add Config Variants And Validation

**Files:**
- Modify: `src/config.rs`

- [ ] Add `JavaScript { path: String }` and `Python { path: String }` variants to `ExecutorConfig`.

Use explicit serde names so TOML uses `javascript`, not `java_script`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutorConfig {
    Shell {
        command: String,
    },
    Script {
        path: String,
    },
    #[serde(rename = "javascript")]
    JavaScript {
        path: String,
    },
    #[serde(rename = "python")]
    Python {
        path: String,
    },
    Http {
        method: HttpMethod,
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default = "default_true")]
        follow_redirects: bool,
    },
}
```

- [ ] Extend hook validation so empty paths fail for all script-like executors.

Add match arms near the existing shell command validation:

```rust
match &hook.executor {
    ExecutorConfig::Shell { command } if command.is_empty() => {
        errors.push(format!("{prefix}.executor.command must be non-empty"));
    }
    ExecutorConfig::Script { path }
    | ExecutorConfig::JavaScript { path }
    | ExecutorConfig::Python { path } if path.is_empty() =>
    {
        errors.push(format!("{prefix}.executor.path must be non-empty"));
    }
    _ => {}
}
```

- [ ] Add config tests for TOML parsing and validation.

Add tests in `src/config.rs`:

```rust
#[test]
fn javascript_executor_config_from_toml() {
    let toml = r#"
        [[hooks]]
        name = "Deploy JS"
        slug = "deploy-js"

        [hooks.executor]
        type = "javascript"
        path = "data/scripts/deploy.js"
    "#;

    let config: AppConfig = Figment::new()
        .merge(Data::<Toml>::string(toml))
        .extract()
        .expect("parse config");

    let ExecutorConfig::JavaScript { path } = &config.hooks[0].executor else {
        panic!("expected javascript executor");
    };
    assert_eq!(path, "data/scripts/deploy.js");
}

#[test]
fn python_executor_config_from_toml() {
    let toml = r#"
        [[hooks]]
        name = "Deploy Python"
        slug = "deploy-python"

        [hooks.executor]
        type = "python"
        path = "data/scripts/deploy.py"
    "#;

    let config: AppConfig = Figment::new()
        .merge(Data::<Toml>::string(toml))
        .extract()
        .expect("parse config");

    let ExecutorConfig::Python { path } = &config.hooks[0].executor else {
        panic!("expected python executor");
    };
    assert_eq!(path, "data/scripts/deploy.py");
}
```

Add a validation test that sets each script-like path to an empty string and
asserts the error contains `executor.path must be non-empty`.

- [ ] Run:

```sh
cargo test executor_config_from_toml
```

- [ ] Commit:

```sh
git add src/config.rs
git commit -m "feat: add script runtime executor config"
```

---

## Task 2: Add Shared Executor Resolution

**Files:**
- Modify: `src/executor/mod.rs`
- Modify: `src/routes/hooks.rs`
- Modify: `src/routes/executions.rs`
- Modify: `src/barriers/mod.rs`

- [ ] Add a script runtime enum and update `ResolvedExecutor`.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptRuntime {
    Direct,
    JavaScript,
    Python,
}

#[derive(Clone)]
pub enum ResolvedExecutor {
    Shell {
        command: String,
    },
    Script {
        path: PathBuf,
        runtime: ScriptRuntime,
    },
    Http {
        method: HttpMethod,
        url: String,
        headers: HashMap<String, String>,
        body: Option<String>,
        follow_redirects: bool,
    },
}
```

- [ ] Add `resolve_executor`.

```rust
pub fn resolve_executor(config: &ExecutorConfig, payload_json: &str) -> ResolvedExecutor {
    match config {
        ExecutorConfig::Shell { command } => {
            let interpolated = if let Ok(payload_value) =
                serde_json::from_str::<serde_json::Value>(payload_json)
            {
                interpolate_command(command, &payload_value).into_owned()
            } else {
                command.clone()
            };
            ResolvedExecutor::Shell {
                command: interpolated,
            }
        }
        ExecutorConfig::Script { path } => ResolvedExecutor::Script {
            path: PathBuf::from(path),
            runtime: ScriptRuntime::Direct,
        },
        ExecutorConfig::JavaScript { path } => ResolvedExecutor::Script {
            path: PathBuf::from(path),
            runtime: ScriptRuntime::JavaScript,
        },
        ExecutorConfig::Python { path } => ResolvedExecutor::Script {
            path: PathBuf::from(path),
            runtime: ScriptRuntime::Python,
        },
        ExecutorConfig::Http {
            method,
            url,
            headers,
            body,
            follow_redirects,
        } => {
            let payload_value: serde_json::Value = serde_json::from_str(payload_json)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            let interpolated_url = interpolate_command(url, &payload_value).into_owned();
            let interpolated_body = body
                .as_deref()
                .map(|b| interpolate_command(b, &payload_value).into_owned());
            ResolvedExecutor::Http {
                method: *method,
                url: interpolated_url,
                headers: headers.clone(),
                body: interpolated_body,
                follow_redirects: *follow_redirects,
            }
        }
    }
}
```

Import `ExecutorConfig` and `crate::interpolation::interpolate_command` in
`src/executor/mod.rs`.

- [ ] Update dispatch to pass the runtime:

```rust
ResolvedExecutor::Script { path, runtime } => {
    script::run_script(pool, &ctx, path, *runtime).await
}
```

- [ ] Replace duplicated route/barrier resolution with calls to
`resolve_executor(&hook.executor, payload_json)`.

- [ ] Add resolver tests for direct, JavaScript, Python, shell interpolation,
and HTTP interpolation.

- [ ] Run:

```sh
cargo test executor::tests::resolve
```

- [ ] Commit:

```sh
git add src/executor/mod.rs src/routes/hooks.rs src/routes/executions.rs src/barriers/mod.rs
git commit -m "refactor: centralize executor resolution"
```

---

## Task 3: Generalize Script Runtime Execution

**Files:**
- Modify: `src/executor/script.rs`

- [ ] Change the signature:

```rust
pub async fn run_script(
    pool: &SqlitePool,
    ctx: &ExecutionContext,
    path: &Path,
    runtime: ScriptRuntime,
) -> ExecutionResult
```

- [ ] Build commands from runtime candidates.

Use these candidates:

```rust
fn runtime_candidates(runtime: ScriptRuntime) -> &'static [&'static str] {
    match runtime {
        ScriptRuntime::Direct => &[],
        ScriptRuntime::JavaScript => &["node"],
        ScriptRuntime::Python => &["python3", "python"],
    }
}
```

For `Direct`, run `Command::new(path)`. For interpreter runtimes, run
`Command::new(candidate).arg(path)`. If a candidate returns
`ErrorKind::NotFound`, try the next candidate. If all candidates are missing,
write a stderr line like:

```text
failed to spawn python runtime; tried: python3, python
```

Only treat `ErrorKind::NotFound` as a missing runtime when `cwd` is not set or
the configured `cwd` exists. If `ctx.cwd` is set and does not exist, fail
immediately with the original spawn error and do not try the next Python
candidate. This prevents a bad working directory from being logged as a missing
runtime.

- [ ] Keep environment, payload fields, cwd, stdout/stderr capture, timeout,
DB status, and result handling identical to the current direct script executor.

- [ ] Add tests using fake runtimes in a temporary `PATH`.

Create fake Unix executables in a temp dir:

```sh
#!/bin/sh
exec "$@"
```

For the fake `node` and `python3` cases, set `ctx.env.insert("PATH", temp_bin)`.
Use actual temporary script files that print `SENDWORD_PAYLOAD` or a payload
field.

- [ ] Add dedicated env propagation tests for JavaScript and Python runtimes.

For JavaScript, use a fake `node` that executes a shell script standing in for
the JS file. The script should print values supplied by the runner:

```sh
#!/bin/sh
printf '%s|%s|%s|%s\n' "$CUSTOM_ENV" "$SENDWORD_EXECUTION_ID" "$SENDWORD_PAYLOAD" "$SENDWORD_FIELD_ACTION"
```

Set `ctx.env.insert("CUSTOM_ENV", "from-hook")`, set `ctx.payload_json` to
`{"action":"deploy"}`, and assert stdout contains the hook env var, execution
id, raw payload, and flattened payload field.

For Python, use the same fake-runtime strategy with a fake `python3` and a
script that prints the same environment variables. This proves the Rust runner
passes env vars into interpreter-launched runtimes, which is the behavior real
Node.js `process.env` and Python `os.environ` depend on.

- [ ] Add a fallback test where `python3` is absent and fake `python` exists.

- [ ] Add an invalid `cwd` test for Python runtime fallback.

Set `ctx.cwd` to a nonexistent directory while a fake `python3` is present on
`PATH`. Assert the execution fails cleanly, stderr mentions the spawn/cwd
failure, and the fake `python` fallback is not invoked.

- [ ] Add a missing-runtime test with `PATH` set to an empty temp dir and assert
status `failed`, no exit code, and stderr mentions the tried runtime.

- [ ] Run:

```sh
cargo test executor::script::tests::
```

- [ ] Commit:

```sh
git add src/executor/script.rs src/executor/mod.rs
git commit -m "feat: run javascript and python script runtimes"
```

---

## Task 4: Preserve Executor Type In The Hook Writer And Form

**Files:**
- Modify: `src/config_writer.rs`
- Modify: `src/routes/hooks.rs`
- Modify: `templates/hook_form.html`
- Modify: `templates/hook_detail.html`

- [ ] Change `HookFormData` to carry `executor: ExecutorConfig` instead of
`command: String`.

```rust
pub struct HookFormData {
    pub name: String,
    pub slug: String,
    pub description: String,
    pub enabled: bool,
    pub executor: ExecutorConfig,
    pub cwd: Option<String>,
    pub env: HashMap<String, String>,
    pub timeout: Option<Duration>,
    pub retries: Option<RetryFormData>,
    pub auth: Option<HookAuthConfig>,
    pub payload: Option<PayloadSchema>,
    pub trigger_rules: Option<TriggerRules>,
}
```

- [ ] Write executor tables from the enum.

```rust
fn executor_table(executor: &ExecutorConfig) -> Table {
    let mut table = Table::new();
    match executor {
        ExecutorConfig::Shell { command } => {
            table.insert("type", toml_string("shell"));
            table.insert("command", toml_string(command));
        }
        ExecutorConfig::Script { path } => {
            table.insert("type", toml_string("script"));
            table.insert("path", toml_string(path));
        }
        ExecutorConfig::JavaScript { path } => {
            table.insert("type", toml_string("javascript"));
            table.insert("path", toml_string(path));
        }
        ExecutorConfig::Python { path } => {
            table.insert("type", toml_string("python"));
            table.insert("path", toml_string(path));
        }
        ExecutorConfig::Http {
            method,
            url,
            headers,
            body,
            follow_redirects,
        } => {
            table.insert("type", toml_string("http"));
            table.insert("method", toml_string(&format!("{method:?}").to_uppercase()));
            table.insert("url", toml_string(url));
            if !headers.is_empty() {
                table.insert("headers", Item::Table(string_map_table(headers)));
            }
            if let Some(body) = body {
                table.insert("body", toml_string(body));
            }
            table.insert("follow_redirects", toml_bool(*follow_redirects));
        }
    }
    table
}

fn string_map_table(values: &HashMap<String, String>) -> Table {
    let mut table = Table::new();
    let mut keys: Vec<&String> = values.keys().collect();
    keys.sort();
    for key in keys {
        table.insert(key, toml_string(&values[key]));
    }
    table
}
```

- [ ] Extend `HookForm` with:

```rust
#[serde(default)]
executor_type: String,
```

Keep `command: String` as the value field.

- [ ] Parse `executor_type`.

Default an empty `executor_type` to `shell` for compatibility with existing
form posts.

```rust
let executor = match form.executor_type.trim() {
    "" | "shell" => ExecutorConfig::Shell {
        command: form.command.trim().to_owned(),
    },
    "script" => ExecutorConfig::Script {
        path: form.command.trim().to_owned(),
    },
    "javascript" => ExecutorConfig::JavaScript {
        path: form.command.trim().to_owned(),
    },
    "python" => ExecutorConfig::Python {
        path: form.command.trim().to_owned(),
    },
    other => return Err(format!("unknown executor type '{other}'")),
};
```

- [ ] Update the hook form template with a select named `executor_type` and
options for shell, script, javascript, and python. Keep the input named
`command`.

- [ ] Add small inline JavaScript to update the command label, placeholder, and
hint based on selected type.

- [ ] Update edit form context so existing script-like executors preselect the
correct type.

- [ ] Update detail rendering so `javascript` and `python` show as their own
types and managed script links work for script-like executors.

- [ ] Add writer and route tests:
  - writer writes `type = "javascript"` and `path = "...js"`;
  - writer writes `type = "python"` and `path = "...py"`;
  - posting `executor_type=javascript` creates a JavaScript hook;
  - editing a JavaScript hook preserves JavaScript type.

- [ ] Run:

```sh
cargo test config_writer::tests:: routes::hooks::tests::
```

- [ ] Commit:

```sh
git add src/config_writer.rs src/routes/hooks.rs templates/hook_form.html templates/hook_detail.html
git commit -m "feat: expose script runtime executors in hook form"
```

---

## Task 5: Document Script Runtime Executors

**Files:**
- Modify: `README.md`

- [ ] Add sample executor snippets for `script`, `javascript`, and `python` to
README.md.

Document:
- direct scripts require executable permissions and a shebang;
- JavaScript requires `node` on `PATH`;
- Python tries `python3`, then `python`;
- Docker image includes Node.js and Python;
- payload data is available through `SENDWORD_PAYLOAD`, `payload.json`, and
  `SENDWORD_FIELD_*`.

- [ ] Do not add active hooks to `sendword.toml`.

If `sendword.toml` is touched, only add commented examples. The file is an
active loadable local config.

- [ ] Run:

```sh
git diff --check -- README.md sendword.toml
```

- [ ] Commit:

```sh
git add README.md sendword.toml
git commit -m "docs: document javascript and python script executors"
```

---

## Task 6: Final Verification

**Files:**
- No code edits unless verification finds a bug.

- [ ] Format:

```sh
cargo fmt --all
```

- [ ] Run focused tests:

```sh
cargo test executor::script::tests::
cargo test config_writer::tests::
cargo test routes::hooks::tests::
```

- [ ] Run full tests:

```sh
cargo test
```

- [ ] Run package/build checks:

```sh
cargo build --release --locked
```

- [ ] Run diff whitespace check:

```sh
git diff --check
```

- [ ] Commit any formatting-only changes if `cargo fmt` changed files:

```sh
git status --short
git add src/config.rs src/executor/mod.rs src/executor/script.rs src/routes/hooks.rs src/routes/executions.rs src/barriers/mod.rs src/config_writer.rs templates/hook_form.html templates/hook_detail.html sendword.toml README.md
git commit -m "style: format script runtime executors"
```
