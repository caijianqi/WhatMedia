/**
 * Web 平台播放器后端：`<video>` + hls.js。
 *
 * 职责边界：**只做翻译**。把 HTMLMediaElement / hls.js 的事件翻译成
 * `PlayerCore.patch()` 和 `emit()`。所有"状态怎么变"的判断都在 core 里，
 * 所以这一层几乎不含逻辑，也就几乎不需要浏览器才能测。
 *
 * 几条被现实逼出来的处理：
 *   · **HLS 优先用 hls.js 而不是原生**。Safari 原生 HLS 表现很好，但只有它能用；
 *     hls.js 在所有浏览器上行为一致，还暴露多码率档位。所以默认走 hls.js，
 *     只有在 hls.js 不可用（MSE 缺失）时才退回原生。
 *   · **音轨切换不做**。见 `PlayerCapabilities.canSwitchAudioTrack`。
 *   · **自动播放被拦截要报 `autoplay-blocked`** 而不是静默失败 ——
 *     静默失败的后果是用户点播放没反应，且日志里什么都没有。
 */

import { PlayerCore } from './core'
import { capabilitiesFor } from './capabilities'
import type {
  LoadOptions,
  MediaPlayer,
  PlayerCapabilities,
  PlayerErrorCode,
  PlayerEvent,
  PlayerSnapshot,
  PlayerTarget,
  PlaybackSource,
  Track,
} from './types'

/**
 * hls.js 的最小接口。
 *
 * 不直接 import hls.js 的类型有两个好处：hls.js 是按需动态加载的（打包体积），
 * 以及测试里可以塞一个 fake 进来，无需真的解析 m3u8。
 */
export interface HlsLike {
  loadSource(url: string): void
  attachMedia(video: HTMLVideoElement): void
  destroy(): void
  readonly levels: Array<{ height?: number; bitrate?: number; name?: string }>
  currentLevel: number
  on(event: string, cb: (...args: unknown[]) => void): void
  off(event: string, cb: (...args: unknown[]) => void): void
}

export type HlsConstructor = new (config?: Record<string, unknown>) => HlsLike

/** hls.js 的事件名。写成常量是因为它们不带类型，拼错了不会报错。 */
const HLS_EVENTS = {
  error: 'hlsError',
  manifestParsed: 'hlsManifestParsed',
  levelSwitched: 'hlsLevelSwitched',
} as const

export interface WebPlayerOptions {
  /** 注入 hls.js 构造器。测试用 fake；生产环境留空则按需 `import('hls.js')`。 */
  hlsCtor?: HlsConstructor
  /** 强制用原生 HLS（Safari）。默认 false —— 见文件头说明。 */
  preferNativeHls?: boolean
  /** 覆盖能力声明。测试用。 */
  capabilities?: PlayerCapabilities
}

export class WebPlayer implements MediaPlayer {
  readonly capabilities: PlayerCapabilities

  private readonly core: PlayerCore
  private readonly options: WebPlayerOptions

  private video: HTMLVideoElement | null = null
  private hls: HlsLike | null = null
  private fullscreenTarget: HTMLElement | null = null

  /** 元数据到手后才能设 currentTime，所以先存着。 */
  private pendingStartAt = 0
  /** 卸载标志：异步回调回来时实例可能已经销毁了。 */
  private disposed = false

  private onHlsError: ((...args: unknown[]) => void) | null = null
  private onHlsManifestParsed: ((...args: unknown[]) => void) | null = null
  private onHlsLevelSwitched: ((...args: unknown[]) => void) | null = null

  constructor(options: WebPlayerOptions = {}) {
    this.options = options
    // Web 后端就是 web 能力 —— 不按运行环境猜，见 capabilitiesFor 的说明。
    this.capabilities = options.capabilities ?? capabilitiesFor('web')
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
    if (this.video) return // 幂等

    this.fullscreenTarget = target.fullscreenTarget ?? target.container

    const video = document.createElement('video')
    video.playsInline = true
    video.controls = false // 控制条自己画，原生控制条样式改不动
    video.preload = 'metadata'
    // 画面填满容器，letterbox 交给 CSS。
    video.style.width = '100%'
    video.style.height = '100%'
    video.style.objectFit = 'contain'
    video.style.background = '#000'

    this.video = video
    target.container.appendChild(video)
    this.bindVideoEvents(video)

    // 初始状态同步一次，避免 UI 读到默认值（比如真实音量其实是 1，没问题，
    // 但如果将来从 localStorage 恢复音量，这里就是必要的）。
    this.core.patch({ volume: video.volume, muted: video.muted, rate: video.playbackRate })
  }

  async load(source: PlaybackSource, options: LoadOptions = {}): Promise<void> {
    const video = this.requireVideo()
    this.core.resetForLoad()
    this.teardownPlayback()

    this.pendingStartAt = options.startAt ?? 0
    video.loop = false

    switch (source.kind) {
      case 'progressive':
        video.src = source.url
        break

      case 'hls':
        await this.loadHls(video, source.url)
        break

      case 'local-file':
        // Web 端读不了本地路径。这不是错误恢复能解决的情况，
        // 要在 UI 层就避免走到这里（能力里 `localFile: false`）。
        this.core.fail('decode', 'Web 播放器无法直接读取本地文件路径')
        return
    }

    if (options.autoplay) await this.play()
  }

  private async loadHls(video: HTMLVideoElement, url: string): Promise<void> {
    if (this.options.preferNativeHls && this.nativeHlsSupported(video)) {
      video.src = url
      return
    }

    const HlsCtor = this.options.hlsCtor ?? (await loadHlsJs())
    if (!HlsCtor) {
      // hls.js 加载不了（老浏览器 / 网络问题）→ 退原生。
      if (this.nativeHlsSupported(video)) {
        video.src = url
        return
      }
      this.core.fail('decode', '当前浏览器既不支持 hls.js 也不支持原生 HLS')
      return
    }

    const hls = new HlsCtor({ enableWorker: true })
    this.hls = hls

    this.onHlsError = (_evt: unknown, data: unknown) => {
      const d = data as { fatal?: boolean; type?: string } | undefined
      this.handleHlsError(d)
    }

    this.onHlsManifestParsed = () => {
      this.core.patch({ qualities: this.hlsLevels() })
      this.core.emit({ type: 'trackslisted' })
    }

    this.onHlsLevelSwitched = (_evt: unknown, data: unknown) => {
      const level = (data as { level?: number } | undefined)?.level
      const id = level != null ? String(level) : null
      this.core.patch({ currentQualityId: id })
    }

    hls.on(HLS_EVENTS.error, this.onHlsError)
    hls.on(HLS_EVENTS.manifestParsed, this.onHlsManifestParsed)
    hls.on(HLS_EVENTS.levelSwitched, this.onHlsLevelSwitched)

    hls.attachMedia(video)
    hls.loadSource(url)
  }

  private handleHlsError(data: { fatal?: boolean; type?: string } | undefined): void {
    // 非致命错误 hls.js 自己会重试，报出来只会吓到用户。
    if (!data?.fatal) return
    const code: PlayerErrorCode =
      data.type === 'networkError' ? 'network' : data.type === 'mediaError' ? 'decode' : 'unknown'
    const message =
      code === 'network'
        ? '视频流加载失败，网络中断或服务端已停止转码'
        : '视频解码失败，可能是编码不被浏览器支持'
    this.core.fail(code, message)
  }

  private nativeHlsSupported(video: HTMLVideoElement): boolean {
    return typeof video.canPlayType === 'function'
      ? video.canPlayType('application/vnd.apple.mpegurl') !== ''
      : false
  }

  private hlsLevels(): Track[] {
    if (!this.hls) return []
    return this.hls.levels.map((lv, i) => ({
      id: String(i),
      label: lv.height ? `${lv.height}p` : (lv.name ?? `档位 ${i + 1}`),
      // 自动档（hls.js 里 currentLevel = -1）不在这里列出，由 UI 单独提供。
      default: false,
    }))
  }

  private bindVideoEvents(video: HTMLVideoElement): void {
    const patch = (p: Partial<PlayerSnapshot>) => {
      if (!this.disposed) this.core.patch(p)
    }

    video.addEventListener('loadedmetadata', () => {
      patch({
        duration: Number.isFinite(video.duration) ? video.duration : 0,
        state: this.core.state.state === 'loading' ? 'ready' : this.core.state.state,
      })
      if (this.pendingStartAt > 0) {
        try {
          video.currentTime = this.pendingStartAt
        } catch {
          // 某些流不支持 seek（直播 / 未索引的文件）。忽略，从头播即可。
        }
        this.pendingStartAt = 0
      }
    })

    video.addEventListener('durationchange', () => {
      if (Number.isFinite(video.duration)) patch({ duration: video.duration })
    })

    video.addEventListener('timeupdate', () => {
      patch({ position: video.currentTime, buffered: this.bufferedEnd(video) })
      this.core.emit({
        type: 'timeupdate',
        position: video.currentTime,
        duration: Number.isFinite(video.duration) ? video.duration : 0,
      })
    })

    video.addEventListener('progress', () => patch({ buffered: this.bufferedEnd(video) }))

    video.addEventListener('playing', () => {
      patch({ state: 'playing' })
      this.core.emit({ type: 'playing' })
    })

    video.addEventListener('waiting', () => {
      patch({ state: 'buffering' })
      this.core.emit({ type: 'waiting' })
    })

    video.addEventListener('pause', () => {
      // 只有真的暂停了才置 paused；ended 之后浏览器也会发 pause。
      if (video.ended) return
      patch({ state: 'paused' })
      this.core.emit({ type: 'paused', byUser: true })
    })

    video.addEventListener('ended', () => {
      patch({ state: 'ended' })
      this.core.emit({ type: 'ended' })
    })

    video.addEventListener('volumechange', () => {
      patch({ volume: video.volume, muted: video.muted })
    })

    video.addEventListener('ratechange', () => patch({ rate: video.playbackRate }))

    video.addEventListener('error', () => this.handleMediaError(video))
  }

  private handleMediaError(video: HTMLVideoElement): void {
    const code = video.error?.code ?? 0
    // 1 = ABORTED：通常是换源导致的，不是真错误，静默即可。
    if (code === 1) return
    const mapped: PlayerErrorCode = code === 2 ? 'network' : code === 3 ? 'decode' : 'unknown'
    const message =
      mapped === 'network'
        ? '视频加载中断，请检查网络'
        : mapped === 'decode'
          ? '浏览器无法解码这个视频（格式不支持或文件损坏）'
          : '播放失败'
    this.core.fail(mapped, message)
  }

  private bufferedEnd(video: HTMLVideoElement): number {
    const b = video.buffered
    if (!b || b.length === 0) return 0
    return b.end(b.length - 1)
  }

  async play(): Promise<void> {
    const video = this.requireVideo()
    try {
      await video.play()
      this.core.patch({ state: 'playing' })
    } catch (err) {
      // 自动播放被拦截。报出来，让 UI 显示一个"点击播放"的大按钮。
      const name = (err as { name?: string })?.name
      if (name === 'NotAllowedError') {
        this.core.fail('autoplay-blocked', '浏览器阻止了自动播放，需要你点一下')
        return
      }
      this.core.fail('decode', '播放失败')
    }
  }

  async pause(): Promise<void> {
    const video = this.video
    if (!video) return
    video.pause()
    this.core.patch({ state: 'paused' })
  }

  async seek(seconds: number): Promise<void> {
    const video = this.requireVideo()
    const target = Math.max(0, seconds)
    try {
      video.currentTime = target
      this.core.patch({ position: target })
    } catch {
      // 不支持 seek 的流。不置错误态 —— 用户还能继续看。
    }
  }

  async setVolume(v: number): Promise<void> {
    const video = this.requireVideo()
    const clamped = Math.min(1, Math.max(0, v))
    video.volume = clamped
    this.core.patch({ volume: clamped })
  }

  async setMuted(muted: boolean): Promise<void> {
    const video = this.requireVideo()
    video.muted = muted
    this.core.patch({ muted })
  }

  async setRate(rate: number): Promise<void> {
    const video = this.requireVideo()
    // 浏览器对过小/过大的速率会抛错，这里夹到安全区间。
    const clamped = Math.min(4, Math.max(0.25, rate))
    video.playbackRate = clamped
    this.core.patch({ rate: clamped })
  }

  async selectAudioTrack(_id: string): Promise<void> {
    // 浏览器 <video> 切不了音轨（见 types.ts 的说明）。
    // 静默返回：UI 应当先查 `capabilities.canSwitchAudioTrack` 再决定显不显示这个菜单；
    // 真走到这里也报不出一个用户能处理的错误。
  }

  async selectQuality(id: string): Promise<void> {
    if (!this.hls) return
    const level = Number(id)
    if (!Number.isFinite(level)) return
    this.hls.currentLevel = level
    this.core.patch({ currentQualityId: id })
  }

  async setFullscreen(on: boolean): Promise<void> {
    const el = this.fullscreenTarget
    if (!el) return
    try {
      if (on) await el.requestFullscreen()
      else if (document.fullscreenElement) await document.exitFullscreen()
    } catch {
      // 全屏被拒（没有用户手势 / 权限策略）。用户还能继续看，不算错误。
    }
  }

  /** 卸载当前源但保留播放器实例，可以再 `load`。 */
  async stop(): Promise<void> {
    this.teardownPlayback()
    if (this.video) {
      this.video.removeAttribute('src')
      this.video.load()
    }
    this.core.setState('idle', null)
  }

  async destroy(): Promise<void> {
    this.disposed = true
    this.teardownPlayback()
    const video = this.video
    if (video) {
      video.remove()
    }
    this.video = null
    this.fullscreenTarget = null
    this.core.destroy()
  }

  private teardownPlayback(): void {
    const hls = this.hls
    if (hls) {
      if (this.onHlsError) hls.off(HLS_EVENTS.error, this.onHlsError)
      if (this.onHlsManifestParsed) hls.off(HLS_EVENTS.manifestParsed, this.onHlsManifestParsed)
      if (this.onHlsLevelSwitched) hls.off(HLS_EVENTS.levelSwitched, this.onHlsLevelSwitched)
      hls.destroy()
    }
    this.hls = null
    this.onHlsError = null
    this.onHlsManifestParsed = null
    this.onHlsLevelSwitched = null
  }

  private requireVideo(): HTMLVideoElement {
    if (!this.video) throw new Error('WebPlayer 未 initialize')
    return this.video
  }
}

/**
 * 按需加载 hls.js。
 *
 * 动态 import 的理由：Direct Play 的用户完全用不到它，不该为它付出首屏体积。
 * 加载失败返回 null 让调用方退原生 HLS，而不是让整个播放器挂掉。
 */
async function loadHlsJs(): Promise<HlsConstructor | null> {
  try {
    const mod = (await import('hls.js')) as unknown as { default?: HlsConstructor }
    return mod.default ?? null
  } catch {
    return null
  }
}