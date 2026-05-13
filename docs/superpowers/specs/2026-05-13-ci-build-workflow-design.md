# CI Build Workflow Design Spec

## Motivation

The first release/distribution task is to add a narrow GitHub Actions build lane
for the `sendword` Rust binary. This is not a release workflow and does not
publish artifacts. Its purpose is to catch package-level build breakage on the
two desktop/server platforms we intend to distribute first: Linux and Windows.

## Scope

This task adds one workflow:

- `.github/workflows/build.yml`

The workflow runs only on pushes to `main` when one of these files changes:

- `Cargo.toml`
- `Cargo.lock`
- `rust-toolchain.toml`
- `.github/workflows/build.yml`

The workflow is build-only. It does not run tests, upload binaries, publish to
crates.io, build Docker images, or create GitHub releases. Those are separate
`br` tasks in the release sequence.

## Workflow Design

The workflow is named `Build` and contains one matrix job:

- `ubuntu-latest`
- `windows-latest`

Each matrix entry runs the same steps:

1. Check out the repository.
2. Install the Rust toolchain from the repository's `rust-toolchain.toml`.
3. Restore/save a Cargo-aware cache.
4. Build the binary with `cargo build --release --locked`.

The toolchain step uses `actions-rust-lang/setup-rust-toolchain@v1` without an
explicit `toolchain` input. In that mode, the action reads `rust-toolchain.toml`
from the repository root and installs the requested channel, profile, and
components. This keeps CI tied to the same pinned toolchain as local
development.

The cache step uses `Swatinem/rust-cache@v2`. It is a Rust-specific cache action
that includes Cargo registry/git dependencies and dependency build artifacts,
and its cache key accounts for `Cargo.toml`, `Cargo.lock`, and
`rust-toolchain.toml`. The cache step runs after toolchain installation so the
cache key includes the active compiler version.

## Error Handling

GitHub Actions handles workflow failures through the job exit status:

- If toolchain installation fails, the matrix entry fails before building.
- If cache restore fails, the workflow continues because caches are an
  optimization, not a correctness requirement.
- If `cargo build --release --locked` fails on either platform, the workflow
  fails.
- If `Cargo.lock` is not in sync with `Cargo.toml`, `--locked` fails the build
  instead of updating the lockfile in CI.

## Testing

Static validation is enough for this task because the change is declarative CI
configuration:

- Run a YAML parse check if a local checker is available.
- Run `git diff --check`.
- Inspect the workflow trigger, matrix, toolchain action, cache action, and
  build command before committing.

The actual cross-platform behavior is verified by GitHub Actions after the
workflow lands on `main`.
