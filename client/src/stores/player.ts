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
  RecoveryCoordinator,
  type ExternalSubtitle,
  type MediaPlayer,
  type PlaybackSource,
  type PlayerSnapshot,
  type PlayerTarget,
} from '../player'
import * as api from '../api/client'
import type { PlayerErrorCode } from '../player'
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

  // ── 断网恢复 ────────────────────────────────────────────
  //
  // 集中在这里而不是写在页面里：重连要同时用到"会话信息""播放器实例"
  // "进度上报"三样东西，只有 store 全都拿得到。漏一处的表现是
  // "网络恢复后播放器还是死的，用户得退出去重新点一遍"。
  const recovery = new RecoveryCoordinator()
  /** 正在重连中。UI 据此显示"正在恢复…"而不是一个终态错误。 */
  const recovering = ref(false)
  /** 网络提示。只在真的有影响时才有值，别拿它刷存在感。 */
  const netNote = ref<string | null>(null)

  /** 上次载入的源与参数。重连要重放同一份。 */
  let lastSource: PlaybackSource | null = null
  let lastLoadOptions: { startAt?: number; autoplay?: boolean; externalSubtitles?: ExternalSubtitle[] } = {}
  let retryTimer: number | undefined
  /** 上次 `open()` 的参数。地址失效时要能整段重来一次。 */
  let lastOpen: {
    target: PlayerTarget
    itemId: string
    opts: { startMs?: number; autoplay?: boolean; fileId?: string; client?: string }
  } | null = null

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
    lastOpen = { target, itemId, opts }
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

      const source = await pickSource(view)
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
          recovery.recordPosition(e.position, e.duration)
          recovery.markHealthy()
        } else if (e.type === 'error') {
          onPlayerError(e.error.code, e.error.message)
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

      lastSource = source
      lastLoadOptions = {
        startAt: (opts.startMs ?? 0) / 1000,
        autoplay: opts.autoplay ?? true,
        externalSubtitles: external,
      }
      recovery.reset()
      await p.load(source, lastLoadOptions)
    } catch (e) {
      error.value = describeError(e)
    } finally {
      busy.value = false
    }
  }

  /**
   * 播放器报错。
   *
   * 分三类处理，**只有网络类值得重试**：
   *   · 网络 → 退避重连，用记住的位置续播
   *   · 解码失败 → 直接报错，让服务端重新决策（重试一百次也解不了）
   *   · 源不存在 → HLS 任务可能已被服务端回收，重新开一次会话
   */
  function onPlayerError(code: PlayerErrorCode, message: string): void {
    if (code === 'not-found' && canReopen()) {
      // 不是网络问题，而是地址没了。等退避没有意义，立刻重开会话。
      void reopenAt(recovery.beginAttempt())
      return
    }
    if (!recovery.onError(code)) {
      error.value = message || '播放失败'
      recovering.value = false
      return
    }
    scheduleRetry()
  }

  /** 是否能重开会话。没有 itemId 就没法重开。 */
  function canReopen(): boolean {
    return lastOpen !== null && sessionId.value !== null
  }

  /** 安排一次重试。网络恢复时会被 `onNetworkChange` 提前叫醒。 */
  function scheduleRetry(): void {
    recovering.value = true
    error.value = null
    netNote.value = '网络中断，正在重连…'
    if (retryTimer) window.clearTimeout(retryTimer)
    retryTimer = window.setTimeout(() => void doReload(), recovery.nextDelayMs())
  }

  /** 重放上一次的载入。 */
  async function doReload(): Promise<void> {
    const p = player.value
    const src = lastSource
    if (!p || !src) {
      recovering.value = false
      return
    }
    const at = recovery.beginAttempt()
    await p.load(src, { ...lastLoadOptions, startAt: at, autoplay: true })
  }

  /** 重开一次会话（地址失效时用）。 */
  async function reopenAt(startSeconds: number): Promise<void> {
    const o = lastOpen
    if (!o) return
    await open(o.target, o.itemId, { ...o.opts, startMs: Math.round(startSeconds * 1000) })
  }

  /** 网络状态变化。由页面的 online/offline 与 media3 的事件共同驱动。 */
  function onNetworkChange(online: boolean): void {
    const shouldRetryNow = recovery.onNetworkChange(online)
    if (!online) {
      // 断网时先给个提示，但**不弹错误** ——
      // 有些切换会在几秒内自己恢复，弹一个终态错误反而吓人。
      netNote.value = '网络已断开'
      return
    }
    netNote.value = null
    if (shouldRetryNow && recovery.state !== 'idle') {
      if (retryTimer) window.clearTimeout(retryTimer)
      void doReload()
    }
  }

  /**
   * 三个地址互斥，按服务端给的挑。
   *
   * 挑出来之后**还要再签一次名**：`<video src>` 与 hls.js 只在同域时带 Cookie，
   * 而电视上的 WebView、ExoPlayer、AVPlayer 根本不带。签名 URL 是六种客户端
   * 唯一共用的通道，所以这里无条件签 —— 签名失败才退回原地址（浏览器那条通道）。
   */
  async function pickSource(view: api.SessionView): Promise<PlaybackSource | null> {
    const raw = view.url ?? view.hls_url ?? view.stream_url
    if (!raw) return null
    const url = await signed(raw)
    if (view.hls_url) return { kind: 'hls', url }
    return { kind: 'progressive', url }
  }

  async function signed(url: string): Promise<string> {
    try {
      const r = await api.signStreamUrl(url)
      return r.url || url
    } catch {
      // 签名端点不可用（老服务端）时退回原地址：浏览器还带着 Cookie。
      return url
    }
  }

  async function close(): Promise<void> {
    // 顺序：先补发进度，再停会话，最后销毁播放器。
    // 反过来的话播放器已经没了，最后一次进度就丢了。
    //
    // 重连定时器必须先清掉：不然关页面后它还会触发一次 load，
    // 而播放器已经销毁了 —— 表现为"退出播放后又自己响了一下"。
    if (retryTimer) window.clearTimeout(retryTimer)
    retryTimer = undefined
    recovery.reset()
    recovering.value = false
    netNote.value = null

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
        await fetch(api.stopHlsUrl(hlsJob), { method: 'POST', headers: api.authHeaders() })
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
    recovering,
    netNote,
    onNetworkChange,
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
