# Mosaic 常用命令入口。P0 的 DoD 就是这三条能在双架构上都跑通：
#   make dev / make build / make test

SHELL := /bin/bash
IMAGE ?= ghcr.io/mosaic-media/mosaic
TAG   ?= dev

.PHONY: help dev test lint fmt build docker docker-push clean migrations

help:
	@grep -E '^[a-zA-Z_-]+:.*?# .*$$' $(MAKEFILE_LIST) | awk 'BEGIN{FS=":.*?# "}{printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

dev:            # 本地开发：监听 8080，日志为 pretty
	RUST_LOG=info MOSAIC_LOG_FORMAT=pretty cargo run --bin mosaic-server

test:           # 全量测试（CI 在 amd64 与 arm64 各跑一遍）
	cargo test --workspace

lint:           # CI 门禁：-D warnings，任何警告都是错误
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all
	cargo clippy --workspace --all-targets --fix --allow-dirty

build:          # 本地双架构无意义，这里只出本机 release 二进制
	cargo build --release --bin mosaic-server --bin mosaic-cli

docker:         # 多架构镜像（需要 buildx；首次执行需 docker buildx create --use）
	docker buildx build --platform linux/amd64,linux/arm64 -t $(IMAGE):$(TAG) --load .

docker-push:    # 打 tag 后推送双架构镜像
	docker buildx build --platform linux/amd64,linux/arm64 -t $(IMAGE):$(TAG) --push .

migrations:     # 列出迁移文件及执行顺序
	cargo run --bin mosaic-cli -- migrations --root .

clean:
	cargo clean
	rm -rf .tmp
