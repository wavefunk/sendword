#!/usr/bin/env sh
set -eu

version="$(
    awk '
        /^\[package\]/ { in_package = 1; next }
        /^\[/ { in_package = 0 }
        in_package && $1 == "version" {
            gsub(/"/, "", $3)
            print $3
            exit
        }
    ' Cargo.toml
)"

tag="${RELEASE_TAG:-${CI_COMMIT_TAG:-}}"
if [ -z "$tag" ]; then
    case "${CI_COMMIT_REF:-}" in
        refs/tags/*) tag="${CI_COMMIT_REF#refs/tags/}" ;;
    esac
fi

expected_tag="v$version"
if [ "$tag" != "$expected_tag" ]; then
    echo "Release tag $tag does not match Cargo.toml version $version; expected $expected_tag" >&2
    exit 1
fi

dry_run="${PUBLISH_CRATE_DRY_RUN:-false}"
if [ "$dry_run" != "true" ] && [ -z "${CARGO_REGISTRY_TOKEN:-}" ]; then
    echo "CARGO_REGISTRY_TOKEN is required to publish to crates.io" >&2
    exit 1
fi

set -- --locked
if [ "$dry_run" = "true" ]; then
    set -- "$@" --dry-run
fi
if [ "${PUBLISH_CRATE_ALLOW_DIRTY:-false}" = "true" ]; then
    set -- "$@" --allow-dirty
fi

cargo publish "$@"
