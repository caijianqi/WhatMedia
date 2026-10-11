# WhatMedia 常用命令入口。P0 的 DoD 就是这三条能在双架构上都跑通：
#   make dev / make build / make test

SHELL := /bin/bash
IMAGE ?= ghcr.io/whatmedia/whatmedia
TAG   ?= dev

.PHONY: help dev test lint fmt build docker docker-push clean migrations compat openapi openapi-check icons desktop-check

help:
	@grep -E '^[a-zA-Z_-]+:.*?# .*$$' $(MAKEFILE_LIST) | awk 'BEGIN{FS=":.*?# "}{printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

dev:            # 本地开发：监听 8080，日志为 pretty
	RUST_LOG=info WHATMEDIA_LOG_FORMAT=pretty cargo run --bin whatmedia-server

test:           # 全量测试（CI 在 amd64 与 arm64 各跑一遍）
	cargo test --workspace

lint:           # CI 门禁：-D warnings，任何警告都是错误
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all
	cargo clippy --workspace --all-targets --fix --allow-dirty

icons:          # 生成桌面端占位图标（全新克隆里缺 icon.ico 会编译失败）
	python3 scripts/gen-icons.py

desktop-check:  # 桌面壳（Tauri）编译检查：它不在主 workspace 里，要单独跑
	cd client/src-tauri && cargo check

build:          # 本地双架构无意义，这里只出本机 release 二进制
	cargo build --release --bin whatmedia-server --bin whatmedia-cli

docker:         # 多架构镜像（需要 buildx；首次执行需 docker buildx create --use）
	docker buildx build --platform linux/amd64,linux/arm64 -t $(IMAGE):$(TAG) --load .

docker-push:    # 打 tag 后推送双架构镜像
	docker buildx build --platform linux/amd64,linux/arm64 -t $(IMAGE):$(TAG) --push .

migrations:     # 列出迁移文件及执行顺序
	cargo run --bin whatmedia-cli -- migrations --root .

compat:         # 播放兼容性矩阵（P3 DoD 覆盖率判定依据）
	scripts/compat-matrix.sh

openapi:        # 重新导出契约并刷新前端类型（改了 API 就跑一次）
	cargo run --features openapi --bin whatmedia-cli -- openapi
	cd client && npm run gen:api

openapi-check:  # CI 门禁：契约与生成物必须和代码同步，不许只改一边
	cargo run --features openapi --bin whatmedia-cli -- openapi
	cd client && npm run gen:api
	git diff --exit-code -- docs/openapi/openapi.json client/src/api/schema.d.ts

clean:
	cargo clean
	rm -rf .tmp
