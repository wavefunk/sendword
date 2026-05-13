# Crates.io Publishing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prepare `sendword` for crates.io publication and add a tag-triggered workflow that publishes an installable binary crate.

**Architecture:** Package metadata and the crate file include list live in `Cargo.toml`. The GitHub Actions workflow uses crates.io Trusted Publishing and publishes only from version tags. Runtime static assets are embedded into release builds with `rust-embed`, so binaries installed by `cargo install sendword` and later standalone release binaries can serve dashboard assets from any working directory.

**Tech Stack:** Cargo package metadata, GitHub Actions, `actions-rust-lang/setup-rust-toolchain@v1`, `Swatinem/rust-cache@v2`, `rust-lang/crates-io-auth-action@v1`, Axum static routes, `rust-embed`, `mime_guess`.

---

## File Structure

- Modify `Cargo.toml`: add crates.io metadata, an explicit package include list, and static embedding dependencies.
- Modify `Cargo.lock`: record `rust-embed` and `mime_guess` dependency resolution.
- Modify `src/server.rs`: replace filesystem static serving with an embedded static asset route.
- Create `.github/workflows/publish-crate.yml`: tag-triggered trusted publishing workflow.
- Create `docs/release/crates-io.md`: maintainer setup and release instructions.
- Reference `docs/superpowers/specs/2026-05-13-crates-io-publishing-design.md`: approved design, including the package include list.

## Task 1: Prepare Package Metadata

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Test: `Cargo.toml` through `cargo metadata` and `cargo publish --dry-run`

- [ ] **Step 1: Add crates.io metadata to the package section**

In `Cargo.toml`, update the `[package]` section from:

```toml
[package]
name = "sendword"
version = "0.0.2"
authors = ["wavefunk"]
description = "Simple HTTP webhook to command runner sidecar. Frontend for managing hooks, JSON state for config portability, SQLite for execution history and logs."
edition = "2024"
```

to:

```toml
[package]
name = "sendword"
version = "0.0.2"
authors = ["wavefunk"]
description = "Simple HTTP webhook to command runner sidecar. Frontend for managing hooks, JSON state for config portability, SQLite for execution history and logs."
edition = "2024"
license = "MIT"
homepage = "https://sendword.online"
repository = "https://github.com/wavefunk/sendword"
readme = "README.md"
keywords = ["webhook", "automation", "sidecar", "http", "cli"]
categories = ["command-line-utilities", "web-programming::http-server"]
include = [
    "src/**",
    "templates/**",
    "static/**",
    "migrations/**",
    "/Cargo.toml",
    "/Cargo.lock",
    "/build.rs",
    "/README.md",
    "/LICENSE",
    "/sendword.toml",
    "/sqlx.toml",
]
```

- [ ] **Step 2: Add static embedding dependencies**

In `Cargo.toml`, add these dependencies near the other runtime dependencies:

```toml
rust-embed = "8.11.0"
mime_guess = "2.0.5"
```

- [ ] **Step 3: Verify metadata parses and update the lockfile**

Run:

```bash
nix develop -c cargo metadata --no-deps --format-version 1
```

Expected: exits 0 and the `sendword` package JSON includes:

```text
"license":"MIT"
"repository":"https://github.com/wavefunk/sendword"
"homepage":"https://sendword.online"
```

## Task 2: Embed Static Assets For Installed Binaries

**Files:**
- Modify: `src/server.rs`
- Test: `src/server.rs` unit test

- [ ] **Step 1: Update imports**

In `src/server.rs`, remove these imports:

```rust
use std::path::PathBuf;
use tower_http::services::ServeDir;
```

Then change the Axum imports from:

```rust
use axum::Router;
use axum::extract::{State, connect_info::IntoMakeServiceWithConnectInfo};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
```

to:

```rust
use axum::body::Body;
use axum::extract::{Path, State, connect_info::IntoMakeServiceWithConnectInfo};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
```

- [ ] **Step 2: Add embedded static assets and response helper**

In `src/server.rs`, replace the existing `static_dir()` helper with:

```rust
#[derive(rust_embed::RustEmbed)]
#[folder = "static"]
struct StaticAssets;

pub fn embedded_static_response(path: &str) -> Response {
    let path = path.trim_start_matches('/');
    let Some(file) = StaticAssets::get(path) else {
        return (StatusCode::NOT_FOUND, "static asset not found").into_response();
    };

    let content_type = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Body::from(file.data.into_owned()).into_response();
    let header_value =
        HeaderValue::from_str(content_type.as_ref()).unwrap_or(HeaderValue::from_static(
            "application/octet-stream",
        ));
    response.headers_mut().insert(header::CONTENT_TYPE, header_value);
    response
}

async fn static_asset(Path(path): Path<String>) -> Response {
    embedded_static_response(&path)
}
```

- [ ] **Step 3: Route `/static` through the embedded handler**

In `src/server.rs`, change:

```rust
pub fn router(state: Arc<AppState>, auth_router: Router) -> Router {
    let static_dir = ServeDir::new(static_dir());

    Router::new()
        .merge(crate::routes::router())
        .nest_service("/static", static_dir)
```

to:

```rust
pub fn router(state: Arc<AppState>, auth_router: Router) -> Router {
    Router::new()
        .merge(crate::routes::router())
        .route("/static/{*path}", get(static_asset))
```

- [ ] **Step 4: Add a focused unit test**

At the bottom of `src/server.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::embedded_static_response;
    use axum::http::{StatusCode, header};

    #[test]
    fn embedded_static_response_serves_css_asset() {
        let response = embedded_static_response("css/wavefunk.css");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/css")
        );
    }

    #[test]
    fn embedded_static_response_404s_missing_asset() {
        let response = embedded_static_response("missing.css");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
```

- [ ] **Step 5: Run the focused test**

Run:

```bash
nix develop -c cargo test server::tests::embedded_static_response
```

Expected: two tests run and pass.

## Task 3: Add The Publish Workflow

**Files:**
- Create: `.github/workflows/publish-crate.yml`
- Test: `.github/workflows/publish-crate.yml` through `actionlint`

- [ ] **Step 1: Create the workflow file**

Create `.github/workflows/publish-crate.yml` with exactly this content:

```yaml
name: Publish Crate

on:
  push:
    tags:
      - "v*.*.*"

permissions:
  contents: read
  id-token: write

concurrency:
  group: "publish-crate-${{ github.ref }}"
  cancel-in-progress: false

jobs:
  publish:
    name: Publish to crates.io
    runs-on: ubuntu-latest
    environment: release

    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Setup Rust toolchain
        uses: actions-rust-lang/setup-rust-toolchain@v1

      - name: Cache Cargo
        uses: Swatinem/rust-cache@v2
        continue-on-error: true

      - name: Verify tag matches package version
        shell: bash
        run: |
          package_version="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json, sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
          expected_tag="v${package_version}"
          if [ "${GITHUB_REF_NAME}" != "${expected_tag}" ]; then
            echo "Tag ${GITHUB_REF_NAME} does not match Cargo package version ${expected_tag}" >&2
            exit 1
          fi

      - name: Package dry run
        run: cargo publish --dry-run --locked

      - name: Authenticate to crates.io
        id: auth
        uses: rust-lang/crates-io-auth-action@v1

      - name: Publish crate
        run: cargo publish --locked
        env:
          CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}
```

- [ ] **Step 2: Run actionlint**

Run:

```bash
nix shell nixpkgs#actionlint -c actionlint .github/workflows/publish-crate.yml
```

Expected: exits 0 with no diagnostics.

## Task 4: Document Crates.io Release Setup

**Files:**
- Create: `docs/release/crates-io.md`

- [ ] **Step 1: Create the release documentation**

Create `docs/release/crates-io.md` with exactly this content:

````markdown
# Crates.io Release Setup

`sendword` is published as a binary crate so users can install it with:

```sh
cargo install sendword
```

## One-Time Setup

1. Confirm the crate name is still available:

   ```sh
   curl -f https://index.crates.io/se/nd/sendword
   ```

   A 404 means the name has not been allocated in the sparse index.

2. Check the package contents:

   ```sh
   cargo package --list
   ```

3. Run a local publish dry run:

   ```sh
   cargo publish --dry-run --locked
   ```

4. Publish the first version manually if crates.io requires an existing crate before trusted publishing can be configured:

   ```sh
   cargo publish --locked
   ```

5. In crates.io, configure Trusted Publishing for this repository:

   - Repository: `wavefunk/sendword`
   - Workflow: `.github/workflows/publish-crate.yml`
   - Environment: `release`

## Release Flow

1. Update `version` in `Cargo.toml`.
2. Commit the version change.
3. Tag the release with the package version:

   ```sh
   git tag v0.0.2
   git push origin v0.0.2
   ```

4. GitHub Actions publishes the crate when the tag matches the Cargo package version.
````

## Task 5: Validate And Commit

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `src/server.rs`
- Create: `.github/workflows/publish-crate.yml`
- Create: `docs/release/crates-io.md`

- [ ] **Step 1: Check package contents**

Run:

```bash
nix develop -c cargo package --list --allow-dirty
```

Expected: exits 0 and lists only files intentionally included in the crate package. The output must not include `.direnv/`, `website/`, `.beads/`, or `docs/superpowers/` entries.

- [ ] **Step 2: Run cargo publish dry run**

Run:

```bash
nix develop -c cargo publish --dry-run --locked --allow-dirty
```

Expected: exits 0. The output packages `sendword v0.0.2` and verifies the package. `--allow-dirty` is allowed only for local verification because this worktree has unrelated user changes.

- [ ] **Step 3: Check whitespace**

Run:

```bash
git diff --check -- Cargo.toml Cargo.lock src/server.rs .github/workflows/publish-crate.yml docs/release/crates-io.md
```

Expected: exits 0 with no whitespace errors.

- [ ] **Step 4: Confirm only intended implementation files changed**

Run:

```bash
git status --short Cargo.toml Cargo.lock src/server.rs .github/workflows/publish-crate.yml docs/release/crates-io.md
```

Expected output includes only:

```text
 M Cargo.toml
 M Cargo.lock
 M src/server.rs
?? .github/workflows/publish-crate.yml
?? docs/release/crates-io.md
```

- [ ] **Step 5: Commit implementation**

Run:

```bash
git add Cargo.toml Cargo.lock src/server.rs .github/workflows/publish-crate.yml docs/release/crates-io.md
git commit -m "release: add crates.io publishing"
```

Expected: commit succeeds and includes only the five implementation files.

- [ ] **Step 6: Close the br task after review approval**

Run:

```bash
br close sendword-oc4.2 --reason "Crates.io metadata and trusted publishing workflow added"
```

Expected: `sendword-oc4.2` is closed, making `sendword-oc4.3` the next ready task.

- [ ] **Step 6: Commit the br status update**

Run:

```bash
git add .beads/issues.jsonl
git commit -m "chore: close crates.io publishing task"
```

Expected: commit succeeds and includes only `.beads/issues.jsonl`.

## Self-Review Notes

- Spec coverage: the plan adds metadata, package include rules, trusted publishing, tag/version verification, embedded static assets for installed binaries, and maintainer docs.
- Scope: no GitHub release binary artifacts, curl installer, Docker images, or JavaScript/Python executor behavior are included.
- Validation: local dry-run packaging verifies the crate can be packaged and built; `actionlint` verifies the workflow syntax; focused unit tests cover found and missing embedded static assets.
