/**
 * WhatMedia API 客户端。
 *
 * 约定（DEVELOPMENT-PLAN.md §七）：
 *   · 前缀 /api/v1，全 JSON
 *   · 错误统一 { error: { code, message, trace_id } }
 *
 * **类型来源**：领域类型全部来自 `./schema` —— 它由服务端导出的
 * OpenAPI 契约生成（`make openapi` → `npm run gen:api`），**不手写**。
 * 本文件只保留传输层与端点函数：这两样契约表达不了，但也不该手写类型。
 *
 * 之所以值得这么绕一圈：手写类型的漂移是静默的。少一个字段不报错，
 * 只在某个页面渲染成 `undefined`，而且往往是上线几周后才被看见。
 */

import type { components } from './schema'

// ─────────────────────────────────────────────────────────────
// 领域类型：全部由契约生成
// ─────────────────────────────────────────────────────────────

export type MediaKind = components['schemas']['MediaKind']
export type PlaybackStrategy = components['schemas']['PlaybackStrategy']
export type ItemSort = components['schemas']['ItemSort']
export type SortOrder = components['schemas']['SortOrder']

export type ItemSummary = components['schemas']['ItemSummary']
export type ItemDetail = components['schemas']['ItemDetail']
export type MediaStream = components['schemas']['MediaStream']
export type ItemPage = components['schemas']['Page_ItemSummary']
export type SeriesPage = components['schemas']['Page_SeriesSummary']
export type EpisodePage = components['schemas']['Page_EpisodeSummary']
export type PageInfo = components['schemas']['PageInfo']

export type Library = components['schemas']['Library']
export type HealthReport = components['schemas']['HealthReport']
export type HardwareProfileReport = components['schemas']['HardwareProfileView']

export type DecideView = components['schemas']['DecideView']
export type ReasonView = components['schemas']['ReasonView']
export type SessionView = components['schemas']['SessionView']
export type SubtitleView = components['schemas']['SubtitleView']
export type SubtitleItemView = components['schemas']['SubtitleItemView']
export type CreateSessionBody = components['schemas']['CreateSessionBody']
export type ResumePoint = components['schemas']['ResumePoint']

/**
 * 列表查询参数。
 *
 * 契约里 `sort` / `order` / `page` / `page_size` 是必填（utoipa 看不到
 * serde 的 default），但前端从来只想覆盖其中一两个 —— 所以在这里放宽成全可选。
 * **这是唯一一处手写放宽**，写在明处而不是悄悄改生成物。
 */
export type ItemQuery = Partial<components['schemas']['ItemQuery']>

// ─────────────────────────────────────────────────────────────
// 传输层
// ─────────────────────────────────────────────────────────────

export type ErrorDetail = components['schemas']['ErrorDetail']

export class ApiError extends Error {
  constructor(
    public readonly detail: ErrorDetail,
    public readonly status: number,
  ) {
    super(detail.message)
    this.name = 'ApiError'
  }

  /** 服务端没起来 / 断网。与业务错误码要区分对待 —— 前者该提示"连不上"。 */
  get isNetwork(): boolean {
    return this.status === 0
  }
}

const BASE = import.meta.env.VITE_WHATMEDIA_API ?? '/api/v1'

function qs(params: object): string {
  const sp = new URLSearchParams()
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null || v === '') continue
    sp.set(k, String(v))
  }
  const s = sp.toString()
  return s ? `?${s}` : ''
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let res: Response
  try {
    res = await fetch(`${BASE}${path}`, {
      headers: { Accept: 'application/json' },
      ...init,
    })
  } catch {
    // fetch 只在网络层失败时抛（服务端没起来、断网）。
    // 不包一层的话，UI 上只会看到一句看不懂的 TypeError。
    throw new ApiError({ code: 'NETWORK', message: '连不上服务端', trace_id: null }, 0)
  }

  if (!res.ok) {
    let detail: ErrorDetail = {
      code: 'UNKNOWN',
      message: `HTTP ${res.status}`,
      trace_id: null,
    }
    try {
      const body = await res.json()
      if (body?.error) detail = body.error
    } catch {
      /* 非 JSON 响应，保留默认 detail */
    }
    throw new ApiError(detail, res.status)
  }

  if (res.status === 204) return undefined as T
  const text = await res.text()
  return (text ? JSON.parse(text) : undefined) as T
}

function post<T>(path: string, body?: unknown): Promise<T> {
  return request<T>(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
}

// ─────────────────────────────────────────────────────────────
// 端点
// ─────────────────────────────────────────────────────────────

export function health(): Promise<HealthReport> {
  return request<HealthReport>('/health')
}

export function listMedia(q: ItemQuery = {}): Promise<ItemPage> {
  return request<ItemPage>(`/media${qs(q)}`)
}

export function listMovies(q: ItemQuery = {}): Promise<ItemPage> {
  return request<ItemPage>(`/movies${qs(q)}`)
}

/** 剧集列表。`SeriesSummary` 没有海报字段，别指望它能直接喂给海报墙。 */
export function listSeries(q: ItemQuery = {}): Promise<SeriesPage> {
  return request<SeriesPage>(`/series${qs(q)}`)
}

export function listEpisodes(q: {
  series_id?: string
  season_id?: string
  page?: number
  page_size?: number
} = {}): Promise<EpisodePage> {
  return request<EpisodePage>(`/episodes${qs(q)}`)
}

export function getItem(id: string): Promise<ItemDetail> {
  return request<ItemDetail>(`/items/${id}`)
}

export function search(q: string, page: ItemQuery = {}): Promise<ItemPage> {
  return request<ItemPage>(`/search${qs({ q, ...page })}`)
}

/** 决策预览。不产生副作用（不建会话、不启动 ffmpeg）。 */
export function decidePlayback(params: {
  item_id?: string
  file_id?: string
  client?: string
  network_kbps?: number
  start_ms?: number
}): Promise<DecideView> {
  return request<DecideView>(`/playback/decide${qs(params)}`)
}

/**
 * 建播放会话。
 *
 * **注意这个端点有副作用**：它会在需要时启动 ffmpeg 转码任务。
 * 只是想预览决策结果时用 `decidePlayback`，不要调它 ——
 * 否则每打开一次详情页就会在服务端留下一路转码。
 */
export function createSession(body: CreateSessionBody): Promise<SessionView> {
  return post<SessionView>('/playback/sessions', body)
}

export function heartbeat(
  id: string,
  positionMs: number,
  durationMs?: number | null,
): Promise<void> {
  return post<void>(`/playback/sessions/${id}/heartbeat`, {
    position_ms: Math.round(positionMs),
    duration_ms: durationMs ?? null,
  })
}

export function stopSession(id: string, positionMs?: number): Promise<void> {
  return post<void>(
    `/playback/sessions/${id}/stop`,
    positionMs === undefined ? null : { position_ms: Math.round(positionMs) },
  )
}

export function resumeList(limit = 20): Promise<ResumePoint[]> {
  return request<ResumePoint[]>(`/playback/resume${qs({ limit })}`)
}

export function itemSubtitles(itemId: string): Promise<SubtitleItemView[]> {
  return request<SubtitleItemView[]>(`/items/${itemId}/subtitles`)
}

export function hardwareProfile(): Promise<HardwareProfileReport> {
  return request<HardwareProfileReport>('/hardware/profile')
}

// ── 播放地址（不经过 JSON，直接给播放器用）────────────────────

/**
 * Direct Play / Remux 的地址。
 *
 * 这些都是**给 `<video src>` 用的**，不走 `request()`：
 * 一是响应不是 JSON，二是 Range 请求必须让浏览器自己发 ——
 * 经过 fetch 会把整段视频拉进内存。
 */
export function directStreamUrl(fileId: string): string {
  return `${BASE}/stream/${fileId}`
}

export function remuxStreamUrl(fileId: string): string {
  return `${BASE}/stream/${fileId}/remux`
}

export function hlsMasterUrl(jobId: string): string {
  return `${BASE}/hls/${jobId}/master.m3u8`
}

export function stopHlsUrl(jobId: string): string {
  return `${BASE}/hls/${jobId}/stop`
}

export function externalSubtitleUrl(itemId: string, trackId: string): string {
  return `${BASE}/subtitles/external${qs({ item_id: itemId, track_id: trackId })}`
}
