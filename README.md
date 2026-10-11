# WhatMedia

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

**P2：元数据与图片（已完成）** · P1 / P0 已交付

### P2 交付物

| 交付物 | 状态 |
|---|---|
| `metadata` MetadataProvider 链：**TMDB + Bangumi**（可插拔、可注入 fixture） | ✅ |
| 本地 NFO 作为独立优先级源（NFO > 手动锁定 > Provider） | ✅ |
| 候选打分与自动确认判定（标题编辑距离 × 年份，分差不足则转人工） | ✅ |
| **字段级锁定**：用户改过的字段任何自动来源都不覆盖 | ✅ |
| 刮削任务队列：**可断点续跑**、崩溃任务自动回收、可重试失败项 | ✅ |
| 图片缓存：LRU + 按时间 / 体积 / 类型清理（still 先于 poster 牺牲） | ✅ |
| 缺图占位 SVG（零依赖生成，不产生破图） | ✅ |
| FFmpeg 缩略图与多尺寸变体（参数构造 + 进程生命周期，离线可测） | ✅ |
| 类型 / 演职员 / 人物关系入库 | ✅ |
| API：`/scrape` `/scrape/stats` `/metadata/candidates` `/metadata/review` `/items/{id}/identify` `/items/{id}/locks` `/items/{id}/images/{kind}` | ✅ |
| `whatmedia-cli scrape` / `candidates` / `identify` / `lock` / `review` / `images` | ✅ |

### P1 交付物

| 交付物 | 状态 |
|---|---|
| `storage` Local 实现 + 路径安全四件套 | ✅ |
| `media` 文件名解析器（52 条语料 100% 通过） | ✅ [语料](./test-media/naming/cases.jsonl) |
| `media` FFprobe 封装（超时 / 离线可测 / HDR 判定） | ✅ |
| `media` NFO 读取（movie / tvshow / episodedetails） | ✅ |
| `library` 入库编排（**确定性 UUIDv5**，重扫不产生重复） | ✅ |
| `scanner` 遍历 / 增量指纹 / 并发 / 可取消可暂停 | ✅ |
| `database` SQLx + SQLite（WAL），唯一允许写 SQL 的 crate | ✅ |
| API：`/libraries` `/media` `/movies` `/series` `/episodes` `/items/{id}` `/search` `/tasks` `/scan` | ✅ |
| `whatmedia-cli scan` —— 扫一个目录变成可查询的库 | ✅ |
| 中文检索（FTS5 trigram + 短词降级 LIKE） | ✅ |

### P0 交付物

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
| `WHATMEDIA_HOST` | `0.0.0.0` | 监听地址 |
| `WHATMEDIA_PORT` | `8080` | 监听端口 |
| `WHATMEDIA_CONFIG_DIR` | `/config` | 配置与数据库 |
| `WHATMEDIA_DATA_DIR` | `/data` | 运行时数据 |
| `WHATMEDIA_CACHE_DIR` | `/cache` | 元数据/图片/转码缓存 |
| `WHATMEDIA_LOG_DIR` | `/logs` | 日志 |
| `WHATMEDIA_DATABASE_URL` | `sqlite:/config/whatmedia.db?mode=rwc` | 数据库连接 |
| `WHATMEDIA_LOG_LEVEL` | `info` | 日志级别 |
| `WHATMEDIA_LOG_FORMAT` | `pretty` | `pretty` 或 `json` |
| `WHATMEDIA_MIGRATIONS_DIR` | 仓库内 `migrations/` | 容器里是 `/usr/local/share/whatmedia/migrations` |
| `WHATMEDIA_METADATA_TMDB_API_KEY` | 无 | TMDB v3 Key。不配则刮削端点返回 501 |
| `WHATMEDIA_METADATA_TMDB_BEARER` | 无 | TMDB v4 Token |
| `WHATMEDIA_METADATA_BANGUMI` | `false` | `1`/`true` 启用 Bangumi（动漫） |
| `WHATMEDIA_METADATA_LANGUAGE` | `zh-CN` | 刮削语言 |
| `WHATMEDIA_METADATA_RPS` | `4` | 每秒请求上限，别打爆 Provider |
| `WHATMEDIA_METADATA_AUTO_ACCEPT` | `true` | 关掉则全部进人工确认队列 |
| `WHATMEDIA_IMAGES_DIR` | `<cache_dir>/images` | 图片缓存目录 |
| `WHATMEDIA_IMAGES_MAX_MB` | `1024` | 缓存上限，超出按 LRU 清理 |

### 端到端跑一遍

```bash
# 1) 建库并扫描一个目录
export WHATMEDIA_DATABASE_URL="sqlite:./whatmedia.db?mode=rwc"
cargo run --bin whatmedia-cli -- scan --path /path/to/media --name "电影库"

# 加了 FFmpeg 就带上 --probe，能拿到时长 / 码率 / 视频音频字幕轨
cargo run --bin whatmedia-cli -- scan --path /path/to/media --probe

# 2) 查
cargo run --bin whatmedia-cli -- list
cargo run --bin whatmedia-cli -- search --q " Matrix"
cargo run --bin whatmedia-cli -- libraries

# 3) 起服务
cargo run --bin whatmedia-server
curl -s "http://127.0.0.1:8080/api/v1/media?page_size=20"
curl -s "http://127.0.0.1:8080/api/v1/series"
curl -s "http://127.0.0.1:8080/api/v1/search?q=Frieren"
```

冒烟：

```bash
cargo run --bin whatmedia-server
curl -s http://127.0.0.1:8080/healthz
# {"status":"ok","version":"0.1.0","arch":"x86_64","os":"linux","..."}
```

### 命名解析语料

P1 的验收标准之一是"季/集解析在语料上准确率 ≥ 99%"，语料在
[`test-media/naming/cases.jsonl`](./test-media/naming/cases.jsonl)，回归测试消费它：

```bash
cargo test -p whatmedia-media --test naming_corpus
```

新加语料时请在那个文件的头部规则下补充：**必须是真实世界里出现过的命名**。

### 性能基线

```bash
# 生成 10,000 个文件并测全量 / 增量扫描耗时（默认不跑，耗时几分钟）
cargo test -p whatmedia-scanner -- --ignored --nocapture scan_10k_baseline
```

## 仓库结构

```
apps/       whatmedia-server / whatmedia-cli / whatmedia-agent
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
