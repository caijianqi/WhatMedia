# ADR 0001 · 播放决策与字幕降级策略

- 状态：已采纳（P3 实现）
- 日期：2026-10-09
- 影响范围：`whatmedia-playback` / `whatmedia-subtitle` / `whatmedia-transcoder` / `whatmedia-api`

---

## 1. 背景

"能不能播"取决于四组事实的组合：客户端能解什么、片源是什么、服务端有什么、
网络有多宽。这四者任意一项变化，结论都可能变。如果把判断散落在 HTTP 处理器里，
很快就会出现"同一个文件在 Web 上能播、在移动端不能播"却查不出原因的情况。

字幕又多一层复杂度：客户端支持的字幕格式差异极大（浏览器只认 VTT，
mpv 连 PGS 位图字幕都能自己渲染），而服务端只有"转换 / 烧录 / 放弃"三种补救手段，
其中烧录要重编码视频，代价高一个数量级。

## 2. 决定

### 2.1 决策是纯函数

`whatmedia_playback::decide(DecisionInput) -> PlaybackDecision` 不碰 IO、不写库、
不启动进程。所有输入（客户端能力、片源画像、服务端能力、带宽）由调用方先探测好再传入。

理由：决策逻辑是整个播放链路里最需要被测试覆盖的部分，
一旦里面混进 IO，就只能靠真机验证，而真机永远凑不齐所有设备组合。

### 2.2 降级链单向，且必须留下原因

```text
DirectPlay → Remux → Transcode → Unsupported
```

只能往前不能往后。每次降级都要往 `PlaybackDecision.reasons` 里追加一条 `Reason`
（含机器可读的 `code` 与面向用户的 `message`）。

理由：用户问"为什么我的 4K 原盘在转码"时必须有据可查。
一个不能解释自己的决策等于黑盒，排障时只能靠猜。

`Remux` 允许只转音频（`VideoAction::Copy` + `AudioAction::Encode`）。
MKV + H264 + DTS 在浏览器上就是这条路径 —— 视频完全没问题，没必要重编码。

### 2.3 字幕降级链：Direct → Convert → BurnIn → Drop

| 模式 | 含义 | 代价 |
|---|---|---|
| `Direct` | 原样交给客户端 | 0 |
| `Convert` | 服务端转 SRT / VTT（文本互转） | 极低 |
| `BurnIn` | 服务端把字幕烧进画面 | **必须重编码视频** |
| `Drop` | 放弃这条字幕，播放继续 | 0 |

两条硬性规则：

1. **位图字幕（PGS / DVD SUB / DVB SUB）不做 OCR。**
   文档 §51 没承诺 OCR，做 OCR 要引入一整个重度依赖，收益却只在极少数片源上。
   位图字幕只有"客户端自己渲染 / 服务端烧录 / 丢弃"三条路。
2. **字幕问题永远不能让播放起不来。**
   任何降级失败都落到 `Drop`，绝不因为字幕把整条播放路径判成 `Unsupported`。

> 实现注记：早期版本在客户端已支持位图字幕时仍走 `BurnIn`，
> 导致桌面端放蓝光原盘也在转码。是兼容性矩阵测试（`crates/playback/tests/compat_matrix.rs`）
> 把这个问题逼出来的 —— 这也是坚持写矩阵测试而不是写单元测试的理由。

### 2.4 覆盖率指标按"是否重编码"衡量，而不是按纯 DirectPlay

浏览器不认 MKV，Web 端的 Direct Play 覆盖率天然不可能高。
把它拿来和桌面端平均，会得到一个无论如何都达不到的数字，那样的"未达标"没有指导意义。

因此：

- 桌面客户端：断言 **Direct Play 加权覆盖率 ≥ 90%**
- Web 客户端：断言 **无需重编码覆盖率 ≥ 55%**，且 `Unsupported` 必须为 0
- 综合：断言 **无需重编码覆盖率 ≥ 70%**

语料带频率权重（1080p H264 权重 20，4K DV 原盘权重 2），
按权重而不是按条数统计。语料外置为 `test-media/compat/cases.jsonl`，
由 `crates/playback/tests/compat_matrix.rs` 消费，可经 `make compat` 打出完整表格。

> **语料守卫**：这几个枚举都带 `#[serde(untagged)] Other(String)` 兜底，
> 写错的 codec 名不会报错，而是静默变成"没人认识的编码"，覆盖率数字会悄悄变化。
> 所以加载时显式断言 codec 没有被识别成 `Other`。（`truehd` 要写成 `true_hd`，坑就是这么踩出来的。）

### 2.5 HTTP 层的失败语义

| 情况 | 响应 | 理由 |
|---|---|---|
| 需要 remux/转码但服务端没有 ffmpeg | **501** + `FFMPEG_UNAVAILABLE` | 前端据此提示"请安装 ffmpeg"。用 500 只会弹一个毫无意义的网络错误 |
| Range 越界 / 多段 / 畸形 | **416** + `Content-Range: bytes */{total}` | 播放器靠它判断文件长度，返回 200 全量会让 Seek 前先下载完整个文件 |
| 未接数据库 | **503** + `DB_UNAVAILABLE` | 与 501 区分开：一个是"功能没做"，一个是"后端没准备好" |
| HLS 分片还没生成 | **404** + `NOT_READY` | hls.js 会重试，这是协议内的行为，不是错误 |

### 2.6 流式输出目录必须落库

HLS 分片目录同时登记到内存（`StreamRegistry`）与数据库（`transcode_session` 表）。

理由：进程被 `kill -9` 或容器被强制重启时，正在写的分片目录不会有人清理，
放任不管 `/cache` 会一路涨到磁盘满。只有落库，重启后才能按登记表回收
（`reap_orphan_streams`）。判据是"数据库里登记的目录"，不扫描整个 cache 目录 ——
后者会把别的实例正在用的目录也删掉（多容器共挂一个卷时尤其危险）。

## 3. 后果

### 正面

- 决策逻辑 100% 可离线测试，P3 的兼容性矩阵不必依赖真机。
- 每个降级都能向用户解释，排障成本大幅下降。
- 字幕降级行为被显式钉住，不会出现"字幕导致全片转码"的隐性退化。

### 负面 / 代价

- `MediaCapability` 需要完整的流信息。没有 ffprobe 的库，
  容器与编码未知，决策会保守地落到 Remux / Transcode。
- 位图字幕在 Web 上必然触发重编码。这是接受了的代价（不做 OCR）。
- 客户端能力目前靠 `client_id` 查预置档，P4 要做成真实上报 + 探测。
- 流式输出多一次数据库写入。换来的是崩溃重启后不留僵尸分片，值得。

## 4. 验证方式

- **决策层**：`crates/playback/tests/compat_matrix.rs`（数据驱动，语料外置）
- **HTTP 层**：`crates/api/tests/playback_smoke.rs`（21 项，真实 HTTP 往返 + 真实 SQLite + 真实文件系统）
- **回放**：`make compat` 打出每个客户端 × 每条语料的决策表与加权覆盖率
