# Crates.io Release Setup

`sendword` is not currently published as a binary crate. If crates.io
publishing is added later, users should be able to install it with:

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

5. Decide how crates.io publishing should be automated.

   The repository no longer keeps GitHub or Forgejo Actions workflows. If
   publishing is added to Woodpecker, create a crates.io API token and store it
   as a Woodpecker secret. The publish step should expose that secret as
   `CARGO_REGISTRY_TOKEN` before running `cargo publish --locked`.

## Release Flow

1. Update `version` in `Cargo.toml`.
2. Commit the version change.
3. Tag the release with the package version:

   ```sh
   git tag v0.0.2
   git push origin v0.0.2
   ```

4. Publish the crate manually with `cargo publish --locked`, or push a matching
   tag after a Woodpecker crates.io publishing workflow has been added and
   configured with a crates.io API token.
