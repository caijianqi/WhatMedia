/**
 * Media3 / ExoPlayer 播放器后端（Android / Android TV）。
 *
 * ## 与其它后端的形态差异
 *
 * `web.ts` 用 `<video>`、`mpv.ts` 用**独立窗口**。这一层是第三种：
 * **原生播放器画在 WebView 之下**，Vue 渲染的控制条浮在画面之上。
 *
 * 这么排的理由：Android WebView 不支持 AC3/DTS/TrueHD（国内下载片源的常态音轨），
 * 且 HLS 支持依赖厂商实现，有的机型黑屏但不报错。原生播放器没有这些问题。
 * 代价是要多维护一条 Kotlin 侧的插件，且图层叠加要真机验证（S1）。
 *
 * ## 这一层做什么
 *
 * 与 `mpv.ts` 完全同构：**只做翻译**，不含逻辑。
 * 事件 → `PlayerCore.patch()` / `emit()`，方法 → `invoke`。
 * 于是可以用一个假 bridge 在 jsdom 里测完，不需要 Android SDK。
 *
 * ## 一个必须说清楚的边界
 *
 * 轨道的 **label 与 id 由 Rust 侧生成**（`crates/media3`），不是本文件。
 * Kotlin 只送原始值过来。所以这里拿到的 `Track` 已经是成品，
 * 不要在这里再改 label —— 那会让 Rust 那边的测试失去意义。
 */

import { PlayerCore } from './core'
import { capabilitiesFor } from './capabilities'
import type {
  LoadOptions,
  MediaPlayer,
  PlayerCapabilities,
  PlayerEvent,
  PlayerErrorCode,
  PlayerSnapshot,
  PlayerState,
  PlayerTarget,
  PlaybackSource,
  Track,
} from './types'

/** 事件通道名。与 Rust `whatmedia_media3::EVENT_CHANNEL` 一致。 */
export const MEDIA3_EVENT_CHANNEL = 'media3://event'

const P = 'plugin:media3|'

/**
 * 与 Tauri 的唯一接触面。
 *
 * 抽成接口的理由同 `mpv.ts`：测试塞 fake，不必有 Android 运行时。
 */
export interface Media3Bridge {
  invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>
  listen<T = unknown>(event: string, handler: (payload: T) => void): Promise<() => void>
}

/** Rust `PlayerEvent` 的线格式。字段名与 serde 输出一致（kebab-case）。 */
export type Media3WireEvent =
  | { type: 'state-change'; from: string; to: string }
  | { type: 'time-update'; position: number; duration: number }
  | { type: 'ended' }
  | { type: 'error'; error: { code: string; message: string } }
  | { type: 'tracks-listed' }
  | { type: 'waiting' }
  | { type: 'playing' }
  | { type: 'paused'; by_user: boolean }
  | { type: 'pip-changed'; active: boolean }
  | { type: 'remote-key'; key: string }
  | { type: 'network-changed'; online: boolean; kind: string }

/** Kotlin 原始轨道 → 本层要的 Track（由 Rust 生成，这里是成品）。 */
interface Media3SnapshotPayload {
  state: string
  position: number
  duration: number
  volume: number
  muted: boolean
  rate: number
  buffered: number
  audioTracks: Track[]
  qualities: Track[]
  currentAudioTrackId: string | null
  currentQualityId: string | null
  error: { code: string; message: string } | null
}

/**
 * 是否跑在 Tauri 壳里。
 *
 * 与 `mpv.ts` 的判据一致：看 Tauri 注入的内部对象，不看 UA。
 * UA 里带 "Tauri" 的第三方内嵌页会误判。
 */
export function tauriAvailable(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/**
 * 造一个真实的 Tauri bridge。
 *
 * 动态 import：`@tauri-apps/api` 在纯 Web 构建里没有意义，静态 import 会打进首屏。
 */
export async function tauriBridge(): Promise<Media3Bridge | null> {
  if (!tauriAvailable()) return null
  try {
    const [{ invoke }, { listen }] = await Promise.all([
      import('@tauri-apps/api/core'),
      import('@tauri-apps/api/event'),
    ])
    return {
      invoke: (cmd, args) => invoke(cmd, args),
      listen: async (event, handler) => {
        const unlisten = await listen<unknown>(event, (e) => handler(e.payload as never))
        return () => unlisten()
      },
    }
  } catch {
    return null
  }
}

/**
 * 探测原生插件是否真的可用。
 *
 * **不能只看"是不是 Android"**：插件没注册、权限没开、Kotlin 侧初始化失败，
 * 都会让命令调用抛异常。任何一种情况下若按 media3 上报能力，
 * 服务端就会给出实际播放器放不了的流。
 *
 * 所以这里真的去调一次命令。用 `media3_status`（最轻的一条），
 * 且**必须在超时内返回** —— 探测卡住会拖慢首屏。
 */
export async function media3Available(bridge?: Media3Bridge): Promise<boolean> {
  const b = bridge ?? (await tauriBridge())
  if (!b) return false
  try {
    await b.invoke(`${P}media3_status`)
    return true
  } catch {
    return false
  }
}

export interface Media3PlayerOptions {
  /** 注入 bridge。测试用 fake；生产环境留空则走 `tauriBridge()`。 */
  bridge?: Media3Bridge
  capabilities?: PlayerCapabilities
}

/** 线格式状态 → 前端状态。认不出来返回 undefined，**不猜**。 */
function toState(s: string): PlayerState | undefined {
  const known = ['idle', 'loading', 'ready', 'playing', 'paused', 'buffering', 'ended', 'error']
  return known.includes(s) ? (s as PlayerState) : undefined
}

function toErrorCode(c: string): PlayerErrorCode {
  const known = ['network', 'decode', 'not-found', 'autoplay-blocked']
  return known.includes(c) ? (c as PlayerErrorCode) : 'unknown'
}

export class Media3Player implements MediaPlayer {
  readonly capabilities: PlayerCapabilities

  private readonly core: PlayerCore
  private bridge: Media3Bridge | null
  private unlisten: (() => void) | null = null
  private disposed = false
  /** 载入参数。重连（网络恢复）要用同一份参数重放。 */
  private lastSource: PlaybackSource | null = null
  private lastOptions: LoadOptions = {}
  /** 最后一次已知位置。网络恢复后从这里续播。 */
  private resumeAt = 0

  constructor(options: Media3PlayerOptions = {}) {
    this.capabilities = options.capabilities ?? capabilitiesFor('media3')
    this.core = new PlayerCore(this.capabilities)
    this.bridge = options.bridge ?? null
  }

  get state(): PlayerSnapshot {
    return this.core.state
  }

  subscribe(listener: (s: PlayerSnapshot) => void): () => void {
    return this.core.subscribe(listener)
  }

  on(listener: (e: PlayerEvent) => void): () => void {
    return this.core.on(listener)
  }

  async initialize(_target: PlayerTarget): Promise<void> {
    // 画面由原生层绘制，**不在 WebView 里挂任何元素**。
    // 这是与 web.ts 最大的差别：container 参数在这里没有意义。
    //
    // 但订阅必须建立：事件流是 Kotlin 主动推的。
    if (this.unlisten) return
    const bridge = await this.ensureBridge()
    if (!bridge) {
      this.core.fail('unknown', 'Media3 播放器不可用（不在 Android 壳里）')
      return
    }
    this.unlisten = await bridge.listen<Media3WireEvent>(MEDIA3_EVENT_CHANNEL, (ev) => {
      this.handleWireEvent(ev)
    })
  }

  async load(source: PlaybackSource, options: LoadOptions = {}): Promise<void> {
    if (this.disposed) return
    this.lastSource = source
    this.lastOptions = options
    this.resumeAt = options.startAt ?? 0
    this.core.resetForLoad()

    const bridge = await this.ensureBridge()
    if (!bridge) {
      this.core.fail('unknown', 'Media3 播放器不可用')
      return
    }

    try {
      await bridge.invoke(`${P}media3_load`, {
        url: source.kind === 'hls' ? source.url : (source as { url?: string }).url ?? '',
        kind: source.kind === 'hls' ? 'hls' : 'progressive',
        startAt: this.resumeAt,
        autoplay: options.autoplay ?? true,
      })
      this.core.setState('loading')
    } catch (e) {
      this.core.fail('unknown', describe(e, '载入失败'))
    }
  }

  /**
   * 用上次的源与位置重新载入。
   *
   * 给 P6 的"网络恢复可续播"用。**不重置 `lastOptions`** ——
   * 重连不是新播放，不该把用户选的音轨/字幕冲掉。
   */
  async reloadFromLastPosition(): Promise<void> {
    if (!this.lastSource) return
    const at = Math.max(0, this.resumeAt)
    await this.load(this.lastSource, { ...this.lastOptions, startAt: at, autoplay: true })
  }

  async stop(): Promise<void> {
    this.callVoid('media3_stop')
    this.core.setState('idle')
  }

  async destroy(): Promise<void> {
    if (this.disposed) return
    this.callVoid('media3_destroy')
    this.unlisten?.()
    this.unlisten = null
    this.disposed = true
    this.core.destroy()
  }

  async play(): Promise<void> {
    this.callVoid('media3_play')
  }

  async pause(): Promise<void> {
    this.callVoid('media3_pause')
  }

  async seek(seconds: number): Promise<void> {
    const target = Number.isFinite(seconds) ? Math.max(0, seconds) : 0
    this.resumeAt = target
    this.core.patch({ position: target })
    this.callVoid('media3_seek', { position: target })
  }

  async setVolume(v: number): Promise<void> {
    const vol = Math.min(1, Math.max(0, Number.isFinite(v) ? v : 1))
    this.core.patch({ volume: vol, muted: vol === 0 })
    this.callVoid('media3_set_volume', { volume: vol })
  }

  async setMuted(muted: boolean): Promise<void> {
    this.core.patch({ muted })
    this.callVoid('media3_set_muted', { muted })
  }

  async setRate(rate: number): Promise<void> {
    const r = Number.isFinite(rate) ? Math.min(4, Math.max(0.25, rate)) : 1
    this.core.patch({ rate: r })
    this.callVoid('media3_set_rate', { rate: r })
  }

  async selectAudioTrack(id: string): Promise<void> {
    this.core.patch({ currentAudioTrackId: id })
    this.callVoid('media3_select_audio', { id })
  }

  async selectQuality(id: string): Promise<void> {
    this.core.patch({ currentQualityId: id })
    this.callVoid('media3_select_quality', { id })
  }

  /**
   * 全屏。
   *
   * Android 上没有"全屏"这个 DOM 概念（页面本来就是全屏的），
   * 所以这里静默成功 —— 报一个用户处理不了的错只会污染日志。
   */
  async setFullscreen(_on: boolean): Promise<void> {}

  /** 画中画。 */
  async enterPip(): Promise<void> {
    this.callVoid('media3_set_pip', { enabled: true })
  }

  /** 通知栏元数据。锁屏后唯一能看出"在播什么"的地方。 */
  async setMetadata(meta: {
    title: string
    subtitle?: string
    artworkUrl?: string
    durationMs?: number
  }): Promise<void> {
    this.callVoid('media3_set_metadata', {
      title: meta.title,
      subtitle: meta.subtitle ?? '',
      artworkUrl: meta.artworkUrl ?? null,
      durationMs: meta.durationMs ?? 0,
    })
  }

  // ── 内部 ────────────────────────────────────────────────

  private async ensureBridge(): Promise<Media3Bridge | null> {
    if (this.bridge) return this.bridge
    const built = await tauriBridge()
    if (built) this.bridge = built
    return this.bridge
  }

  /** 命令调用。失败**不抛**，转成错误事件 —— 见 types.ts 的约定。 */
  private callVoid(cmd: string, args: Record<string, unknown> = {}): void {
    const bridge = this.bridge
    if (!bridge) return
    void bridge.invoke(`${P}${cmd}`, args).catch((e) => {
      this.core.fail('unknown', describe(e, cmd))
    })
  }

  private handleWireEvent(ev: Media3WireEvent): void {
    if (this.disposed || !ev) return
    switch (ev.type) {
      case 'state-change': {
        const to = toState(ev.to)
        if (!to) return
        this.core.setState(to)
        if (to === 'buffering') this.core.emit({ type: 'waiting' })
        break
      }
      case 'time-update':
        this.resumeAt = ev.position
        this.core.patch({ position: ev.position, duration: ev.duration })
        this.core.emit({ type: 'timeupdate', position: ev.position, duration: ev.duration })
        break
      case 'playing':
        this.core.setState('playing')
        this.core.emit({ type: 'playing' })
        break
      case 'paused':
        this.core.setState('paused')
        this.core.emit({ type: 'paused', byUser: ev.by_user })
        break
      case 'waiting':
        this.core.setState('buffering')
        this.core.emit({ type: 'waiting' })
        break
      case 'ended':
        this.core.setState('ended')
        this.core.emit({ type: 'ended' })
        break
      case 'error':
        this.core.fail(toErrorCode(ev.error?.code ?? ''), ev.error?.message ?? '播放失败')
        break
      case 'tracks-listed':
        void this.refreshTracks()
        this.core.emit({ type: 'trackslisted' })
        break
      case 'pip-changed':
        // PiP 变化不上报给上层 UI 之外的东西，但记下来 ——
        // PiP 里前端要隐藏控制条，否则小窗全是控件。
        this.core.emit({ type: 'statechange', from: this.core.state.state, to: this.core.state.state })
        break
      case 'remote-key':
        // 遥控器按键由 TV 焦点系统订阅（见 lib/tv.ts），这里不处理。
        break
      case 'network-changed':
        // 由 reconnect.ts 订阅。
        break
      default:
        break
    }
  }

  /**
   * 轨道列表变化后重新拉一次快照。
   *
   * **两趟调用是故意的**：
   *   1. `media3_status` 问 Kotlin 要原值（Media3 的 `Tracks`，没有 label）
   *   2. `media3_snapshot` 让 Rust 把原值变成带 label 的 `Track`
   *
   * 合成一趟也做得到，但那要把"标签怎么取"挪进 Kotlin，
   * 而 Kotlin 侧在当前开发环境里跑不了任何测试。宁可多一趟 IPC。
   */
  private refreshTracks(): void {
    const bridge = this.bridge
    if (!bridge) return
    void (async () => {
      try {
        const raw = await bridge.invoke<{ state?: string; tracks?: string }>(`${P}media3_status`)
        if (!raw) return
        const tracks = typeof raw.tracks === 'string' ? JSON.parse(raw.tracks) : raw.tracks
        const snap = await bridge.invoke<Media3SnapshotPayload>(`${P}media3_snapshot`, {
          state: raw.state ?? this.core.state.state,
          raw: tracks ?? {},
        })
        if (!snap) return
        this.core.patch({
          audioTracks: snap.audioTracks ?? [],
          qualities: snap.qualities ?? [],
          currentAudioTrackId: snap.currentAudioTrackId ?? null,
          currentQualityId: snap.currentQualityId ?? null,
        })
      } catch (e) {
        // 拉快照失败不该把播放器打进错误态 —— 轨道菜单空着而已，
        // 画面和声音还正常。报个终态错误会让用户以为片子挂了。
        console.warn('[whatmedia] 拉取 media3 轨道列表失败', e)
      }
    })()
  }
}

function describe(e: unknown, prefix: string): string {
  const msg = e instanceof Error ? e.message : typeof e === 'string' ? e : String(e)
  return `${prefix}：${msg || '未知错误'}`
}
