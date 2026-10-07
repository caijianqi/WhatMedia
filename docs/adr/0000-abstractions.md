# ADR 0000：六个核心抽象契约签名冻结

- 状态：**已接受**（P0，2026-10-08）
- 决策人：单人项目即本人，但改动需显式重开本 ADR
- 相关：111.md §4.1–4.5、§24、§31、§47、§53、§61、§63

## 背景

Mosaic 是一个人、约两年、126 节全量的项目。范围不砍的前提下，唯一能控制风险的手段是**让后期工作变成"加实现"而不是"改结构"**。

六个 Provider/Plugin 抽象是全部扩展的支点。它们的签名一旦在中期变动，所有已写实现都要跟着改。因此必须在写第一行业务代码之前把签名定死。

## 决策

以下六个 trait 的**既有方法签名冻结**，定义在 `crates/core/src/provider.rs`：

| # | Trait | 职责 | 首次实现 |
|---|---|---|---|
| ① | `StorageProvider` | 上层媒体系统不得关心文件位于什么存储 | Local（P1）→ SMB/NFS/WebDAV/S3/STRM/OpenList（P5） |
| ② | `MetadataProvider` | 元数据不得绑定单一数据源 | NFO + TMDB（P2）→ TVDB/Bangumi/豆瓣/Fanart |
| ③ | `HardwareAccelerationProvider` | 把不同硬件统一起来 | Software（P3）→ QSV/NVENC/VAAPI/VT/RKMPP（P6） |
| ④ | `ModelProvider` | 不绑定单一大模型服务商 | 任意 OpenAI 兼容（P8） |
| ⑤ | `MediaPlayer` | Web/桌面/移动/TV 共用同一套上层接口 | Web `<video>`+hls.js（P3）→ mpv（P4）→ Media3（P7） |
| ⑥ | `AiTool` | AI 只能通过 Tool 触达业务，禁止直连 SQL/Shell/FS | P8 |

### 允许与禁止

**允许（无需重开 ADR）**
- 新增**带默认实现**的方法
- 新增类型（枚举变体、结构体字段用 `Option`/`Default`）

**禁止（必须先改本 ADR 并说明理由）**
- 修改既有方法名、参数列表、返回类型
- 删除既有方法
- 改变既有方法的语义（例如 `range_read` 的 end 是否含端点）

## 关键设计点

### ① `range_read` 必须存在
Direct Play 与代理播放都依赖 HTTP Range。如果 StorageProvider 只暴露 `read()`，S3/STRM 这类远程存储在播放时会被迫全量下载。**这一条决定了远程媒体能不能播。**

### ③ 硬件后端返回参数片段而非完整命令
`decoder_args()` / `encoder_args()` / `scale_filter()` / `tonemap_filter()` 返回 `Vec<String>` 片段，由 transcoder 拼装。这样：
- 新增硬件后端不需要动 transcoder
- RKMPP 可以做成**可切换的 media-runtime 后端**（FFmpeg 主线对 Rockchip 支持不完整，实践中常依赖 ffmpeg-rockchip fork）
- 硬件失败时可以整体回退到 `Software`，调用方无感知

### ⑤ `subscribe()` 返回 broadcast receiver
播放状态、进度、错误统一走一个事件通道。SSE、WebSocket、UI 三处消费者共用，避免三套状态机。

### ⑥ `ToolRisk` + `confirmation` 双保险
`AiTool::risk()` 声明危险等级；`ToolContext::confirmation` 为空时，Dangerous 工具**必须**返回 `MosaicError::AiDenied`。

这是 111.md §4.4 / §62 / §92 的"AI 不直连 SQL/Shell/FS"在代码层的强制点。它同时支撑 §62 的 Plan → Preview → Confirm → Execute 流程。

## 后果

- **正面**：P5 加 SMB 时不需要动 media/scanner；P6 加 RKMPP 时不需要动 transcoder；P8 加 AI 时不需要动任何业务 crate。
- **负面**：P0 阶段这些 trait 只有一个（或零个）实现，看起来像过度设计。**这是有意为之** —— 空实现的成本远低于中期返工。
- **约束**：`crates/core` 不得依赖任何其他 mosaic crate（CI 用 cargo-deny 强制）。

## 复核时点

- P1 结束时结合真实媒体数据复核一次
- P5 接入第二种存储时复核 ①
- P6 接入第二种硬件时复核 ③
- P8 接入 AI 时复核 ⑥
