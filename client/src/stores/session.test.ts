/**
 * 会话 store 的行为。
 *
 * 这里 mock 掉 `api` 模块而不是起服务端：session store 的价值在于
 * **边界情况的分支**（审批中 / 掉线 / 未安装），这些分支用真实服务端
 * 很难稳定复现，而用 mock 可以一行一个。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useSessionStore } from './session'

const api = vi.hoisted(() => ({
  token: null as string | null,
  meResult: { id: 'u1', username: 'owner', role: 'admin' } as unknown,
  loginOutcome: 'ok' as 'ok' | 'approval' | 'locked',
  probeStatus: 403,
}))

vi.mock('../api/client', () => {
  class ApiError extends Error {
    constructor(
      public detail: Record<string, unknown>,
      public status: number,
    ) {
      super(String(detail.message ?? 'err'))
    }
  }
  return {
    ApiError,
    setSessionToken(t: string | null) {
      api.token = t
    },
    setUnauthorizedHandler: vi.fn(),
    me: async () => api.meResult,
    bootstrapAdmin: async () => {
      throw new ApiError({ code: 'FORBIDDEN', message: 'closed' }, api.probeStatus)
    },
    login: async () => {
      if (api.loginOutcome === 'approval') {
        throw new ApiError(
          { code: 'APPROVAL_REQUIRED', message: 'x', approval_id: 'ap-9' },
          403,
        )
      }
      if (api.loginOutcome === 'locked') {
        throw new ApiError({ code: 'ACCOUNT_LOCKED', message: 'x' }, 429)
      }
      return { token: 'tok-1', user: { id: 'u1', username: 'owner', role: 'admin' } }
    },
    logout: async () => {},
    logoutAll: async () => {},
    isApprovalRequired: (e: unknown) =>
      e instanceof ApiError && e.detail.code === 'APPROVAL_REQUIRED',
  }
})

function store() {
  setActivePinia(createPinia())
  localStorage.clear()
  return useSessionStore()
}

describe('session store', () => {
  beforeEach(() => {
    api.token = null
    api.loginOutcome = 'ok'
    api.probeStatus = 403
  })

  it('登录成功后把票据交给传输层并记住身份', async () => {
    const s = store()
    expect(await s.login('owner', 'password123')).toBe(true)
    expect(s.isAuthed).toBe(true)
    expect(s.isAdmin).toBe(true)
    expect(api.token).toBe('tok-1')
    expect(localStorage.getItem('whatmedia.session.token')).toBe('tok-1')
  })

  it('儿童设备等待批准时，失败必须是可见的而不是静默的', async () => {
    const s = store()
    api.loginOutcome = 'approval'
    expect(await s.login('kid', 'password123')).toBe(false)
    expect(s.isAuthed).toBe(false)
    expect(s.approvalId).toBe('ap-9')
    expect(s.error).toContain('家长')
  })

  it('错误码翻译成人话而不是把 code 甩给用户', async () => {
    const s = store()
    api.loginOutcome = 'locked'
    expect(await s.login('owner', 'password123')).toBe(false)
    expect(s.error).toContain('锁定')
  })

  it('退出后票据与身份一起清掉', async () => {
    const s = store()
    await s.login('owner', 'password123')
    await s.signOut(false)
    expect(s.isAuthed).toBe(false)
    expect(api.token).toBeNull()
    expect(localStorage.getItem('whatmedia.session.token')).toBeNull()
  })

  it('bootstrap 用 403 反推"已经装过"', async () => {
    const s = store()
    api.probeStatus = 403
    await s.bootstrap()
    expect(s.needsSetup).toBe(false)
  })

  it('bootstrap 用 400 认出"从未安装过"', async () => {
    const s = store()
    api.probeStatus = 400
    await s.bootstrap()
    expect(s.needsSetup).toBe(true)
  })

  it('在途会话失效（401）时本地也一并清掉，不留假象', async () => {
    const s = store()
    await s.login('owner', 'password123')
    expect(s.isAuthed).toBe(true)
    s.forget()
    expect(s.isAuthed).toBe(false)
    expect(api.token).toBeNull()
  })
})
