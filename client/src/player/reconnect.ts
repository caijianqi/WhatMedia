/**
 * 断网 → 重连 → 续播。P6 DoD 最后一条："网络切换（WiFi ↔ 4G）不崩溃，可恢复播放"。
 *
 * ## 为什么必须自己写
 *
 * 三件事 native 播放器都不会替你做：
 *
 * 1. **ExoPlayer 断网后不会自己重连**。它抛一个 IO 错误后就停在 STATE_IDLE，
 *    网络回来了它也不知道 —— 用户得退出去重新点一遍。
 * 2. `hls.js` 会重试，但**只重试分片请求**，master playlist 404 或
 *    整个连接被换掉（WiFi → 4G 会换 IP）时它救不回来。
 * 3. **续播位置**没有任何一层会记住。重连后从 0 开始，用户等于白看。
 *
 * ## 可测性
 *
 * 退避时序、放弃条件、续播位置全部是纯函数与纯状态机，
 * 不需要真的断网就能测 —— 真断网测试既慢又不稳定。
 */

import type { PlayerErrorCode } from './types'

/** 重试策略。 */
export interface ReconnectPolicy {
  /** 最多重试几次。超过就放弃，交给 UI 报错。 */
  maxAttempts: number
  /** 首次重试前的等待（毫秒）。 */
  baseDelayMs: number
  /** 退避上限（毫秒）。不设上限时第 8 次就要等 4 分钟。 */
  maxDelayMs: number
}

export const DEFAULT_RECONNECT_POLICY: ReconnectPolicy = {
  maxAttempts: 5,
  baseDelayMs: 800,
  maxDelayMs: 15_000,
}

/**
 * 续播回退量（秒）。
 *
 * 从"断掉的那一刻"往前退一点再播。原因有二：
 *   · 断掉前最后几秒的画面/声音可能是不完整的（分片只下了一半）；
 *   · 用户需要一点上下文才能接上剧情，卡在正中间很突兀。
 * 2 秒是刚好能接上又不显得跳过的量。
 */
export const RESUME_REWIND_SECONDS = 2

/** 只有网络类错误值得重试。解码失败重试一百次还是解不了。 */
export function isRetryableError(code: PlayerErrorCode): boolean {
  return code === 'network'
}

/**
 * 第 `attempt` 次（从 0 开始）重试前要等多久。
 *
 * 指数退避 + 上限。没有上限的话第 8 次要等 100 秒以上，
 * 用户早就以为应用死了。
 */
export function delayForAttempt(attempt: number, policy = DEFAULT_RECONNECT_POLICY): number {
  if (attempt < 0) return 0
  const raw = policy.baseDelayMs * 2 ** attempt
  return Math.min(raw, policy.maxDelayMs)
}

/**
 * 续播位置。
 *
 * 从最后已知位置往前退，但不小于 0 ——
 * 在片头 1 秒处断网，续播位置算出来是 -1，传给播放器会出奇怪的行为。
 */
export function resumePosition(lastPosition: number, duration = Number.POSITIVE_INFINITY): number {
  const p = Number.isFinite(lastPosition) ? Math.max(0, lastPosition) : 0
  if (!Number.isFinite(duration) || duration <= 0) return Math.max(0, p - RESUME_REWIND_SECONDS)
  return Math.min(Math.max(0, p - RESUME_REWIND_SECONDS), duration)
}

/** 协调器的状态。 */
export type RecoveryState = 'idle' | 'waiting' | 'gave-up'

/**
 * 重连协调器。
 *
 * 持有"现在该不该重试、还要等多久、已经试了几次"这三件事，
 * **不持有定时器** —— 定时器由调用方按 `nextDelayMs()` 自己起，
 * 这样测试里不用等真实时间。
 */
export class RecoveryCoordinator {
  private readonly policy: ReconnectPolicy
  private _state: RecoveryState = 'idle'
  private _attempt = 0
  private _lastPosition = 0
  private _duration = 0
  private _online = true

  constructor(policy: ReconnectPolicy = DEFAULT_RECONNECT_POLICY) {
    this.policy = policy
  }

  get state(): RecoveryState {
    return this._state
  }

  get attempt(): number {
    return this._attempt
  }

  /** 当前是否认为在线。 */
  get online(): boolean {
    return this._online
  }

  /** 记住位置。每次 timeupdate 调用。 */
  recordPosition(position: number, duration: number): void {
    if (Number.isFinite(position)) this._lastPosition = Math.max(0, position)
    if (Number.isFinite(duration)) this._duration = Math.max(0, duration)
  }

  /** 成功开始播放：一切归零。 */
  markHealthy(): void {
    this._state = 'idle'
    this._attempt = 0
  }

  /**
   * 出错。
   *
   * 返回 `true` 表示"应当安排一次重试"，调用方再用 `nextDelayMs()` 取等待时长。
   * 返回 `false` 表示放弃（不可重试的错误，或次数用尽）。
   */
  onError(code: PlayerErrorCode): boolean {
    if (!isRetryableError(code)) {
      this._state = 'gave-up'
      return false
    }
    if (this._attempt >= this.policy.maxAttempts) {
      this._state = 'gave-up'
      return false
    }
    this._state = 'waiting'
    return true
  }

  /**
   * 网络状态变化。
   *
   * **离线时立即进入等待**（即使还没收到播放器错误）——
   * 有些网络切换不会让播放器立刻报错，只是缓冲卡住，
   * 等它报错可能要很久。
   *
   * **恢复在线时立即返回 true**：网络都回来了就没必要把退避计时器走完，
   * 那会让用户多等好几秒。
   */
  onNetworkChange(online: boolean): boolean {
    const was = this._online
    this._online = online
    if (online) {
      if (was && this._state === 'idle') return false
      return this._state === 'waiting' || !was
    }
    if (this._state === 'idle') this._state = 'waiting'
    return false
  }

  /** 下一次重试要等多久。 */
  nextDelayMs(): number {
    return delayForAttempt(this._attempt, this.policy)
  }

  /**
   * 真的发起一次重试。返回续播位置。
   *
   * 调用后 attempt 自增，所以 `nextDelayMs()` 会在下一次返回更长的等待。
   */
  beginAttempt(): number {
    this._attempt += 1
    return resumePosition(this._lastPosition, this._duration)
  }

  /** 放弃。UI 应当据此提示"换个播放方式"并让服务端重新决策。 */
  giveUp(): void {
    this._state = 'gave-up'
  }

  /** 换片 / 卸载时重置。 */
  reset(): void {
    this._state = 'idle'
    this._attempt = 0
    this._lastPosition = 0
    this._duration = 0
  }
}
