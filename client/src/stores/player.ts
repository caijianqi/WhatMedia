/**
 * 播放会话 store。
 *
 * 这是三个东西的交汇点：播放器抽象、进度上报策略、服务端会话。
 * 页面只跟这个 store 打交道，不直接碰 `MediaPlayer` 也不直接发 heartbeat。
 *
 * 集中接线的原因：进度上报这类事**漏一处就丢数据**（用户拖到一半关页面，
 * 续播点退回十分钟），分散在各页面里迟早会有一处忘掉。放这里只有一份。
 */

import { defineStore } from 'pinia'
import { computed, ref, shallowRef } from 'vue'
import {
  ProgressReporter,
  createPlayer,
  initialSnapshot,
  resolveBackend,
  type ExternalSubtitle,
  type MediaPlayer,
  type PlaybackSource,
  type PlayerSnapshot,
  type PlayerTarget,
} from '../player'
import * as api from '../api/client'
import type { PlaybackStrategy, ReasonView, SubtitleView } from '../api/client'

/**
 * 从 `/api/v1/hls/{job}/master.m3u8` 里取出 job id。
 *
 * 服务端只给了完整 URL，而停止转码要的是 job id。解析而不是另存一份，
 * 是为了避免"URL 和 job id 对不上"这类状态不一致。
 */
function hlsJobId(url: string | null | undefined): string | null {
  if (!url) return null
  const m = /\/hls\/([^/]+)\/master\.m3u8/.exec(url)
  return m ? (m[1] ?? null) : null
}

export const usePlayerStore = defineStore('player', () => {
  // shallowRef：播放器实例不需要深层响应式，深代理一个带 DOM 引用的对象
  // 既浪费又容易引发奇怪的依赖追踪。
  const player = shallowRef<MediaPlayer | null>(null)

  const snapshot = ref<PlayerSnapshot>(initialSnapshot())
  const sessionId = ref<string | null>(null)
  const strategy = ref<PlaybackStrategy | null>(null)
  const reasons = ref<ReasonView[]>([])
  const subtitles = ref<SubtitleView[]>([])
  const busy = ref(false)
  const error = ref<string | null>(null)

  /** HLS 任务 id。关闭时要显式停掉，否则 ffmpeg 会一直跑到空闲回收。 */
  let hlsJob: string | null = null
  let reporter: ProgressReporter | null = null
  let unsubState: (() => void) | null = null
  let unsubEvent: (() => void) | null = null

  const canSwitchAudio = computed(() => player.value?.capabilities.canSwitchAudioTrack ?? false)
  const canSwitchSubtitle = computed(
    () => player.value?.capabilities.canSwitchSubtitleTrack ?? false,
  )
  const hasQualities = computed(() => snapshot.value.qualities.length > 0)

  /**
   * 打开一个条目。
   *
   * 顺序有讲究：**先建会话再建播放器**。
   * 会话会顺带启动转码任务，而播放器拿到的是转码输出地址 ——
   * 反过来做的话播放器会对一个还不存在的地址发请求。
   */
  async function open(
    target: PlayerTarget,
    itemId: string,
    opts: { startMs?: number; autoplay?: boolean; fileId?: string; client?: string } = {},
  ): Promise<void> {
    busy.value = true
    error.value = null
    try {
      // 后端决定能力档，也就决定了服务端怎么决策 —— 所以必须先定后端，
      // 再用同一个判断去建会话。反过来的话会出现"服务端按 mpv 给了 MKV，
      // 播放器却是 Web"这种无处报错的错配。
      const backend = await resolveBackend()

      const view = await api.createSession({
        item_id: itemId,
        file_id: opts.fileId ?? null,
        client: opts.client ?? (backend === 'mpv' ? 'tauri-mpv' : 'web'),
        start_ms: opts.startMs ?? 0,
      })

      sessionId.value = view.session_id
      strategy.value = view.strategy
      reasons.value = view.reasons
      subtitles.value = view.subtitles
      hlsJob = hlsJobId(view.hls_url)

      const source = pickSource(view)
      if (!source) {
        error.value = view.reason || '服务端没有给出可用的播放地址'
        return
      }

      const p = createPlayer({ backend })
      player.value = p
      await p.initialize(target)

      unsubState = p.subscribe((s) => {
        snapshot.value = s
      })
      unsubEvent = p.on((e) => {
        if (e.type === 'timeupdate') {
          reporter?.onTimeUpdate(e.position, e.duration)
        }
      })

      reporter = new ProgressReporter((posSec, durSec) => {
        const id = sessionId.value
        if (!id) return
        return api.heartbeat(id, posSec * 1000, durSec === undefined ? null : durSec * 1000)
      })

      const external: ExternalSubtitle[] = view.subtitles
        .filter((s) => s.url)
        .map((s) => ({
          url: s.url ?? undefined,
          label: s.language ?? s.track_id,
          language: s.language ?? undefined,
        }))

      await p.load(source, {
        startAt: (opts.startMs ?? 0) / 1000,
        autoplay: opts.autoplay ?? true,
        externalSubtitles: external,
      })
    } catch (e) {
      error.value = describeError(e)
    } finally {
      busy.value = false
    }
  }

  /** 三个地址互斥，按服务端给的挑。 */
  function pickSource(view: api.SessionView): PlaybackSource | null {
    if (view.url) return { kind: 'progressive', url: view.url }
    if (view.hls_url) return { kind: 'hls', url: view.hls_url }
    if (view.stream_url) return { kind: 'progressive', url: view.stream_url }
    return null
  }

  async function close(): Promise<void> {
    // 顺序：先补发进度，再停会话，最后销毁播放器。
    // 反过来的话播放器已经没了，最后一次进度就丢了。
    reporter?.flush()
    reporter = null

    const id = sessionId.value
    if (id) {
      try {
        await api.stopSession(id, Math.round(snapshot.value.position * 1000))
      } catch {
        // 停止失败不该拦住用户关页面。
      }
      sessionId.value = null
    }

    if (hlsJob) {
      try {
        await fetch(api.stopHlsUrl(hlsJob), { method: 'POST' })
      } catch {
        /* 同上 */
      }
      hlsJob = null
    }

    unsubState?.()
    unsubEvent?.()
    unsubState = null
    unsubEvent = null

    await player.value?.destroy()
    player.value = null
    snapshot.value = initialSnapshot()
    strategy.value = null
    reasons.value = []
    subtitles.value = []
  }

  async function play(): Promise<void> {
    await player.value?.play()
  }
  async function pause(): Promise<void> {
    await player.value?.pause()
    reporter?.onPause(snapshot.value.position, snapshot.value.duration || undefined)
  }
  async function seek(seconds: number): Promise<void> {
    await player.value?.seek(seconds)
    reporter?.onSeek(seconds, snapshot.value.duration || undefined)
  }
  async function setVolume(v: number): Promise<void> {
    await player.value?.setVolume(v)
  }
  async function setMuted(m: boolean): Promise<void> {
    await player.value?.setMuted(m)
  }
  async function setRate(r: number): Promise<void> {
    await player.value?.setRate(r)
  }
  async function selectSubtitle(id: string | null): Promise<void> {
    await player.value?.selectSubtitleTrack(id)
  }
  async function selectQuality(id: string): Promise<void> {
    await player.value?.selectQuality(id)
  }
  async function setFullscreen(on: boolean): Promise<void> {
    await player.value?.setFullscreen(on)
  }

  /** 页面即将卸载时调用。用 sendBeacon 是因为普通 fetch 在卸载时发不出去。 */
  function flushBeforeUnload(): void {
    const id = sessionId.value
    if (!id) return
    const pos = Math.round(snapshot.value.position * 1000)
    if (pos <= 0) return
    const body = JSON.stringify({ position_ms: pos })
    navigator.sendBeacon?.(`/api/v1/playback/sessions/${id}/stop`, body)
  }

  return {
    snapshot,
    sessionId,
    strategy,
    reasons,
    subtitles,
    busy,
    error,
    canSwitchAudio,
    canSwitchSubtitle,
    hasQualities,
    open,
    close,
    play,
    pause,
    seek,
    setVolume,
    setMuted,
    setRate,
    selectSubtitle,
    selectQuality,
    setFullscreen,
    flushBeforeUnload,
  }
})

function describeError(e: unknown): string {
  if (e instanceof api.ApiError) {
    if (e.isNetwork) return '连不上服务端，检查服务是否在运行'
    // 这两条要让用户看懂"为什么播不了"，而不是甩一个 HTTP 状态码。
    if (e.detail.code === 'PLAYBACK_UNSUPPORTED') return e.detail.message
    if (e.detail.code === 'FFMPEG_UNAVAILABLE') return '服务端没有 ffmpeg，无法转码这个片源'
    return e.detail.message
  }
  return e instanceof Error ? e.message : String(e)
}
