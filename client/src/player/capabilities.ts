/**
 * 客户端能力声明。
 *
 * **数据来自 `capabilities.json`，不在这里手写。**
 *
 * 原因：这份数据有两个消费者 —— 前端 UI（决定显示哪些按钮）和
 * 服务端 `ClientCapability`（决定要不要转码）。两边各写一份，
 * 迟早会出现"服务端觉得你能放 PGS，前端却给了个烧录开关"这种错位，
 * 而且这种错位不会报错，只会让用户遇到一个莫名其妙的失败。
 *
 * 所以单一真源放在 JSON 里，前端 import，`crates/playback/tests/client_capability_parity.rs`
 * 读同一个文件跟 Rust 侧的预置档逐字段比对。任一侧改了没同步，CI 就红。
 */

import rawCapabilities from './capabilities.json'
import type { PlayerCapabilities, PlayerClientId } from './types'

interface RawClient {
  id: PlayerClientId
  containers: string[]
  videoCodecs: string[]
  audioCodecs: string[]
  subtitleCodecs: string[]
  bitmapSubtitles: boolean
  maxBitrateKbps?: number
  hls: boolean
  localFile: boolean
  canSwitchAudioTrack: boolean
  canSwitchSubtitleTrack: boolean
  /** 给人看的说明，不参与比对。 */
  note: string
}

const raw = rawCapabilities as { clients: Record<string, RawClient> }

function toCapabilities(c: RawClient): PlayerCapabilities {
  return {
    clientId: c.id,
    containers: c.containers,
    videoCodecs: c.videoCodecs,
    audioCodecs: c.audioCodecs,
    subtitleCodecs: c.subtitleCodecs,
    bitmapSubtitles: c.bitmapSubtitles,
    maxBitrateKbps: c.maxBitrateKbps,
    hls: c.hls,
    localFile: c.localFile,
    canSwitchAudioTrack: c.canSwitchAudioTrack,
    canSwitchSubtitleTrack: c.canSwitchSubtitleTrack,
  }
}

export const CLIENT_CAPABILITIES: Record<PlayerClientId, PlayerCapabilities> = Object.fromEntries(
  Object.values(raw.clients).map((c) => [c.id, toCapabilities(c)]),
) as Record<PlayerClientId, PlayerCapabilities>

/**
 * 播放器后端。
 *
 * `media3` 只在 Android / Android TV 上出现，且**只有当原生插件真的注册成功**时才算；
 * 探测不到必须回落 web，理由见 `resolveBackend` 的注释。
 */
export type PlayerBackend = 'web' | 'mpv' | 'media3'

/**
 * 按**实际使用的播放器后端**取能力，而不是按运行环境取。
 *
 * 这个区分是要紧的：桌面端（Tauri）如果因为"跑在桌面上"就声明 `tauri-mpv` 能力，
 * 服务端会决策成 Direct Play 并给出一个 MKV 地址 —— 而 mpv 后端还没就绪、
 * 实际在用 Web 播放器时，浏览器根本放不了 MKV。
 * 于是用户会遇到"明明是本地文件却播不了"，而且服务端日志里一切正常。
 *
 * 所以：能力跟着后端走。换后端就必须换能力。
 */
export function capabilitiesFor(backend: PlayerBackend): PlayerCapabilities {
  switch (backend) {
    case 'mpv':
      return CLIENT_CAPABILITIES['tauri-mpv']
    case 'media3':
      // 能力档跟着**实际在用的播放器**走，不跟着"跑在什么设备上"走。
      // 在 Android 上却用 WebView 播（插件没注册成功）时，
      // 若按 android-exoplayer 上报，服务端会给出 AC3 的 Direct Play，
      // 而实际播放器放不了 —— 用户遇到"明明是本机能播的格式却播不了"。
      return CLIENT_CAPABILITIES['android-exoplayer']
    default:
      return CLIENT_CAPABILITIES.web
  }
}

/**
 * 同步的默认后端。
 *
 * 恒为 `web`，即使跑在桌面壳里：探测 mpv 要一次异步 IPC，而这个函数的调用方
 * （比如 `createPlayer`）是同步的。要按"mpv 在不在"决策，用 `resolveBackend()`。
 *
 * 保守是有理由的：宣称自己能放 MKV 却用浏览器去放，比一开始就按 web 决策更糟 ——
 * 前者是"明明是本地文件却播不了"，后者只是多一次转码。
 */
export function defaultBackend(): PlayerBackend {
  return 'web'
}
