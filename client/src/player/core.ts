/**
 * 播放器状态机核心。
 *
 * **为什么要从平台后端里拆出来**：Web 后端要操作 `<video>`，mpv 后端要走 Tauri IPC，
 * 两者都没法在 jsdom / 单元测试里跑起来。但"状态怎么变迁""什么时候该上报进度"
 * 这些逻辑恰恰是最容易写错、又最该被测试的部分。
 * 所以把这部分做成纯数据 + 纯函数：后端只负责"把原生事件翻译成一次 `patch` 或 `emit`"。
 *
 * 这样测试就成了：`core.patch({ state: 'playing' })`，断言订阅者收到了什么。
 * 不需要浏览器，不需要 mpv，不需要真的播一帧。
 */

import type {
  PlayerCapabilities,
  PlayerErrorCode,
  PlayerError,
  PlayerEvent,
  PlayerSnapshot,
  PlayerState,
} from './types'

/** 初始状态。每次 `load` 都会重置回它（位置除外，见 `resetForLoad`）。 */
export function initialSnapshot(): PlayerSnapshot {
  return {
    state: 'idle',
    position: 0,
    duration: 0,
    volume: 1,
    muted: false,
    rate: 1,
    buffered: 0,
    audioTracks: [],
    subtitleTracks: [],
    qualities: [],
    currentAudioTrackId: null,
    currentSubtitleTrackId: null,
    currentQualityId: null,
    error: null,
  }
}

/**
 * 浅比较。
 *
 * 数组用引用判等就够：轨道列表只在元数据到手时整体换一次，
 * 不会每次 timeupdate 都产生新数组。真要是那样，问题在后端而不是这里。
 */
function shallowEqual(a: PlayerSnapshot, b: PlayerSnapshot): boolean {
  const keys = Object.keys(a) as (keyof PlayerSnapshot)[]
  for (const k of keys) {
    if (a[k] !== b[k]) return false
  }
  return true
}

type StateListener = (snapshot: PlayerSnapshot) => void
type EventListener = (event: PlayerEvent) => void

export class PlayerCore {
  readonly capabilities: PlayerCapabilities

  private snapshot: PlayerSnapshot = initialSnapshot()
  private stateListeners = new Set<StateListener>()
  private eventListeners = new Set<EventListener>()
  private destroyed = false

  constructor(capabilities: PlayerCapabilities) {
    this.capabilities = capabilities
  }

  get state(): PlayerSnapshot {
    return this.snapshot
  }

  /**
   * 合并状态并通知订阅者。
   *
   * **没有变化就不通知** —— `timeupdate` 每秒来 4 次，其中大半是同一个值
   * （尤其是 hls.js 在低精度模式下）。每次都通知会让 Vue 白白重渲染整个控制条。
   */
  patch(partial: Partial<PlayerSnapshot>): void {
    if (this.destroyed) return
    const next: PlayerSnapshot = { ...this.snapshot, ...partial }
    if (shallowEqual(this.snapshot, next)) return

    const from = this.snapshot.state
    this.snapshot = next

    if (from !== next.state) {
      this.emit({ type: 'statechange', from, to: next.state })
    }
    for (const l of this.stateListeners) l(next)
  }

  /** 只改状态，附带可选错误信息。错误处理走这里，保持一处入口。 */
  setState(state: PlayerState, error?: PlayerError | null): void {
    this.patch(error === undefined ? { state } : { state, error })
  }

  /** 把播放器置为错误态。错误码决定 UI 是"重试"还是"换个策略"。 */
  fail(code: PlayerErrorCode, message: string): void {
    const error: PlayerError = { code, message }
    this.patch({ state: 'error', error })
    this.emit({ type: 'error', error })
  }

  /**
   * 重置为载入新源之前的状态。
   *
   * **保留音量 / 静音 / 速率**：用户调过音量，切下一集时不该被重置 ——
   * 这是播放器该有的行为，几乎所有主流播放器都这么做。
   */
  resetForLoad(): void {
    const keep = {
      volume: this.snapshot.volume,
      muted: this.snapshot.muted,
      rate: this.snapshot.rate,
    }
    this.snapshot = { ...initialSnapshot(), ...keep, state: 'loading' }
    for (const l of this.stateListeners) l(this.snapshot)
  }

  emit(event: PlayerEvent): void {
    if (this.destroyed) return
    for (const l of this.eventListeners) l(event)
  }

  subscribe(listener: StateListener): () => void {
    this.stateListeners.add(listener)
    return () => this.stateListeners.delete(listener)
  }

  on(listener: EventListener): () => void {
    this.eventListeners.add(listener)
    return () => this.eventListeners.delete(listener)
  }

  /** 销毁后所有 patch / emit 变成空操作，避免后端在卸载后还在推事件。 */
  destroy(): void {
    this.destroyed = true
    this.stateListeners.clear()
    this.eventListeners.clear()
  }
}
