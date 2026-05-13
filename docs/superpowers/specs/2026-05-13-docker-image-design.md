# Docker Image Publishing Design Spec

## Motivation

`sendword` needs an official container image that includes:

- the `sendword` binary,
- Node.js, so webhook scripts can call JavaScript tools,
- Python, so webhook scripts can call Python tools.

This task builds and publishes the image. It does not add first-class
JavaScript/Python executor configuration; that remains a separate runtime design
and implementation task.

## Image Shape

Use a multi-stage Dockerfile:

1. `chef` stage based on `lukemathwalker/cargo-chef:latest-rust-1`.
2. `planner` stage generates `recipe.json`.
3. `builder` stage caches dependencies with `cargo chef cook`, then builds
   `sendword` in release mode.
4. Runtime stage based on `debian:bookworm-slim`.

The runtime image installs:

- `ca-certificates`
- `nodejs`
- `npm`
- `python3`
- `python3-pip`

It copies `/app/target/release/sendword` to `/usr/local/bin/sendword`, creates
`/data`, exposes port `8080`, and defaults to:

```dockerfile
CMD ["sendword", "serve"]
```

The embedded static assets from the previous task mean the image does not need
to copy the `static/` directory into the runtime stage.

## Ignore Rules

Add `.dockerignore` so the Docker context excludes build products and unrelated
local/project state:

- `target/`
- `.direnv/`
- `.git/`
- `.beads/`
- `.codex/`
- `.claude/`
- `website/dist/`
- `website/.eigen_cache/`
- `node_modules/`
- `.playwright/`
- `.playwright-cli/`
- `data/`

## Publishing Workflow

Add `.github/workflows/docker.yml`.

The workflow runs:

- on pushes to `main` when Docker/image inputs change,
- on version tags `v*.*.*`,
- manually with `workflow_dispatch`.

It publishes to GitHub Container Registry:

```text
ghcr.io/wavefunk/sendword
```

Permissions:

- `contents: read`
- `packages: write`

The workflow uses Docker's official actions:

- `docker/setup-buildx-action@v3`
- `docker/login-action@v3`
- `docker/metadata-action@v5`
- `docker/build-push-action@v7`

Build caching uses the GitHub Actions cache backend:

```yaml
cache-from: type=gha
cache-to: type=gha,mode=max
```

The Dockerfile also uses BuildKit cache mounts for Cargo registry, Cargo git,
and target build output during the dependency and final build steps.

## Tags

`docker/metadata-action` produces:

- semver tags for version tags, for example `0.0.2`,
- `latest` for version tags,
- `main` for pushes to main.

## Documentation

Add `docs/release/docker.md` with:

- the image name,
- pull/run examples,
- notes that Node.js and Python are installed,
- local build smoke-test commands.

## Validation

Local validation:

- `docker build -t sendword:local .`
- `docker run --rm sendword:local sendword --help`
- `docker run --rm sendword:local node --version`
- `docker run --rm sendword:local python3 --version`
- `nix shell nixpkgs#actionlint -c actionlint .github/workflows/docker.yml`
- `git diff --check`

If local Docker is unavailable, the Dockerfile and workflow can still be
statically reviewed, but the task remains weakly verified until a Docker build
or GitHub Actions run succeeds.
