/**
 * 播放器后端选择与对外出口。
 *
 * 上层（页面 / store）只从这里拿播放器，不直接 `new WebPlayer()` ——
 * 这样后端怎么选、怎么退化，改的是这一个文件，而不是散落在各页面里的实例化。
 */

import { WebPlayer, type WebPlayerOptions } from './web'
import { MpvPlayer, probeMpv, tauriAvailable, type MpvPlayerOptions } from './mpv'
import {
  Media3Player,
  media3Available,
  tauriAvailable as tauriShell,
  type Media3PlayerOptions,
} from './media3'
import { capabilitiesFor, defaultBackend, type PlayerBackend } from './capabilities'
import type { MediaPlayer } from './types'

export * from './types'
export { PlayerCore, initialSnapshot } from './core'
export { ProgressReporter } from './progress'
export { WebPlayer } from './web'
export { MpvPlayer, probeMpv } from './mpv'
export type { MpvBridge, MpvEventPayload, MpvPlayerOptions, MpvStatus } from './mpv'
export { Media3Player, MEDIA3_EVENT_CHANNEL, media3Available } from './media3'
export type { Media3Bridge, Media3PlayerOptions, Media3WireEvent } from './media3'
export { CLIENT_CAPABILITIES, capabilitiesFor, defaultBackend } from './capabilities'
export type { HlsConstructor, HlsLike, WebPlayerOptions } from './web'
export { RecoveryCoordinator, delayForAttempt, resumePosition } from './reconnect'
export type { ReconnectPolicy } from './reconnect'
export { tauriAvailable } from './mpv'

export interface CreatePlayerOptions extends WebPlayerOptions, MpvPlayerOptions, Media3PlayerOptions {
  /** 指定后端。不传则用 `defaultBackend()`。 */
  backend?: PlayerBackend
}

/**
 * 探测此刻**真的能用**的后端。
 *
 * 与 `defaultBackend()` 的区别：那个是同步的、只看约定；这个会去问 Rust 侧
 * mpv 在不在。桌面端机器上没装 mpv 是常态，探测不到就必须按 web 决策 ——
 * 否则服务端会给出 MKV 的 Direct Play 地址，而实际在播的是浏览器。
 */
export async function resolveBackend(): Promise<PlayerBackend> {
  if (!tauriAvailable()) return 'web'

  // Android 优先于 mpv：Tauri 移动端上没有 mpv，
  // 反过来探测会白等一次 IPC，且拿到一个"有 mpv 但用不了"的假结果。
  if (await media3Available()) return 'media3'

  try {
    const status = await probeMpv()
    return status?.available ? 'mpv' : 'web'
  } catch {
    return 'web'
  }
}

/**
 * 创建播放器。
 *
 * **能力跟着后端走**：选了 mpv 后端却用 web 能力（或反过来）会导致
 * 服务端给一个实际播放器放不了的流。见 `capabilitiesFor` 的说明。
 */
export function createPlayer(options: CreatePlayerOptions = {}): MediaPlayer {
  const backend = options.backend ?? defaultBackend()

  switch (backend) {
    case 'media3':
      // 与 mpv 分支同样的处理：探测不到就回落 web，
      // 并且**能力档也按 web 上报**。
      // 不然服务端会按 android-exoplayer 给出 AC3 的 Direct Play，
      // 而实际在播的是 WebView —— 用户遇到"明明能播的格式播不了"。
      if (!tauriShell()) {
        console.warn(
          '[whatmedia] media3 后端只能在 Android 壳里使用，已回落 Web 播放器；' +
            '能力档按 web 上报，不会让服务端给出放不出来的 Direct Play',
        )
        return new WebPlayer({ ...options, capabilities: capabilitiesFor('web') })
      }
      return new Media3Player({ ...options, capabilities: capabilitiesFor('media3') })

    case 'mpv':
      // mpv 后端只在桌面壳里有意义。Web 构建里走到这里说明调用方没先探测 ——
      // 明确记录后回落，而不是静默假装成功：
      // 静默回落的后果是"客户端明明说了能放 MKV，结果放不了"。
      if (!tauriAvailable()) {
        console.warn(
          '[whatmedia] mpv 后端只能在桌面壳里使用，已回落 Web 播放器；' +
            '能力档也按 web 上报，不会让服务端给出放不出来的 Direct Play',
        )
        return new WebPlayer({ ...options, capabilities: capabilitiesFor('web') })
      }
      return new MpvPlayer({ ...options, capabilities: capabilitiesFor('mpv') })

    case 'web':
    default:
      return new WebPlayer({ ...options, capabilities: capabilitiesFor('web') })
  }
}
