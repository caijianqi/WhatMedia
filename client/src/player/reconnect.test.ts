import { describe, expect, it } from 'vitest'
import {
  DEFAULT_RECONNECT_POLICY,
  RESUME_REWIND_SECONDS,
  RecoveryCoordinator,
  delayForAttempt,
  isRetryableError,
  resumePosition,
} from './reconnect'

describe('isRetryableError', () => {
  it('只有网络错误值得重试', () => {
    expect(isRetryableError('network')).toBe(true)
    expect(isRetryableError('decode')).toBe(false)
    expect(isRetryableError('not-found')).toBe(false)
    expect(isRetryableError('unknown')).toBe(false)
  })
})

describe('delayForAttempt', () => {
  it('指数退避', () => {
    const p = { maxAttempts: 10, baseDelayMs: 1000, maxDelayMs: 100_000 }
    expect(delayForAttempt(0, p)).toBe(1000)
    expect(delayForAttempt(1, p)).toBe(2000)
    expect(delayForAttempt(2, p)).toBe(4000)
  })

  it('有上限 —— 第 8 次不能让人等几分钟', () => {
    expect(delayForAttempt(8, DEFAULT_RECONNECT_POLICY)).toBe(DEFAULT_RECONNECT_POLICY.maxDelayMs)
    expect(delayForAttempt(20, DEFAULT_RECONNECT_POLICY)).toBe(
      DEFAULT_RECONNECT_POLICY.maxDelayMs,
    )
  })

  it('负数不当成有效次数', () => {
    expect(delayForAttempt(-1)).toBe(0)
  })
})

describe('resumePosition', () => {
  it('往前退一点，给用户接上剧情的上下文', () => {
    expect(resumePosition(600, 7200)).toBe(600 - RESUME_REWIND_SECONDS)
  })

  it('片头处不会变成负数', () => {
    // 在 1 秒处断网，退 2 秒会算出 -1 —— 传给播放器会出现奇怪行为。
    expect(resumePosition(1, 7200)).toBe(0)
    expect(resumePosition(0, 7200)).toBe(0)
  })

  it('不会退到片长之外', () => {
    expect(resumePosition(100, 100)).toBe(100 - RESUME_REWIND_SECONDS)
  })

  it('时长未知时不夹上限', () => {
    expect(resumePosition(600)).toBe(600 - RESUME_REWIND_SECONDS)
  })

  it('非法位置归 0', () => {
    expect(resumePosition(Number.NaN, 7200)).toBe(0)
    expect(resumePosition(-50, 7200)).toBe(0)
  })
})

describe('RecoveryCoordinator', () => {
  it('网络错误安排重试，解码错误直接放弃', () => {
    const c = new RecoveryCoordinator()
    expect(c.onError('network')).toBe(true)
    expect(c.state).toBe('waiting')

    const c2 = new RecoveryCoordinator()
    expect(c2.onError('decode')).toBe(false)
    expect(c2.state).toBe('gave-up')
  })

  it('次数用尽后放弃，不再无限重试', () => {
    const c = new RecoveryCoordinator({ maxAttempts: 2, baseDelayMs: 10, maxDelayMs: 100 })
    expect(c.onError('network')).toBe(true)
    c.beginAttempt()
    expect(c.onError('network')).toBe(true)
    c.beginAttempt()
    expect(c.onError('network')).toBe(false)
    expect(c.state).toBe('gave-up')
    expect(c.attempt).toBe(2)
  })

  it('等待时长随次数增长', () => {
    const c = new RecoveryCoordinator({ maxAttempts: 5, baseDelayMs: 100, maxDelayMs: 10_000 })
    c.onError('network')
    expect(c.nextDelayMs()).toBe(100)
    c.beginAttempt()
    expect(c.nextDelayMs()).toBe(200)
    c.beginAttempt()
    expect(c.nextDelayMs()).toBe(400)
  })

  it('断网时即使还没报错也进入等待 —— 有些切换不会立刻报错', () => {
    const c = new RecoveryCoordinator()
    expect(c.state).toBe('idle')
    c.onNetworkChange(false)
    expect(c.state).toBe('waiting')
    expect(c.online).toBe(false)
  })

  it('网络恢复时提示可以立刻重试，不必等完退避计时', () => {
    const c = new RecoveryCoordinator()
    c.onError('network')
    c.onNetworkChange(false)
    expect(c.onNetworkChange(true)).toBe(true)
    expect(c.online).toBe(true)
  })

  it('一直在线且没出错时不骚扰调用方', () => {
    const c = new RecoveryCoordinator()
    expect(c.onNetworkChange(true)).toBe(false)
  })

  it('播放成功后归零，之前的重试不计入下一部片', () => {
    const c = new RecoveryCoordinator()
    c.onError('network')
    c.beginAttempt()
    c.beginAttempt()
    c.markHealthy()
    expect(c.state).toBe('idle')
    expect(c.attempt).toBe(0)
  })

  it('beginAttempt 返回续播位置', () => {
    const c = new RecoveryCoordinator()
    c.recordPosition(600, 7200)
    c.onError('network')
    expect(c.beginAttempt()).toBe(600 - RESUME_REWIND_SECONDS)
  })

  it('没记过位置就从头开始，不返回 NaN', () => {
    const c = new RecoveryCoordinator()
    c.onError('network')
    expect(c.beginAttempt()).toBe(0)
  })

  it('reset 清空一切', () => {
    const c = new RecoveryCoordinator()
    c.recordPosition(999, 1000)
    c.onError('network')
    c.beginAttempt()
    c.reset()
    expect(c.state).toBe('idle')
    expect(c.attempt).toBe(0)
  })

  it('典型流程：断网 → 等 → 恢复 → 续播', () => {
    const c = new RecoveryCoordinator({ maxAttempts: 3, baseDelayMs: 500, maxDelayMs: 8000 })
    c.recordPosition(1234, 7200)
    // 1. 播放器报网络错误
    expect(c.onError('network')).toBe(true)
    expect(c.nextDelayMs()).toBe(500)
    // 2. 系统报告离线，随后又在线
    c.onNetworkChange(false)
    expect(c.onNetworkChange(true)).toBe(true)
    // 3. 发起重试，拿到续播位置
    const at = c.beginAttempt()
    expect(at).toBe(1234 - RESUME_REWIND_SECONDS)
    // 4. 这次成功了
    c.markHealthy()
    expect(c.state).toBe('idle')
  })
})
