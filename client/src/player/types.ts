/**
 * 播放器抽象层契约。
 *
 * **为什么要有这一层**：Web 用 `<video>` + hls.js，桌面端用 mpv/libmpv。
 * 两者能力差异巨大 —— mpv 能自己渲染 PGS 位图字幕、能直接吃 MKV，浏览器一样都不行。
 * 上层页面若直接依赖 `<video>`，桌面端就要整套重写。
 * 所以上层只认识这个接口，并且**必须能查询后端能力**（`capabilities`），
 * 不能假设"所有后端都一样"。
 *
 * **与 111.md §53 的差异（有意为之）**：§53 写的是 `get_position()` / `get_duration()`
 * 这类轮询式 getter。在 Vue 里轮询是反模式 —— 位置每 250ms 变一次，
 * 要么用 `setInterval` 把整个组件树吵醒，要么干脆不更新。
 * 改成 `state` 快照 + `subscribe()` 订阅：底层有事件（timeupdate / mpv 的 property change）
 * 就推一次，UI 只在真的变了的时候重渲染。
 */

// ─────────────────────────────────────────────────────────────
// 单位约定（重要）
// ─────────────────────────────────────────────────────────────
//
// 播放器内部一律用**秒**（浮点），与 HTMLMediaElement 一致 —— 少一个转换点就少一处 bug。
// 与后端通信（heartbeat 的 position_ms）时才转毫秒，转换只发生在 `api/` 层一处。

/** 播放器生命周期状态。 */
export type PlayerState =
  | 'idle'
  | 'loading'
  /** 已拿到元数据，可以播。 */
  | 'ready'
  | 'playing'
  | 'paused'
  /** 播放中数据不够了。与 `loading` 是两回事 —— 见下方说明。 */
  | 'buffering'
  | 'ended'
  | 'error'

/**
 * 轨道。音轨 / 字幕轨 / 画质档共用。
 */
export interface Track {
  /** 后端给的下标（音轨/字幕轨）或档位 id（画质）。 */
  id: string
  label: string
  language?: string
  /** 是否默认选中。 */
  default?: boolean
  /** 强制显示的字幕（forced）。 */
  forced?: boolean
}

/** 播放源。三种形态对应 P3 的三种策略。 */
export type PlaybackSource =
  /** Direct Play / Remux：一个可以直接喂给播放器的 URL。 */
  | { kind: 'progressive'; url: string }
  /** Transcode：HLS master playlist。 */
  | { kind: 'hls'; url: string }
  /** 桌面端直读本地文件（走 Tauri 的 file-access，不经过 HTTP）。 */
  | { kind: 'local-file'; path: string }

/**
 * 后端能力声明。
 *
 * **这份声明必须和服务端 `ClientCapability` 的预置档保持一致** ——
 * 服务端拿它决定"要不要转码"，前端拿它决定"要不要显示某个按钮"。
 * 两边各写一份是必然会漂移的，所以 `clientId` 必须显式声明，
 * 让"前端声称是 web"和"服务端按 web 决策"成为同一件事。
 *
 * 一致性由 `crates/playback/tests/client_capability_parity.rs` 钉住。
 */
/**
 * 客户端 id。取值必须与服务端 `ClientCapability::by_id()` 认得的字面量一致
 * （`crates/playback/src/capability.rs`），否则服务端会按未知客户端回落到 web 档。
 */
export type PlayerClientId = 'web' | 'tauri-mpv' | 'android-exoplayer' | 'apple' | 'minimal'

export interface PlayerCapabilities {
  clientId: PlayerClientId
  /** 能直接播放的容器。 */
  containers: string[]
  videoCodecs: string[]
  audioCodecs: string[]
  /** **能自己渲染**的字幕格式。不在这个列表里的，服务端必须烧录或转换。 */
  subtitleCodecs: string[]
  /** 能否自己渲染 PGS / DVD SUB 等位图字幕。Web 永远 false。 */
  bitmapSubtitles: boolean
  maxBitrateKbps?: number
  /** 能否播 HLS。 */
  hls: boolean
  /** 能否直读本地文件路径（Web 永远不能）。 */
  localFile: boolean
  /**
   * 播放中能否切换音轨。
   *
   * **Web 端是 false，这是个硬约束不是实现偷懒**：`HTMLMediaElement.audioTracks`
   * 在绝大多数浏览器里要么不存在、要么只读，且已被移出标准；
   * 换成"再拉一次流"又会导致从头开始。所以 Web 端要换音轨只能让**服务端重新决策**，
   * 前端不该给一个点了没反应的按钮。
   */
  canSwitchAudioTrack: boolean
  /** 播放中能否切换字幕轨。Web 端仅对 `<track>` 注入的外挂字幕成立。 */
  canSwitchSubtitleTrack: boolean
}

/**
 * 播放器对外暴露的完整状态快照。
 *
 * 不可变：每次变更都是新对象，这样 Vue 的响应式系统和 `subscribe` 的比较
 * 都能用引用判等，不需要深比较。
 */
export interface PlayerSnapshot {
  state: PlayerState
  /** 当前位置（秒）。 */
  position: number
  /** 总时长（秒）。未知时为 0。 */
  duration: number
  volume: number
  muted: boolean
  rate: number
  /** 已缓冲到的位置（秒）。用于画缓冲条。 */
  buffered: number
  audioTracks: Track[]
  subtitleTracks: Track[]
  /** 画质档。HLS 多码率才有，单码率为空数组 —— UI 据此隐藏"画质"按钮。 */
  qualities: Track[]
  currentAudioTrackId: string | null
  /** null 表示字幕关闭。 */
  currentSubtitleTrackId: string | null
  currentQualityId: string | null
  /** 可恢复的错误信息。`state === 'error'` 时有意义。 */
  error: PlayerError | null
}

export interface PlayerError {
  /** 机器可读的错误码。前端据此决定"重试"还是"换策略重新决策"。 */
  code: PlayerErrorCode
  message: string
}

export type PlayerErrorCode =
  /** 网络断开 / 请求失败。 */
  | 'network'
  /** 解码失败（容器或编码后端不支持）。 */
  | 'decode'
  /** 源不存在（404）。 */
  | 'not-found'
  /** 浏览器自动播放被拦截（没有用户手势）。 */
  | 'autoplay-blocked'
  | 'unknown'

/**
 * 播放器发出的瞬时事件。
 *
 * 与 `state` 的区别：`state` 是"当前是什么样"，`event` 是"刚刚发生了什么"。
 * 例如"缓冲结束恢复播放"会发 `playing` 事件，但 state 只是从 buffering 回到 playing
 * —— 只靠 state 无法区分"首次开始播"和"卡顿后恢复"，而这两种情况用户需要看到的提示不同。
 */
export type PlayerEvent =
  | { type: 'statechange'; from: PlayerState; to: PlayerState }
  /**
   * 播放进度。与 `state.position` 内容相同，但只在**值得上报**的时刻触发
   * （见 `ProgressPolicy`），避免每 250ms 吵醒整个组件树。
   */
  | { type: 'timeupdate'; position: number; duration: number }
  | { type: 'ended' }
  | { type: 'error'; error: PlayerError }
  /** 轨道列表加载完成（元数据到手）。 */
  | { type: 'trackslisted' }
  /** 缓冲停滞。 */
  | { type: 'waiting' }
  | { type: 'playing' }
  /** 播放被暂停。 */
  | { type: 'paused'; byUser: boolean }

/**
 * 进度上报策略。
 *
 * **为什么需要它**：DoD 要求"播放进度跨端同步误差 < 5s"。
 * 天真做法是每秒上报一次，一部 2 小时的片会打 7200 次请求 —— 服务端不需要这么细。
 * 反过来，只在暂停和退出时上报，断电/崩溃就会丢掉最多一整段的进度。
 * 所以：常规节流 + 关键节点（暂停/停止/页面隐藏/跳转）立即上报。
 */
export interface ProgressPolicy {
  /** 常规上报间隔（秒）。默认 10。 */
  intervalSeconds: number
  /** seek 后是否立即上报。默认 true —— seek 是最容易丢失的大跨度变更。 */
  reportOnSeek: boolean
  /** 暂停/停止时是否立即上报。默认 true。 */
  reportOnPause: boolean
}

export const DEFAULT_PROGRESS_POLICY: ProgressPolicy = {
  intervalSeconds: 10,
  reportOnSeek: true,
  reportOnPause: true,
}

/** 挂载目标。 */
export interface PlayerTarget {
  /** 要挂载播放器画面的 DOM 容器。 */
  container: HTMLElement
  /** 全屏时要全屏的元素。不传则用 `container`。 */
  fullscreenTarget?: HTMLElement
}

export interface LoadOptions {
  /** 起始位置（秒）。用于续播。 */
  startAt?: number
  /**
   * 是否自动播放。**注意浏览器自动播放策略**：没有用户手势时会被拦截，
   * 此时后端应当报 `autoplay-blocked` 而不是静默失败。
   */
  autoplay?: boolean
  /** 外挂字幕。Web 端通过 `<track>` 注入；mpv 端直接 `sub-add`。 */
  externalSubtitles?: ExternalSubtitle[]
}

/**
 * 外挂字幕源。对应 P3 `/api/v1/subtitles/external` 的输出。
 *
 * Web 与桌面端取值不同：Web 只能拿服务端转换后的 URL（`<track>` 要同源或 CORS），
 * 桌面端可以直接读文件路径。两个字段都可选，由后端挑自己能用的那个。
 */
export interface ExternalSubtitle {
  /** 服务端转换后的 URL（Web 用）。 */
  url?: string
  /** 本地文件路径（桌面端用）。 */
  path?: string
  language?: string
  label: string
  /** 是否默认显示。 */
  default?: boolean
}

/**
 * 播放器实例。所有平台后端（Web / mpv / 将来的 ExoPlayer）都实现它。
 *
 * 三条约定：
 *   1. 所有方法返回 Promise，即使底层是同步的 —— 桌面端走 IPC，一定是异步的。
 *      现在就统一成异步，将来换后端不用改调用方签名。
 *   2. 方法**不抛异常**。失败时把状态置为 `error` 并发出 `error` 事件 ——
 *      播放器是长生命周期对象，抛异常会让调用方漏掉清理。
 *   3. `state` 是只读快照；要监听变化用 `subscribe`。
 */
export interface MediaPlayer {
  /** 后端能力。创建后即可读，不必等 `initialize`。 */
  readonly capabilities: PlayerCapabilities

  /**
   * 一次性准备（Web 上是挂 DOM 元素，mpv 上是创建实例）。
   * 幂等：重复调用无副作用。
   */
  initialize(target: PlayerTarget): Promise<void>

  /** 载入一个播放源。会重置所有轨道与位置。 */
  load(source: PlaybackSource, options?: LoadOptions): Promise<void>

  /** 卸载并释放资源。调用后可以再次 `load`。 */
  stop(): Promise<void>

  /** 彻底销毁。销毁后实例不可再用。 */
  destroy(): Promise<void>

  play(): Promise<void>
  pause(): Promise<void>

  /** 跳转到指定位置（秒）。 */
  seek(seconds: number): Promise<void>

  /** 音量 0..1。 */
  setVolume(v: number): Promise<void>
  setMuted(muted: boolean): Promise<void>
  /** 播放速率，1 为正常。 */
  setRate(rate: number): Promise<void>

  selectAudioTrack(id: string): Promise<void>
  /** 传 null 表示关闭字幕。 */
  selectSubtitleTrack(id: string | null): Promise<void>
  selectQuality(id: string): Promise<void>

  /**
   * 请求 / 退出全屏。
   *
   * 不是所有后端都支持。不支持时应当静默成功（UI 元素本来就不会出现），
   * 而不是报错 —— 报一个用户无法处理的错只会污染日志。
   */
  setFullscreen(on: boolean): Promise<void>

  /** 当前状态快照。 */
  readonly state: PlayerSnapshot

  /** 订阅状态变更。返回取消订阅函数。 */
  subscribe(listener: (snapshot: PlayerSnapshot) => void): () => void

  /** 订阅事件。返回取消订阅函数。 */
  on(listener: (event: PlayerEvent) => void): () => void
}

/** 播放器工厂。 */
export type PlayerFactory = () => MediaPlayer
