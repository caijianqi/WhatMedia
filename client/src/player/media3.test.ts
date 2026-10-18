import { describe, expect, it } from 'vitest'
import { MEDIA3_EVENT_CHANNEL, Media3Player, type Media3Bridge, type Media3WireEvent } from './media3'
import { CLIENT_CAPABILITIES } from './capabilities'

/** 假 bridge：记录所有 invoke，允许主动推事件。 */
function fakeBridge() {
  const calls: Array<{ cmd: string; args: Record<string, unknown> }> = []
  const listeners = new Map<string, (p: unknown) => void>()
  const bridge: Media3Bridge = {
    invoke: async (cmd, args = {}) => {
      calls.push({ cmd, args })
      return undefined as never
    },
    listen: async (event, handler) => {
      listeners.set(event, handler as (p: unknown) => void)
      return () => listeners.delete(event)
    },
  }
  return {
    bridge,
    calls,
    emit(ev: Media3WireEvent) {
      const h = listeners.get(MEDIA3_EVENT_CHANNEL)
      h?.(ev)
    },
  }
}

function last<T>(arr: T[]): T | undefined {
  return arr.length ? arr[arr.length - 1] : undefined
}

function player() {
  const f = fakeBridge()
  const p = new Media3Player({ bridge: f.bridge, capabilities: CLIENT_CAPABILITIES['android-exoplayer'] })
  return { ...f, p }
}

describe('Media3Player 能力', () => {
  it('用 android-exoplayer 档，且能切音轨切字幕', () => {
    const { p } = player()
    expect(p.capabilities.clientId).toBe('android-exoplayer')
    expect(p.capabilities.canSwitchAudioTrack).toBe(true)
    expect(p.capabilities.canSwitchSubtitleTrack).toBe(true)
    // 位图字幕（PGS）不支持 —— 服务端必须烧录或转换，
    // 这条错了用户会看到"字幕加载了但一条都不显示"。
    expect(p.capabilities.bitmapSubtitles).toBe(false)
  })
})

describe('Media3Player 载入', () => {
  it('HLS 源带上 kind=hls 与起始位置', async () => {
    const { p, calls } = player()
    await p.load({ kind: 'hls', url: 'http://x/master.m3u8' }, { startAt: 30, autoplay: true })
    const load = calls.find((c) => c.cmd.endsWith('media3_load'))
    expect(load?.args).toMatchObject({ url: 'http://x/master.m3u8', kind: 'hls', startAt: 30, autoplay: true })
  })

  it('progressive 源不标成 hls', async () => {
    const { p, calls } = player()
    await p.load({ kind: 'progressive', url: 'http://x/a.mp4' })
    expect(calls.find((c) => c.cmd.endsWith('media3_load'))?.args).toMatchObject({ kind: 'progressive' })
  })

  it('外挂字幕转成 Kotlin 认得的形状，并猜 MIME', async () => {
    const { p, calls } = player()
    await p.load({ kind: 'hls', url: 'u' }, {
      externalSubtitles: [{ label: '中文', url: 'http://x/a.ass?t=1', language: 'chi', default: true }],
    })
    const subs = calls.find((c) => c.cmd.endsWith('media3_load'))?.args?.externalSubtitles as unknown[]
    expect(subs).toHaveLength(1)
    expect(subs[0]).toMatchObject({ label: '中文', language: 'chi', default: true, mime: 'text/x-ssa' })
  })

  it('字幕 URL 缺失时给空串而不是 undefined —— Kotlin 侧会直接崩', async () => {
    const { p, calls } = player()
    await p.load({ kind: 'hls', url: 'u' }, { externalSubtitles: [{ label: 'x' }] })
    const subs = calls.find((c) => c.cmd.endsWith('media3_load'))?.args?.externalSubtitles as Array<Record<string, unknown>>
    expect(typeof subs[0]!.url).toBe('string')
  })
})

describe('Media3Player 事件翻译', () => {
  it('state-change 落到核心状态机', async () => {
    const { p, emit } = player()
    await p.initialize({ container: document.createElement('div') })
    emit({ type: 'state-change', from: 'loading', to: 'playing' } as Media3WireEvent)
    expect(p.state.state).toBe('playing')
  })

  it('未知状态字面量不改状态 —— 不猜', async () => {
    const { p, emit } = player()
    await p.initialize({ container: document.createElement('div') })
    const before = p.state.state
    emit({ type: 'state-change', from: 'loading', to: 'wat' } as unknown as Media3WireEvent)
    expect(p.state.state).toBe(before)
  })

  it('错误码映射到前端错误码，未知的归 unknown', async () => {
    const { p, emit } = player()
    await p.initialize({ container: document.createElement('div') })
    emit({ type: 'error', error: { code: 'network', message: '断网' } })
    expect(p.state.state).toBe('error')
    expect(p.state.error?.code).toBe('network')

    const q = player()
    await q.p.initialize({ container: document.createElement('div') })
    q.emit({ type: 'error', error: { code: '???', message: '' } })
    expect(q.p.state.error?.code).toBe('unknown')
  })

  it('time-update 同时更新位置与时长，并发 timeupdate 事件', async () => {
    const { p, emit } = player()
    await p.initialize({ container: document.createElement('div') })
    const events: string[] = []
    p.on((e) => events.push(e.type))
    emit({ type: 'time-update', position: 12.5, duration: 7200 })
    expect(p.state.position).toBe(12.5)
    expect(p.state.duration).toBe(7200)
    expect(events).toContain('timeupdate')
  })

  it('buffering 补发 waiting —— 与首次起播要区分开', async () => {
    const { p, emit } = player()
    await p.initialize({ container: document.createElement('div') })
    const events: string[] = []
    p.on((e) => events.push(e.type))
    emit({ type: 'state-change', from: 'playing', to: 'buffering' })
    expect(events).toContain('waiting')
    expect(p.state.state).toBe('buffering')
  })
})

describe('Media3Player 控制', () => {
  it('seek 负数归 0，NaN 也归 0', async () => {
    const { p, calls } = player()
    await p.seek(-5)
    expect(last(calls)?.args).toMatchObject({ position: 0 })
    await p.seek(Number.NaN)
    expect(last(calls)?.args).toMatchObject({ position: 0 })
  })

  it('音量夹在 0..1', async () => {
    const { p, calls } = player()
    await p.setVolume(3)
    expect(last(calls)?.args).toMatchObject({ volume: 1 })
    await p.setVolume(-1)
    expect(last(calls)?.args).toMatchObject({ volume: 0 })
  })

  it('速率夹在 0.25..4 —— 超出范围 ExoPlayer 会抛异常', async () => {
    const { p, calls } = player()
    await p.setRate(100)
    expect(last(calls)?.args).toMatchObject({ rate: 4 })
    await p.setRate(0)
    expect(last(calls)?.args).toMatchObject({ rate: 0.25 })
  })

  it('关闭字幕传 null，不是空串', async () => {
    const { p, calls } = player()
    await p.selectSubtitleTrack(null)
    expect(last(calls)?.args).toMatchObject({ id: null })
  })

  it('全屏静默成功 —— Android 上没有 DOM 全屏这回事，报错只会污染日志', async () => {
    const { p } = player()
    await expect(p.setFullscreen(true)).resolves.toBeUndefined()
  })
})

describe('Media3Player 断网续播', () => {
  it('reloadFromLastPosition 用上次的源与位置，且保留用户选过的轨道', async () => {
    const { p, calls } = player()
    await p.load({ kind: 'hls', url: 'http://x/m.m3u8' }, { startAt: 0 })
    await p.seek(600)
    await p.selectAudioTrack('g0:t1')
    calls.length = 0

    await p.reloadFromLastPosition()
    const load = calls.find((c) => c.cmd.endsWith('media3_load'))
    expect(load?.args).toMatchObject({ url: 'http://x/m.m3u8', startAt: 600, autoplay: true })
    // 重连不该把用户选的音轨冲掉 —— 这里没有重新 selectAudioTrack 调用
    expect(calls.some((c) => c.cmd.endsWith('media3_select_audio'))).toBe(false)
  })

  it('没载入过就调用 reload 不炸', async () => {
    const { p } = player()
    await expect(p.reloadFromLastPosition()).resolves.toBeUndefined()
  })
})

describe('Media3Player 桥不可用时', () => {
  it('不在 Tauri 壳里时载入失败并给出错误，而不是静默无反应', async () => {
    // 没有注入 bridge，也没有 window.__TAURI_INTERNALS__
    const p = new Media3Player({ capabilities: CLIENT_CAPABILITIES['android-exoplayer'] })
    await p.load({ kind: 'hls', url: 'u' })
    expect(p.state.state).toBe('error')
  })
})

describe('命令名', () => {
  it('一律走 plugin:media3| 前缀 —— 漏了前缀是 "command not found"', async () => {
    const { p, calls } = player()
    await p.play()
    expect(last(calls)?.cmd).toBe('plugin:media3|media3_play')
  })
})

describe('销毁', () => {
  it('销毁后再 load 不做事，且取消订阅', async () => {
    const { p, calls, emit } = player()
    await p.initialize({ container: document.createElement('div') })
    await p.destroy()
    calls.length = 0
    await p.load({ kind: 'hls', url: 'u' })
    expect(calls).toHaveLength(0)
    // 销毁后推事件也不该改状态
    emit({ type: 'state-change', from: 'idle', to: 'playing' })
    expect(p.state.state).toBe('idle')
  })

  it('destroy 是幂等的', async () => {
    const { p } = player()
    await p.initialize({ container: document.createElement('div') })
    await p.destroy()
    await expect(p.destroy()).resolves.toBeUndefined()
  })
})

describe('轨道刷新（两趟 IPC）', () => {
  it('先问 Kotlin 要原值，再让 Rust 生成带 label 的 Track', async () => {
    const f = fakeBridge()
    // 第一趟：Kotlin 的原值（没有 label）
    // 第二趟：Rust 翻译后的成品
    const tracksJson = JSON.stringify({
      audio: [{ group: 0, index: 0, language: 'eng', selected: true }],
      text: [{ group: 1, index: 0, language: 'chi' }],
      video: [{ group: 2, index: 0, height: 1080, selected: true }],
    })
    const typed = {
      audioTracks: [{ id: 'g0:t0', label: '英语' }],
      subtitleTracks: [{ id: 'g1:t0', label: '中文' }],
      qualities: [{ id: 'g2:t0', label: '1080P' }],
      currentAudioTrackId: 'g0:t0',
      currentSubtitleTrackId: null,
      currentQualityId: 'g2:t0',
    }
    const bridge: Media3Bridge = {
      invoke: async (cmd, args = {}) => {
        f.calls.push({ cmd, args })
        if (cmd.endsWith('media3_status')) return { state: 'playing', tracks: tracksJson } as never
        if (cmd.endsWith('media3_snapshot')) return typed as never
        return undefined as never
      },
      listen: f.bridge.listen,
    }
    const p = new Media3Player({ bridge, capabilities: CLIENT_CAPABILITIES['android-exoplayer'] })
    await p.initialize({ container: document.createElement('div') })

    f.emit({ type: 'tracks-listed' })
    // 两趟 IPC 都是异步的，等一轮微任务
    await Promise.resolve()
    await Promise.resolve()
    await Promise.resolve()

    expect(p.state.audioTracks[0]?.label).toBe('英语')
    expect(p.state.subtitleTracks[0]?.label).toBe('中文')
    expect(p.state.qualities[0]?.label).toBe('1080P')
  })

  it('拉轨道失败不把播放器打进错误态 —— 画面和声音还正常', async () => {
    const { p, emit } = player()
    await p.initialize({ container: document.createElement('div') })
    emit({ type: 'tracks-listed' })
    await Promise.resolve()
    await Promise.resolve()
    await Promise.resolve()
    expect(p.state.state).not.toBe('error')
  })
})

describe('通知栏元数据', () => {
  it('缺省字段补成 Kotlin 侧认得的类型，不给 undefined', async () => {
    const { p, calls } = player()
    await p.setMetadata({ title: '星际穿越' })
    expect(last(calls)?.args).toMatchObject({ title: '星际穿越', subtitle: '', artworkUrl: null, durationMs: 0 })
  })
})
