/**
 * mpv 播放器后端：**外部 mpv 进程 + JSON IPC**。
 *
 * ## 一个必须先说清楚的形态差异
 *
 * mpv 跑在**自己的窗口**里，不是嵌在 WebView 画面里。
 * 这是明确的选择，不是"还没做完"：
 *   · 嵌入式（把 mpv 的 surface 塞到 WebView 画面之下）要在三个平台上各写一套
 *     窗口句柄桥接，且在 Windows 上和 WebView2 的合成器抢得很难看；
 *   · 外部窗口模式下，mpv 缺失 / 崩溃都只是**退化**（回 Web 播放器），
 *     嵌入式时它出问题就是整个客户端起不来。
 *
 * 代价是控制条必须自己画在 Web 这边（本来就是这样设计的），
 * 且"点击画面暂停"这类交互要在控制条上做。这是可接受的成本。
 *
 * ## 这一层做什么
 *
 * 与 `web.ts` 一样：**只做翻译**。把 mpv 的 IPC 事件翻译成
 * `PlayerCore.patch()` / `emit()`，把 `MediaPlayer` 的方法翻译成 IPC 命令。
 * "状态怎么变"的判断全在 `core.ts`，所以这里几乎不含逻辑，
 * 也就能用一个假 bridge 在 jsdom 里测完 —— 不需要真的装 mpv。
 */

import { PlayerCore } from './core'
import { capabilitiesFor } from './capabilities'
import type {
  ExternalSubtitle,
  LoadOptions,
  MediaPlayer,
  PlayerCapabilities,
  PlayerEvent,
  PlayerSnapshot,
  PlayerTarget,
  PlaybackSource,
  Track,
} from './types'

/** Rust 侧 `MpvEvent` 的序列化形状。字段名与 Rust 保持一致（snake_case）。 */
export interface MpvEventPayload {
  name: string
  property?: string | null
  data?: unknown
  request_id?: number | null
  error?: string | null
  /** `end-file` 的 reason：`eof` / `error` / `quit` / `stop`。 */
  reason?: string | null
}

/** `mpv_status` 的返回。 */
export interface MpvStatus {
  available: boolean
  path?: string | null
  version?: string | null
  reason?: string | null
}

/**
 * 与 Tauri 的唯一接触面。
 *
 * 抽成接口有两个理由：
 *   1. 测试里可以塞 fake，不必真的启动 mpv（也不必在 jsdom 里假装 window.__TAURI_INTERNALS__）；
 *   2. 将来换成别的壳（比如插件形式）时改的是这一处。
 */
export interface MpvBridge {
  invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>
  listen<T = unknown>(event: string, handler: (payload: T) => void): Promise<() => void>
}

/** 事件名。拼错不会报错，所以写成常量。 */
export const MPV_EVENT_CHANNEL = 'mpv://event'
export const MPV_CLOSED_CHANNEL = 'mpv://closed'

/**
 * 是否跑在 Tauri 壳里。
 *
 * 用 `__TAURI_INTERNALS__` 而不是 `navigator.userAgent`：
 * UA 里带 "Tauri" 的浏览器插件/内嵌页会误判，而这个内部对象是 Tauri 注入的、造不了假。
 */
export function tauriAvailable(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/**
 * 造一个真实的 Tauri bridge。
 *
 * 动态 import：`@tauri-apps/api` 在纯 Web 构建里没有意义，静态 import 会把它打进首屏。
 */
export async function tauriBridge(): Promise<MpvBridge | null> {
  if (!tauriAvailable()) return null
  try {
    const [{ invoke }, { listen }] = await Promise.all([
      import('@tauri-apps/api/core'),
      import('@tauri-apps/api/event'),
    ])
    return {
      invoke: (cmd, args) => invoke(cmd, args),
      listen: async (event, handler) => {
        // payload 在 Tauri 的类型里是 unknown，这里由调用方声明具体类型。
        const unlisten = await listen<unknown>(event, (e) => handler(e.payload as never))
        return () => unlisten()
      },
    }
  } catch {
    // 没有 Tauri 运行时（比如 dev 模式下用浏览器打开）
    return null
  }
}

/** 探测 mpv。桌面上可用与否决定"要不要显示 mpv 选项"。 */
export async function probeMpv(mpvPath?: string): Promise<MpvStatus | null> {
  const bridge = await tauriBridge()
  if (!bridge) return null
  try {
    return await bridge.invoke<MpvStatus>('mpv_status', { mpv_path: mpvPath ?? null })
  } catch {
    return null
  }
}

export interface MpvPlayerOptions {
  /** 注入 bridge。测试用 fake；生产环境留空则走 `tauriBridge()`。 */
  bridge?: MpvBridge
  capabilities?: PlayerCapabilities
  /** 覆盖 mpv 可执行文件路径。留空走探测。 */
  mpvPath?: string
  /** 硬件解码。默认 `auto-safe` —— 桌面端默认不信任所有解码器。 */
  hwdec?: string
}

/** mpv `track-list` 里的条目。只取用得上的字段。 */
interface MpvTrack {
  id?: number
  type?: string
  lang?: string
  title?: string
  selected?: boolean
  forced?: boolean
  codec?: string
}

export class MpvPlayer implements MediaPlayer {
  readonly capabilities: PlayerCapabilities

  private readonly core: PlayerCore
  private readonly options: MpvPlayerOptions

  private bridge: MpvBridge | null = null
  private container: HTMLElement | null = null
  /** 外部窗口模式下，画面不在页面里 —— 这里放的是一句说明，不是 video 元素。 */
  private placeholder: HTMLElement | null = null

  private unlistenEvent: (() => void) | null = null
  private unlistenClosed: (() => void) | null = null

  private nextRequestId = 1
  /** 等待中的异步请求。目前只有"取轨道列表"用到。 */
  private pendingTrackListId: number | null = null

  private disposed = false
  /** 是否正在播（mpv 进程活着）。 */
  private started = false
  /** 用户要求的暂停状态。mpv 的 pause 属性变化事件会覆盖它，所以要记住意图。 */
  private paused = false

  constructor(options: MpvPlayerOptions = {}) {
    this.options = options
    this.capabilities = options.capabilities ?? capabilitiesFor('mpv')
    this.core = new PlayerCore(this.capabilities)
  }

  get state(): PlayerSnapshot {
    return this.core.state
  }

  subscribe(listener: (s: PlayerSnapshot) => void): () => void {
    return this.core.subscribe(listener)
  }

  on(listener: (event: PlayerEvent) => void): () => void {
    return this.core.on(listener)
  }

  async initialize(target: PlayerTarget): Promise<void> {
    if (this.container) return // 幂等

    this.container = target.container
    this.bridge = this.options.bridge ?? (await tauriBridge())
    if (!this.bridge) {
      // 走到这里说明调用方没先看 `tauriAvailable()`。报出来而不是静默失败：
      // 静默失败的表现是"点播放什么都没发生"。
      this.core.fail('unknown', 'mpv 后端只能在桌面壳里使用')
      return
    }

    this.renderPlaceholder(target.container)

    this.unlistenEvent = await this.bridge.listen<MpvEventPayload>(
      MPV_EVENT_CHANNEL,
      (ev) => this.handleEvent(ev),
    )
    this.unlistenClosed = await this.bridge.listen(MPV_CLOSED_CHANNEL, () => {
      // mpv 窗口被用户关掉。这不是"播完了"，回到 idle 而不是 ended ——
      // 报 ended 会让 UI 弹出"下一集"，而用户只是关了窗口。
      this.started = false
      if (this.core.state.state !== 'idle') this.core.setState('idle', null)
    })
  }

  /**
   * 占位说明。
   *
   * **为什么要占位**：上层页面按 `PlayerTarget.container` 定位播放器。
   * 什么都不放的话，用户看到的是一块空白，会以为播放失败了。
   */
  private renderPlaceholder(container: HTMLElement): void {
    const el = document.createElement('div')
    el.className =
      'flex h-full w-full flex-col items-center justify-center gap-2 bg-black text-center text-sm text-wm-muted'
    const line1 = document.createElement('p')
    line1.textContent = '正在用 mpv 播放（独立窗口）'
    const line2 = document.createElement('p')
    line2.className = 'text-xs text-wm-muted'
    line2.textContent = '画面在 mpv 窗口里，下面的控制条仍然可用'
    el.append(line1, line2)
    container.appendChild(el)
    this.placeholder = el
  }

  async load(source: PlaybackSource, options: LoadOptions = {}): Promise<void> {
    const bridge = this.bridge
    if (!bridge) {
      this.core.fail('unknown', 'mpv 后端未初始化')
      return
    }

    this.core.resetForLoad()
    await this.stopSession()

    const url = source.kind === 'local-file' ? source.path : source.url
    const subs = (options.externalSubtitles ?? [])
      // mpv 要的是文件路径；Web 用的 URL 在这边也能用（mpv 自己会下载），
      // 但优先用 path —— 直读本地文件是桌面端的意义所在。
      .map((s: ExternalSubtitle) => ({ path: s.path ?? s.url ?? '', lang: s.language ?? null }))
      .filter((s) => s.path.length > 0)

    try {
      await bridge.invoke<string>('mpv_start', {
        args: {
          url,
          start_seconds: options.startAt ?? 0,
          title: null,
          subtitles: subs,
          hwdec: this.options.hwdec ?? 'auto-safe',
          mpv_path: this.options.mpvPath ?? null,
        },
      })
      this.started = true
      // autoplay：mpv 启动即播（--keep-open=always 让它播完停住而不是退出）。
      this.paused = !(options.autoplay ?? true)
      this.core.patch({ state: this.paused ? 'ready' : 'loading' })
    } catch (e) {
      this.started = false
      this.core.fail('unknown', describe(e, '启动 mpv 失败'))
    }
  }

  /** 只停进程，不碰状态 —— `load` 里复用，避免重复代码。 */
  private async stopSession(): Promise<void> {
    if (!this.started) return
    try {
      await this.bridge?.invoke('mpv_stop')
    } catch {
      // 进程可能已经没了（用户关了窗口）。忽略。
    }
    this.started = false
  }

  // ── 事件翻译 ──────────────────────────────────────────────

  private handleEvent(ev: MpvEventPayload): void {
    if (this.disposed) return

    // 异步命令的响应。目前只有 track-list 用。
    if (ev.name === 'response') {
      if (this.pendingTrackListId != null && ev.request_id === this.pendingTrackListId) {
        this.pendingTrackListId = null
        if (ev.error && ev.error !== 'success') return // 读不到轨道就保持空列表
        this.applyTrackList(ev.data)
      }
      // 其余响应（set_property 之类）没有订阅者，直接丢。
      return
    }

    switch (ev.name) {
      case 'file-loaded':
        this.core.patch({ state: this.paused ? 'ready' : 'playing' })
        this.requestTrackList()
        break

      case 'property-change':
        this.handlePropertyChange(ev)
        break

      case 'end-file':
        this.handleEnd(ev.reason)
        break

      case 'shutdown':
        this.started = false
        this.core.setState('idle', null)
        break

      case 'playback-restart':
        // seek 完成后 mpv 会发这个。位置以 time-pos 为准，这里只补一个 playing。
        if (!this.paused) this.core.patch({ state: 'playing' })
        break

      default:
        break
    }
  }

  private handleEnd(reason: string | null | undefined): void {
    this.started = false
    switch (reason) {
      case 'eof':
        this.core.patch({ state: 'ended' })
        this.core.emit({ type: 'ended' })
        break
      case 'error':
        // 区分不出是网络还是解码：mpv 的 reason 只给个 error。
        // 报 decode 而不是 unknown —— 用户至少知道是"这个片放不了"，
        // 而不是什么都没发生。
        this.core.fail('decode', 'mpv 无法播放这个文件（格式不支持或文件损坏）')
        break
      default:
        // quit / stop / 换源：不是错误，也不是播完。
        this.core.setState('idle', null)
    }
  }

  private handlePropertyChange(ev: MpvEventPayload): void {
    const name = ev.property
    if (!name) return

    switch (name) {
      case 'time-pos': {
        const pos = num(ev.data)
        if (pos == null) return
        this.core.patch({ position: pos })
        this.core.emit({ type: 'timeupdate', position: pos, duration: this.core.state.duration })
        break
      }
      case 'duration': {
        const d = num(ev.data)
        if (d != null) this.core.patch({ duration: d })
        break
      }
      case 'pause': {
        const paused = ev.data === true
        this.paused = paused
        if (paused) {
          this.core.patch({ state: 'paused' })
        } else if (this.core.state.state !== 'ended') {
          this.core.patch({ state: 'playing' })
          this.core.emit({ type: 'playing' })
        }
        break
      }
      case 'eof-reached':
        // keep-open 下 mpv 不会自己退出，靠这个属性知道播完了。
        if (ev.data === true) this.handleEnd('eof')
        break
      default:
        break
    }
  }

  private requestTrackList(): void {
    const id = this.nextRequestId++
    this.pendingTrackListId = id
    void this.bridge
      ?.invoke('mpv_send', { command: ['get_property', 'track-list'], request_id: id })
      .catch(() => {
        this.pendingTrackListId = null
      })
  }

  private applyTrackList(data: unknown): void {
    const list = Array.isArray(data) ? (data as MpvTrack[]) : []
    const audio: Track[] = []
    const subs: Track[] = []
    let currentAudio: string | null = null
    let currentSub: string | null = null

    for (const t of list) {
      if (!t || typeof t.id !== 'number') continue
      const track: Track = {
        id: String(t.id),
        label: t.title || t.lang || defaultLabel(t.type, t.id),
        language: t.lang ?? undefined,
        default: t.selected ?? false,
        forced: t.forced ?? false,
      }
      if (t.type === 'audio') {
        audio.push(track)
        if (t.selected) currentAudio = track.id
      } else if (t.type === 'sub') {
        subs.push(track)
        if (t.selected) currentSub = track.id
      }
    }

    this.core.patch({
      audioTracks: audio,
      subtitleTracks: subs,
      currentAudioTrackId: currentAudio,
      currentSubtitleTrackId: currentSub,
    })
    this.core.emit({ type: 'trackslisted' })
  }

  // ── 控制 ──────────────────────────────────────────────────

  async play(): Promise<void> {
    await this.send(['set_property', 'pause', false])
    this.paused = false
    this.core.patch({ state: 'playing' })
  }

  async pause(): Promise<void> {
    await this.send(['set_property', 'pause', true])
    this.paused = true
    this.core.patch({ state: 'paused' })
    this.core.emit({ type: 'paused', byUser: true })
  }

  async seek(seconds: number): Promise<void> {
    const target = Math.max(0, seconds)
    // absolute+exact：拖到哪就是哪，不吸附关键帧。
    await this.send(['seek', target, 'absolute+exact'])
    this.core.patch({ position: target })
  }

  async setVolume(v: number): Promise<void> {
    // mpv 的 volume 是 0..100，我们内部是 0..1。转换只在这里发生。
    const clamped = Math.min(1, Math.max(0, v))
    await this.send(['set_property', 'volume', Math.round(clamped * 100)])
    this.core.patch({ volume: clamped })
  }

  async setMuted(muted: boolean): Promise<void> {
    await this.send(['set_property', 'mute', muted])
    this.core.patch({ muted })
  }

  async setRate(rate: number): Promise<void> {
    const clamped = Math.min(4, Math.max(0.25, rate))
    await this.send(['set_property', 'speed', clamped])
    this.core.patch({ rate: clamped })
  }

  async selectAudioTrack(id: string): Promise<void> {
    await this.send(['set_property', 'aid', toTrackValue(id)])
    this.core.patch({ currentAudioTrackId: id })
  }

  async selectSubtitleTrack(id: string | null): Promise<void> {
    // mpv 关字幕用字符串 "no"，不是 0 也不是 false。
    await this.send(['set_property', 'sid', id === null ? 'no' : toTrackValue(id)])
    this.core.patch({ currentSubtitleTrackId: id })
  }

  /**
   * mpv 不参与画质档位：桌面端走 Direct Play / 本地文件，没有多码率可切。
   *
   * 静默返回是**正确的行为**而不是偷懒：能力里 `hls` 为 true 但 `qualities`
   * 恒为空数组，UI 因此不会画这个菜单；真走到这里报一个错只会污染日志。
   */
  async selectQuality(_id: string): Promise<void> {}

  async setFullscreen(on: boolean): Promise<void> {
    // 全屏的是 mpv 自己的窗口，不是页面元素。
    await this.send(['set_property', 'fullscreen', on])
  }

  async stop(): Promise<void> {
    await this.stopSession()
    this.core.setState('idle', null)
  }

  async destroy(): Promise<void> {
    this.disposed = true
    await this.stopSession()
    this.unlistenEvent?.()
    this.unlistenClosed?.()
    this.unlistenEvent = null
    this.unlistenClosed = null
    this.placeholder?.remove()
    this.placeholder = null
    this.container = null
    this.core.destroy()
  }

  private async send(command: (string | number | boolean)[]): Promise<void> {
    if (!this.bridge) return
    try {
      await this.bridge.invoke('mpv_send', { command, request_id: null })
    } catch (e) {
      // 控制命令失败不该让播放器进错误态（用户还能继续看），
      // 但也不该完全无声 —— 至少留一行日志。
      console.warn('[whatmedia] mpv 命令失败:', command[0], describe(e, ''))
    }
  }
}

/**
 * 轨道 id 转 mpv 的值。
 *
 * mpv 的 aid/sid 收数字轨号，也收 "no" / "auto" 这类字符串。
 * 我们的 Track.id 是字符串化的数字，这里转回数字；转不动就原样传字符串。
 */
function toTrackValue(id: string): number | string {
  const n = Number(id)
  return Number.isFinite(n) ? n : id
}

function defaultLabel(type: string | undefined, id: number): string {
  if (type === 'audio') return `音轨 ${id}`
  if (type === 'sub') return `字幕 ${id}`
  return `轨道 ${id}`
}

function num(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

function describe(e: unknown, prefix: string): string {
  const msg = e instanceof Error ? e.message : typeof e === 'string' ? e : String(e)
  return prefix ? `${prefix}：${msg}` : msg
}
