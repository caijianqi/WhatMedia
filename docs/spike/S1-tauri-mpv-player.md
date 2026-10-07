# Spike S1：Tauri 2 + mpv/libmpv 桌面播放器插件

- 阶段：P0，第 1–3 周
- 时间盒：**1.5–2 周**（含双架构各验一次）
- 负责人：本人
- 状态：待启动

## 为什么必须在 P0 做

桌面端播放器是 Mosaic 唯一**可能导致架构重估**的技术点。111.md §18 建议用 mpv/libmpv，但没有验证过它在 Tauri 2 窗口里到底能不能用。

早两周知道答案，比第 43 周（P4）才知道便宜一百倍。

## 要回答的三个问题

### Q1：libmpv 能否作为独立渲染层嵌入 Tauri 窗口，UI 能否叠加？

需要验证：
- mpv 渲染到独立窗口 / 子窗口，Tauri 的 WebView 在其之上叠加控制层
- 还是反过来（WebView 在下，原生 Surface 在上，控制层用 WebView 画）
- 两种方案在 Windows（WebView2）、Linux（WebKitGTK）、macOS（WKWebView）上表现是否一致

**验证方式**：一个最小 Tauri 应用，窗口里放一个 `<video>` 大小的空白区域，下面挂 mpv；上面叠一个半透明的 Vue 控制条，点击能暂停。

### Q2：三端静态链接与打包体积

- Windows：libmpv 的 DLL 分发与许可
- macOS：.app bundle 内的 dylib 与签名
- Linux：依赖系统 libmpv 还是静态打包（发行版差异大，ARM64 尤其）

### Q3：ARM64 Linux 上 EGL/GL 驱动是否可用

**这是双架构首发最容易翻车的地方。** RK3588 上的 Mali GPU 驱动、EGL 支持在不同发行版上差异极大。必须在 R5S 上真机验证一次。

### Q4：许可证影响

mpv 是 GPLv2+，libmpv 是 LGPLv2.1+。动态链接 LGPL 库对 MIT/Apache 双许可的 Mosaic 可接受，但**静态链接会触发 GPL 传染**。需要确认分发方式。

## 产出

1. 一个能播 MKV + ASS 字幕 + 多音轨的 Win/Linux Demo
2. R5S 上的 ARM64 验证结果（能跑/不能跑/降级方案）
3. `docs/adr/0001-player-backend.md`

## 决策点

| 结论 | 后续 |
|---|---|
| **Go（mpv）** | P4 按 §18 实现 `plugins/media-player` |
| **降级** | Web 端方案为主：HLS + fMP4，字幕服务端烧录；桌面端退化为"套壳浏览器 + 系统默认播放器拉起" |
| **部分 Go** | 桌面 Windows/Linux 用 mpv，macOS 用 AVFoundation；或 ARM64 Linux 暂不支持原生播放 |

## 备选（若 mpv 不可行）

1. `libVLC`（LGPL，ARM 支持较好）
2. GStreamer（Rust 绑定成熟，但字幕与 HDR 处理弱）
3. 直接调用系统播放器：`mpv` / `vlc` / `IINA` 命令行拉起（体验差但零风险）

## 时间盒纪律

到期无论结果如何必须出结论。**不允许"再调研一周"。** 拿不到完整答案就按"部分 Go + 明确降级路径"推进，把未知项写进 `已知限制清单`。
