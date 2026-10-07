# syntax=docker/dockerfile:1.7
# Mosaic Server —— 多阶段构建，供 docker buildx 产出 linux/amd64 + linux/arm64。
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
    cargo build --release --bin mosaic-server --bin mosaic-cli

FROM debian:bookworm-slim

RUN groupadd -r -g 1000 mosaic && useradd -r -u 1000 -g mosaic mosaic \
    && mkdir -p /config /data /cache /logs \
    && chown -R mosaic:mosaic /config /data /cache /logs

COPY --from=builder /src/target/release/mosaic-server /usr/local/bin/mosaic-server
COPY --from=builder /src/target/release/mosaic-cli /usr/local/bin/mosaic-cli
COPY migrations /usr/local/share/mosaic/migrations

USER mosaic
WORKDIR /config
EXPOSE 8080

ENV MOSAIC_MIGRATIONS_DIR=/usr/local/share/mosaic/migrations \
    MOSAIC_CONFIG_DIR=/config \
    MOSAIC_DATA_DIR=/data \
    MOSAIC_CACHE_DIR=/cache \
    MOSAIC_LOG_DIR=/logs \
    MOSAIC_DATABASE_URL=sqlite:/config/mosaic.db?mode=rwc \
    MOSAIC_LOG_FORMAT=json

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["mosaic-cli", "config", "> /dev/null"] || exit 1

ENTRYPOINT ["mosaic-server"]
