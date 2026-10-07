# Spike S2：Tauri 2 移动端原生播放器插件

- 阶段：P0，第 1–3 周（**结论先出，实现在 P7**）
- 时间盒：**2 周**
- 负责人：本人
- 状态：待启动

## 为什么在 P0 就做，而实现在 P7

111.md §12/§19 定了一条重要架构：**Android 的 Kotlin、iOS 的 Swift 只做 Tauri Plugin 内部实现，不建独立业务 App。**

这条架构能不能成立，完全取决于"原生播放器能否塞进 Tauri 应用"。如果不行，整个移动端方案要重估 —— 而这个结论不能等到第 64 周（P7）才知道。

所以：**P0 验证，P7 实现。**

## 要回答的问题

### Q1：Media3/ExoPlayer 能否作为独立 Surface 嵌入，UI 能否叠加？

- `PlayerView` / `SurfaceView` 与 Tauri WebView 的层级关系
- 在 WebView 之上叠加原生播放器（UI 被遮住）vs 之下（触摸事件被吃掉）
- 触摸事件、手势（亮度/音量/快进）如何路由

### Q2：字幕兼容性（这是最大的已知坑）

| 字幕 | Media3/ExoPlayer 支持情况 | Mosaic 的处理 |
|---|---|---|
| SRT / VTT | 支持良好 | 直接传 |
| SSA / ASS | 部分支持，**复杂特效会退化** | 服务端转 SRT 或 burn-in |
| PGS（蓝光位图） | **不支持** | 必须 burn-in |

> ⚠️ 111.md §5.4 把"高级影音用户（PGS / ASS / 原盘 / Remux）"列为目标用户之一。
> 这与 Media3 的字幕能力存在直接冲突。**字幕降级策略必须写进已知限制清单，不要假装支持。**

### Q3：系统能力是否稳定

MediaSession、PiP、后台播放、通知栏控制、锁屏控制、蓝牙耳机控制 —— 逐个验证，不要假设。

### Q4：Dolby Vision / HDR

Android 端 DV 支持碎片化严重（设备 + SoC + 厂商三方决定）。记录哪些能播、哪些降级。

## 产出

1. 一个能播本地 MP4 + HLS 的 Android Demo
2. 字幕兼容性实测表（SRT / ASS / PGS 各一条）
3. `docs/adr/0002-mobile-player.md`

## 决策点

| 结论 | 后续 |
|---|---|
| **Go（原生插件）** | P7 按 §55 / §57 实现 `plugins/media-player` 的 Android/iOS 分支 |
| **降级** | 移动端只走 HLS / fMP4（`<video>` 或原生简易播放器），字幕一律服务端烧录；放弃 PGS 原盘直播 |
| **不可行** | 移动端改为 Kotlin/Swift 原生壳承载 Web 内容（即放弃"一套 Tauri 覆盖六端"），需重估 P7 工作量 |

## 环境准备（本周就要开始，不等 Spike 启动）

- Android Studio + SDK 34+ / NDK
- 一台真 Android 手机（模拟器测不了 MediaSession 与 PiP）
- Android TV 真机或 Google TV 盒子（P7 才需要，但现在可以顺便借到）
- Apple 开发者账号（iOS 部分，审批要时间，提前准备）

## 时间盒纪律

同 S1：到期必出结论。i/OS 部分如果账号没下来，**允许只出 Android 结论**，iOS 标注为"P7 前补验"。
