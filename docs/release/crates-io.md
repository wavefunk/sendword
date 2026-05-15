# Crates.io Release Setup

`sendword` is published as a binary crate. Users can install it with:

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

4. Create a crates.io API token and store it as the Woodpecker secret
   `cargo_registry_token`.

The Woodpecker release workflow exposes that secret to the `publish-crate` step
as `CARGO_REGISTRY_TOKEN`. The step uses the same Cargo registry and target
cache volume as the release artifact build, and it runs before `build-artifacts`
so Cargo package verification warms the cache for the release binaries.

## Release Flow

1. Update `version` in `Cargo.toml`.
2. Commit the version change.
3. Tag the release with the package version:

   ```sh
   git tag v0.0.2
   git push origin v0.0.2
   ```

4. Push a matching tag. Woodpecker validates the tag, publishes the crate to
   crates.io, then builds and uploads the other release artifacts.
