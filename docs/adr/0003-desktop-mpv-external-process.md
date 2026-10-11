# ADR-0003：桌面播放器走外部 mpv 进程，不做嵌入式 libmpv

**状态**：已接受
**日期**：2026-10-11
**相关**：ADR-0002（播放器抽象层）、111.md §53、DEVELOPMENT-PLAN.md §S1

---

## 背景

P4 要给桌面端一个能放 MKV / ASS / PGS / 多音轨的播放器。可选形态有三条：

1. **静态链接 libmpv**，画面渲染到 Tauri 窗口里；
2. **libmpv 作为独立 Surface 层**嵌在 WebView 之下；
3. **外部 mpv 进程 + JSON IPC**，画面在 mpv 自己的窗口里。

原计划（DEVELOPMENT-PLAN.md P4）写的是"Tauri Plugin（mpv/libmpv）"，没区分这三条。这里把选择记下来。

---

## 决定

**选 3：外部进程 + JSON IPC。** 上层仍是 ADR-0002 定义的那套 `MediaPlayer` 接口，
页面不知道画面在哪渲染。

---

## 理由

### 为什么不做静态链接（1）

- **双架构打包成本**。链 libmpv 意味着 x86_64 与 ARM64 各要编一份 mpv 及其全部依赖
  （ffmpeg、placebo、lua…），还要和 Tauri 的链接器设置对齐。P5 的 R5S 交付会再撞一次。
- **失败模式**。静态链接时 mpv 出问题就是整个桌面端起不来；外部进程模式下，
  mpv 缺失只是**降级**到内嵌 Web 播放器，用户还能看。

### 为什么不做嵌入式（2）

三个平台各要写一套窗口句柄桥接（Windows 的 HWND / macOS 的 NSView / Linux 的 X11+Wayland），
且 Windows 上要和 WebView2 的合成器抢 Z 序 —— 这一条没有可离线验证的路径，
只能上真机调，而我们的排期里没有给"桌面端 UI 合成"留单独的 Spike。

注意：**S1（DEVELOPMENT-PLAN.md）问的是移动端 Media3 能否嵌入 Tauri WebView**，
是 Android 上的另一个问题。桌面端 mpv 后端**不依赖 S1 的结论** —— 外部进程形态绕开了
"嵌入"这个难题，所以它可以先落地。

### 外部进程的代价（明确接受）

- 画面不在页面里：控制条仍然画在 Web 这边（本来就是这样设计的），
  容器内渲染一句"正在用 mpv 播放（独立窗口）"的占位说明，而不是留一块空白。
- 要自己看住进程：退出前 `quit` + kill，Unix socket 文件要兜底清理。
- 控制延迟比进程内调用高一点（一次 IPC 往返）。对"点暂停""拖进度条"这类操作无感。

---

## 协议细节（踩到的两个坑）

mpv 的 JSON IPC 有两个地方容易写错，且**写错不会报错，只会静默不工作**：

1. **`request_id` 是命令对象的顶层字段**，不是 command 数组的元素：
   `{"command":["get_property","track-list"],"request_id":7}`。
   塞进数组里会被当成参数，响应里永远带不回同一个 id，
   于是"取轨道列表"这类请求永远匹配不上。（已由 `ipc::tests` 钉住。）
2. **`observe_property` 逐条发**，不用"一次观察多个属性"的批量形式。
   批量形式是后加的、各版本支持程度不一致，逐条在 ≥0.34 上都成立。

另外 `end-file` 的原因在 `reason` 字段而不是 `data`，
所以事件结构里单独留了 `reason` —— 否则"播完了"和"解不动了"在上层看起来一模一样。

---

## 降级路径

- 探测不到 mpv（PATH 与常见安装位置都没有，或版本 < 0.34）→ `resolveBackend()` 返回 `web`，
  能力按 web 上报，服务端不会给出浏览器放不出来的 MKV 直出地址。
- 桌面壳里 mpv 可用 → 后端为 `mpv`，能力按 `tauri-mpv` 上报，建会话时也用同一个 client id。
  **两处必须用同一个判断**：服务端按 mpv 给了 MKV 而播放器其实是 Web，
  会变成"明明是本地文件却播不了"，且服务端日志里一切正常。

---

## 后果

- 桌面端的"点击画面暂停"要在控制条上做（画面不在 DOM 里）。
- 全屏的是 mpv 自己的窗口，不是页面元素 —— `setFullscreen` 走 `set_property fullscreen`。
- 图标：tauri-build 在 Windows 上必须有 `icons/icon.ico`（`bundle.icon` 为空也一样），
  仓库里用 `scripts/gen-icons.py` 生成占位图标，真实品牌图标直接覆盖同名文件即可。
