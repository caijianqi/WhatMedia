/**
 * 进度上报策略的测试。
 *
 * 用假时钟而不是真 setTimeout —— 真等 10 秒的测试没人愿意跑。
 */

import { describe, expect, it, vi } from 'vitest'
import { ProgressReporter } from './progress'
import type { ProgressPolicy } from './types'

function setup(policy: Partial<ProgressPolicy> = {}) {
  const sent: { position: number; duration?: number }[] = []
  let clock = 1_000_000
  const send = vi.fn((position: number, duration?: number) => {
    sent.push({ position, duration })
  })
  const reporter = new ProgressReporter(send, () => clock, policy)
  return {
    reporter,
    send,
    sent,
    advance: (ms: number) => {
      clock += ms
    },
  }
}

describe('ProgressReporter', () => {
  it('节流：间隔内反复上报只发一次', () => {
    const { reporter, send, advance } = setup()

    reporter.onTimeUpdate(1)
    advance(1000)
    reporter.onTimeUpdate(2)
    advance(1000)
    reporter.onTimeUpdate(3)

    expect(send).toHaveBeenCalledTimes(1)
    expect(send).toHaveBeenCalledWith(1, undefined)
  })

  it('超过间隔后可以再发', () => {
    const { reporter, send, advance } = setup()

    reporter.onTimeUpdate(1)
    advance(11_000)
    reporter.onTimeUpdate(20)

    expect(send).toHaveBeenCalledTimes(2)
    expect(send).toHaveBeenLastCalledWith(20, undefined)
  })

  it('位置没变就不发 —— 暂停时浏览器仍会零星触发 timeupdate', () => {
    const { reporter, send, advance } = setup()

    reporter.onTimeUpdate(5)
    advance(30_000)
    reporter.onTimeUpdate(5)

    expect(send).toHaveBeenCalledTimes(1)
  })

  it('seek 立即上报，无视节流', () => {
    const { reporter, send } = setup()

    reporter.onTimeUpdate(1) // 刚发过
    reporter.onSeek(3600)

    expect(send).toHaveBeenCalledTimes(2)
    expect(send).toHaveBeenLastCalledWith(3600, undefined)
  })

  it('暂停立即上报', () => {
    const { reporter, send } = setup()

    reporter.onTimeUpdate(1)
    reporter.onPause(42)

    expect(send).toHaveBeenLastCalledWith(42, undefined)
  })

  it('reportOnSeek 关掉后 seek 不再触发上报', () => {
    const { reporter, send } = setup({ reportOnSeek: false })

    reporter.onSeek(3600)

    expect(send).not.toHaveBeenCalled()
  })

  it('flush 补发被节流挡下的位置', () => {
    const { reporter, send, advance } = setup()

    reporter.onTimeUpdate(1)
    advance(2000)
    reporter.onTimeUpdate(120) // 被节流挡住
    expect(send).toHaveBeenCalledTimes(1)

    reporter.flush()

    expect(send).toHaveBeenCalledTimes(2)
    expect(send).toHaveBeenLastCalledWith(120, undefined)
  })

  it('flush 幂等：连续两次只发一次', () => {
    const { reporter, send } = setup()

    reporter.onTimeUpdate(60) // 这是第一次上报
    reporter.flush()
    reporter.flush()

    // 第一次 onTimeUpdate 已经发了，flush 不该再发同一个位置。
    expect(send).toHaveBeenCalledTimes(1)
  })

  it('还没播过就 flush 是空操作 —— 不该发 position=0 把续播点冲掉', () => {
    const { reporter, send } = setup()

    reporter.flush()

    expect(send).not.toHaveBeenCalled()
  })

  it('duration 会一并带上', () => {
    const { reporter, send } = setup()

    reporter.onTimeUpdate(30, 7200)

    expect(send).toHaveBeenCalledWith(30, 7200)
  })
})
