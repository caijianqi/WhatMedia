// Mosaic API 客户端。
//
// 约定（见 DEVELOPMENT-PLAN.md §七）：
//   · 前缀 /api/v1，全 JSON
//   · 错误统一 { error: { code, message, trace_id } }
//   · 类型由 OpenAPI codegen 生成，禁止手写 —— 本文件是过渡期手写版本，
//     P1 接入 utoipa 后改为生成产物。

export interface HealthReport {
  status: string
  version: string
  arch: string
  os: string
  started_at: string
  uptime_secs: number
}

export interface ErrorDetail {
  code: string
  message: string
  trace_id: string | null
}

export class ApiError extends Error {
  constructor(public readonly detail: ErrorDetail, public readonly status: number) {
    super(detail.message)
    this.name = 'ApiError'
  }
}

const BASE = import.meta.env.VITE_MOSAIC_API ?? '/api/v1'

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    headers: { Accept: 'application/json' },
    ...init,
  })

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

  return (await res.json()) as T
}

export function health(): Promise<HealthReport> {
  return request<HealthReport>('/health')
}
