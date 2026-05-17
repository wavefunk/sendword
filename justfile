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
