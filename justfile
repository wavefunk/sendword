default:
    @just --list

run:
    cargo run

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

dev:
    cargo run

build:
    cargo build --release

vendor-design:
    cp ../design/css/wavefunk.css static/css/wavefunk.css
    cp ../design/css/01-tokens.css static/css/01-tokens.css
    cp ../design/css/02-base.css static/css/02-base.css
    cp ../design/css/03-layout.css static/css/03-layout.css
    cp ../design/css/04-components.css static/css/04-components.css
    cp ../design/css/05-utilities.css static/css/05-utilities.css
    cp ../design/css/06-marketing.css static/css/06-marketing.css
    cp ../design/css/fonts/MartianGrotesk-VF.woff2 static/css/fonts/
    cp ../design/css/fonts/MartianMono-VF.woff2 static/css/fonts/
    cp ../design/js/echo.js static/js/echo.js
