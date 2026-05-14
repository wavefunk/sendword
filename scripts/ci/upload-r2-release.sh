#!/usr/bin/env sh
set -eu

out_dir="${RELEASE_OUT_DIR:-target/release-upload}"
env_file="$out_dir/release.env"
if [ -f "$env_file" ]; then
    # shellcheck disable=SC1090
    . "$env_file"
fi

tag="${RELEASE_TAG:-${CI_COMMIT_TAG:-}}"
if [ -z "$tag" ]; then
    case "${CI_COMMIT_REF:-}" in
        refs/tags/*) tag="${CI_COMMIT_REF#refs/tags/}" ;;
    esac
fi
if [ -z "$tag" ]; then
    echo "RELEASE_TAG or CI_COMMIT_TAG is required for R2 upload" >&2
    exit 1
fi

bucket="${R2_BUCKET:-sendword-releases}"
wrangler="${WRANGLER_BIN:-./.wrangler/node_modules/.bin/wrangler}"

content_type() {
    case "$1" in
        *.tar.gz) echo "application/gzip" ;;
        *.zip) echo "application/zip" ;;
        *.sh) echo "text/x-shellscript; charset=utf-8" ;;
        *.ps1) echo "text/plain; charset=utf-8" ;;
        SHA256SUMS) echo "text/plain; charset=utf-8" ;;
        *) echo "application/octet-stream" ;;
    esac
}

upload_one() {
    src="$1"
    key="$2"
    type="$(content_type "$(basename "$src")")"
    "$wrangler" r2 object put "$bucket/$key" --file "$src" --content-type "$type"
}

for src in "$out_dir"/*; do
    [ -f "$src" ] || continue
    name="$(basename "$src")"
    [ "$name" = "release.env" ] && continue
    upload_one "$src" "$tag/$name"
    upload_one "$src" "latest/$name"
done

echo "Uploaded release artifacts to R2 bucket $bucket under $tag/ and latest/"
