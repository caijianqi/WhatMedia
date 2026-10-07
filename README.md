# Mosaic

> 把分散的媒体，拼成一个智能的影音空间。

AI 原生、自托管、多来源、跨平台的家庭影音平台。

- 服务端：Rust（Axum + Tokio + SQLx）
- 客户端：Vue 3 + TypeScript + Tauri 2（Windows / macOS / Linux / Android / Android TV / iOS）
- 媒体引擎：FFmpeg
- 双架构：linux/amd64 + linux/arm64，从第一天起同步

📄 **需求与技术方案**：[111.md](./111.md)
🗓 **开发计划**：[DEVELOPMENT-PLAN.md](./DEVELOPMENT-PLAN.md)（一人全职 · 全量范围 · 双架构首发）

---

## 当前状态

**P0：工程地基（进行中）**

| P0 交付物 | 状态 |
|---|---|
| Monorepo + Cargo workspace（20 crates / 3 apps） | ✅ |
| 六个核心抽象契约签名冻结 | ✅ [ADR 0000](./docs/adr/0000-abstractions.md) |
| 媒体领域模型（MediaItem/Version/File/Stream） | ✅ |
| 优先级任务队列（播放抢占） | ✅ |
| 配置系统（TOML + 环境变量 + 目录布局） | ✅ |
| `/healthz` 与统一错误信封 | ✅ |
| 数据库全表骨架 migration | ✅ |
| Dockerfile 多阶段 + docker-compose | ✅ |
| 双架构 CI（amd64 + arm64 self-hosted） | ✅ |
| Makefile / versions.lock | ✅ |
| Spike S1（Tauri+mpv 桌面播放器） | ⏳ [计划](./docs/spike/S1-tauri-mpv-player.md) |
| Spike S2（Tauri 移动端播放器） | ⏳ [计划](./docs/spike/S2-tauri-mobile-player.md) |
| Spike S3（FFmpeg 进程池与资源隔离） | ⏳ |
| Spike S4（中文检索） | ⏳ 已定方案：FTS5 trigram |
| R5S 接入为 arm64 CI runner | ⏳ 待执行 |

## 快速开始

```bash
# 开发模式（监听 8080，日志 pretty）
make dev

# 测试
make test

# CI 门禁（fmt + clippy -D warnings）
make lint

# 多架构镜像（需 buildx）
make docker
```

环境变量覆盖（完整列表见 `crates/config/src/lib.rs`）：

| 变量 | 默认 | 说明 |
|---|---|---|
| `MOSAIC_HOST` | `0.0.0.0` | 监听地址 |
| `MOSAIC_PORT` | `8080` | 监听端口 |
| `MOSAIC_CONFIG_DIR` | `/config` | 配置与数据库 |
| `MOSAIC_DATA_DIR` | `/data` | 运行时数据 |
| `MOSAIC_CACHE_DIR` | `/cache` | 元数据/图片/转码缓存 |
| `MOSAIC_LOG_DIR` | `/logs` | 日志 |
| `MOSAIC_DATABASE_URL` | `sqlite:/config/mosaic.db?mode=rwc` | 数据库连接 |
| `MOSAIC_LOG_LEVEL` | `info` | 日志级别 |
| `MOSAIC_LOG_FORMAT` | `pretty` | `pretty` 或 `json` |

冒烟：

```bash
cargo run --bin mosaic-server
curl -s http://127.0.0.1:8080/healthz
# {"status":"ok","version":"0.1.0","arch":"x86_64","os":"linux","..."}
```

## 仓库结构

```
apps/       mosaic-server / mosaic-cli / mosaic-agent
crates/     core config database auth media library scanner metadata
            storage playback transcoder subtitle search recommendation
            task ai plugin hardware diagnostics api
migrations/ SQLx 迁移（SQLite 与 PostgreSQL 共用一份）
client/     Vue 3 + Tauri 2（六端共享）
plugins/    Tauri 插件
docs/       adr/ spike/ benchmark/
```

**依赖分层铁律**：`core` 不依赖任何 crate → `storage`/`metadata`/`hardware` 只依赖 `core`+`config` → `media`/`library`/`scanner` 依赖下层 → `api` 依赖全部。禁止反向与循环依赖。

## 双架构说明

**ARM64 支持 ≠ RKMPP。** 分两层：

- **第一层「能跑」（P0 起，硬要求）**：Rust 双 target、CI 双架构、`buildx` 多架构镜像、FFmpeg/libmpv 的 ARM64 构建产物。
- **第二层「跑得好」（P6）**：NEON 优化、RKMPP/RGA 硬件编解码与零拷贝。

R5S（RK3588S）在本项目中扮演两个角色：ARM64 自托管 CI runner，以及最低性能基线设备。

## 版本矩阵

见 [versions.lock](./versions.lock)。**以该文件为准，不以 111.md 里写的版本号为准。**

## 许可

MIT OR Apache-2.0
