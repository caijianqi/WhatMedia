# WhatMedia

> 把分散的媒体，拼成一个智能的影音空间。

AI 原生、自托管、多来源、跨平台的家庭影音平台。

- 服务端：Rust（Axum + Tokio + SQLx）
- 客户端：Vue 3 + TypeScript + Tauri 2（Windows / macOS / Linux / Android / Android TV / iOS）
- 媒体引擎：FFmpeg
- 双架构：linux/amd64 + linux/arm64，从第一天起同步

---

## 当前状态

**P8：电视直播（L0 已交付）** + **多用户鉴权已上线（默认开启）** + **收藏 / 通知 / 可写设置 / 存储统计** · P0 – P7 已交付 · 24 crates

端到端冒烟：`make dev` → 打开 Web 走首次安装向导建管理员 → 扫描目录 → 浏览 → 播放 → 问 AI → 导入 M3U 看直播。

> **所有 `/api/v1` 端点默认都要登录**，没有关闭鉴权的开关。

### 交付物 · 多用户鉴权（原 P9，提前落地）

默认开启，**没有开关**。三条硬规则参见 [`crates/auth`](./crates/auth/src/lib.rs) 的模块文档。

| 交付物 | 状态 |
|---|---|
| `crates/auth`：argon2id 口令摘要、票据、签名 URL、角色与分级策略（39 条单测，含 RFC 4231 HMAC 向量） | ✅ |
| 数据模型 `migrations/0006_auth.sql`：`session` 表 + `user`/`device` 补齐 + `approval_request` | ✅ |
| **会话可吊销**：票据只落 SHA-256，支持"登出所有设备"；停用账户立刻踢下线 | ✅ |
| 角色 `admin / user / child` + 9 个能力点；儿童账户的分级闸门在服务端裁决 | ✅ |
| **儿童设备家长审批**：新设备登录 → 403 `APPROVAL_REQUIRED` → 家长放行 → 重新登录 | ✅ |
| 登录爆破保护：连败 N 次锁账户（不按 IP —— 家里全是一个出口 IP） | ✅ |
| 流端点双通道：**Cookie**（浏览器）+ **签名 URL**（`<video>` / hls.js / ExoPlayer） | ✅ |
| 鉴权中间件：白名单之外一律 401，且 401 早于任何内部状态判断 | ✅ |
| 前端：首次安装向导 + 登录页 + 路由守卫 + 顶栏身份/退出；播放器走签名 URL | ✅ |
| 多实例共享签名密钥：`WHATMEDIA_SIGNING_KEY` | ✅ |
| PIN 二次确认（家长审批用） | ⬜ |
| 邀请码 / 家长审批推送 | ⬜ |

### 交付物 · 浏览体验（影音墙界面）

界面参照飞牛影音那类"影音墙"：首页 **Hero 轮播 + 横向海报行 + 分类 tab**，详情页补上**演职员**与**季 / 集**。

| 交付物 | 状态 |
|---|---|
| 首页 Hero 轮播（取评分最高的 5 部，自动轮播 + 渐变兜底，不留黑块） | ✅ |
| 横向海报行：继续观看（带进度条）/ 最近添加 / 高分推荐 | ✅ |
| **分类 tab 由数据驱动**：`GET /api/v1/media/kinds` 只返回真有内容的类型 | ✅ |
| 详情页演职员：演员头像行 + 幕后按职能归并为"导演：某某、某某" | ✅ |
| 详情页剧集：`GET /api/v1/items/{id}/series` 反查所属剧 → 季 tab → 集列表 | ✅ |
| `ItemDetail` 带 `genres`，详情页渲染成类型 chips | ✅ |
| 每一块独立加载、独立报错：刮削挂了不影响"最近添加" | ✅ |
| 新端点全部在鉴权之后；`credits` / `series` 失败时详情页静默降级 | ✅ |
| 测试：database 层 `media_browse.rs` + API 层 `browse_endpoints.rs`（共 13 条） | ✅ |

> **分类 tab 不硬编码类型清单**。扫描目前只产出 `movie` 与 `episode`，
> 硬编码"电影/剧集/动画/纪录片/综艺"会在绝大多数库里留下几个点进去永远是空的 tab ——
> 那比没有 tab 更糟，因为它让人以为自己的库坏了。

### 交付物 · 界面改版（原型紫系 + 左侧栏）

按 UI 原型重做外壳与色板。**原型里写死的演示数据一条都没搬** —— 能落地的才做，
落不了地的一律不画（详见下面"诚实留白"）。

| 交付物 | 状态 |
|---|---|
| 色板换成原型紫 `#795bff`（页面底 `#090e1b`、面板 `#10182a`、半透明描边、16px 圆角、大柔阴影） | ✅ |
| **238px 左侧栏**：品牌 + 分组导航（发现内容 / 我的 / 工具与设置）+ 底部服务端状态卡 | ✅ |
| 顶栏全局搜索：走真实 `GET /api/v1/search`，**防抖 250ms + 取消上一发** | ✅ |
| 顶栏通知：未读角标是服务端真的未读数；待家长审批的设备数单独列出 | ✅ |
| 通用状态件 `EmptyState`（空库 / 无结果 / 失败 / 未配置四种语义）/ `LoadingSkeleton` | ✅ |
| 媒体库：类型 chips 改数据驱动（去掉硬编码）、网格 / 列表切换（记在服务端） | ✅ |
| 详情页收藏按钮：乐观更新，失败回滚；端点不可用时整颗按钮不渲染 | ✅ |
| 手持设备转底栏，TV 上去掉搜索框（遥控器输不了中文） | ✅ |
| 测试：新增 EmptyState / SettingControl / TopBar / FavoritesView 四组（共 25 条） | ✅ |

> **Tailwind 配置现在只有 `client/tailwind.config.ts` 一份。**
> 同目录原本还有一个 `tailwind.config.js`（旧蓝 `#5b8cff` 色板）。Tailwind v3 的
> cosmiconfig 解析顺序是 `.js` 先于 `.ts`，新色板会被它**整份遮蔽** —— 症状是
> 编译期报 ``The `text-wm-sub` class does not exist``，界面却仍是旧蓝。旧的 `.js` 已删除。

### 交付物 · 收藏 / 通知 / 可写设置 / 存储统计

原型画了这四块，服务端一个字段都没有。与其在界面上放一排点不动的假按钮，
不如把地基补上 —— 见 [`migrations/0007_social.sql`](./migrations/0007_social.sql)。

| 交付物 | 状态 |
|---|---|
| 收藏：`PUT /items/{id}/favorite`（幂等）+ `GET` + `GET /me/favorites`（带条目标题，不逐个拉详情） | ✅ |
| 通知：扫描 / 刮削完成时**真的写入**；每人一行（不是广播行），"已读"互不干扰 | ✅ |
| **可写设置**：`GET/PUT/DELETE /api/v1/settings`，只认白名单（路径 / 端口 / 数据库 / 密钥一律不可写） | ✅ |
| 设置覆盖在启动时套用到配置；被拒掉的项由 `GET /settings` 露出来，不静默忽略 | ✅ |
| 存储统计：`GET /api/v1/system/storage`，按库给出条目数 / 文件数 / 字节数（来自 `size_bytes`，不扫盘） | ✅ |
| 测试：database 层 `social_repo.rs`（15 条）+ API 层 `social_endpoints.rs`（13 条）+ config 7 条 | ✅ |

三条诚实留白，**不要把它们补成假数据**：

1. **存储卡片不画"已用 / 总量"进度条**。跨平台拿不到可靠的磁盘总容量
   （Windows `GetDiskFreeSpaceExW` / Unix `statvfs`），画一条分母未知的进度条等于编数据。
   这里画的是**各库之间的相对占比** —— 那个是精确值。
2. **server 级设置全部标注"改完需重启"**。刮削器、图片缓存、转码运行时都在启动时构造，
   改配置没法热替换它们。宁可如实说要重启，也不假装即时生效。
3. **侧栏状态卡显示的是运行时长，不是"已连接"**。"已连接"是自证的 ——
   它写死在页面上，跟服务端是不是活着无关；运行时长每分钟都在变，变了就说明服务活着。

### P8 交付物 · 电视直播

原始需求稿里**没有**直播章节 —— 这是本阶段新开的方向。

| 交付物 | 状态 |
|---|---|
| 数据模型 `migrations/0005_live_tv.sql`：`live_source` / `live_channel` / `live_program` / `live_recording` / `live_session` | ✅ |
| `crates/live`：M3U 与 XMLTV 解析（15 条语料测试，覆盖 BOM / CRLF / 属性别名 / 裸 URL / 显示名带逗号） | ✅ |
| `OutputSpec::LiveHls`：滚动窗口 + `delete_segments`，**绝不写 `EXT-X-ENDLIST`**；输入侧 `-re` / `genpts` / 重连 | ✅ |
| **频道级流复用**：同一频道多人观看只起一路 ffmpeg（引用计数，归零才停） | ✅ |
| API：源管理 / 同步 / 频道列表 / 起播 / 停止，全部进 OpenAPI 契约 | ✅ |
| 前端 `LiveView`（分组筛选、LIVE 徽标、离开频道时释放引用） | ✅ |
| **L1** 转码档位、时移（30 分钟窗口）、录制落 `media_item` | ⬜ |
| **L2** XMLTV 抓取与节目单时间轴 | ⬜ |
| **L3** HDHomeRun / DVB tuner | ⬜ |

> 频道**刻意不进 `media_item`**：直播没有版本、文件、时长、刮削、续播与"看完"，
> 硬塞进去会让扫描、刮削、续播、8 个 AI 工具全都为"无文件无时长的实体"加分支。
> 只有**录制产物**会回过头成为 `media_item` —— 录制出来的东西是资产，直播流不是。

### P7 交付物 · AI 媒体管家

| 交付物 | 状态 |
|---|---|
| ModelProvider 抽象：OpenAI 兼容线 + Ollama 线（覆盖 DeepSeek / Qwen / GLM / vLLM / llama.cpp），**不绑定单一厂商** | ✅ |
| 首批 8 个工具 + 注册表 + 风险分级（只读自动执行 / 写 / 危险） | ✅ |
| **Plan → Preview → Confirm → Execute**：一次性令牌 + 参数指纹 + 操作者绑定 + TTL | ✅ |
| 离线中文意图解析（RuleEngine）：没配模型也能答，§59 七条样例 **7/7** | ✅ |
| 5 个 Agent + MCP Server（对外只暴露只读工具）/ MCP Client | ✅ |
| **依赖守卫**：`crates/ai/tests/no_direct_access.rs` 编译期钉死"AI 不可直连 SQL / FS / Process" | ✅ |
| API：`POST /ai/chat` `GET /ai/tools` `POST /ai/confirm`，全部进 OpenAPI 契约 | ✅ |
| 前端 AI 助手页 + 危险操作确认工单 UI | ✅ |
| 降级不连累核心：模型不可用时走规则引擎并标 `degraded`；`/ai/*` 挂掉时核心端点仍 200 | ✅ |
| 真实模型下的工具调用协议（`tool_calls` / Ollama 版本差异） | ⬜ |

### P6 交付物 · 移动端与 TV

| 交付物 | 状态 |
|---|---|
| `crates/media3` 协议与语义层（错误码分类 / 语言归一化 / 轨道 id / seek 收敛 / MediaSession，39 条测试） | ✅ |
| `client/src/player/media3.ts` 第三个播放器后端（22 条测试，可注入假 bridge） | ✅ |
| `plugins/tauri-plugin-media3/android/`（Kotlin：ExoPlayer + 图层 + 后台播放 + 遥控器 + 网络切换） | ✅ |
| Android 工程搭建，`AndroidManifest` 里 TV 三个易漏点已写进注释 | ✅ |
| **真机验证** —— 本机未装 Android SDK / NDK，**工程一次都没编译过** | ⬜ 待真机 |

### P5 交付物 · ARM / Docker

| 交付物 | 状态 |
|---|---|
| 硬件转码失败自动静默降级到软件转码（ADR-0004 + `spawn_with_fallback`） | ✅ |
| HDR→SDR tone mapping 在真 ffmpeg 上实跑通过（PQ/HLG → bt709，含 QSV 路径） | ✅ |
| 性能基线表：Windows 列已填真实数字，R5S 列留空 | ✅ |
| Docker 交付物：FFmpeg 静态构建入镜像、健康检查、`compose` 含 Rockchip 直通模板 | ✅ |
| R5S 真机 / `docker compose up` 走通 / 双架构镜像 CI | ⬜ 待真机 |

> 基线表的关键读数：**4K HDR10 → 1080p SDR 软件转码只有 0.79x**，撑不住一路实时。
> 这不是 bug，是 CPU tone mapping 的固有代价 —— 它直接决定了 4K HDR 必须走硬件或客户端直播。

### P4 交付物 · 客户端

| 交付物 | 状态 |
|---|---|
| 播放器抽象层 MediaPlayer：**Web**（`<video>` + hls.js）/ **mpv**（外部进程 + JSON IPC）/ **media3** | ✅ |
| 页面：首页 / 媒体库 / 详情页 / 播放器页 / 设置 / AI 助手 | ✅ |
| 进度上报接线（节流 + 关键节点 + `sendBeacon` 兜底） | ✅ |
| 前后端能力声明单一真源（`capabilities.json` + 一致性测试，注入漂移会红） | ✅ |
| OpenAPI 契约即单一真源 + **契约覆盖率守卫**（加了路由忘进契约会红） | ✅ |
| 桌面壳：系统托盘 / 全局快捷键 / 单实例 / 关窗不退出 / mpv 进程桥接 | ✅ |
| 三端安装包、mpv 播 MKV 真机验、跨端进度误差 < 5s | ⬜ 待真机 |

### P3 交付物 · 播放引擎

| 交付物 | 状态 |
|---|---|
| Playback Decision：客户端能力 × 媒体能力 × 服务端能力 × 网络 → Direct Play / Remux / Transcode / Unsupported | ✅ |
| Direct Play：HTTP Range 直出 `/stream/{id}`，支持 Seek | ✅ |
| Remux：FFmpeg `-c copy` 流式输出 | ✅ |
| Transcode：HLS（`master.m3u8` + 分片），可取消，取消后进程与临时文件 100% 清理 | ✅ |
| FFmpeg Runner：参数构造 / 生命周期 / 日志采集 / 超时 / 重试 / 取消 / 并发上限 | ✅ |
| PlaybackSession：创建 / 心跳 / 关闭 / 续播点；`/playback/resume` | ✅ |
| `subtitle`：内嵌与外挂识别、SRT / ASS / SSA / VTT 抽取转换、轨选择、烧录开关 | ✅ |
| `hardware`：VAAPI / NVENC / QSV / VideoToolbox 探测 | ✅ |
| 兼容性矩阵自动化（`make compat`）：**Web 客户端 0% Unsupported** | ✅ |
| 转码首帧 < 3s / Direct Play 覆盖率 ≥ 70% | ⬜ 待真机 |

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
| Monorepo + Cargo workspace（22 crates / 3 apps） | ✅ |
| 六个核心抽象契约签名冻结 | ✅ |
| 媒体领域模型（MediaItem/Version/File/Stream） | ✅ |
| 优先级任务队列（播放抢占） | ✅ |
| 配置系统（TOML + 环境变量 + 目录布局） | ✅ |
| `/healthz` 与统一错误信封 | ✅ |
| 数据库全表骨架 migration | ✅ |
| Dockerfile 多阶段 + docker-compose | ✅ |
| 双架构 CI（amd64 + arm64 self-hosted） | ✅ |
| Makefile / versions.lock | ✅ |
| Spike S1（Tauri + mpv 桌面播放器） | ✅ Go —— 桌面走**外部进程 + JSON IPC**（见 ADR-0003） |
| Spike S2（Tauri 移动端播放器 / RK3588 硬转） | ✅ Go（Media3 原生插件）；RKMPP 参数保留但真机验证后置 |
| Spike S3（FFmpeg 进程池与资源隔离） | ✅ 任务优先级队列 + 并发上限 + 播放抢占 |
| Spike S4（中文检索） | ✅ FTS5 trigram（够用、零依赖），媒体量级过 5 万再考虑 Tantivy |
| R5S 接入为 arm64 CI runner | ⬜ 待执行 —— 这也是 P0/P5 里"arm64 均通过"两项卡住的唯一原因 |

## 启动教程

### 前置条件

| 依赖 | 版本 | 什么场合才需要 |
|---|---|---|
| Rust | ≥ 1.98（MSRV，见 `versions.lock`） | 始终 |
| Node | 22.x | 只有跑前端才需要 |
| FFmpeg + ffprobe | 任意较新版本 | 转码 / 缩略图要 FFmpeg；`scan --probe` 要 ffprobe |

不装 FFmpeg **也能跑**：扫库、浏览、Direct Play 都正常，只是不能转码 —— 服务端启动时会
探测并打印可用后端（`转码后端可用 backend=...`），一个都没有也能起来。

### 一、开发启动（两个终端）

> ⚠️ **先设环境变量，否则会踩坑。** 配置默认走容器路径（`/config` `/data` `/cache` `/logs`）。
> Linux / macOS 上直接跑会去写根目录然后因权限失败；Windows 上会**静默**在当前盘符建出
> `D:\config` 这样的目录 —— 服务起来了，但你在项目里根本找不到数据库。
> 本地开发一律显式指定：

```bash
export WHATMEDIA_DATABASE_URL="sqlite:./.dev-data/whatmedia.db?mode=rwc"
export WHATMEDIA_CACHE_DIR="./.dev-data/cache"
export WHATMEDIA_LOG_DIR="./.dev-data/logs"
```

**终端 1 —— 服务端**（监听 `0.0.0.0:8080`）：

```bash
make dev          # = RUST_LOG=info WHATMEDIA_LOG_FORMAT=pretty cargo run --bin whatmedia-server
```

看到 `监听中 addr=0.0.0.0:8080` 就是起来了。冒烟：

```bash
curl -s http://127.0.0.1:8080/healthz
# {"status":"ok","version":"0.1.0","arch":"x86_64","os":"...","uptime_secs":3}
```

**终端 2 —— 前端**（<http://127.0.0.1:1420>）：

```bash
cd client && npm install && npm run dev
```

前端端口是 **1420**（不是 8080，也不是 vite 默认的 5173）。`/api` 由 vite 代理到
`127.0.0.1:8080`（见 `client/vite.config.ts` 的 `server.proxy`）。要指向别处的服务端：

```bash
WHATMEDIA_SERVER=http://192.168.1.10:8080 npm run dev
```

**常见坑：**

| 现象 | 原因 | 解法 |
|---|---|---|
| `os error 10013` /「以一种访问权限不允许的方式做了一个访问套接字的尝试」 | 8080 被别的进程占了 | `WHATMEDIA_PORT=8081 make dev`，前端代理跟着设 `WHATMEDIA_SERVER=http://127.0.0.1:8081` |
| 服务起来了，项目里却找不到数据库文件 | 默认走容器路径 `/config`，Windows 上会落到当前盘符根（实测建在 `D:\config`） | 显式设 `WHATMEDIA_DATABASE_URL`（见上） |
| 打开 8080 只有 JSON，没有页面 | 服务端不托管前端 | 访问前端的 1420，或另配静态托管 |
| 扫完了但没有海报和简介 | 没配 TMDB Key，刮削端点返回 501 | `WHATMEDIA_METADATA_TMDB_API_KEY=...` |

### 二、让它第一次有数据

服务端起来时库是空的。扫一个目录进去：

```bash
# --probe 会调 ffprobe 拿时长/码率/音轨字幕轨，慢一些但信息全
cargo run --bin whatmedia-cli -- scan --path /path/to/media --name "电影库" --probe
cargo run --bin whatmedia-cli -- list
cargo run --bin whatmedia-cli -- search --q "Matrix"
```

也可以走 API：`POST /api/v1/scan`（前端设置页的"扫描"按钮走的就是它）。

**只想看界面、手上没有媒体文件**时，用演示数据：

```bash
make seed-demo      # 12 部电影 + 1 部剧（2 季 11 集）+ 演职员 + 3 条续播点
```

它只写浏览用的元数据，**不造假 `media_file`** —— 所以这些条目能浏览、能看详情，
但点播放会报"没有可播放的文件"。这是刻意的：假造一条指向不存在文件的版本，
会让后面排查真实播放问题的人被误导。

### 三、常用命令

```bash
make dev            # 起服务端（8080）
make seed-demo      # 灌演示数据（只写元数据，不造假媒体文件）
make test           # 全量后端测试
make lint           # CI 门禁：fmt --check + clippy -D warnings
make compat         # 播放兼容性矩阵（P3 覆盖率依据）
make openapi        # 改了 API 后重新导出契约 + 刷新前端类型
make desktop-check  # 桌面壳（Tauri）单独检查，它不在主 workspace 里
make docker         # 多架构镜像（需 buildx）
```

前端：

```bash
cd client
npm run dev         # 开发服务器 1420
npm run test        # vitest
npm run typecheck   # vue-tsc --noEmit
npm run gen:api     # 从 openapi.json 重新生成 src/api/schema.d.ts
```

### 环境变量

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
| `WHATMEDIA_AI_ENABLED` | `false` | AI 助手总开关。关掉时 `/ai/*` 返回 503，核心功能不受影响 |
| `WHATMEDIA_AI_PROVIDER` | 空 | `openai` / `deepseek` / `qwen` / `glm` / `ollama` / `vllm` / `llama_cpp` / `custom`（前七种自带默认 base_url） |
| `WHATMEDIA_AI_BASE_URL` | provider 默认 | 留空用默认值。Ollama 通常 `http://localhost:11434` |
| `WHATMEDIA_AI_MODEL` | 空 | 模型名。留空则用规则引擎（**照样能答**，回复标 `degraded`） |
| `WHATMEDIA_AI_API_KEY` | 无 | 密钥本身。配置里存的是**变量名**不是值（`api_key_env`），避免误提交 |
| `WHATMEDIA_AI_TIMEOUT_SECS` | `30` | 单次模型请求超时，本地小主机上的模型别设太短 |
| `WHATMEDIA_AI_MAX_TOOL_ROUNDS` | `6` | 一次对话最多几轮工具调用，防循环失控的硬闸 |
| `WHATMEDIA_AI_ALLOW_DANGEROUS` | `false` | 危险工具开关。**关掉时它们连 `tools/list` 都不出现**，不是调用时才拒绝 |
| `WHATMEDIA_AI_MCP` | `false` | 对外暴露 MCP Server（只出只读工具） |

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

# 4) 问 AI（没配模型也能答，走规则引擎）
curl -s -X POST http://127.0.0.1:8080/api/v1/ai/chat \
  -H 'content-type: application/json' \
  -d '{"message":"找出没有海报的电影"}'
curl -s http://127.0.0.1:8080/api/v1/ai/tools
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

# 本地快速复跑（不必每次等满 10k；正式数字仍以默认 10,000 为准）
WHATMEDIA_SCAN_BASELINE_N=2000 cargo test -p whatmedia-scanner -- --ignored --nocapture scan_10k_baseline
```

这条基线**不只计时**，它还断言 `found == N` / `failed == 0` / 全量 `added == N` / 增量 `skipped == N`
—— 只打印数字的话，少扫一个文件不会红，基线就守不住任何东西。

本机实测（Windows x86_64 / 8 核 / **debug 构建** / `probe=false` / 并发 8）：

| 规模 | 全量扫描 | 增量扫描 |
|---|---|---|
| 10,000 | ~104s | ~1.0s |
| 1,000 | 10.2s | 0.07s |

回归时请**固定口径**再比：debug/release 差一倍以上，`probe` 开关会拉来 ffprobe 的开销。

> 加断言之前，这条基线只打印 `added` 不校验 —— 于是它跑出了 `新增 9999` 却没人觉得不对。
> 补上 `found == N` / `failed == 0` 后立刻抓到：全量扫描偶发丢 3~4 个文件
> （万分之几的概率，SQLite 并发锁冲突），且第二次扫描总能补上。
> 现在 `process_one` 会对瞬时错误重试最多 3 次。详见 P1 的 DoD 备注。

## 部署到实际环境

### Docker

```bash
cp docker/compose.yml docker-compose.yml
# 编辑：把 /path/to/media 换成你自己的媒体目录（保持 :ro）
docker compose up -d
docker compose logs -f
```

> ⚠️ **镜像只包含服务端，不含 Web 界面。**
> 服务端目前没有静态文件托管（未匹配路由就是 404），`Dockerfile` 也不构建前端。
> 所以容器起来后 `http://<host>:8080` 只有 API 和 `/healthz`，浏览器打开**看不到界面**。
> 要用 Web 界面，三条路：
> 1. 开发用：`npm run dev`（1420，代理到 8080）
> 2. 长期用：`npm run build` 产出 `client/dist`，交给 nginx / Caddy 托管静态文件，
>    并把 `/api` 反代到 8080
> 3. 桌面 / 移动端：Tauri 客户端（P4 / P6 待真机，安装包还没出）
>
> 换句话说：**Docker 部署现在只解决"跑起来能扫库能播"，不解决"有个网页能点"。**

三个容易踩的点（compose 文件里有更详细的注释）：

三个容易踩的点（compose 文件里有更详细的注释）：

1. **PUID / PGID** —— 镜像以 uid 1000 运行。宿主机用户不是 1000 时，`/config` 里写出的
   文件会变成 `1000:1000`，你在宿主机上改不动。
2. **媒体目录只读挂载 `:ro`** —— WhatMedia 不需要写媒体文件，只读能挡掉"删库"类事故
   （包括 AI 工具被诱导去删，见 ADR-0006）。
3. **硬件直通默认注释掉** —— 只有需要转码时才开。驱动不对时服务端会**静默**降级到软件
   转码：不报错，只是变慢。

### 裸机（无 Docker）

```bash
cargo build --release --bin whatmedia-server --bin whatmedia-cli
# 产物：target/release/whatmedia-server、target/release/whatmedia-cli
```

`systemd` 单元示例（`/etc/systemd/system/whatmedia.service`）：

```ini
[Unit]
Description=WhatMedia
After=network.target

[Service]
Type=simple
User=whatmedia
Environment=WHATMEDIA_DATABASE_URL=sqlite:/var/lib/whatmedia/whatmedia.db?mode=rwc
Environment=WHATMEDIA_CACHE_DIR=/var/lib/whatmedia/cache
Environment=WHATMEDIA_LOG_DIR=/var/log/whatmedia
Environment=WHATMEDIA_LOG_FORMAT=json
Environment=WHATMEDIA_MIGRATIONS_DIR=/usr/local/share/whatmedia/migrations
ExecStart=/usr/local/bin/whatmedia-server
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

裸机部署**必须**设 `WHATMEDIA_MIGRATIONS_DIR` —— 二进制不带迁移文件，默认的那个路径
只在 `cargo run` 时存在（编译期写死的仓库路径）。

```bash
sudo systemctl daemon-reload && sudo systemctl enable --now whatmedia
```

### 首次启动后要做的事

| 步骤 | 命令 / 位置 | 不做会怎样 |
|---|---|---|
| 扫媒体库 | 设置页「扫描」或 `whatmedia-cli scan --path ...` | 库是空的，什么都没有 |
| 配 TMDB Key | `WHATMEDIA_METADATA_TMDB_API_KEY` 或 `config.toml` 的 `[metadata]` | 刮削端点返回 501，没有海报和简介 |
| 配 AI（可选） | `[ai]` 段或 `WHATMEDIA_AI_*` | AI 助手页不可用；不配则退化成规则引擎 |
| **创建管理员** | Web 首次安装向导，或 `make dev-user WHATMEDIA_BOOTSTRAP_USER=admin WHATMEDIA_BOOTSTRAP_PASSWORD=...` | 所有 `/api/v1` 端点一律 401，前端停在登录页 |

### ⚠️ 暴露到公网之前

API **默认要求登录**，但有几条仍然要自己兜住：

1. **必须开 HTTPS。** 登录口令与会话票据都在明文 HTTP 上等于没鉴权。
2. **多实例部署要配 `WHATMEDIA_SIGNING_KEY`**（32 字节 base64url）。
   不配时每个进程各自随机一把钥匙，另一个实例签出的播放 URL 会被拒。
3. **管理员资格只给该给的人。** 首个管理员由首次安装向导产生，之后只能由它邀请。
4. 反向代理还要注意：Direct Play 依赖 **HTTP Range**（确认 Nginx / Caddy 没有剥掉
   `Range` 头），HLS 是长连接流式响应（别开过短的 `proxy_read_timeout`）。

反向代理还要注意两点：Direct Play 依赖 **HTTP Range**（确认 Nginx / Caddy 没有剥掉
`Range` 头），HLS 是长连接流式响应（别开过短的 `proxy_read_timeout`）。

## 仓库结构

```
apps/       whatmedia-server（HTTP 服务） / whatmedia-cli（运维调试）
crates/     18 个：core config database media library scanner metadata
            storage playback transcoder subtitle task hardware diagnostics
            mpv media3 ai api
migrations/ SQLx 迁移（SQLite 与 PostgreSQL 共用一份）
client/     Vue 3 + Tauri 2（六端共享）
plugins/    Tauri 插件（有各自的 workspace，不在主 workspace 里）
docs/       adr/ spike/ benchmark/ proposal/
```

**依赖分层铁律**：`core` 不依赖任何 crate → `storage`/`metadata`/`hardware` 只依赖 `core`+`config` → `media`/`library`/`scanner` 依赖下层 → `api` 依赖全部。禁止反向与循环依赖。

两个 workspace 之外的成员：`client/src-tauri`（桌面/移动端壳）与 `plugins/tauri-plugin-media3`
（Android 原生播放器）。它们由 `Cargo.toml` 的 `exclude` 排除，所以 `cargo test --workspace`
**不会**带上它们 —— 改了桌面壳要单独跑 `make desktop-check`。

**已实现的边界**：检索没有独立的 `search` crate，走的是 `database` 的 FTS5 + `api` 的 `/search`；
推荐也没有独立 crate，启发式规则在 `ai` 的 `recommend_media` 工具里。
原先为它们预留的空壳 crate 已于 2026-10-16 删除 —— 留着只会让人以为那里有东西。

**已知缺口**

- API 层**零鉴权**，所有路由匿名可达 —— 包括扫库、重新识别这类写操作。
- 服务端**不托管 Web 前端**，Docker 镜像里只有 API。见「部署到实际环境」。

## 双架构说明

**ARM64 支持 ≠ RKMPP。** 分两层：

- **第一层「能跑」（P0 起，硬要求）**：Rust 双 target、CI 双架构、`buildx` 多架构镜像、FFmpeg/libmpv 的 ARM64 构建产物。
- **第二层「跑得好」（P6）**：NEON 优化、RKMPP/RGA 硬件编解码与零拷贝。

R5S（RK3588S）在本项目中扮演两个角色：ARM64 自托管 CI runner，以及最低性能基线设备。

## 版本矩阵

见 [versions.lock](./versions.lock)。**以该文件为准，不以需求稿里写的版本号为准。**

## 许可

MIT OR Apache-2.0
