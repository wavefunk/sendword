# Crates.io Publishing Design Spec

## Motivation

`sendword` must be installable as a Rust binary crate with:

```sh
cargo install sendword
```

This task prepares the crate for crates.io publication and adds a tag-triggered
publish workflow. It does not build GitHub release binaries, provide a curl
installer, or build Docker images; those remain separate `br` tasks.

## Current State

The manifest already defines a binary target:

```toml
[[bin]]
name = "sendword"
path = "src/main.rs"
```

The manifest has `name`, `version`, `description`, and `edition`, but it lacks
the publishing metadata Cargo recommends for crates.io discovery:

- `license` or `license-file`
- `homepage`
- `repository`
- `readme`
- `keywords`
- `categories`

The sparse registry index currently returns 404 for `sendword`, so the crate
name appears available at the registry-index level. The first successful
publication still allocates the name permanently.

Templates are embedded at compile time through `minijinja-embed`, but static UI
assets are currently served from the process working directory. A binary
installed with `cargo install sendword` can be launched from any directory, so
static asset lookup must not depend on `./static`.

## Publishing Approach

Use crates.io Trusted Publishing for the GitHub Actions publish workflow. The
workflow requests a short-lived crates.io token through
`rust-lang/crates-io-auth-action@v1`, then runs `cargo publish --locked` with
that token.

This requires a one-time crates.io setup step outside the repository:

1. Publish the first crate version manually if crates.io still requires that for
   new crates before trusted publishing can be configured.
2. Configure the trusted publisher in crates.io for this GitHub repository and
   workflow.
3. Push a tag matching the package version, for example `v0.0.2`.

The workflow lives in `.github/workflows/publish-crate.yml` and runs on tags
matching `v*.*.*`. It does not run on ordinary pushes. The job uses the `release`
environment and requests only these permissions:

- `contents: read`
- `id-token: write`

The workflow verifies that the git tag matches the Cargo package version before
publishing. A tag `v0.0.2` may publish package version `0.0.2`; a mismatch fails
before authentication and publication.

## Manifest Changes

Add publish metadata to `Cargo.toml`:

```toml
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

The explicit `include` list keeps website content, Playwright snapshots, Beads
state, and local planning files out of the published crate while retaining all
files needed to build and run the binary from a crate package.

## Static Asset Lookup

Add a small helper in `src/server.rs`:

```rust
pub fn static_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static")
}
```

Use `ServeDir::new(static_dir())` in the router. This points static serving at
the packaged crate source directory, which exists for binaries built by
`cargo install`. It also works in local development because
`CARGO_MANIFEST_DIR` is the repository root.

This is intentionally not a complete solution for later standalone GitHub
release binaries because those binaries are built on CI machines whose source
paths do not exist on user machines. The release-artifact task will address that
distribution format separately.

## Documentation

Add `docs/release/crates-io.md` with the maintainer steps:

- Check the crate name and package contents.
- Run `cargo publish --dry-run --locked`.
- Perform the initial manual publish if required.
- Configure trusted publishing in crates.io for
  `.github/workflows/publish-crate.yml`.
- Tag releases as `v<package-version>`.

## Testing

Use local static verification for repository changes:

- `nix develop -c cargo publish --dry-run --locked --allow-dirty`
- `nix shell nixpkgs#actionlint -c actionlint .github/workflows/publish-crate.yml`
- `git diff --check`

The `--allow-dirty` flag is only for local verification in this dirty worktree.
The GitHub Actions workflow must not use `--allow-dirty`.
