/**
 * 播放进度上报器。
 *
 * 需求两头夹：
 *   · DoD 要求"进度跨端同步误差 < 5s" —— 意味着不能只在退出时上报一次，
 *     否则在另一台设备上续播会退回很久以前。
 *   · 一部 2 小时的片按每秒上报是 7200 次请求，服务端根本不需要这么细，
 *     而且移动端这么做是在烧用户的电量和流量。
 *
 * 取法：**常规节流 + 关键节点立即上报**。
 *   · 常规：每 `intervalSeconds`（默认 10s）最多一次。
 *   · 立即：seek、暂停、停止、页面即将卸载。
 *
 * 为什么特殊对待 seek：seek 是进度跨度最大的操作，也是最容易丢的 ——
 * 用户拖到 1:20:00 再关掉，如果只靠节流，很可能一条都没发出去就走了。
 *
 * 为什么单独提供 `flush()`：页面隐藏 (`visibilitychange`) 和关闭 (`pagehide`) 时，
 * 普通 fetch 很可能发不出去，需要调用方改用 `sendBeacon` 这类通道。
 * 而 `flush` 是幂等的，所以可以放心在暂停和卸载时各调一遍。
 */

import { DEFAULT_PROGRESS_POLICY, type ProgressPolicy } from './types'

/** 上报动作。`durationSeconds` 未知时传 undefined。 */
export type ProgressSender = (
  positionSeconds: number,
  durationSeconds?: number,
) => void | Promise<void>

export class ProgressReporter {
  private readonly send: ProgressSender
  private readonly policy: ProgressPolicy

  private lastSentAt = 0
  /** 已经上报过的位置。用来让 `flush` 幂等 —— 同一个位置不重复发。 */
  private lastSentPosition = -1
  private lastPosition = -1
  private lastDuration: number | undefined = undefined

  /**
   * @param send 真正的网络上报。由调用方注入，这样测试不需要 fetch。
   * @param now  取时间的函数。注入是为了测试能用假时钟 —— 真 `setTimeout` 会拖慢测试。
   */
  constructor(
    send: ProgressSender,
    private readonly now: () => number = () => Date.now(),
    policy: Partial<ProgressPolicy> = {},
  ) {
    this.send = send
    this.policy = { ...DEFAULT_PROGRESS_POLICY, ...policy }
  }

  /**
   * 播放中的例行上报。内部节流。
   *
   * **位置没变就不发**：暂停状态下浏览器仍可能零星触发 timeupdate，
   * 那不值得一条请求。
   */
  onTimeUpdate(positionSeconds: number, durationSeconds?: number): void {
    // 位置倒退（seek 到更早）也算变化，所以比的是"是否不同"而不是"是否变大"。
    if (positionSeconds === this.lastPosition) return
    this.lastPosition = positionSeconds
    this.lastDuration = durationSeconds

    const elapsed = (this.now() - this.lastSentAt) / 1000
    if (elapsed < this.policy.intervalSeconds) return

    this.doSend(positionSeconds, durationSeconds)
  }

  /** seek 后调用。命中策略就立即上报。 */
  onSeek(positionSeconds: number, durationSeconds?: number): void {
    this.lastPosition = positionSeconds
    this.lastDuration = durationSeconds
    if (!this.policy.reportOnSeek) return
    this.doSend(positionSeconds, durationSeconds)
  }

  /** 暂停 / 停止时调用。 */
  onPause(positionSeconds: number, durationSeconds?: number): void {
    this.lastPosition = positionSeconds
    this.lastDuration = durationSeconds
    if (!this.policy.reportOnPause) return
    this.doSend(positionSeconds, durationSeconds)
  }

  /**
   * 立即补发一次尚未上报的位置，无视节流。
   *
   * **幂等**：已经发过的位置不会再发，所以可以放心在 `pause` 和 `pagehide` 里各调一遍。
   * 没有位置（用户还没开播就关页面）时是空操作 —— 发一条 position=0
   * 会把真实的续播点冲掉。
   */
  flush(): void {
    if (this.lastPosition < 0) return
    if (this.lastPosition === this.lastSentPosition) return
    this.doSend(this.lastPosition, this.lastDuration)
  }

  private doSend(position: number, duration?: number): void {
    this.lastSentAt = this.now()
    this.lastSentPosition = position
    void this.send(position, duration)
  }
}
