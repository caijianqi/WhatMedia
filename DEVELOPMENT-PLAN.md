# Mosaic 开发计划（基于 111.md V1.0 分析产出）

> 配套文档：`111.md — Mosaic 完整需求、功能与技术方案 V1.0`  
> 生成日期：2026-10-08  
> 计划版本：v1（草案，待团队规模确认后校准工时）

---

## 0. 前置假设（如与实际不符请指出，我据此重排）

| 项       | 假设值                                                    |
| ------- | ------------------------------------------------------ |
| 团队规模    | 3 人：1 资深 Rust 后端 / 1 全栈（Vue3+Tauri）/ 1 移动端与构建（可兼职 0.5） |
| 单人全职备选  | 若仅 1 人，工时按 **1.8 倍** 折算，且必须砍到 P0–P4                    |
| 首发目标硬件  | R5S（RK3588S）/ ARM64 + Docker（按文档 §首发方向）                |
| 首个可交付形态 | Docker 服务端 + Web UI + Windows/Linux 桌面端                |
| 商业模式    | 开源自托管，无订阅（按 §MVP 暂缓）                                   |
| 仓库形态    | Monorepo（按 §113 推荐）                                    |

---

## 一、文档评估

### 1.1 这份文档做对了什么

| 维度    | 评价                                                                                                                                |
| ----- | --------------------------------------------------------------------------------------------------------------------------------- |
| 领域抽象  | **核心资产**。MediaItem / MediaVersion / MediaFile / MediaStream 四层模型（§28）正确解决"一个作品≠一个文件"，是 Mosaic 与 Jellyfin/Emby 拉开差距的地方，必须最先落地、最晚改动 |
| 解耦设计  | 四个 Provider（Storage / Metadata / Hardware / Model）+ MediaPlayer Plugin + AI Tool 边界清晰，是后续所有扩展的支点                                  |
| 技术栈收敛 | Rust + Vue3 + Tauri2 + FFmpeg + SQLite/PG，一套主栈覆盖 6 个端，避免了"每个端一套业务代码"的灾难                                                           |
| 客户端策略 | Kotlin/Swift 只做 Tauri Plugin 内部实现、不做独立业务 App（§12/§19）——这是正确的，也是最大的成本节省点                                                           |
| 产品边界  | 明确不做 BT/PT/下载/网盘/资源搜索（§122），避免范围失控                                                                                                |
| AI 安全 | AI → Tool → Authorization → Service，禁止直连 SQL/Shell/FS（§4.4/§62/§92）——设计先行，非常必要                                                    |

### 1.2 必须补齐的缺口（按严重程度排序）

| #  | 缺口               | 现状                                                                          | 影响              | 建议动作                                  |
| -- | ---------------- | --------------------------------------------------------------------------- | --------------- | ------------------------------------- |
| G1 | **没有字段级数据模型**    | §77–83 只列了实体名                                                               | 无法开工写 migration | P0 阶段补齐 ER 与索引（见 §四）                  |
| G2 | **没有 API 规范**    | §87 只列 15 条路径                                                               | 前后端无法并行         | P0 出 OpenAPI 3.1 + 错误码 + 分页约定         |
| G3 | **没有验收标准（DoD）**  | §107 只有 API P95<300ms                                                       | 无法判断"做完"        | 每阶段补量化 DoD（本计划已给）                     |
| G4 | **技术风险未验证**      | 直接把 RKMPP/RGA、Tauri2 移动端播放器写进路线图                                            | 这是两个可能导致返工的黑洞   | **必须先做 Spike**（见 §三）                  |
| G5 | **版本路线自相矛盾**     | §114 桌面端在 V0.7、Android 在 V0.8；但 §115 MVP 又要求 Windows/Linux/macOS/Android 全做 | 排期无依据           | 统一为"垂直切片优先"（见 §二）                     |
| G6 | **"扫描不影响播放"缺手段** | §107 只有目标，无机制                                                               | 家庭小主机上必踩        | 任务优先级 + nice/cgroup + 限速 + 播放抢占       |
| G7 | **中文搜索未定义**      | §75 写 SQLite FTS5 → Tantivy                                                 | FTS5 无内置中文分词器   | Spike S4 决策                           |
| G8 | **版本基线写死**       | Rust 1.99.0 / FFmpeg 9.0.2 / Media3 1.11.1                                  | 环境漂移导致构建失败      | P0 改为"最低版本 + 锁定矩阵"，写入 `versions.lock` |

### 1.3 三个我要明确唱反调的判断

1. **不要按 V0.1 → V1.0 十个版本顺序推进。**  
   文档把"完整媒体库""完整播放引擎""ARM""多来源""AI""桌面""移动TV"串成线性阶段，看起来合理，实际上会让"能端到端播一部片子"这件事推迟到第 12 周以后。没有早期闭环，就没有真实反馈，后面九个版本全部建立在假设上。**必须先打一条从头穿到尾的细线，再横向加宽。**
2. **RK3588 硬件转码不要当作"支持"，要当作"实验特性 + 分级降级"。**  
   FFmpeg 主线对 Rockchip MPP 的支持不完整，实践中通常依赖 Rockchip 官方/社区 fork 的 ffmpeg-rockchip；RGA 零拷贝链路更是强依赖内核与库版本。文档 §48/§49 把它写成一级平台能力，风险敞口过大。正确做法：把它封装成**可切换的 media-runtime 后端**，并让 §49 的"自动降级链"成为默认行为——Direct → Remux → HW Transcode → SW Transcode，**任何一环失败都要能静默降级，而不是报错**。
3. **"所有客户端统一 Tauri 2"是对的，但播放器插件是最大的单点风险，且文档低估了它。**  
   Android 端 Media3/ExoPlayer 对 PGS 位图字幕、复杂 ASS 特效、Dolby Vision 的支持都不完整；原生 Surface 与 Tauri WebView 的图层叠加（播放器在 WebView 之下还是之上、UI 如何叠加）也需要实测。**这部分必须在 M0 用两周做原型验证，而不是等到第 29 周才碰。**

---

## 二、开发策略：垂直切片优先

```
                    Mosaic 推进模型

   阶段      P0   P1   P2   P3   P4   P5   P6   P7
              │    │    │    │    │    │    │    │
  地基  ──────████████████████████████████████████
  媒体内核 ────────███████████████████████████████
  元数据 ───────────────██████████████████████████
  播放引擎 ──────────────────█████████████████████
  客户端 ───────────────────────██████████████████
  ARM/Docker ────────────────────────█████████████
  移动/TV ─────────────────────────────────███████
  AI ──────────────────────────────────────────███

  ★ P3 结束 = 第一个真正可用的 Mosaic（Docker + Web + 桌面）
  ★ P5 结束 = R5S/ARM64 交付，达到文档 §V1.0 的"低资源/ARM友好"
  ★ P6/P7 = 文档 §V0.8 / §V0.6 的完整形态
```

**三条收敛原则：**

1. **每个阶段结束时必须有可演示的东西**，哪怕只支持一种容器、一种编码、一个平台。
2. **抽象接口先落地，实现后补。** StorageProvider 第一个实现只做 Local；MetadataProvider 第一个实现只做 NFO + TMDB。接口一定义，后面加实现是并行任务，不是串行任务。
3. **先做"能播"，再做"播得好"。** Direct Play 跑通 > Remux > 软件转码 > 硬件转码 > HDR Tone Mapping > Dolby Vision。每一级都是一个可独立交付的价值增量。

---

## 三、关键技术攻关（Spike）清单 —— P0 必须完成

> **规则：每个 Spike 有时间盒（Timebox），到期必须出结论（Go / No-Go / 降级方案）。不允许无限期调研。**

| ID     | 攻关项                    | 时间盒 | 关键问题                                                                                                 | 产出与决策点                                                                               |
| ------ | ---------------------- | --- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| **S1** | Tauri 2 移动端原生播放器插件     | 2 周 | ① Media3 能否作为独立 Surface 层嵌入 Tauri WebView 之下，UI 能否叠加？② PGS/ASS 字幕渲染方案？③ MediaSession/PiP/后台是否稳定？     | 一个可播放本地 MP4 + HLS 的 Android Demo。**决策：Go（原生插件）/ 降级（仅 HLS+fMP4，字幕服务端烧录）**             |
| **S2** | RK3588 硬件转码链路          | 2 周 | ① mainline FFmpeg vs ffmpeg-rockchip 哪个可用？② MPP 解码 + RGA 缩放 + MPP 编码能否零拷贝？③ HDR→SDR tone mapping 性能？ | 一份实测报告（含 FPS/CPU/温度）。**决策：RKMPP 进主线 / 仅做解码 / 暂时纯软件转码**                               |
| **S3** | FFmpeg 进程池与资源隔离        | 1 周 | 扫描/刮削/转码并发时如何保证播放不卡？                                                                                 | 任务优先级队列 + `nice`/cgroup/线程亲和 + 限速方案；给出 R5S 上的并发上限数值                                  |
| **S4** | 中文搜索方案                 | 3 天 | FTS5 无中文分词器                                                                                          | **决策：FTS5 trigram（够用、零依赖）/ jieba 扩展 / 直接上 Tantivy**。建议先 trigram，媒体量级过 5 万再引入 Tantivy |
| **S5** | SQLite ↔ PostgreSQL 双栈 | 3 天 | SQLx 编译期校验 + 方言差异（UPSERT、JSON、全文索引）                                                                  | 抽象层设计 + 迁移脚本双份。**决策：MVP 只做 SQLite，PG 抽象预留但 P5 再验证**                                  |
| **S6** | 大库扫描性能基线               | 3 天 | 10 万文件全量扫描耗时、内存峰值                                                                                    | 建立基准数据与后续回归基线；确定 ffprobe 并发度                                                         |

**S1 与 S2 是项目的生死线，必须在 P0 内出结论。** 若 S1 失败，整个"一套 Tauri 覆盖六端"的架构需要重估；若 S2 失败，R5S 首发方向需要改为"ARM64 优先、硬件转码后补"。

---

## 四、阶段计划（P0 – P7）

### P0 · 工程地基与决策验证 —— 第 1–3 周

**目标：把不确定性变成确定结论，把仓库与 CI 跑通。**

**范围**

- Monorepo 初始化（结构见 §五）
- 版本矩阵锁定：Rust / FFmpeg（构建方式：静态构建 vs 系统包）/ Node / Tauri CLI / Android SDK-NDK / Media3 → 写入 `versions.lock`
- CI：GitHub Actions
  - `cargo fmt --check` + `cargo clippy -- -D warnings` + `cargo test`
  - `cargo sqlx prepare` 离线校验
  - linux amd64 + arm64（arm64 用自托管 R5S runner）
  - Vitest + `vue-tsc`
  - `docker buildx` 多架构镜像
  - `tauri build`（先 Windows + Linux）
- `config` crate：TOML + 环境变量覆盖；目录布局 `/config` `/data` `/cache` `/logs`
- `tracing` 基础设施：`trace_id` / `task_id` / `session_id` 贯穿
- Spike S1–S6 全部启动，S1/S2 出结论

**交付物**：空服务可 `docker compose up` 启动，`GET /healthz` 返回 200；CI 全绿；6 份 Spike 报告。

**DoD**

- [ ] 全新克隆仓库，`make dev` 一条命令启动服务端 + Web
- [ ] `make build` 产出 amd64/arm64 两个镜像
- [ ] `make test` 在 amd64 与 arm64 均通过
- [ ] S1/S2 结论已写入 `docs/adr/`，架构路线确定

---

### P1 · 媒体内核垂直切片 —— 第 4–9 周

**目标：本地目录 → 扫描 → ffprobe → 识别 → 入库 → API 可查。这是全项目的第一根主梁。**

**范围**

- `core`：MediaItem / MediaVersion / MediaFile / MediaStream 领域模型（**此模型一旦确定，后续只允许加字段不允许重构**）
- `database`：migrations + SQLx（见 §六表结构）
- `storage`：StorageProvider trait + **Local 实现**；路径安全四件套（canonicalize / root boundary / symlink check / permission check）
- `scanner`：首次/全量/增量（mtime+size 指纹）、扩展名过滤、并发控制、可取消可暂停
- FFprobe Runner：进程封装、超时、JSON 解析 → MediaFile + MediaStream
- 文件名解析器：标题 / 年份 / 季 / 集 / 分辨率 / 视频编码 / 色深 / HDR / 音频 / 来源 / 版本 / Release Group
- NFO 读取：movie.nfo / tvshow.nfo / episode.nfo
- `api` v1：`/libraries` `/media` `/movies` `/series` `/episodes` `/search`；统一分页 / 排序 / 过滤 / 错误码
- `task`：扫描任务 + SSE 进度推送

**交付物**：命令行 `mosaic-cli scan --path /media` 能扫出一个可查询的库；Web 端能看到海报墙（图片占位符即可）。

**DoD**

- [ ] 10,000 文件样本库全量扫描耗时建立基线（先测后定标）
- [ ] 无变更增量扫描 ≤ 30s
- [ ] 季/集解析在 `test-media` 命名语料上准确率 ≥ 99%
- [ ] 路径穿越 / 符号链接逃逸 有单元测试覆盖并通过
- [ ] 列表 API P95 < 300ms（10 万条数据下）

---

### P2 · 元数据与图片 —— 第 10–14 周

**目标：让媒体库"看得懂"。**

**范围**

- `metadata`：MetadataProvider trait + **TMDB / TVDB** + **Bangumi**（动漫）+ **Local NFO**
- 优先级链（§32）：NFO > 手动锁定 > Provider，**字段级锁定**
- 匹配流程：文件名/年份 → 搜索 → 候选打分 → 自动确认 / 人工确认 API
- 图片系统：poster / backdrop / still / logo；FFmpeg 生成缩略图与多尺寸；缓存（LRU + 按大小/时间/类型清理）；缺图占位
- 人物 / 类型 / 演职员表关系

**DoD**（2026-10-08 实测状态）

- [x] 刮削任务可断点续跑、可限流（`ratelimit` 最小间隔 + 退避重试；默认 4 rps 可配）
- [x] 元数据写入后不覆盖用户手动修改的字段（字段级锁定 + NFO 优先级链）
- [x] 图片缓存 LRU 与清理策略（按时间 / 体积 / 类型三道闸，离线可测）
- [ ] **1,000 部电影样本刮削命中率 ≥ 95%** —— 需真实 TMDB Key 在真机跑，阈值尚未用真实数据校准
- [ ] **图片缓存命中率 ≥ 90%（二次访问）** —— 同上，需真机二次访问数据
- [ ] `metadata_job` 与 `image_cache` 在 10 万条量级下的查询性能基线

> 上面三条未勾选项的共同前提都是"有真实 Provider 配额"。
> 当前所有逻辑用 `FixtureTransport` 离线验证了正确性，
> 但**打分阈值（0.86 / 0.45 / 0.06）是拍脑袋定的**，必须用真实样本回来校准。

---

### P3 · 播放引擎 —— 第 15–21 周 ★第一个真正可用的版本

**目标：能播。这是产品的生死线。**

**范围**

- Playback Decision：Client Capability + Media Capability + Server Capability + Network → Direct Play / Remux / Transcode / Unsupported
- **Direct Play**：HTTP Range 直出 `GET /stream/{id}`，支持 Seek
- **Remux**：FFmpeg `-c copy` → fMP4 / MPEG-TS 流式输出
- **Transcode**：HLS（`master.m3u8` + 分片）、fMP4；分片缓存与复用；可取消
- FFmpeg Runner：参数构造、进程生命周期、日志采集、超时/重试/取消、并发上限与优先级（S3 成果落地）
- PlaybackSession：创建 / 心跳 / 关闭 / 续播点
- `subtitle`：内嵌与外挂识别；SRT / ASS / SSA / VTT 抽取与转换；字幕轨选择；烧录开关
- `hardware`：HardwareProfile 探测（CPU / 内存 / DRM / VAAPI / NVENC / QSV / VideoToolbox）
- **字幕降级策略（明确写进文档）**：PGS / DVD SUB 等客户端不支持的位图字幕 → 自动转 burn-in 或转 SRT（OCR 不做）


**DoD**

- [ ] 兼容性矩阵自动化跑通：H264/HEVC/AV1/VP9 × 8bit/10bit × AAC/AC3/EAC3/DTS/TrueHD × SRT/ASS/PGS × MP4/MKV/TS
- [ ] 常见家庭格式 Direct Play 覆盖率 ≥ 70%
- [ ] 转码首帧时间 < 3s；Seek 后恢复 < 1.5s
- [ ] 转码任务可取消、取消后进程与临时文件 100% 清理
- [ ] 播放期间触发全量扫描，播放不卡顿（S3 验收）

---

### P4 · 客户端：Web + Tauri 桌面 —— 第 22–28 周

**目标：从"能播"到"好用"。**

**范围**

- Vue3 + TS + Vite + Tailwind + Element Plus；API 类型由 OpenAPI 自动生成（避免手写漂移）
- 页面：首页（继续观看 / 最近添加 / 最近观看 / 我的收藏）、媒体库（网格/列表、筛选/排序/分页）、详情页（版本切换 / 音轨 / 字幕 / 演职员）、播放器页、设置（库管理 / 用户 / 转码 / 存储 / 任务中心）
- 播放器抽象层 MediaPlayer API：Web（`<video>` + hls.js）↔ Tauri Plugin（mpv/libmpv）——**同一套上层调用接口**
- Tauri 插件首批：media-player（mpv）、file-access、notification、device、download、storage、pip
- 桌面能力：系统托盘、全局快捷键、单实例、文件关联

**DoD**

- [ ] Windows / macOS / Linux 三端可安装、可直连服务器、完整走通"浏览→播放→续播"
- [ ] 桌面端 mpv 播放器支持 MKV + ASS + 多音轨 + 外挂字幕
- [ ] 播放进度跨端同步（Web ↔ 桌面误差 < 5s）
- [ ] 安装包体积与冷启动时间建立基线

---

### P5 · ARM / R5S / Docker 交付 —— 第 29–33 周 ★达成文档 §V1.0 "低资源 / ARM 友好"

**目标：让它在真正该跑的地方跑得动。**

**范围**

- R5S 专项模式（§49）：启动自检 → HardwareProfile → 自动降级链 Direct → Remux → HW Transcode → SW Transcode
- RKMPP / RGA 接入：按 S2 结论做成**可切换的 media-runtime 后端**（mainline / ffmpeg-rockchip 二选一，通过配置或构建特性切换）
- HDR：HDR10 / HLG / Dolby Vision 识别与展示；HDR→SDR tone mapping（先 OpenCL/CPU，RGA 后补）
- Docker：多架构镜像、compose 模板、设备直通（`/dev/dri` `/dev/mpp_service` `/dev/rga`）、非 root 运行、健康检查
- 性能基准：R5S 上跑 4K H264 / 4K HEVC / 4K HEVC 10bit / HDR10 / AV1 / DTS / PGS / HDR→SDR，记录 CPU / 内存 / 温度 / 功耗 / VPU / FPS / 转码速度 / 延迟，形成公开基线表（`docs/benchmark/`）

**DoD**

- [ ] R5S 上 1080p H264 硬件转码稳定 ≥ 1 路实时
- [ ] `docker compose up` 后 5 分钟内完成初始化并可播放
- [ ] 镜像同时提供 amd64 与 arm64，CI 自动构建
- [ ] 基线表发布到仓库，作为后续性能回归的比较基准
- [ ] 硬件转码失败时自动静默降级到软件转码，用户无感知

---

### P6 · 移动端与 TV —— 第 34–40 周

> **前置条件：S1 结论为 Go。** 若为降级方案，本阶段缩减为"仅 HLS 播放 + 服务端烧录字幕"。

**范围**

- Tauri 2 Android 工程搭建（§58 注意事项：WebView 版本、权限、生命周期、后台限制）
- Media3 / ExoPlayer 播放器插件（§55）：MediaSession、PiP、后台播放、通知栏控制、锁屏控制、蓝牙耳机、音轨/字幕/画质切换
- 移动端 UI 适配：触摸目标、安全区、竖屏/横屏、手势
- Android TV（§15/§16）：10-foot UI、D-Pad 导航、Focus 管理、遥控器按键映射（返回/播放/暂停/快进快退）
- iOS（§19）：AVFoundation 插件、后台播放、AirPlay、PiP、系统媒体控制（需 Apple 开发者账号 + 真机）

**DoD**

- [ ] Android 手机真机：Direct / HLS / 外挂字幕 / 音轨切换 / 画质切换 / 后台 / 通知栏 全部可用
- [ ] Android TV 真机：全遥控器操作可完成"选片→播放→调字幕→退出"
- [ ] iOS 至少达到 Alpha（可登录、可浏览、可播 HLS）
- [ ] 网络切换（WiFi ↔ 4G）不崩溃，可恢复播放

---

### P7 · AI 媒体管家与生态 —— 第 41–48 周

**范围**

- `ai`：ModelProvider（OpenAI 兼容 / DeepSeek / Qwen / GLM / Ollama / vLLM / llama.cpp），**不绑定单一厂商**
- Tool 首批 8 个：`search_media` `get_media` `scan_library` `identify_media` `find_missing_poster` `find_duplicate` `recommend_media` `get_playback_capability`
- 权限模型（§62）：只读工具自动执行；危险工具（删除 / 移动 / 重命名 / 批量修改 / 清理）强制走 **Plan → Preview → Confirm → Execute**
- 5 个 Agent：Media / Library / Subtitle / Maintenance / Recommendation
- MCP Server（对外暴露）+ MCP Client（调用外部）（§64）
- 收尾：多用户 / 家庭账户 / 儿童模式 / 设备管理、插件系统（§73/§74）、备份恢复（§97/§98）、诊断（§95）、审计（§96）、i18n（§105）、主题（§106）

**DoD**

- [ ] 文档 §59 的 7 条自然语言指令样例通过率 ≥ 80%
- [ ] 所有写操作均有预览与二次确认，不可绕过
- [ ] AI 路径经过静态检查确认无法触达 SQL / Shell / Filesystem
- [ ] 断网 / 模型不可用时，AI 功能降级为不可用，核心媒体功能不受影响

---

## 五、仓库结构与工程规范

```
mosaic/                          # Monorepo
├── apps/
│   ├── mosaic-server/           # 服务端二进制入口
│   ├── mosaic-cli/              # 运维/调试 CLI
│   └── mosaic-agent/            # 独立 AI 进程（可选，P7）
├── crates/
│   ├── core/                    # 领域模型（最稳定，改动需评审）
│   ├── config/  database/  auth/
│   ├── media/  library/  scanner/  metadata/
│   ├── storage/                 # StorageProvider + 各实现
│   ├── playback/  transcoder/  subtitle/
│   ├── search/  recommendation/
│   ├── task/                    # 任务队列与优先级（S3 核心）
│   ├── ai/                      # Agent + Tool + MCP
│   ├── plugin/  hardware/  diagnostics/
│   └── api/                     # Axum 路由层（唯一对外出口）
├── client/                      # Vue3 + Tauri2
│   ├── src/                     # 共享 UI（views/stores/api/player/ai/platform）
│   └── src-tauri/
├── plugins/                     # 9 个 Tauri 插件
├── migrations/                  # SQLx
├── docker/                      # compose + Dockerfile（多架构）
├── runtime/                     # FFmpeg 构建/打包脚本与版本锁定
├── tests/                       # 集成测试 + 性能基准
├── test-media/                  # 媒体兼容语料库（生成脚本 + 清单）
├── docs/                        # adr/ benchmark/ api/
└── versions.lock                # 版本矩阵（G8）
```

**依赖分层铁律（CI 用 `cargo-deny` 强制）：**

> `core` 不依赖任何 crate；`storage`/`metadata`/`hardware` 只依赖 `core`+`config`；`media`/`library`/`scanner` 依赖下层；`api` 依赖全部；**禁止反向依赖与循环依赖**。

**规范**

- 分支：`main`（受保护）+ `feat/*` `fix/*` `spike/*`；PR 必须 CI 绿 + 1 人评审
- 提交：Conventional Commits（`feat(media): ...`）
- API 变更必须同步更新 OpenAPI，`client/src/api` 类型由 OpenAPI codegen 生成，**禁止手写**
- DB 变更只允许走 migrations，禁止改历史 migration
- 每个 crate 的公开 API 变更需在 PR 描述中显式说明

---

## 六、核心数据模型落地（补齐 G1）

> 仅列关键表与关键字段，完整 ER 在 P1 输出。

| 表                               | 关键字段                                                                                                                                                                          | 关键索引                                                               |
| ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| `library`                       | id, name, type, storage_provider, root_path, scan_policy, metadata_providers, naming_policy                                                                                   | —                                                                  |
| `media_item`                    | id, library_id, kind(movie/episode/...), title, original_title, year, sort_title, overview, runtime_ms, rating, locked_fields(json), metadata_provider, provider_id, added_at | `(library_id, kind)`, `provider_id`, FTS5(title, original_title)   |
| `media_version`                 | id, item_id, name(2160P REMUX 等), source, resolution, video_codec, hdr_type, is_default                                                                                       | `(item_id)`                                                        |
| `media_file`                    | id, version_id, storage_provider, path, container, size_bytes, duration_ms, bitrate, file_fingerprint(mtime+size), probe_json, is_available                                   | **UNIQUE(storage_provider, path)**, `fingerprint`                  |
| `media_stream`                  | id, file_id, stream_type(video/audio/subtitle), index, codec, language, title, is_default, is_forced, width/height/bit_depth/hdr, channels                                    | `(file_id, stream_type)`                                           |
| `series` / `season` / `episode` | series: id,title,year / season: id,series_id,season_number / episode: id,season_id,episode_number,item_id                                                                     | `(series_id, season_number)`, `(season_id, episode_number)` UNIQUE |
| `image`                         | id, item_id, kind(poster/backdrop/still/logo), url_or_path, width, height, cached_path                                                                                        | `(item_id, kind)`                                                  |
| `person` / `credit`             | person: id,name,image / credit: person_id,item_id,role,character,order                                                                                                        | `(item_id)`                                                        |
| `playback_session`              | id, user_id, item_id, file_id, device_id, position_ms, strategy(direct/remux/transcode), updated_at                                                                           | `(user_id, updated_at DESC)`                                       |
| `transcode_job`                 | id, session_id, ffmpeg_args, hw_accel, state, progress, output_dir, pid                                                                                                       | `(state)`                                                          |
| `task`                          | id, kind, payload, state, priority, progress, error, created_at                                                                                                               | `(state, priority)`                                                |

**要点**

- `file_fingerprint = (mtime_ns, size)` 是增量扫描的核心，不可只用一个
- `locked_fields` 用 JSON 存字段级锁定，支撑 §32 元数据优先级
- `probe_json` 保留 ffprobe 原始输出，避免重新探测
- FTS5 用 **trigram tokenizer**（S4 结论），中文可用；数据量大时再切 Tantivy

---

## 七、API 与协议落地（补齐 G2）

| 项         | 约定                                                                                                         |
| --------- | ---------------------------------------------------------------------------------------------------------- |
| 前缀        | `/api/v1`，全 JSON，`application/json; charset=utf-8`                                                         |
| 分页        | `?page=1&page_size=50`，响应统一 `{ data: [], page: { total, page, page_size } }`                               |
| 排序/过滤     | `?sort=added_at:desc&filter[year]=2024`                                                                    |
| 错误码       | 统一 `{ error: { code, message, trace_id } }`；HTTP 状态 + 业务码双轨（例：`MEDIA_NOT_FOUND` / `TRANSCODE_UNSUPPORTED`） |
| 鉴权        | `Authorization: Bearer <access_token>`；四类 Token（Access/Refresh/Device/API/AI Key）**互不混用**（§90）             |
| SSE       | `/api/v1/events` — 扫描/刮削/转码/AI/任务进度                                                                        |
| WebSocket | `/api/v1/ws` — 播放状态、设备状态、远程控制、实时同步                                                                         |
| 幂等        | 写操作支持 `Idempotency-Key`                                                                                    |
| 文档        | OpenAPI 3.1 由代码生成（`utoipa`），CI 校验与代码同步                                                                     |

---

## 八、测试策略

| 层级   | 覆盖内容                                                                       | 工具                                     | 门禁                        |
| ---- | -------------------------------------------------------------------------- | -------------------------------------- | ------------------------- |
| 单元   | 文件名解析、路径安全、播放决策、FFmpeg 参数构造                                                | `cargo test`                           | 覆盖率 ≥ 70%（核心 crate ≥ 85%） |
| 集成   | 扫描→入库→API 全链路（用临时目录 + SQLite 内存库）                                          | `cargo test` + testcontainers          | 每 PR                      |
| 契约   | OpenAPI 与客户端类型一致性                                                          | codegen diff                           | 每 PR                      |
| 媒体兼容 | §109 矩阵：H264/HEVC/AV1/VP9 × 8/10bit × HDR10/HLG/DV × 8 种音频 × 6 种字幕 × 7 种容器 | 自建 `test-media` 语料 + 脚本化播放断言           | P3 起每版本                   |
| 性能   | §107/§108：API P95、首页 P95、R5S 转码 FPS/CPU/温度                                 | `criterion` + 自建基准脚本                   | 每次 release；退化 > 10% 阻断    |
| 客户端  | 播放/Seek/字幕/音轨/HDR/后台/PiP/网络切换                                              | Playwright（Web）+ 真机手工 checklist（移动/TV） | 每版本                       |
| 安全   | 路径穿越、符号链接逃逸、越权访问、AI 越权                                                     | 专项测试集                                  | 每 PR                      |


**`test-media` 语料库**：用脚本从公开样本 + FFmpeg 合成生成（不入库大文件，只存生成脚本与清单），CI 按需生成。

---

## 九、风险登记册

| #   | 风险                      | 概率    | 影响     | 缓解措施                                                   | 检查点            |
| --- | ----------------------- | ----- | ------ | ------------------------------------------------------ | -------------- |
| R1  | Tauri 2 移动端播放器插件不可行（S1） | 中     | **致命** | M0 原型验证；降级为 HLS + 服务端烧录字幕；极端情况 Android 端改用原生壳 + Web 内容 | 第 3 周          |
| R2  | RK3588 硬件转码链路不稳定（S2）    | **高** | 高      | 封装为可切换 runtime 后端；默认自动降级到软件转码；性能目标下调并公开基线              | 第 3 周 + 第 33 周 |
| R3  | 范围失控（126 节全做）           | **高** | **致命** | 严格执行垂直切片；P7 的功能逐个按价值重排，做不完就砍                           | 每阶段评审          |
| R4  | 媒体领域模型早期定错，后期重构         | 中     | **致命** | P1 完成时做一次模型评审，邀请真实影音用户参与；此后只允许加字段                      | 第 9 周          |
| R5  | 扫描/刮削拖垮小主机              | 中     | 高      | S3 优先级队列 + 限速 + 播放抢占；默认扫描并发度按 CPU 核数自适应                | 第 21 周         |
| R6  | FFmpeg 版本/构建漂移导致行为不一致   | 中     | 中      | `versions.lock` + Docker 内固定构建产物 + CI 矩阵验证             | 持续             |
| R7  | 字幕兼容（PGS/ASS/复杂特效）      | **高** | 中      | 明确降级策略（burn-in / 转 SRT）；不做 OCR                         | 第 21 周         |
| R8  | 中文搜索效果不达预期              | 中     | 中      | S4 决策；trigram 起步，量级上来后切 Tantivy                        | 第 3 周 + 第 14 周 |
| R9  | 移动端应用商店 / iOS 签名合规      | 中     | 中      | iOS 定位为 Alpha，P6 最后做；预留账号与证书时间                         | 第 34 周         |
| R10 | 单人/小团队长期作战疲劳            | 中     | 高      | 强制每阶段有可演示产出；P4 结束时对外发一次可用版本                            | 第 28 周         |

---

## 十、立即行动清单

### Now（本周）

1. 确认团队规模与首发平台（R5S 是否真的是第一目标）——**这一条会直接改变排期**
2. 起 Monorepo 骨架，只放 `core` + `config` + 空 `api`，跑通 CI
3. 启动 **S1（Tauri 播放器插件）** 与 **S2（RK3588 转码）** 两个 Spike，各自两周时间盒
4. 锁定版本矩阵，写 `versions.lock`（G8）

### Next（第 2–3 周）

1. 补齐 `media_item / media_file / media_stream` 的字段级设计与 migration（G1）
2. 输出 OpenAPI 骨架与错误码规范（G2）
3. Spike S3–S6 并行推进
4. 与真实影音用户对一次媒体模型设计（R4）

### Later（不要现在做）

- 任何 AI 相关代码
- 任何移动端代码（等 S1 结论）
- SMB/NFS/WebDAV/S3/STRM/OpenList（等 Local 跑通）
- 多用户 / 插件市场 / DLNA
- PostgreSQL（MVP 只做 SQLite，抽象预留）

---

## 十一、总览时间表

| 阶段                | 周期  | 累计（3 人） | 累计（1 人） | 里程碑产出                |
| ----------------- | --- | ------- | ------- | -------------------- |
| P0 地基与 Spike      | 3 周 | 3 周     | 5 周     | 空服务可跑 + 6 份 Spike 结论 |
| P1 媒体内核           | 6 周 | 9 周     | 16 周    | 扫描→入库→可查询            |
| P2 元数据            | 5 周 | 14 周    | 25 周    | 海报墙 + 搜索可用           |
| P3 播放引擎           | 7 周 | 21 周    | 37 周    | **★ 第一个可用版本**        |
| P4 Web+桌面端        | 7 周 | 28 周    | 50 周    | **★ 对外发布可用版**        |
| P5 ARM/R5S/Docker | 5 周 | 33 周    | —       | 达成"低资源/ARM友好"        |
| P6 移动与 TV         | 7 周 | 40 周    | —       | 六端统一                 |
| P7 AI 与生态         | 8 周 | 48 周    | —       | V1.0 GA              |

> 1 人全职建议只做到 **P4（约 50 周）** 并对外发布，P5–P7 视反馈再决定；  
> 3 人小组约 **48 周（11 个月）** 到 V1.0。

---

## 附：需要修订 111.md 的 8 处

1. §114 版本路线 → 改为垂直切片（本计划 §二），删除与 §115 的口径冲突
2. §77–83 → 补字段级模型与索引（本计划 §六）
3. §87 → 补 OpenAPI、错误码、分页、幂等（本计划 §七）
4. §107 → 补各阶段量化 DoD 与性能下限（本计划各阶段 DoD）
5. §48/§49 → RK3588 标注为"实验特性 + 自动降级"，不做硬承诺
6. §50 → 补字幕降级策略（PGS/ASS → burn-in 或转 SRT，不做 OCR）
7. §75 → 明确中文搜索方案（trigram 起步）
8. §1/§26/§59 → 删除写死的版本号，改为 `versions.lock` 引用
