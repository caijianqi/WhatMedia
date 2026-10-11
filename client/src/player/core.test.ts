/**
 * 状态机核心的测试。
 *
 * 这一层不需要浏览器 —— 这正是把它从平台后端里拆出来的理由。
 */

import { describe, expect, it, vi } from 'vitest'
import { PlayerCore, initialSnapshot } from './core'
import type { PlayerCapabilities } from './types'

const caps: PlayerCapabilities = {
  clientId: 'web',
  containers: ['mp4'],
  videoCodecs: ['h264'],
  audioCodecs: ['aac'],
  subtitleCodecs: ['vtt'],
  bitmapSubtitles: false,
  hls: true,
  localFile: false,
  canSwitchAudioTrack: false,
  canSwitchSubtitleTrack: true,
}

function core(): PlayerCore {
  return new PlayerCore(caps)
}

describe('PlayerCore', () => {
  it('初始状态是 idle 且没有错误', () => {
    const c = core()
    expect(c.state.state).toBe('idle')
    expect(c.state.error).toBeNull()
    expect(c.state.position).toBe(0)
  })

  it('patch 会通知订阅者', () => {
    const c = core()
    const listener = vi.fn()
    c.subscribe(listener)

    c.patch({ position: 12.5 })

    expect(listener).toHaveBeenCalledTimes(1)
    expect(c.state.position).toBe(12.5)
  })

  it('值没变就不通知 —— timeupdate 每秒来 4 次，不该每次都重渲染', () => {
    const c = core()
    const listener = vi.fn()
    c.patch({ position: 5 })
    c.subscribe(listener)

    c.patch({ position: 5 })
    c.patch({ position: 5 })

    expect(listener).not.toHaveBeenCalled()
  })

  it('状态变化会额外发 statechange 事件', () => {
    const c = core()
    const events: string[] = []
    c.on((e) => events.push(e.type))

    c.patch({ state: 'playing' })

    expect(events).toEqual(['statechange'])
  })

  it('statechange 带上 from 与 to', () => {
    const c = core()
    c.patch({ state: 'buffering' })
    let captured: { from: string; to: string } | null = null
    c.on((e) => {
      if (e.type === 'statechange') captured = { from: e.from, to: e.to }
    })

    c.patch({ state: 'playing' })

    expect(captured).toEqual({ from: 'buffering', to: 'playing' })
  })

  it('fail 置为 error 态并发出 error 事件', () => {
    const c = core()
    const events: { code: string; message: string }[] = []
    c.on((e) => {
      if (e.type === 'error') events.push(e.error)
    })

    c.fail('network', '断了')

    expect(c.state.state).toBe('error')
    expect(c.state.error?.code).toBe('network')
    expect(events).toHaveLength(1)
  })

  it('resetForLoad 保留音量静音速率，其余归零', () => {
    const c = core()
    c.patch({ volume: 0.3, muted: true, rate: 1.5, position: 900, state: 'ended' })

    c.resetForLoad()

    expect(c.state.volume).toBe(0.3)
    expect(c.state.muted).toBe(true)
    expect(c.state.rate).toBe(1.5)
    expect(c.state.position).toBe(0)
    expect(c.state.state).toBe('loading')
    expect(c.state.error).toBeNull()
  })

  it('取消订阅后就收不到通知', () => {
    const c = core()
    const listener = vi.fn()
    const unsub = c.subscribe(listener)

    unsub()
    c.patch({ position: 1 })

    expect(listener).not.toHaveBeenCalled()
  })

  it('destroy 之后的 patch 是空操作 —— 后端卸载后不该还在推状态', () => {
    const c = core()
    const listener = vi.fn()
    c.subscribe(listener)

    c.destroy()
    c.patch({ position: 42 })

    expect(listener).not.toHaveBeenCalled()
    expect(c.state.position).toBe(0)
  })

  it('initialSnapshot 每次返回新对象，不共享引用', () => {
    const a = initialSnapshot()
    const b = initialSnapshot()
    a.position = 99
    expect(b.position).toBe(0)
    expect(a.audioTracks).not.toBe(b.audioTracks)
  })
})
