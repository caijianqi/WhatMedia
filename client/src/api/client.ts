/**
 * WhatMedia API 客户端。
 *
 * 约定（开发计划 §七）：
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
export type CreditsView = components['schemas']['CreditsView']
export type CreditRow = components['schemas']['CreditRow']
export type SeasonSummary = components['schemas']['SeasonSummary']
export type SeriesRef = components['schemas']['SeriesRef']
export type KindCount = components['schemas']['KindCount']
export type MediaStream = components['schemas']['MediaStream']
export type ItemPage = components['schemas']['Page_ItemSummary']
export type EpisodePage = components['schemas']['Page_EpisodeSummary']
export type EpisodeSummary = components['schemas']['EpisodeSummary']
export type PageInfo = components['schemas']['PageInfo']

export type Library = components['schemas']['Library']
export type HealthReport = components['schemas']['HealthReport']
export type HardwareProfileReport = components['schemas']['HardwareProfileView']

// ── 鉴权（多用户）────────────────────────────────────────────
export type UserOut = components['schemas']['UserOut']
export type LoginResult = components['schemas']['LoginResult']
export type ApprovalRow = components['schemas']['ApprovalRow']
export type SignedUrlOut = components['schemas']['SignedUrlOut']
/** 服务端认三个角色。前端不自行发明新值。 */
export type Role = 'admin' | 'user' | 'child'

/** 儿童设备等待家长批准时，服务端会在 403 的 error 里附一个审批编号。 */
export interface ApprovalRequired {
  approval_id?: string
}

// ── AI（P7）──────────────────────────────────────────────
export type AiChatReply = components['schemas']['AiChatReply']
export type PendingActionView = components['schemas']['PendingActionView']
export type ToolSpecView = components['schemas']['ToolSpecView']
export type ToolListReply = components['schemas']['ToolListReply']

/** 角色。服务端认这五个，缺省 adult。 */
export type AiRole = 'owner' | 'administrator' | 'adult' | 'child' | 'guest'

export interface AiActor {
  id: string
  role: AiRole
}

export type DecideView = components['schemas']['DecideView']
export type ReasonView = components['schemas']['ReasonView']
export type SessionView = components['schemas']['SessionView']
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

/**
 * 会话票据。
 *
 * 放在模块里而不是 store 里，是因为**播放器要用的 URL 也依赖它**：
 * 那些地址不走 `request()`，但同样需要凭据。让它们从 store 里取会形成
 * api ← store ← api 的循环依赖。
 *
 * 内存里这一份是权威值，localStorage 只是它的持久化镜像 ——
 * Tauri 的 WebView 与测试环境都可能读不到 localStorage，
 * 那种情况下"能继续用"比"记得住"重要。
 */
let sessionToken: string | null = null

export function setSessionToken(token: string | null): void {
  sessionToken = token
}

/** 手写 fetch（例如停 HLS）也要带上凭据。 */
export function authHeaders(): Record<string, string> {
  return sessionToken ? { Authorization: `Bearer ${sessionToken}` } : {}
}

/** 会话失效回调。由 store 注册，避免 api 反向依赖 store。 */
let onUnauthorized: (() => void) | null = null

export function setUnauthorizedHandler(fn: () => void): void {
  onUnauthorized = fn
}

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
    const headers: Record<string, string> = { Accept: 'application/json' }
    if (sessionToken) headers.Authorization = `Bearer ${sessionToken}`
    res = await fetch(`${BASE}${path}`, { headers, ...init })
  } catch {
    // fetch 只在网络层失败时抛（服务端没起来、断网）。
    // 不包一层的话，UI 上只会看到一句看不懂的 TypeError。
    throw new ApiError({ code: 'NETWORK', message: '连不上服务端', trace_id: null }, 0)
  }

  if (!res.ok) {
    // 401 说明票据过期或被吊销。交给 store 处理（跳登录），
    // 而不是让每个调用方各自判断 —— 那必然会有漏的。
    if (res.status === 401 && onUnauthorized) onUnauthorized()
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

function del<T>(path: string): Promise<T> {
  return request<T>(path, { method: 'DELETE' })
}

function put<T>(path: string, body: unknown): Promise<T> {
  return request<T>(path, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body ?? {}),
  })
}

function patch<T>(path: string, body?: unknown): Promise<T> {
  return request<T>(path, {
    method: 'PATCH',
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

/** 各类型的条目数。只返回有内容的类型，首页据此渲染分类 tab。 */
export function mediaKinds(): Promise<KindCount[]> {
  return request<KindCount[]>('/media/kinds')
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

/** 演职员。`cast` 与 `crew` 由服务端按 role 分好，前端不再自行分组。 */
export function itemCredits(id: string): Promise<CreditsView> {
  return request<CreditsView>(`/items/${id}/credits`)
}

/**
 * 条目所属的剧。电影 / 音乐返回 null —— 那时详情页就不渲染季列表。
 *
 * 扫描出来的是"一集一个条目"，所以这里是从条目反查剧，不是拿剧查条目。
 */
export function itemSeries(id: string): Promise<SeriesRef | null> {
  return request<SeriesRef | null>(`/items/${id}/series`)
}

/** 一部剧的季列表（含每季集数）。 */
export function seriesSeasons(seriesId: string): Promise<SeasonSummary[]> {
  return request<SeasonSummary[]>(`/series/${seriesId}/seasons`)
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

export function hardwareProfile(): Promise<HardwareProfileReport> {
  return request<HardwareProfileReport>('/hardware/profile')
}

// ── 播放地址（不经过 JSON，直接给播放器用）────────────────────

/**
 * 停 HLS 的地址。
 *
 * 不走 `request()`：响应不是 JSON，而且这个请求常常是页面卸载时发出的，
 * 走 fetch 反而会被提前取消。
 */
export function stopHlsUrl(jobId: string): string {
  return `${BASE}/hls/${jobId}/stop`
}

// ── AI 助手（P7）────────────────────────────────────────────

/**
 * 提问。
 *
 * 危险工具不会在这里生效：它们只在回复的 `pending` 里返回一张工单，
 * 要真正执行得拿 `pending.token` 再打一次 `confirmAi`。
 */
export function askAi(message: string, actor?: Partial<AiActor>): Promise<AiChatReply> {
  return post<AiChatReply>('/ai/chat', { message, ...actor })
}

/** 确认并执行一张待确认工单。令牌一次性，用过即废。 */
export function confirmAi(token: string, actor?: Partial<AiActor>): Promise<AiChatReply> {
  return post<AiChatReply>('/ai/confirm', { token, ...actor })
}

/**
 * 当前可用的工具清单。
 *
 * 危险工具在未开启时**根本不会出现在这里**，而不是"出现但调用被拒" ——
 * 后者会让模型反复重试一个永远不可能成功的调用。
 */
export function listAiTools(): Promise<ToolSpecView[]> {
  return request<ToolListReply>('/ai/tools').then((r) => r.data)
}

/** AI 不可用（未启用 / 运行时没装配）。与"连不上服务端"要分开提示。 */
export function isAiDisabled(e: unknown): boolean {
  return e instanceof ApiError && (e.detail.code === 'AI_DISABLED' || e.detail.code === 'DB_UNAVAILABLE')
}

// ─────────────────────────────────────────────────────────────
// 电视直播（P8）
// ─────────────────────────────────────────────────────────────

/** 频道。直播没有时长也没有"看完"，所以它刻意不是 MediaItem。 */
export type LiveChannel = components['schemas']['LiveChannelView']
export type LiveSource = components['schemas']['LiveSourceView']
export type LivePlay = components['schemas']['LivePlayView']

export function listLiveSources(): Promise<LiveSource[]> {
  return request<LiveSource[]>('/live/sources')
}

export function createLiveSource(body: {
  name: string
  kind: string
  url?: string
  epg_url?: string
}): Promise<LiveSource> {
  return post<LiveSource>('/live/sources', body)
}

export function deleteLiveSource(id: string): Promise<{ ok: boolean }> {
  return del<{ ok: boolean }>(`/live/sources/${id}`)
}

/** 拉一次订阅并替换频道。返回频道数。 */
export function syncLiveSource(id: string): Promise<{ source_id: string; channels: number }> {
  return post<{ source_id: string; channels: number }>(`/live/sources/${id}/sync`)
}

/** 重新探测指定直播源的频道连通性并过滤失效频道。 */
export function probeLiveSource(id: string): Promise<{ ok: boolean }> {
  return post<{ ok: boolean }>(`/live/sources/${id}/probe`)
}

export function listLiveChannels(): Promise<LiveChannel[]> {
  return request<LiveChannel[]>('/live/channels')
}

/**
 * 起播（或加入已在播的同一频道）。
 *
 * 返回的 `playlist_url` 直接喂 hls.js —— 分片的投递与点播是同一套端点，
 * 前端不需要为直播准备第二条播放路径。
 */
export function playLiveChannel(id: string): Promise<LivePlay> {
  return post<LivePlay>(`/live/channels/${id}/play`)
}

/** 离开频道。引用计数归零后服务端才真正停掉 ffmpeg。 */
export function stopLiveChannel(id: string): Promise<{ ok: boolean }> {
  return post<{ ok: boolean }>(`/live/channels/${id}/stop`)
}

// ─────────────────────────────────────────────────────────────
// 鉴权（多用户）
// ─────────────────────────────────────────────────────────────

/** 创建第一个管理员。**库里已有账户时服务端会 403。** */
export function bootstrapAdmin(input: {
  username: string
  password: string
  display_name?: string
}): Promise<{ id: string }> {
  return post<{ id: string }>('/auth/bootstrap', input)
}

export function login(input: {
  username: string
  password: string
  device_name?: string
  device_kind?: 'web' | 'desktop' | 'mobile' | 'tv'
  platform?: string
}): Promise<LoginResult> {
  return post<LoginResult>('/auth/login', input)
}

export function logout(): Promise<void> {
  return post<void>('/auth/logout', {})
}

/** 登出所有设备。家里的电视丢了就用它。 */
export function logoutAll(): Promise<{ revoked: number }> {
  return post<{ revoked: number }>('/auth/logout-all', {})
}

export function me(): Promise<UserOut> {
  return request<UserOut>('/auth/me')
}

/**
 * 给播放地址签一张有时效的 URL。
 *
 * **播放器发不出自定义请求头**，所以 `<video src>` 与 hls.js 必须走这条路。
 * 浏览器其实还带着 Cookie，但电视上的 WebView 与 ExoPlayer 不一定 ——
 * 统一用签名 URL 才能让六种客户端走同一条路径。
 */
export function signStreamUrl(resource: string): Promise<SignedUrlOut> {
  return post<SignedUrlOut>('/auth/signed-url', { resource })
}

/** 儿童设备等待批准：`APPROVAL_REQUIRED` 的错误码带一个审批编号。 */
export function isApprovalRequired(e: unknown): boolean {
  return e instanceof ApiError && e.detail.code === 'APPROVAL_REQUIRED'
}

// ─────────────────────────────────────────────────────────────
// 收藏
// ─────────────────────────────────────────────────────────────

export type FavoriteRow = components['schemas']['FavoriteRow']
export type FavoriteState = components['schemas']['FavoriteState']
export type FavoritePage = components['schemas']['FavoritePage']

/** 我收藏的条目。带条目标题，前端不用再拉一次详情。 */
export function listMyFavorites(page = 1, pageSize = 50): Promise<FavoritePage> {
  return request<FavoritePage>(`/me/favorites${qs({ page, page_size: pageSize })}`)
}

/** 这个条目我收没收、全站多少人收了。 */
export function getFavorite(itemId: string): Promise<FavoriteState> {
  return request<FavoriteState>(`/items/${itemId}/favorite`)
}

/**
 * 收藏 / 取消收藏。**幂等**：重复设为 true 不会产生第二条记录，
 * 所以按钮可以随便双击。
 */
export function setFavorite(itemId: string, favorite: boolean): Promise<FavoriteState> {
  return put<FavoriteState>(`/items/${itemId}/favorite`, { favorite })
}

// ─────────────────────────────────────────────────────────────
// 通知
// ─────────────────────────────────────────────────────────────

export type NotificationRow = components['schemas']['NotificationRow']
export type NotificationList = components['schemas']['NotificationList']

export function listNotifications(opts: {
  limit?: number
  unreadOnly?: boolean
} = {}): Promise<NotificationList> {
  return request<NotificationList>(
    `/notifications${qs({ limit: opts.limit, unread_only: opts.unreadOnly })}`,
  )
}

export function readNotification(id: string): Promise<void> {
  return post<void>(`/notifications/${id}/read`)
}

/** 全部已读。返回实际清掉的条数。 */
export function readAllNotifications(): Promise<{ updated: number }> {
  return post<{ updated: number }>('/notifications/read-all')
}

// ─────────────────────────────────────────────────────────────
// 设置
// ─────────────────────────────────────────────────────────────

export type SettingEntry = components['schemas']['SettingEntry']
export type SettingsView = components['schemas']['SettingsView']

/**
 * 全部可写设置。
 *
 * 管理员拿到 server + user 两档；普通成员只拿到自己的播放偏好 ——
 * 这个区分是**服务端做的**，前端不该假装过滤。
 */
export function getSettings(): Promise<SettingsView> {
  return request<SettingsView>('/settings')
}

export function putSetting(key: string, value: string): Promise<SettingEntry> {
  return put<SettingEntry>('/settings', { key, value })
}

/** 恢复默认。删掉覆盖行，下次启动（server 级）或立刻（user 级）回到默认。 */
export function resetSetting(key: string): Promise<void> {
  return del<void>(`/settings/${encodeURIComponent(key)}`)
}

// ─────────────────────────────────────────────────────────────
// 存储统计
// ─────────────────────────────────────────────────────────────

export type StorageView = components['schemas']['StorageView']
export type LibraryStorage = components['schemas']['LibraryStorage']

/**
 * 各媒体库的占用。
 *
 * 数字来自数据库里的 `size_bytes`，不是扫盘 —— 所以"某个库占多少"
 * 是精确值，而磁盘总容量服务端不报（跨平台拿不到可靠的"总量"）。
 */
export function storageStats(): Promise<StorageView> {
  return request<StorageView>('/system/storage')
}

// ─────────────────────────────────────────────────────────────
// 服务端目录浏览与扫盘入库
// ─────────────────────────────────────────────────────────────

export type BrowseDirView = components['schemas']['BrowseDirView']
export type DirEntryView = components['schemas']['DirEntryView']
export type ScanRequest = components['schemas']['ScanRequest']
export type ScanAccepted = components['schemas']['ScanAccepted']
export type ScanTaskView = components['schemas']['ScanTaskView']
export type ScanProgress = components['schemas']['ScanProgress']

/**
 * 浏览服务端物理目录树。
 *
 * @param path 目标绝对路径，留空时列出根驱动器（Windows）或根目录（Linux/macOS）
 */
export function browseFsDir(path?: string): Promise<BrowseDirView> {
  return request<BrowseDirView>(`/system/fs-browse${qs({ path })}`)
}

/**
 * 触发指定路径的后台扫描入库任务。
 */
export function startScan(req: ScanRequest): Promise<ScanAccepted> {
  return post<ScanAccepted>('/scan', req)
}

/**
 * 查询服务端当前所有在途后台任务（如媒体扫描任务）。
 */
export function listTasks(): Promise<ScanTaskView[]> {
  return request<ScanTaskView[]>('/tasks')
}

export interface CreateLibraryRequest {
  name: string
  path: string
  kind?: MediaKind
  storage_provider?: string
}

/**
 * 列出所有已配置的媒体库。
 */
export function listLibraries(): Promise<Library[]> {
  return request<Library[]>('/libraries')
}

/**
 * 创建并持久化保存一个新的媒体库物理路径。
 */
export function createLibrary(req: CreateLibraryRequest): Promise<Library> {
  return post<Library>('/libraries', req)
}

// ─────────────────────────────────────────────────────────────
// 元数据刮削（Scrape）
// ─────────────────────────────────────────────────────────────

export type ScrapeRequest = components['schemas']['ScrapeRequest']
export type ScrapeAccepted = components['schemas']['ScrapeAccepted']
export type ScrapeRunView = components['schemas']['ScrapeRunView']
export type ScrapeStats = components['schemas']['ScrapeStats']
export type JobStats = components['schemas']['JobStats']

/**
 * 触发元数据刮削任务。
 *
 * 可针对全部或单个媒体库；支持仅重试失败项。
 */
export function startScrape(req: ScrapeRequest = {}): Promise<ScrapeAccepted> {
  return post<ScrapeAccepted>('/scrape', req)
}

/**
 * 列出所有刮削批次及运行状态。
 */
export function listScrapeRuns(): Promise<ScrapeRunView[]> {
  return request<ScrapeRunView[]>('/scrape/runs')
}

/**
 * 查询指定媒体库或全库的刮削作业统计数据。
 */
export function getScrapeStats(libraryId?: string): Promise<JobStats> {
  return request<JobStats>(`/scrape/stats${qs({ library_id: libraryId })}`)
}

export interface CandidateView {
  provider: string
  provider_id: string
  title: string
  original_title?: string | null
  year?: number | null
  overview?: string | null
  poster?: string | null
  poster_url?: string | null
  backdrop_url?: string | null
  score: number
  confidence: 'auto' | 'review' | 'reject' | string
}

/**
 * 远程图片代理地址（解决 TMDB 官方 CDN 国内浏览器无法直连或被阻断的问题）。
 */
export function imageProxyUrl(url?: string | null): string {
  if (!url) return ''
  if (url.startsWith('http://') || url.startsWith('https://')) {
    return `/api/v1/metadata/image_proxy?url=${encodeURIComponent(url)}`
  }
  return url
}

export function getCandidates(params: {
  item_id: string
  title?: string
  year?: number
}): Promise<CandidateView[]> {
  return request<CandidateView[]>(`/metadata/candidates${qs(params)}`)
}

/**
 * 人工确认或强制按指定 provider/provider_id 重新识别某个条目。
 */
export function identify(
  id: string,
  req: { provider: string; provider_id: string },
): Promise<void> {
  return post<void>(`/items/${id}/identify`, req)
}

// ─────────────────────────────────────────────────────────────
// 存储源管理（Storage Accounts / WebDAV / 百度网盘 / 夸克 / 迅雷 / 移动云盘）
// ─────────────────────────────────────────────────────────────

export interface StorageAccount {
  id: string
  name: string
  driver: 'local' | 'webdav' | 'alist' | string
  endpoint: string
  auth_type: 'none' | 'basic' | 'bearer' | string
  username?: string | null
  password?: string | null
  root_path: string
  direct_stream: boolean
  extra_headers?: Record<string, unknown>
  created_at: string
  updated_at: string
}

export interface CreateStorageRequest {
  name: string
  driver: string
  endpoint: string
  auth_type?: string
  username?: string
  password?: string
  root_path?: string
  direct_stream?: boolean
  extra_headers?: Record<string, unknown>
}

export interface UpdateStorageRequest {
  name?: string
  endpoint?: string
  auth_type?: string
  username?: string
  password?: string
  root_path?: string
  direct_stream?: boolean
  extra_headers?: Record<string, unknown>
}

export interface TestStorageRequest {
  driver: string
  endpoint: string
  auth_type?: string
  username?: string
  password?: string
  root_path?: string
}

export interface TestStorageResponse {
  ok: boolean
  message: string
  latency_ms?: number | null
}

export interface StorageEntry {
  path: string
  name: string
  is_dir: boolean
  size?: number | null
  modified_ns?: number | null
}

export function listStorageAccounts(): Promise<StorageAccount[]> {
  return request<StorageAccount[]>('/storages')
}

export function createStorageAccount(req: CreateStorageRequest): Promise<StorageAccount> {
  return post<StorageAccount>('/storages', req)
}

export function getStorageAccount(id: string): Promise<StorageAccount> {
  return request<StorageAccount>(`/storages/${id}`)
}

export function updateStorageAccount(id: string, req: UpdateStorageRequest): Promise<StorageAccount> {
  return put<StorageAccount>(`/storages/${id}`, req)
}

export function deleteStorageAccount(id: string): Promise<{ ok: boolean }> {
  return del<{ ok: boolean }>(`/storages/${id}`)
}

export function testStorageConnection(req: TestStorageRequest): Promise<TestStorageResponse> {
  return post<TestStorageResponse>('/storages/test', req)
}

export function browseStorage(id: string, path?: string): Promise<StorageEntry[]> {
  return request<StorageEntry[]>(`/storages/${id}/browse${qs({ path })}`)
}

// ── 多源影视聚合 (Phase 3) ─────────────────────────────────

export interface VodEpisode {
  name: string
  play_url: string
  index: number
}

export interface VodRoute {
  source_id: string
  source_name: string
  play_from: string
  episodes: VodEpisode[]
}

export interface AggregatedVodItem {
  id: string
  title: string
  clean_title: string
  sub_title?: string
  year?: number
  poster?: string
  type_name?: string
  remark?: string
  actor?: string
  director?: string
  content?: string
  routes: VodRoute[]
}

export interface AggregateSearchResponse {
  local: ItemPage
  remote: AggregatedVodItem[]
}

export interface VodSourceConfig {
  id: string
  name: string
  api_url: string
  is_active: boolean
  weight: number
  timeout_ms: number
}

/** 双轨多源聚合搜索 (本地库 + 云端 MacCMS 聚合) */
export function searchAggregate(q: string, page: ItemQuery = {}): Promise<AggregateSearchResponse> {
  return request<AggregateSearchResponse>(`/search/aggregate${qs({ q, ...page })}`)
}

/** 采集源列表 */
export function listVodSources(): Promise<VodSourceConfig[]> {
  return request<VodSourceConfig[]>('/settings/vod_sources')
}

/** 导入/添加采集源 (兼容单对象、数组或 MoonTV config.json 格式) */
export function importVodSources(payload: unknown): Promise<{ imported: number }> {
  return post<{ imported: number }>('/settings/vod_sources', payload)
}

/** 删除采集源 */
export function deleteVodSource(id: string): Promise<void> {
  return del<void>(`/settings/vod_sources/${id}`)
}

/** 切换采集源启用/禁用状态 */
export function toggleVodSource(id: string): Promise<VodSourceConfig> {
  return put<VodSourceConfig>(`/settings/vod_sources/${id}/toggle`, {})
}

/** 获取代理播放 URL */
export function getVodStreamProxyUrl(rawUrl: string, sourceId?: string): string {
  return `${BASE}/proxy/vod_stream?url=${encodeURIComponent(rawUrl)}${sourceId ? `&source=${encodeURIComponent(sourceId)}` : ''}`
}

export interface ImportVodResponse {
  item_id: string
  library_id: string
  title: string
  is_new: boolean
}

/** 将云端聚合条目一键导入到本地媒体库 (虚拟流媒体入库)，支持指定线路与默认首选源 */
export function importVodItem(
  item: AggregatedVodItem,
  libraryId?: string,
  options?: { selectedSourceIds?: string[]; defaultSourceId?: string },
): Promise<ImportVodResponse> {
  return post<ImportVodResponse>('/aggregator/import', {
    item,
    library_id: libraryId,
    selected_source_ids: options?.selectedSourceIds,
    default_source_id: options?.defaultSourceId,
  })
}

/** 删除媒体库中的单个条目（及其所有版本、文件、流信息与关联数据） */
export function deleteItem(id: string): Promise<void> {
  return del<void>(`/items/${id}`)
}

/** 删除指定的播放会话记录（如从“继续观看”列表中移除） */
export function deletePlaybackSession(id: string): Promise<void> {
  return del<void>(`/playback/sessions/${id}`)
}

/** 检查某部影片是否已导入媒体库 */
export function checkVodImported(title: string): Promise<{ imported: boolean; item_id: string | null }> {
  return request<{ imported: boolean; item_id: string | null }>(`/aggregator/check_imported?title=${encodeURIComponent(title)}`)
}

/** 获取全网热播 / 最新更新推荐列表 */
export function getHotVodItems(): Promise<AggregatedVodItem[]> {
  return request<AggregatedVodItem[]>('/aggregator/hot')
}

// ─────────────────────────────────────────────────────────────
// 用户与权限管理 (User & Role Management)
// ─────────────────────────────────────────────────────────────

export interface UserAccount {
  id: string
  username: string
  display_name: string | null
  role: 'admin' | 'user' | 'child' | string
  max_rating: string | null
  disabled: boolean
}

export interface CreateUserInput {
  username: string
  password: string
  display_name?: string
  role?: string
  max_rating?: string
  parent_id?: string
}

export interface UpdateUserInput {
  display_name?: string
  password?: string
  role?: string
  max_rating?: string
  disabled?: boolean
}

/** 获取全部用户列表 (仅管理员) */
export function listUsers(): Promise<UserAccount[]> {
  return request<UserAccount[]>('/auth/users')
}

/** 新建用户 (仅管理员) */
export function createUser(input: CreateUserInput): Promise<{ id: string }> {
  return post<{ id: string }>('/auth/users', input)
}

/** 修改用户/重置密码/角色/状态 (仅管理员) */
export function updateUser(id: string, input: UpdateUserInput): Promise<{ status: string }> {
  return patch<{ status: string }>(`/auth/users/${id}`, input)
}

/** 删除用户 (仅管理员) */
export function deleteUser(id: string): Promise<{ status: string }> {
  return del<{ status: string }>(`/auth/users/${id}`)
}


