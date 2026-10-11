# syntax=docker/dockerfile:1.7
# WhatMedia Server —— 多阶段构建，供 docker buildx 产出 linux/amd64 + linux/arm64。
#
# 说明：P0 不含 FFmpeg（P3 播放引擎阶段加入）。
#      届时 FFmpeg 必须在 builder 阶段固定构建产物，并把两个架构各自
#      支持的 codec 清单写进 versions.lock，不要依赖发行版包。

FROM rust:1.98-bookworm AS builder

ARG TARGETARCH
ENV CARGO_INCREMENTAL=0

WORKDIR /src

# 先只拉依赖，最大化 Docker 层缓存命中。
COPY Cargo.toml Cargo.lock* ./
COPY crates ./crates
COPY apps ./apps
RUN mkdir -p crates/*/src apps/*/src && \
    for d in crates/* apps/*; do \
      n=$(basename "$d"); \
      [ -f "$d/src/lib.rs" ] || echo "" > "$d/src/lib.rs"; \
      [ -f "$d/src/main.rs" ] || echo "fn main() {}" > "$d/src/main.rs"; \
    done
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --bin whatmedia-server --bin whatmedia-cli

FROM debian:bookworm-slim

RUN groupadd -r -g 1000 whatmedia && useradd -r -u 1000 -g whatmedia whatmedia \
    && mkdir -p /config /data /cache /logs \
    && chown -R whatmedia:whatmedia /config /data /cache /logs

COPY --from=builder /src/target/release/whatmedia-server /usr/local/bin/whatmedia-server
COPY --from=builder /src/target/release/whatmedia-cli /usr/local/bin/whatmedia-cli
COPY migrations /usr/local/share/whatmedia/migrations

USER whatmedia
WORKDIR /config
EXPOSE 8080

ENV WHATMEDIA_MIGRATIONS_DIR=/usr/local/share/whatmedia/migrations \
    WHATMEDIA_CONFIG_DIR=/config \
    WHATMEDIA_DATA_DIR=/data \
    WHATMEDIA_CACHE_DIR=/cache \
    WHATMEDIA_LOG_DIR=/logs \
    WHATMEDIA_DATABASE_URL=sqlite:/config/whatmedia.db?mode=rwc \
    WHATMEDIA_LOG_FORMAT=json

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["whatmedia-cli", "config", "> /dev/null"] || exit 1

ENTRYPOINT ["whatmedia-server"]
