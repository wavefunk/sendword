# syntax=docker/dockerfile:1.7

FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,id=sendword-target-bookworm,target=/app/target \
    cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,id=sendword-target-bookworm,target=/app/target \
    cargo build --release --bin sendword --locked && \
    cp target/release/sendword /tmp/sendword

FROM debian:bookworm-slim AS runtime
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        nodejs \
        npm \
        python3 \
        python3-pip && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /tmp/sendword /usr/local/bin/sendword

RUN mkdir -p /data
WORKDIR /data

ENV SENDWORD_SERVER__BIND=0.0.0.0

EXPOSE 8080

CMD ["sendword", "serve"]
