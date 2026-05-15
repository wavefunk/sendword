# Release Artifacts

Woodpecker builds release artifacts when a `v*` tag is pushed. The tag must
match the package version in `Cargo.toml`; for example, `Cargo.toml` version
`0.8.7` must be tagged as `v0.8.7`.

The release workflow builds:

- `sendword-x86_64-unknown-linux-gnu.tar.gz`
- `sendword-x86_64-pc-windows-gnu.zip`
- `sendword-installer.sh`
- `sendword-installer.ps1`
- `SHA256SUMS`

Artifacts are uploaded to the `sendword-releases` Cloudflare R2 bucket under
both the versioned tag prefix and `latest/`. The public bucket domain is
expected to be `https://releases.sendword.online`.

## User Install Commands

Crates.io:

```sh
cargo install sendword
```

Linux x86_64:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://releases.sendword.online/latest/sendword-installer.sh | sh
```

Windows PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://releases.sendword.online/latest/sendword-installer.ps1 | iex"
```

The installers default to `latest`. To install a specific version, set
`SENDWORD_VERSION` before running the installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://releases.sendword.online/latest/sendword-installer.sh | SENDWORD_VERSION=v0.8.7 sh
```

## Maintainer Flow

1. Update the package version in `Cargo.toml`.
2. Commit the version change.
3. Push a matching version tag:

   ```sh
   git tag v0.8.7
   git push origin v0.8.7
   ```

Woodpecker validates the tag, publishes the crate to crates.io, builds Linux
and Windows artifacts, uploads the artifacts and installers to R2, and publishes
the Docker image to GHCR.

The release workflow uses `git.wavefunk.io/wavefunk/ci-rust:nightly-2026-01-05`
for validation and artifact builds. That image already contains the pinned Rust
toolchain, Linux and Windows target support, MinGW, OpenSSL headers, pkg-config,
and zip, so the release pipeline does not install those packages at runtime.
The crates.io publish step runs before artifact builds and shares
`/woodpecker-cache/cargo` plus `/woodpecker-cache/target` with them, so publish
verification warms the same Cargo cache used by the release binaries.

## R2 Upload Verification

`wrangler r2 object put` must include `--remote`. Without it, Wrangler writes to
local R2 storage inside the CI container, prints `Upload complete`, and the real
Cloudflare R2 bucket remains empty. The release logs should not say
`Resource location: local`.

After upload, verify one public object:

```sh
curl -f https://releases.sendword.online/latest/SHA256SUMS
```

## Cloudflare Setup

Create a bucket named `sendword-releases` in the same Cloudflare account as
`sendword.online`. Attach the custom domain `releases.sendword.online` to the
bucket and allow public access through that custom domain.

The workflow pulls private CI images from `git.wavefunk.io`. In Woodpecker,
add registry credentials for the `git.wavefunk.io` hostname with package read
access before running a release build. These registry credentials are separate
from step secrets; Woodpecker uses them only when pulling the step images.

The same Woodpecker Cloudflare token used for Pages can be used if it also has
`Account > Workers R2 Storage > Edit`. Otherwise create a separate token and
store it as `cloudflare_api_token`.

Required Woodpecker secrets:

- `cloudflare_api_token`
- `cloudflare_account_id`
- `cargo_registry_token`
- `ghcr_username`
- `ghcr_token`

`cargo_registry_token` must be a crates.io API token with permission to publish
`sendword`.

`ghcr_token` must be a GitHub token with permission to push packages to
`ghcr.io/wavefunk/sendword`.

Enable the Cloudflare and GHCR secrets for tag events. If Woodpecker allows
image/plugin filtering, restrict the Cloudflare secrets to
`git.wavefunk.io/wavefunk/ci-node:node-22-wrangler-4.91.0` and the GHCR secrets
to `woodpeckerci/plugin-docker-buildx`.
