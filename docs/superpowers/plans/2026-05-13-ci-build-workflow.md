# CI Build Workflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a build-only GitHub Actions workflow that compiles the `sendword` binary on Linux and Windows when Cargo inputs change on `main`.

**Architecture:** The workflow is isolated in `.github/workflows/build.yml` and does not alter application code. A matrix job runs the same release build on `ubuntu-latest` and `windows-latest`, installs Rust from `rust-toolchain.toml`, and restores a Cargo-aware cache before building with `--locked`.

**Tech Stack:** GitHub Actions, `actions-rust-lang/setup-rust-toolchain@v1`, `Swatinem/rust-cache@v2`, Cargo, Nix-provided `actionlint`.

---

## File Structure

- Create `.github/workflows/build.yml`: build-only workflow for Cargo manifest changes on `main`.
- Reference `docs/superpowers/specs/2026-05-13-ci-build-workflow-design.md`: approved design; no edits needed.
- No Rust source files, templates, migrations, or website files are modified for this task.

## Task 1: Add The Build Workflow

**Files:**
- Create: `.github/workflows/build.yml`
- Test: `.github/workflows/build.yml` through `actionlint`

- [ ] **Step 1: Create the workflow file**

Create `.github/workflows/build.yml` with exactly this content:

```yaml
name: Build

on:
  push:
    branches: ["main"]
    paths:
      - "Cargo.toml"
      - "Cargo.lock"
      - "rust-toolchain.toml"
      - ".github/workflows/build.yml"

permissions:
  contents: read

concurrency:
  group: "build-${{ github.ref }}"
  cancel-in-progress: true

jobs:
  build:
    name: Build (${{ matrix.os }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os:
          - ubuntu-latest
          - windows-latest

    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Setup Rust toolchain
        uses: actions-rust-lang/setup-rust-toolchain@v1

      - name: Cache Cargo
        uses: Swatinem/rust-cache@v2
        continue-on-error: true

      - name: Build release binary
        run: cargo build --release --locked
```

- [ ] **Step 2: Verify the workflow trigger**

Run:

```bash
sed -n '1,80p' .github/workflows/build.yml
```

Expected: output shows `push`, `branches: ["main"]`, and the four path filters:

```text
Cargo.toml
Cargo.lock
rust-toolchain.toml
.github/workflows/build.yml
```

- [ ] **Step 3: Verify the workflow job shape**

Run:

```bash
rg -n "build:|ubuntu-latest|windows-latest|actions-rust-lang/setup-rust-toolchain@v1|Swatinem/rust-cache@v2|continue-on-error: true|cargo build --release --locked" .github/workflows/build.yml
```

Expected: output shows a single `build` job with:

```text
build:
ubuntu-latest
windows-latest
actions-rust-lang/setup-rust-toolchain@v1
Swatinem/rust-cache@v2
continue-on-error: true
cargo build --release --locked
```

## Task 2: Validate And Commit

**Files:**
- Modify: `.github/workflows/build.yml`
- Test: `.github/workflows/build.yml`

- [ ] **Step 1: Run actionlint**

Run:

```bash
nix shell nixpkgs#actionlint -c actionlint .github/workflows/build.yml
```

Expected: exits 0 with no diagnostics.

- [ ] **Step 2: Check whitespace**

Run:

```bash
git diff --check
```

Expected: exits 0 with no whitespace errors.

- [ ] **Step 3: Confirm only intended files changed for implementation**

Run:

```bash
git status --short .github/workflows/build.yml
```

Expected:

```text
?? .github/workflows/build.yml
```

If the file is already staged, expected output is:

```text
A  .github/workflows/build.yml
```

- [ ] **Step 4: Commit the workflow**

Run:

```bash
git add .github/workflows/build.yml
git commit -m "ci: build rust binary on manifest changes"
```

Expected: commit succeeds and includes only `.github/workflows/build.yml`.

- [ ] **Step 5: Close the br task after the implementation commit**

Run:

```bash
br close sendword-oc4.1 --reason "Build workflow added and statically validated"
```

Expected: `sendword-oc4.1` is closed, making `sendword-oc4.2` the next ready task.

- [ ] **Step 6: Commit the br status update**

Run:

```bash
git add .beads/issues.jsonl
git commit -m "chore: close ci build workflow task"
```

Expected: commit succeeds and includes only `.beads/issues.jsonl`.

## Self-Review Notes

- Spec coverage: the plan creates exactly one workflow, triggers only on pushes to `main`, uses the approved Cargo path filters, builds on Linux and Windows, installs Rust from `rust-toolchain.toml`, uses `Swatinem/rust-cache@v2`, and runs `cargo build --release --locked`.
- Scope: no release artifacts, test jobs, crates.io publishing, Docker images, or runtime script behavior are included.
- Validation: static workflow validation uses `actionlint`; whitespace validation uses `git diff --check`.
