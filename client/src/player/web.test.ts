/**
 * Web 后端的测试（jsdom）。
 *
 * jsdom 里 `<video>` 不会真的播，但**事件翻译**是可以验的 ——
 * 手动 dispatch 一个 `loadedmetadata` / `error`，断言状态机收到了什么。
 * 这覆盖了后端里唯一有逻辑的部分（映射与边界），剩下的本来就是浏览器的事。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { WebPlayer, type HlsConstructor, type HlsLike } from './web'
import type { PlayerCapabilities, PlayerEvent } from './types'

const caps: PlayerCapabilities = {
  clientId: 'web',
  containers: ['mp4', 'webm'],
  videoCodecs: ['h264', 'vp9'],
  audioCodecs: ['aac'],
  subtitleCodecs: ['vtt'],
  bitmapSubtitles: false,
  hls: true,
  localFile: false,
  canSwitchAudioTrack: false,
  canSwitchSubtitleTrack: true,
}

// jsdom 没有实现播放相关方法，先 stub 掉，否则会抛 "Not implemented"。
// currentTime 在 jsdom 里是只读的 0，seek 测试会永远失败，这里改成可写。
const currentTimes = new WeakMap<HTMLMediaElement, number>()

beforeEach(() => {
  document.body.innerHTML = ''
  Object.defineProperty(HTMLMediaElement.prototype, 'play', {
    configurable: true,
    value: vi.fn().mockResolvedValue(undefined),
  })
  Object.defineProperty(HTMLMediaElement.prototype, 'pause', {
    configurable: true,
    value: vi.fn(),
  })
  Object.defineProperty(HTMLMediaElement.prototype, 'load', {
    configurable: true,
    value: vi.fn(),
  })
  Object.defineProperty(HTMLMediaElement.prototype, 'currentTime', {
    configurable: true,
    get(this: HTMLMediaElement) {
      return currentTimes.get(this) ?? 0
    },
    set(this: HTMLMediaElement, v: number) {
      currentTimes.set(this, v)
    },
  })
})

function makeContainer(): HTMLElement {
  const el = document.createElement('div')
  document.body.appendChild(el)
  return el
}

/**
 * 建一个已经 initialize 的播放器，**并把容器一起返回**。
 *
 * 返回容器的原因：测试之间共享 `document.body`，用
 * `document.body.querySelector('video')` 会拿到上一个测试留下的元素 ——
 * 这个坑已经踩过一次了，所以显式持有自己的容器。
 */
async function player(
  opts: { hlsCtor?: HlsConstructor } = {},
): Promise<{ p: WebPlayer; container: HTMLElement; video: HTMLVideoElement }> {
  const container = makeContainer()
  const p = new WebPlayer({ capabilities: caps, ...opts })
  await p.initialize({ container })
  return { p, container, video: container.querySelector('video') as HTMLVideoElement }
}

class FakeHls implements HlsLike {
  static instances: FakeHls[] = []
  levels = [
    { height: 1080, bitrate: 5_000_000 },
    { height: 720, bitrate: 2_500_000 },
  ]
  currentLevel = -1
  destroyed = false
  attached: HTMLVideoElement | null = null
  source: string | null = null
  private handlers = new Map<string, ((...args: unknown[]) => void)[]>()

  constructor(_config?: Record<string, unknown>) {
    FakeHls.instances.push(this)
  }
  loadSource(url: string): void {
    this.source = url
  }
  attachMedia(v: HTMLVideoElement): void {
    this.attached = v
  }
  destroy(): void {
    this.destroyed = true
  }
  on(event: string, cb: (...args: unknown[]) => void): void {
    const list = this.handlers.get(event) ?? []
    list.push(cb)
    this.handlers.set(event, list)
  }
  off(event: string, cb: (...args: unknown[]) => void): void {
    const list = this.handlers.get(event) ?? []
    this.handlers.set(
      event,
      list.filter((h) => h !== cb),
    )
  }
  /** 测试用：手动触发一个事件。 */
  fire(event: string, data?: unknown): void {
    for (const h of this.handlers.get(event) ?? []) h(event, data)
  }
}

describe('WebPlayer', () => {
  it('initialize 会创建 video 元素并挂进容器', async () => {
    const container = makeContainer()
    const p = new WebPlayer({ capabilities: caps })

    await p.initialize({ container })

    const video = container.querySelector('video')
    expect(video).not.toBeNull()
    expect(video!.controls).toBe(false) // 控制条自己画
    expect(video!.playsInline).toBe(true)
  })

  it('initialize 幂等：重复调用不会塞两个 video', async () => {
    const container = makeContainer()
    const p = new WebPlayer({ capabilities: caps })

    await p.initialize({ container })
    await p.initialize({ container })

    expect(container.querySelectorAll('video')).toHaveLength(1)
  })

  it('load progressive 会把 URL 设到 src 并进入 loading', async () => {
    const { p, video } = await player()

    await p.load({ kind: 'progressive', url: '/api/v1/stream/abc' })

    expect(p.state.state).toBe('loading')
    expect(video.getAttribute('src')).toBe('/api/v1/stream/abc')
  })

  it('load 本地文件路径会报 decode 错误而不是静默', async () => {
    const { p } = await player()
    const errors: PlayerEvent[] = []
    p.on((e) => errors.push(e))

    await p.load({ kind: 'local-file', path: 'D:\\media\\a.mkv' })

    expect(p.state.state).toBe('error')
    expect(p.state.error?.code).toBe('decode')
    expect(errors.some((e) => e.type === 'error')).toBe(true)
  })

  it('load HLS 会走 hls.js 并 attach 到 video', async () => {
    FakeHls.instances = []
    const { p } = await player({ hlsCtor: FakeHls as unknown as HlsConstructor })

    await p.load({ kind: 'hls', url: '/api/v1/hls/x/master.m3u8' })

    const hls = FakeHls.instances[0]
    expect(hls).toBeDefined()
    expect(hls!.source).toBe('/api/v1/hls/x/master.m3u8')
    expect(hls!.attached).not.toBeNull()
  })

  it('HLS 清单解析后暴露画质档', async () => {
    FakeHls.instances = []
    const { p } = await player({ hlsCtor: FakeHls as unknown as HlsConstructor })
    await p.load({ kind: 'hls', url: '/x/master.m3u8' })

    FakeHls.instances[0]!.fire('hlsManifestParsed')

    expect(p.state.qualities.map((q) => q.label)).toEqual(['1080p', '720p'])
  })

  it('HLS 致命网络错误会映射成 network，非致命的忽略', async () => {
    FakeHls.instances = []
    const { p } = await player({ hlsCtor: FakeHls as unknown as HlsConstructor })
    await p.load({ kind: 'hls', url: '/x/master.m3u8' })

    FakeHls.instances[0]!.fire('hlsError', { fatal: false, type: 'networkError' })
    expect(p.state.state).not.toBe('error')

    FakeHls.instances[0]!.fire('hlsError', { fatal: true, type: 'networkError' })
    expect(p.state.state).toBe('error')
    expect(p.state.error?.code).toBe('network')
  })

  it('音轨切换是空操作（浏览器做不到，UI 不该给按钮）', async () => {
    const { p } = await player()
    await p.load({ kind: 'progressive', url: '/a.mp4' })

    await p.selectAudioTrack('2')

    expect(p.state.currentAudioTrackId).toBeNull()
    expect(caps.canSwitchAudioTrack).toBe(false)
  })

  it('startAt 会在元数据到手后设成 currentTime', async () => {
    const { p, video } = await player()
    await p.load({ kind: 'progressive', url: '/a.mp4' }, { startAt: 600 })

    Object.defineProperty(video, 'duration', { configurable: true, value: 7200 })
    video.dispatchEvent(new Event('loadedmetadata'))

    expect(video.currentTime).toBe(600)
    expect(p.state.duration).toBe(7200)
    expect(p.state.state).toBe('ready')
  })

  it('媒体错误按 code 映射，ABORTED(1) 不算错误', async () => {
    const { p, video } = await player()
    await p.load({ kind: 'progressive', url: '/a.mp4' })

    Object.defineProperty(video, 'error', { configurable: true, value: { code: 1 } })
    video.dispatchEvent(new Event('error'))
    expect(p.state.state).not.toBe('error')

    Object.defineProperty(video, 'error', { configurable: true, value: { code: 2 } })
    video.dispatchEvent(new Event('error'))
    expect(p.state.error?.code).toBe('network')

    Object.defineProperty(video, 'error', { configurable: true, value: { code: 3 } })
    video.dispatchEvent(new Event('error'))
    expect(p.state.error?.code).toBe('decode')
  })

  it('自动播放被拦截时报 autoplay-blocked 而不是假装成功', async () => {
    const { p } = await player()
    Object.defineProperty(HTMLMediaElement.prototype, 'play', {
      configurable: true,
      value: vi.fn().mockRejectedValue(Object.assign(new Error('blocked'), { name: 'NotAllowedError' })),
    })

    await p.play()

    expect(p.state.error?.code).toBe('autoplay-blocked')
  })

  it('音量会被夹到 0..1', async () => {
    const { p } = await player()

    await p.setVolume(2)
    expect(p.state.volume).toBe(1)
    await p.setVolume(-1)
    expect(p.state.volume).toBe(0)
  })

  it('destroy 后移除 video 并销毁 hls 实例', async () => {
    FakeHls.instances = []
    const container = makeContainer()
    const p = new WebPlayer({
      capabilities: caps,
      hlsCtor: FakeHls as unknown as HlsConstructor,
    })
    await p.initialize({ container })
    await p.load({ kind: 'hls', url: '/x/master.m3u8' })

    await p.destroy()

    expect(container.querySelector('video')).toBeNull()
    expect(FakeHls.instances[0]!.destroyed).toBe(true)
  })

  it('stop 后清空 src 但实例还在，可以再 load', async () => {
    const { p, video } = await player()
    await p.load({ kind: 'progressive', url: '/a.mp4' })

    await p.stop()
    expect(video.getAttribute('src')).toBeNull()
    expect(p.state.state).toBe('idle')

    await p.load({ kind: 'progressive', url: '/b.mp4' })
    expect(video.getAttribute('src')).toBe('/b.mp4')
  })
})
