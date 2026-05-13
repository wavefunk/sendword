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
