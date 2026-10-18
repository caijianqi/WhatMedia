# syntax=docker/dockerfile:1.7
# WhatMedia Server —— 多阶段构建，供 docker buildx 产出 linux/amd64 + linux/arm64。
#
# 三条与"跑得起来"直接相关的约定：
#   1. **FFmpeg 用静态构建，不用发行版包。** apt 的 ffmpeg 在两个架构上
#      codec 可用性完全不同（arm64 上常缺 libx264/libx265），用它会得到
#      一个"能启动但转不了码"的镜像 —— 这种故障不报错，只会静默退化。
#   2. **非 root 运行。** 媒体目录是用户挂进来的，root 写出来会把宿主机上
#      文件的归属搅乱，且回不去。
#   3. **健康检查打真实的 HTTP 端点**，不要打"配置能否解析"。见下面注释。

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

# ── FFmpeg 静态构建 ────────────────────────────────────────────
# 版本号写死在 FFMPEG_VERSION：johnvansickle 的 release 只保留最新版，
# 不写死就变成"每次构建可能拿到不同 ffmpeg"，性能基准将不可比。
# 升级时同步 versions.lock 的 [media] 段。
FROM debian:bookworm-slim AS ffmpeg

ARG TARGETARCH
ARG FFMPEG_VERSION=7.1

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    case "$TARGETARCH" in \
      arm64) arch=arm64 ;; \
      *)     arch=amd64 ;; \
    esac; \
    url="https://johnvansickle.com/ffmpeg/releases/ffmpeg-${FFMPEG_VERSION}-${arch}-static.tar.xz"; \
    curl -fsSL "$url" -o /tmp/ffmpeg.tar.xz; \
    mkdir -p /tmp/ff; \
    tar -xJf /tmp/ffmpeg.tar.xz -C /tmp/ff --strip-components=1 \
      --wildcards '*/ffmpeg' '*/ffprobe'; \
    mv /tmp/ff/ffmpeg /tmp/ff/ffprobe /usr/local/bin/; \
    rm -rf /tmp/ffmpeg.tar.xz /tmp/ff; \
    ffmpeg -version | head -1

FROM debian:bookworm-slim

RUN groupadd -r -g 1000 whatmedia && useradd -r -u 1000 -g whatmedia whatmedia \
    && mkdir -p /config /data /cache /logs \
    && chown -R whatmedia:whatmedia /config /data /cache /logs

COPY --from=builder /src/target/release/whatmedia-server /usr/local/bin/whatmedia-server
COPY --from=builder /src/target/release/whatmedia-cli /usr/local/bin/whatmedia-cli
COPY --from=ffmpeg  /usr/local/bin/ffmpeg  /usr/local/bin/ffmpeg
COPY --from=ffmpeg  /usr/local/bin/ffprobe /usr/local/bin/ffprobe
COPY migrations /usr/local/share/whatmedia/migrations

# wget 只为健康检查：slim 镜像里没有 curl，也没有 busybox wget。
# 单独一层放在 chmod/chown 之后，避免因为它重跑前面的层。
RUN apt-get update && apt-get install -y --no-install-recommends wget \
    && rm -rf /var/lib/apt/lists/*

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

# 打 /healthz，不要打 `whatmedia-cli config`。
# 后者只验证"配置能解析"：进程活着但端口没起来（端口被占、迁移卡住、
# 监听失败）时它照样返回 0 —— 健康检查一直绿，服务其实没人应答。
# 另外 exec 形式下 `> /dev/null` 会被当成参数传给程序，`|| exit 1` 也不会
# 生效（那是 shell 语法）—— 旧版这里写错了，属于"看着有检查、其实没有"。
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD wget -q -O /dev/null "http://127.0.0.1:8080/healthz" || exit 1

ENTRYPOINT ["whatmedia-server"]
