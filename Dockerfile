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

FROM debian:bookworm-slim AS runtime-base
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        nodejs \
        npm \
        python3 \
        python3-pip && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd --gid 1000 sendword && \
    useradd --uid 1000 --gid sendword --home-dir /home/sendword \
        --create-home --shell /usr/sbin/nologin sendword

COPY docker-entrypoint.sh /usr/local/bin/sendword-docker-entrypoint

RUN mkdir -p /data && \
    chown -R sendword:sendword /data /home/sendword && \
    chmod 0755 /usr/local/bin/sendword-docker-entrypoint
WORKDIR /data

ENV SENDWORD_SERVER__BIND=0.0.0.0
ENV SENDWORD_DATABASE__PATH=/data/sendword.db
ENV SENDWORD_LOGS__DIR=/data/logs
ENV SENDWORD_SCRIPTS__DIR=/data/scripts
ENV HOME=/home/sendword

EXPOSE 8080

ENTRYPOINT ["sendword-docker-entrypoint"]
CMD ["sendword", "serve"]

FROM runtime-base AS release-runtime
COPY --chmod=0755 .docker-release/sendword /usr/local/bin/sendword
USER sendword

FROM runtime-base AS runtime
COPY --chmod=0755 --from=builder /tmp/sendword /usr/local/bin/sendword
USER sendword
