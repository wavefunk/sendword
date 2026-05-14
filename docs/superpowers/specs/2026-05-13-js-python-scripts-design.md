# JavaScript and Python Script Executors Design

## Goal

Make JavaScript and Python scripts first-class hook executors while preserving the
existing shell, direct executable script, and HTTP executor behavior.

## Context

Today `ExecutorConfig` is a tagged enum with `shell`, `script`, and `http`.
The existing `script` executor runs the configured file directly, so JavaScript
and Python files only work when they are executable and have a valid shebang.
That is awkward for managed scripts and for the Docker image, which now ships
Node.js and Python specifically for webhook script handlers.

The hook form is also shell-centric. It only submits a `command` field, and the
config writer always writes `type = "shell"`. That means editing a script hook
through the UI can silently convert it to a shell hook.

## Approaches Considered

### Recommended: First-class executor variants

Add explicit `javascript { path }` and `python { path }` variants beside the
existing `script { path }` variant.

Pros:
- TOML stays obvious: `type = "javascript"` or `type = "python"`.
- Existing `script` semantics stay backward compatible.
- Runtime errors can name the missing runtime directly.
- UI can present a simple executor type selector.

Cons:
- Adds variants that must be handled anywhere `ExecutorConfig` is matched.

### Alternative: `script.runtime`

Keep one `script` variant and add `runtime = "direct" | "javascript" |
"python" | "auto"`.

Pros:
- Groups all file-based executors under one variant.
- Leaves fewer top-level executor variants.

Cons:
- Adds a migration-shaped change to an existing variant.
- `auto` invites hidden behavior and extension guessing.
- UI labels become less direct.

### Alternative: File-extension auto detection

Keep `script { path }` and infer runtime from `.js` or `.py`.

Pros:
- No config shape change.

Cons:
- Hidden behavior.
- Extensionless scripts and shebang-based scripts become ambiguous.
- A renamed file can change runtime behavior.

## Decision

Use first-class executor variants:

```toml
[[hooks]]
name = "Deploy JS"
slug = "deploy-js"

[hooks.executor]
type = "javascript"
path = "data/scripts/deploy.js"
```

```toml
[[hooks]]
name = "Deploy Python"
slug = "deploy-python"

[hooks.executor]
type = "python"
path = "data/scripts/deploy.py"
```

Existing direct executable scripts remain:

```toml
[hooks.executor]
type = "script"
path = "data/scripts/deploy.sh"
```

## Runtime Behavior

`script` keeps its current direct execution behavior:

```text
<path>
```

`javascript` runs:

```text
node <path>
```

`python` tries:

```text
python3 <path>
python <path>
```

The Python executor only tries `python` if spawning `python3` fails because the
runtime program is not found. Other spawn failures should be reported
immediately. In particular, an invalid `cwd` must not be reported as a missing
runtime.

All script-like runtimes must share the current script behavior:
- clear the environment before execution;
- apply the whitelisted system environment from `system_env_vars()`;
- apply hook-specific environment variables after that, allowing hook env to
  override `PATH`;
- set `SENDWORD_EXECUTION_ID`, `SENDWORD_HOOK_SLUG`, and `SENDWORD_PAYLOAD`;
- flatten JSON payload leaves into `SENDWORD_FIELD_*` variables;
- write `payload.json` to the execution log directory;
- honor `cwd`;
- stream stdout and stderr to log files;
- enforce timeout and kill the process on expiry;
- mark execution status and exit code in SQLite.

This env handling is part of the JavaScript and Python contract, not an
implementation detail. A Node.js script must be able to read hook env variables
through `process.env`, and a Python script must be able to read them through
`os.environ`. That includes user-defined hook env vars and the `SENDWORD_*`
execution variables.

If a runtime is unavailable, the execution should fail cleanly with:
- status `failed`;
- no exit code;
- a stderr log line naming the runtime candidates that were tried;
- no panic.

## Config And Validation

`ExecutorConfig` gains:

```rust
#[serde(rename = "javascript")]
JavaScript { path: String },
#[serde(rename = "python")]
Python { path: String },
```

Validation must reject empty `path` values for `script`, `javascript`, and
`python`, just as it rejects empty shell commands today.

The config writer should write executor tables from an `ExecutorConfig` value
instead of hard-coding shell output. This prevents UI edits from losing
script-like executor type information.

## UI Impact

The hook form should expose an executor type selector for:
- shell command;
- executable script;
- JavaScript script;
- Python script.

The existing `command` input can remain the submitted value field for backward
compatibility with current tests and old form posts. The label, placeholder, and
hint should change based on the selected executor type.

Edit forms should preselect the executor type for existing shell, script,
javascript, and python hooks. Hook detail should display `javascript` and
`python` clearly and should offer the managed script edit link for all
path-based script executors whose path is under the configured scripts
directory.

HTTP executor editing is not part of this feature. Existing config-level HTTP
support must still compile and run.

## Shared Resolver

Executor resolution is currently duplicated in webhook trigger, approval/replay,
and barrier code. Add one shared resolver that maps `ExecutorConfig` plus
payload JSON to `ResolvedExecutor`. It should:
- interpolate shell commands;
- interpolate HTTP URL and body;
- map `script`, `javascript`, and `python` to one script resolved executor with
  an explicit runtime enum.

This keeps future executor variants from requiring copy/paste updates in three
execution paths.

## Tests

Add focused tests for:
- parsing `javascript` and `python` executor TOML;
- validation failures for empty script-like paths;
- config writer output for shell, script, javascript, and python executors;
- resolver mapping for script-like executor variants;
- JavaScript execution using a fake `node` on `PATH`;
- Python execution using fake `python3` and fallback to fake `python`;
- JavaScript receives hook env vars, `SENDWORD_PAYLOAD`, and flattened
  `SENDWORD_FIELD_*` variables through `process.env`;
- Python receives hook env vars, `SENDWORD_PAYLOAD`, and flattened
  `SENDWORD_FIELD_*` variables through `os.environ`;
- unavailable runtime failure writes a useful stderr log;
- hook form create/edit preserves selected executor type.

Runtime tests should not depend on system Node.js or Python. They should use a
temporary directory containing fake `node`, `python3`, or `python` executables
and set the hook environment `PATH` to that directory.

## Acceptance Criteria

- Existing `script` hooks keep direct executable behavior.
- New TOML `type = "javascript"` runs `node <path>`.
- New TOML `type = "python"` runs `python3 <path>` or falls back to
  `python <path>` when `python3` is not found.
- Missing runtimes fail cleanly and log a runtime-not-found message.
- Non-runtime spawn failures, such as an invalid `cwd`, fail cleanly without
  falling back to another runtime candidate.
- Payload environment, `payload.json`, logging, cwd, timeout, and DB status
  behavior match the current script executor.
- Hook env vars and all `SENDWORD_*` vars are visible inside Node.js
  `process.env` and Python `os.environ`.
- The hook form can create and edit shell, script, javascript, and python hooks
  without converting script-like hooks back to shell.
- HTTP executor support remains compile-safe and runtime behavior is unchanged.
