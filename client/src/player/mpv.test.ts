/**
 * mpv 后端的测试。
 *
 * 全部走 `FakeBridge`，**不需要真的装 mpv**：这一层的职责就是把
 * IPC 事件翻译成 core 的 patch/emit，是纯翻译逻辑。
 * 真正需要 mpv 才能验证的只有"mpv 真的被拉起来了、IPC 真的通了"，
 * 那属于冒烟测试，不在这里。
 */

import { describe, expect, it, vi } from 'vitest'
import { MpvPlayer, MPV_CLOSED_CHANNEL, MPV_EVENT_CHANNEL, type MpvBridge } from './mpv'
import { capabilitiesFor } from './capabilities'
import type { MpvEventPayload } from './mpv'

interface Call {
  cmd: string
  args?: Record<string, unknown>
}

class FakeBridge implements MpvBridge {
  calls: Call[] = []
  private handlers = new Map<string, Set<(p: unknown) => void>>()
  /** 让这些命令抛错，用来测失败路径。 */
  failing = new Set<string>()

  async invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    this.calls.push({ cmd, args })
    if (this.failing.has(cmd)) throw new Error(`${cmd} 失败`)
    return 'mpv-1' as unknown as T
  }

  async listen<T = unknown>(event: string, handler: (p: T) => void): Promise<() => void> {
    const set = this.handlers.get(event) ?? new Set()
    set.add(handler as (p: unknown) => void)
    this.handlers.set(event, set)
    return () => set.delete(handler as (p: unknown) => void)
  }

  emit(event: string, payload: unknown): void {
    for (const h of this.handlers.get(event) ?? []) h(payload)
  }

  listenerCount(event: string): number {
    return this.handlers.get(event)?.size ?? 0
  }

  last(cmd: string): Call | undefined {
    return [...this.calls].reverse().find((c) => c.cmd === cmd)
  }
}

/** 等一下微任务：`requestTrackList` 是 fire-and-forget 的。 */
const flush = () => new Promise((r) => setTimeout(r, 0))

async function makePlayer(): Promise<{ p: MpvPlayer; b: FakeBridge; el: HTMLElement }> {
  const b = new FakeBridge()
  const p = new MpvPlayer({ bridge: b })
  const el = document.createElement('div')
  document.body.appendChild(el)
  await p.initialize({ container: el })
  return { p, b, el }
}

function ev(e: Partial<MpvEventPayload>): MpvEventPayload {
  return { name: 'unknown', ...e }
}

describe('MpvPlayer · 能力与初始化', () => {
  it('能力跟着后端走，是 tauri-mpv 档', () => {
    const p = new MpvPlayer({ bridge: new FakeBridge() })
    expect(p.capabilities.clientId).toBe('tauri-mpv')
    expect(p.capabilities).toEqual(capabilitiesFor('mpv'))
  })

  it('initialize 会订阅事件与关闭两个通道', async () => {
    const { b } = await makePlayer()
    expect(b.listenerCount(MPV_EVENT_CHANNEL)).toBe(1)
    expect(b.listenerCount(MPV_CLOSED_CHANNEL)).toBe(1)
  })

  it('initialize 幂等：重复调用不会重复订阅', async () => {
    const { p, b, el } = await makePlayer()
    await p.initialize({ container: el })
    expect(b.listenerCount(MPV_EVENT_CHANNEL)).toBe(1)
  })

  it('画面在自己窗口里，容器内放的是说明而不是 video 元素', async () => {
    const { el } = await makePlayer()
    expect(el.querySelector('video')).toBeNull()
    expect(el.textContent).toContain('mpv')
  })

  it('没初始化就 load 会明确报错，而不是静默什么都不做', async () => {
    const p = new MpvPlayer({ bridge: new FakeBridge() })
    await p.load({ kind: 'progressive', url: 'http://x/a.mkv' })
    expect(p.state.state).toBe('error')
    expect(p.state.error?.message).toContain('未初始化')
  })
})

describe('MpvPlayer · load', () => {
  it('把 URL 与续播起点交给 mpv_start', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' }, { startAt: 612.5 })
    const call = b.last('mpv_start')
    expect(call?.args?.args).toMatchObject({ url: 'http://srv/a.mkv', start_seconds: 612.5 })
  })

  it('本地文件用 path 而不是 url', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'local-file', path: 'D:\\media\\a.mkv' })
    expect(b.last('mpv_start')?.args?.args).toMatchObject({ url: 'D:\\media\\a.mkv' })
  })

  it('外挂字幕按启动参数带上（启动后 sub-add 会有空窗）', async () => {
    const { p, b } = await makePlayer()
    await p.load(
      { kind: 'progressive', url: 'http://srv/a.mkv' },
      { externalSubtitles: [{ path: 'D:\\a.srt', language: 'chi', label: '中文' }] },
    )
    const args = b.last('mpv_start')?.args?.args as { subtitles: unknown[] }
    expect(args.subtitles).toEqual([{ path: 'D:\\a.srt', lang: 'chi' }])
  })

  it('启动失败进错误态，错误信息要能看懂', async () => {
    const { p, b } = await makePlayer()
    b.failing.add('mpv_start')
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    expect(p.state.state).toBe('error')
    expect(p.state.error?.message).toContain('启动 mpv 失败')
  })
})

describe('MpvPlayer · 事件翻译', () => {
  it('file-loaded 会去取轨道列表，并带上 request_id', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    b.emit(MPV_EVENT_CHANNEL, ev({ name: 'file-loaded' }))
    await flush()

    const call = b.last('mpv_send')
    expect(call?.args?.command).toEqual(['get_property', 'track-list'])
    expect(call?.args?.request_id).toBe(1)
    expect(p.state.audioTracks).toEqual([])
  })

  it('轨道列表按 request_id 对号，并分出音轨/字幕轨与当前选中', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    b.emit(MPV_EVENT_CHANNEL, ev({ name: 'file-loaded' }))
    await flush()

    b.emit(
      MPV_EVENT_CHANNEL,
      ev({
        name: 'response',
        request_id: 1,
        error: 'success',
        data: [
          { id: 1, type: 'video', title: 'video' },
          { id: 2, type: 'audio', lang: 'jpn', selected: true },
          { id: 3, type: 'audio', lang: 'chi' },
          { id: 4, type: 'sub', lang: 'chi', selected: true },
        ],
      }),
    )

    expect(p.state.audioTracks.map((t) => t.id)).toEqual(['2', '3'])
    expect(p.state.subtitleTracks.map((t) => t.id)).toEqual(['4'])
    expect(p.state.currentAudioTrackId).toBe('2')
    expect(p.state.currentSubtitleTrackId).toBe('4')
  })

  it('响应里 request_id 对不上就不认（避免串台）', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    b.emit(MPV_EVENT_CHANNEL, ev({ name: 'file-loaded' }))
    await flush()

    b.emit(
      MPV_EVENT_CHANNEL,
      ev({ name: 'response', request_id: 99, error: 'success', data: [{ id: 1, type: 'audio' }] }),
    )
    expect(p.state.audioTracks).toEqual([])
  })

  it('time-pos 更新位置并触发 timeupdate', async () => {
    const { p, b } = await makePlayer()
    const events: string[] = []
    p.on((e) => events.push(e.type))
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })

    b.emit(
      MPV_EVENT_CHANNEL,
      ev({ name: 'property-change', property: 'time-pos', data: 12.5 }),
    )
    expect(p.state.position).toBe(12.5)
    expect(events).toContain('timeupdate')
  })

  it('duration 为 null 时不把时长写成 0', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    b.emit(MPV_EVENT_CHANNEL, ev({ name: 'property-change', property: 'duration', data: 7200 }))
    expect(p.state.duration).toBe(7200)
    b.emit(MPV_EVENT_CHANNEL, ev({ name: 'property-change', property: 'duration', data: null }))
    expect(p.state.duration).toBe(7200)
  })

  it('pause 属性变化驱动 state，而不是靠前端猜', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    b.emit(MPV_EVENT_CHANNEL, ev({ name: 'property-change', property: 'pause', data: true }))
    expect(p.state.state).toBe('paused')
    b.emit(MPV_EVENT_CHANNEL, ev({ name: 'property-change', property: 'pause', data: false }))
    expect(p.state.state).toBe('playing')
  })

  it('end-file 的 eof 才算播完，error 要报错误', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    b.emit(MPV_EVENT_CHANNEL, ev({ name: 'end-file', reason: 'error' }))
    expect(p.state.state).toBe('error')
    expect(p.state.error?.code).toBe('decode')

    const q = new MpvPlayer({ bridge: b })
    const el = document.createElement('div')
    await q.initialize({ container: el })
    await q.load({ kind: 'progressive', url: 'http://srv/b.mkv' })
    b.emit(MPV_EVENT_CHANNEL, ev({ name: 'end-file', reason: 'eof' }))
    expect(q.state.state).toBe('ended')
  })

  it('用户关掉 mpv 窗口是 idle，不是 ended', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    b.emit(MPV_EVENT_CHANNEL, ev({ name: 'property-change', property: 'pause', data: false }))
    b.emit(MPV_CLOSED_CHANNEL, null)
    // 报 ended 会让 UI 弹"下一集"，而用户只是关了窗口
    expect(p.state.state).toBe('idle')
  })
})

describe('MpvPlayer · 控制', () => {
  it('seek 用 absolute+exact，拖到哪就是哪', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    await p.seek(754.2)
    expect(b.last('mpv_send')?.args?.command).toEqual(['seek', 754.2, 'absolute+exact'])
  })

  it('音量从 0..1 转成 mpv 的 0..100', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    await p.setVolume(0.35)
    expect(b.last('mpv_send')?.args?.command).toEqual(['set_property', 'volume', 35])
    expect(p.state.volume).toBeCloseTo(0.35)
  })

  it('关字幕发 sid=no', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    await p.selectSubtitleTrack(null)
    expect(b.last('mpv_send')?.args?.command).toEqual(['set_property', 'sid', 'no'])
    expect(p.state.currentSubtitleTrackId).toBeNull()
  })

  it('切音轨发数字轨号', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    await p.selectAudioTrack('3')
    expect(b.last('mpv_send')?.args?.command).toEqual(['set_property', 'aid', 3])
  })

  it('画质档不存在：静默返回而不是报错', async () => {
    const { p } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    await expect(p.selectQuality('1080')).resolves.toBeUndefined()
    expect(p.state.state).not.toBe('error')
  })

  it('单条命令失败不该把播放器打进错误态', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    b.failing.add('mpv_send')
    await p.play()
    expect(p.state.state).not.toBe('error')
    expect(warn).toHaveBeenCalled()
    warn.mockRestore()
  })

  it('全屏全的是 mpv 自己的窗口', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    await p.setFullscreen(true)
    expect(b.last('mpv_send')?.args?.command).toEqual(['set_property', 'fullscreen', true])
  })
})

describe('MpvPlayer · 收尾', () => {
  it('stop 只停进程并回到 idle，实例还能再用', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    await p.stop()
    expect(b.last('mpv_stop')).toBeDefined()
    expect(p.state.state).toBe('idle')
  })

  it('换源时先停掉上一个进程，否则会叠两个 mpv', async () => {
    const { p, b } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    await p.load({ kind: 'progressive', url: 'http://srv/b.mkv' })
    const stops = b.calls.filter((c) => c.cmd === 'mpv_stop').length
    expect(stops).toBe(1)
  })

  it('destroy 停进程、退订、清占位', async () => {
    const { p, b, el } = await makePlayer()
    await p.load({ kind: 'progressive', url: 'http://srv/a.mkv' })
    await p.destroy()
    expect(b.last('mpv_stop')).toBeDefined()
    expect(b.listenerCount(MPV_EVENT_CHANNEL)).toBe(0)
    expect(el.children.length).toBe(0)
  })

  it('destroy 之后的事件不再动状态', async () => {
    const { p, b } = await makePlayer()
    await p.destroy()
    const before = p.state.state
    b.emit(MPV_EVENT_CHANNEL, ev({ name: 'property-change', property: 'time-pos', data: 99 }))
    expect(p.state.state).toBe(before)
  })
})
