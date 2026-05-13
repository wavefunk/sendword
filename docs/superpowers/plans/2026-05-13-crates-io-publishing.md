# Crates.io Publishing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prepare `sendword` for crates.io publication and add a tag-triggered workflow that publishes an installable binary crate.

**Architecture:** Package metadata and the crate file include list live in `Cargo.toml`. The GitHub Actions workflow uses crates.io Trusted Publishing and publishes only from version tags. Runtime static assets are served from the packaged crate source path so a binary built by `cargo install sendword` can find its dashboard assets from any working directory.

**Tech Stack:** Cargo package metadata, GitHub Actions, `actions-rust-lang/setup-rust-toolchain@v1`, `Swatinem/rust-cache@v2`, `rust-lang/crates-io-auth-action@v1`, Axum/Tower HTTP static file serving.

---

## File Structure

- Modify `Cargo.toml`: add crates.io metadata and an explicit package include list.
- Modify `src/server.rs`: add `static_dir()` and use it for `ServeDir`.
- Create `.github/workflows/publish-crate.yml`: tag-triggered trusted publishing workflow.
- Create `docs/release/crates-io.md`: maintainer setup and release instructions.
- Reference `docs/superpowers/specs/2026-05-13-crates-io-publishing-design.md`: approved design; no edits needed.

## Task 1: Prepare Package Metadata

**Files:**
- Modify: `Cargo.toml`
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
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "README.md",
    "LICENSE",
    "sendword.toml",
    "sqlx.toml",
]
```

- [ ] **Step 2: Verify metadata parses**

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

## Task 2: Make Static Assets Work For Cargo-Installed Binaries

**Files:**
- Modify: `src/server.rs`
- Test: `src/server.rs` unit test

- [ ] **Step 1: Add `PathBuf` import**

In `src/server.rs`, change:

```rust
use std::net::SocketAddr;
use std::sync::Arc;
```

to:

```rust
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
```

- [ ] **Step 2: Add the static directory helper**

In `src/server.rs`, immediately before `pub fn router`, add:

```rust
pub fn static_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static")
}
```

- [ ] **Step 3: Use the helper in the router**

In `src/server.rs`, change:

```rust
pub fn router(state: Arc<AppState>, auth_router: Router) -> Router {
    let static_dir = ServeDir::new("static");
```

to:

```rust
pub fn router(state: Arc<AppState>, auth_router: Router) -> Router {
    let static_dir = ServeDir::new(static_dir());
```

- [ ] **Step 4: Add a focused unit test**

At the bottom of `src/server.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::static_dir;

    #[test]
    fn static_dir_points_to_packaged_static_assets() {
        let dir = static_dir();

        assert!(dir.ends_with("static"));
        assert!(
            dir.join("css").exists(),
            "static css directory must be included in the crate package"
        );
    }
}
```

- [ ] **Step 5: Run the focused test**

Run:

```bash
nix develop -c cargo test server::tests::static_dir_points_to_packaged_static_assets
```

Expected: one test runs and passes.

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
- Modify: `src/server.rs`
- Create: `.github/workflows/publish-crate.yml`
- Create: `docs/release/crates-io.md`

- [ ] **Step 1: Run cargo publish dry run**

Run:

```bash
nix develop -c cargo publish --dry-run --locked --allow-dirty
```

Expected: exits 0. The output packages `sendword v0.0.2` and verifies the package. `--allow-dirty` is allowed only for local verification because this worktree has unrelated user changes.

- [ ] **Step 2: Check whitespace**

Run:

```bash
git diff --check -- Cargo.toml src/server.rs .github/workflows/publish-crate.yml docs/release/crates-io.md
```

Expected: exits 0 with no whitespace errors.

- [ ] **Step 3: Confirm only intended implementation files changed**

Run:

```bash
git status --short Cargo.toml src/server.rs .github/workflows/publish-crate.yml docs/release/crates-io.md
```

Expected output includes only:

```text
 M Cargo.toml
 M src/server.rs
?? .github/workflows/publish-crate.yml
?? docs/release/crates-io.md
```

- [ ] **Step 4: Commit implementation**

Run:

```bash
git add Cargo.toml src/server.rs .github/workflows/publish-crate.yml docs/release/crates-io.md
git commit -m "release: add crates.io publishing"
```

Expected: commit succeeds and includes only the four implementation files.

- [ ] **Step 5: Close the br task after review approval**

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

- Spec coverage: the plan adds metadata, package include rules, trusted publishing, tag/version verification, cargo-install static asset lookup, and maintainer docs.
- Scope: no GitHub release binary artifacts, curl installer, Docker images, or JavaScript/Python executor behavior are included.
- Validation: local dry-run packaging verifies the crate can be packaged and built; `actionlint` verifies the workflow syntax; focused unit test covers the static asset path.
