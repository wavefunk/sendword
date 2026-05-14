# GitHub Release Artifacts

`sendword` uses cargo-dist to build GitHub Release archives and installer scripts.

## User Install Commands

Unix-like systems:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/wavefunk/sendword/releases/latest/download/sendword-installer.sh | sh
```

Windows PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/wavefunk/sendword/releases/latest/download/sendword-installer.ps1 | iex"
```

## Maintainer Flow

1. Update the package version in `Cargo.toml`.
2. Validate the release plan:

   ```sh
   dist plan --tag v0.0.2
   ```

3. Commit the version change.
4. Push a matching version tag:

   ```sh
   git tag v0.0.2
   git push origin v0.0.2
   ```

The generated Release workflow builds Linux and Windows artifacts, generates checksums, creates shell and PowerShell installers, and uploads them to the GitHub Release.
