# syntax=docker/dockerfile:1.7
# WhatMedia Server —— 多阶段多架构构建（供 GitHub Actions + Docker Buildx 产出 linux/amd64 + linux/arm64）
#
# 架构与安全约定：
#   1. **前端 Web 界面一体化打包**：Node.js 阶段编译 client 并打包进服务端镜像，实现开箱即用。
#   2. **FFmpeg 采用全静态构建**：跨 amd64 与 arm64 架构保证编解码器（h264/hevc等）可用性一致。
#   3. **非 root 用户运行**：内置 whatmedia (1000:1000) 运行，保护宿主机挂载目录权限。
#   4. **真实 HTTP 探针健康检查**：打 /healthz 端点验证实际监听与响应状态。

# ── 1. 前端 Web 构建阶段 ─────────────────────────────────────────
FROM node:22-bookworm-slim AS web-builder

WORKDIR /web
COPY client/package*.json ./
RUN npm ci || npm install
COPY client ./
RUN npm run build

# ── 2. 后端 Rust 构建阶段 ─────────────────────────────────────────
FROM rust:1.98-bookworm AS builder

ARG TARGETARCH
# 限制 codegen units 与关闭 LTO，以极大降低在 QEMU 模拟 arm64 下的链接内存占用与编译耗时
ENV CARGO_INCREMENTAL=0 \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
    CARGO_PROFILE_RELEASE_LTO=off

WORKDIR /src

COPY Cargo.toml Cargo.lock* ./
COPY crates ./crates
COPY apps ./apps

# 编译 release 二进制：缓存挂载根据 TARGETARCH 严格隔离，并将产物持久化
RUN --mount=type=cache,id=cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-${TARGETARCH},target=/src/target \
    cargo build --release -p whatmedia && \
    cp /src/target/release/whatmedia /usr/local/bin/whatmedia

# ── 3. 运行环境阶段 ──────────────────────────────────────────────
FROM debian:bookworm-slim

RUN groupadd -r -g 1000 whatmedia && useradd -r -u 1000 -g whatmedia whatmedia \
    && mkdir -p /config /data /cache /logs /usr/local/share/whatmedia/web \
    && chown -R whatmedia:whatmedia /config /data /cache /logs /usr/local/share/whatmedia

COPY --from=builder              /usr/local/bin/whatmedia /usr/local/bin/whatmedia
COPY --from=mwader/static-ffmpeg:7.1 /ffmpeg             /usr/local/bin/ffmpeg
COPY --from=mwader/static-ffmpeg:7.1 /ffprobe            /usr/local/bin/ffprobe
COPY --from=web-builder          /web/dist                /usr/local/share/whatmedia/web
COPY migrations /usr/local/share/whatmedia/migrations

# 安装 wget 用于容器探针健康检查
RUN apt-get update && apt-get install -y --no-install-recommends wget ca-certificates \
    && rm -rf /var/lib/apt/lists/*

USER whatmedia
WORKDIR /config
EXPOSE 8080

ENV WHATMEDIA_MIGRATIONS_DIR=/usr/local/share/whatmedia/migrations \
    WHATMEDIA_WEB_DIR=/usr/local/share/whatmedia/web \
    WHATMEDIA_CONFIG_DIR=/config \
    WHATMEDIA_DATA_DIR=/data \
    WHATMEDIA_CACHE_DIR=/cache \
    WHATMEDIA_LOG_DIR=/logs \
    WHATMEDIA_DATABASE_URL=sqlite:/config/whatmedia.db?mode=rwc \
    WHATMEDIA_LOG_FORMAT=json

# HTTP 真实探针检查健康状况
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD wget -q -O /dev/null "http://127.0.0.1:8080/healthz" || exit 1

ENTRYPOINT ["whatmedia"]
