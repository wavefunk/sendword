#!/usr/bin/env sh
set -eu

tag="${CI_COMMIT_TAG:-}"
if [ -z "$tag" ]; then
    case "${CI_COMMIT_REF:-}" in
        refs/tags/*) tag="${CI_COMMIT_REF#refs/tags/}" ;;
    esac
fi
if [ -z "$tag" ] && [ "$#" -gt 0 ]; then
    tag="$1"
fi

case "$tag" in
    v*) ;;
    *)
        echo "Release pipeline only accepts v* tags; got '${tag:-<empty>}'" >&2
        exit 1
        ;;
esac

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

if [ -z "$version" ]; then
    echo "Could not read [package] version from Cargo.toml" >&2
    exit 1
fi

expected_tag="v$version"
if [ "$tag" != "$expected_tag" ]; then
    echo "Tag $tag does not match Cargo.toml package version $expected_tag" >&2
    exit 1
fi

mkdir -p target/release-upload
{
    echo "RELEASE_TAG=$tag"
    echo "RELEASE_VERSION=$version"
    echo "R2_BUCKET=${R2_BUCKET:-sendword-releases}"
    echo "R2_PUBLIC_BASE_URL=${R2_PUBLIC_BASE_URL:-https://releases.sendword.online}"
} > target/release-upload/release.env

echo "Validated release tag $tag for package version $version"
