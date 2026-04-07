default:
    @just --list

check:
    cargo check

test:
    cargo test

clippy:
    cargo clippy -- -D warnings

fmt:
    cargo fmt

watch:
    bacon

migrate:
    cargo sqlx migrate run --source migrations

migrate-new NAME:
    cargo sqlx migrate add -r {{NAME}} --source migrations

sqlx-prepare:
    cargo sqlx prepare

sqlx-reset:
    rm -f data/sendword.db data/sendword.db-wal data/sendword.db-shm
    just migrate

npm-install:
    npm install

build-css *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    tmpfile=$(mktemp)
    trap "rm -f $tmpfile" EXIT
    while IFS= read -r line; do
      if [[ "$line" =~ ^@import\ \"\./(.*)\"\; ]]; then
        cat "static/css/src/${BASH_REMATCH[1]}"
      else
        echo "$line"
      fi
    done < static/css/src/app.css > "$tmpfile"
    tailwindcss -i "$tmpfile" -o static/dist/app.css {{ARGS}}

watch-css:
    tailwindcss -i static/css/src/app.css -o static/dist/app.css --watch

build-ts:
    esbuild static/ts/main.ts --bundle --outdir=static/dist --format=esm --target=es2020 --minify

build-ts-dev:
    esbuild static/ts/main.ts --bundle --outdir=static/dist --format=esm --target=es2020 --sourcemap

watch-ts:
    esbuild static/ts/main.ts --bundle --outdir=static/dist --format=esm --target=es2020 --sourcemap --watch

dev:
    #!/usr/bin/env bash
    set -euo pipefail
    just npm-install
    mkdir -p static/dist
    just build-css
    just build-ts-dev
    just watch-css &
    CSS_PID=$!
    just watch-ts &
    TS_PID=$!
    trap "kill $CSS_PID $TS_PID 2>/dev/null" EXIT
    cargo run

build:
    just npm-install
    just build-css --minify
    just build-ts
    cargo build --release
