# Docker Image Publishing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and publish a GHCR Docker image containing `sendword`, Node.js, and Python.

**Architecture:** A multi-stage Dockerfile uses cargo-chef and BuildKit cache mounts for Rust build performance, then copies the built binary into a slim Debian runtime with Node.js and Python installed. A GitHub Actions workflow publishes the image to GHCR with Docker Buildx and GitHub Actions cache-backed layers.

**Tech Stack:** Docker, cargo-chef, Debian bookworm slim, Node.js, Python 3, GitHub Actions, Docker Buildx, GHCR.

---

## File Structure

- Create `Dockerfile`: multi-stage Rust build and runtime image.
- Create `.dockerignore`: keep local/generated files out of Docker context.
- Create `.github/workflows/docker.yml`: GHCR publish workflow with Buildx cache.
- Create `docs/release/docker.md`: image usage and maintainer notes.
- Reference `docs/superpowers/specs/2026-05-13-docker-image-design.md`: approved design; no edits needed.

## Task 1: Add Docker Build Files

**Files:**
- Create: `Dockerfile`
- Create: `.dockerignore`

- [ ] **Step 1: Create Dockerfile**

Create `Dockerfile` with exactly this content:

```dockerfile
# syntax=docker/dockerfile:1.7

FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --bin sendword --locked && \
    cp target/release/sendword /tmp/sendword

FROM debian:bookworm-slim AS runtime
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        nodejs \
        npm \
        python3 \
        python3-pip && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /tmp/sendword /usr/local/bin/sendword

RUN mkdir -p /data
WORKDIR /data

EXPOSE 8080

CMD ["sendword", "serve"]
```

- [ ] **Step 2: Create .dockerignore**

Create `.dockerignore` with exactly this content:

```gitignore
.git/
.beads/
.codex/
.claude/
.direnv/
.playwright/
.playwright-cli/
target/
data/
node_modules/
website/dist/
website/.eigen_cache/
*.db
*.db-shm
*.db-wal
```

## Task 2: Add Docker Publish Workflow

**Files:**
- Create: `.github/workflows/docker.yml`

- [ ] **Step 1: Create workflow**

Create `.github/workflows/docker.yml` with exactly this content:

```yaml
name: Docker

on:
  push:
    branches: ["main"]
    tags:
      - "v*.*.*"
    paths:
      - "Dockerfile"
      - ".dockerignore"
      - "Cargo.toml"
      - "Cargo.lock"
      - "rust-toolchain.toml"
      - "src/**"
      - "templates/**"
      - "static/**"
      - ".github/workflows/docker.yml"
  workflow_dispatch:

permissions:
  contents: read
  packages: write

concurrency:
  group: "docker-${{ github.ref }}"
  cancel-in-progress: true

env:
  REGISTRY: ghcr.io
  IMAGE_NAME: wavefunk/sendword

jobs:
  build:
    name: Build and publish image
    runs-on: ubuntu-latest
    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Set up Docker Buildx
        uses: docker/setup-buildx-action@v3

      - name: Log in to GHCR
        uses: docker/login-action@v3
        with:
          registry: ${{ env.REGISTRY }}
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Extract Docker metadata
        id: meta
        uses: docker/metadata-action@v5
        with:
          images: ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}
          tags: |
            type=ref,event=branch
            type=semver,pattern={{version}}
            type=raw,value=latest,enable=${{ startsWith(github.ref, 'refs/tags/v') }}

      - name: Build and push Docker image
        uses: docker/build-push-action@v7
        with:
          context: .
          push: true
          tags: ${{ steps.meta.outputs.tags }}
          labels: ${{ steps.meta.outputs.labels }}
          cache-from: type=gha
          cache-to: type=gha,mode=max
```

- [ ] **Step 2: Run actionlint**

Run:

```bash
nix shell nixpkgs#actionlint -c actionlint .github/workflows/docker.yml
```

Expected: exits 0 with no diagnostics.

## Task 3: Add Docker Documentation

**Files:**
- Create: `docs/release/docker.md`

- [ ] **Step 1: Create docs**

Create `docs/release/docker.md` with exactly this content:

```markdown
# Docker Image

The official image is published to GitHub Container Registry:

```sh
docker pull ghcr.io/wavefunk/sendword:latest
```

Run with a mounted data directory:

```sh
docker run --rm -p 8080:8080 -v "$PWD/data:/data" ghcr.io/wavefunk/sendword:latest
```

The image includes:

- `sendword`
- Node.js and npm
- Python 3 and pip

Those runtimes are available to webhook scripts and commands that execute inside the container.

## Local Smoke Test

```sh
docker build -t sendword:local .
docker run --rm sendword:local sendword --help
docker run --rm sendword:local node --version
docker run --rm sendword:local python3 --version
```
```

## Task 4: Validate And Commit

**Files:**
- Create: `Dockerfile`
- Create: `.dockerignore`
- Create: `.github/workflows/docker.yml`
- Create: `docs/release/docker.md`

- [ ] **Step 1: Build image locally**

Run:

```bash
docker build -t sendword:local .
```

Expected: exits 0 and creates `sendword:local`.

- [ ] **Step 2: Smoke test sendword binary**

Run:

```bash
docker run --rm sendword:local sendword --help
```

Expected: exits 0 and prints the CLI help.

- [ ] **Step 3: Smoke test Node.js**

Run:

```bash
docker run --rm sendword:local node --version
```

Expected: exits 0 and prints a Node.js version.

- [ ] **Step 4: Smoke test Python**

Run:

```bash
docker run --rm sendword:local python3 --version
```

Expected: exits 0 and prints a Python version.

- [ ] **Step 5: Check whitespace**

Run:

```bash
git diff --check -- Dockerfile .dockerignore .github/workflows/docker.yml docs/release/docker.md
```

Expected: exits 0 with no whitespace errors.

- [ ] **Step 6: Commit implementation**

Run:

```bash
git add Dockerfile .dockerignore .github/workflows/docker.yml docs/release/docker.md
git commit -m "release: add docker image publishing"
```

Expected: commit succeeds and includes only the four implementation files.

- [ ] **Step 7: Close the br task after review approval**

Run:

```bash
br close sendword-oc4.4 --reason "Docker image and GHCR publishing workflow added"
```

Expected: `sendword-oc4.4` is closed, making `sendword-oc4.5` the next ready task.

- [ ] **Step 8: Commit the br status update**

Run:

```bash
git add .beads/issues.jsonl
git commit -m "chore: close docker image task"
```

Expected: commit succeeds and includes only `.beads/issues.jsonl`.

## Self-Review Notes

- Spec coverage: the plan adds the Docker image, GHCR workflow, Buildx cache, Node.js, Python, and docs.
- Scope: no first-class JS/Python executor changes are included.
- Validation: local Docker build and runtime smoke tests prove the image contains `sendword`, Node.js, and Python; actionlint checks workflow syntax; `git diff --check` checks whitespace.
