# Release Artifacts

Woodpecker builds release artifacts when a `v*` tag is pushed. The tag must
match the package version in `Cargo.toml`; for example, `Cargo.toml` version
`0.8.4` must be tagged as `v0.8.4`.

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

Linux x86_64:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://releases.sendword.online/latest/sendword-installer.sh | sh
```

Windows PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://releases.sendword.online/latest/sendword-installer.ps1 | iex"
```

To install a specific version, set `SENDWORD_VERSION` before running the
installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://releases.sendword.online/latest/sendword-installer.sh | SENDWORD_VERSION=v0.8.4 sh
```

## Maintainer Flow

1. Update the package version in `Cargo.toml`.
2. Commit the version change.
3. Push a matching version tag:

   ```sh
   git tag v0.8.4
   git push origin v0.8.4
   ```

Woodpecker validates the tag, builds Linux and Windows artifacts, uploads the
artifacts and installers to R2, and publishes the Docker image to GHCR.

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

The same Woodpecker Cloudflare token used for Pages can be used if it also has
`Account > Workers R2 Storage > Edit`. Otherwise create a separate token and
store it as `cloudflare_api_token`.

Required Woodpecker secrets:

- `cloudflare_api_token`
- `cloudflare_account_id`
- `ghcr_username`
- `ghcr_token`

`ghcr_token` must be a GitHub token with permission to push packages to
`ghcr.io/wavefunk/sendword`.

Enable the Cloudflare and GHCR secrets for tag events. If Woodpecker allows
image/plugin filtering, restrict the Cloudflare secrets to `node:22-slim` and
the GHCR secrets to `woodpeckerci/plugin-docker-buildx`.
