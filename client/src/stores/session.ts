/**
 * 会话 store。
 *
 * 三条不显眼但重要的决定：
 *
 * 1. **票据存在 localStorage，但内存里那一份是权威值。**
 *    刷新页面时先从 localStorage 恢复，再用 `/auth/me` 验一次 ——
 *    没验就直接相信本地值，会出现"服务端重启后前端还以为自己登录着"的假象。
 * 2. **401 是集中处理的。** `api` 模块在收到 401 时回调这里的 `forget()`，
 *    页面不需要各自判断"是不是掉线了"。
 * 3. **首次安装是一个独立状态**（`needsSetup`），不是一个"用户为空"的布尔。
 *    因为服务端只有在**零用户**时才开放 `/auth/bootstrap`，
 *    前端必须区分"还没装"和"装了但登录失败"，否则用户会陷入死循环。
 */

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import * as api from '../api/client'
import type { Role, UserOut } from '../api/client'

const TOKEN_KEY = 'whatmedia.session.token'

export const useSessionStore = defineStore('session', () => {
  const token = ref<string | null>(readStoredToken())
  const user = ref<UserOut | null>(null)
  const needsSetup = ref(false)
  const checking = ref(false)
  const error = ref<string>('')
  /** 儿童设备等待家长批准时，把审批编号露给 UI。 */
  const approvalId = ref<string | null>(null)

  const isAuthed = computed(() => !!user.value && !!token.value)
  const role = computed<Role | null>(() => (user.value?.role as Role) ?? null)
  const isChild = computed(() => role.value === 'child')
  const isAdmin = computed(() => role.value === 'admin')

  function readStoredToken(): string | null {
    try {
      return localStorage.getItem(TOKEN_KEY)
    } catch {
      // Tauri 的某些 WebView 与测试环境拿不到 localStorage。
      // 拿不到就当没登录 —— 最坏是重新输一次口令，不是崩在启动上。
      return null
    }
  }

  function persist(t: string | null): void {
    try {
      if (t) localStorage.setItem(TOKEN_KEY, t)
      else localStorage.removeItem(TOKEN_KEY)
    } catch {
      /* 同上：持久化失败不该影响本次会话 */
    }
  }

  function adopt(t: string, u: UserOut): void {
    token.value = t
    user.value = u
    error.value = ''
    approvalId.value = null
    api.setSessionToken(t)
    persist(t)
  }

  function forget(): void {
    token.value = null
    user.value = null
    approvalId.value = null
    api.setSessionToken(null)
    persist(null)
  }

  /**
   * 启动时的自检：有没有在途会话？这个服务端是不是还没装过？
   *
   * 两次请求是刻意的：一次 `/auth/me` 判断登录态，
   * 失败时再判断是不是"零用户" —— 后者只能通过 bootstrap 是否 403 来反推，
   * 而登录失败与未安装必须用不同的 UI 表达。
   */
  async function bootstrap(): Promise<void> {
    checking.value = true
    try {
      const t = token.value
      if (t) {
        api.setSessionToken(t)
        try {
          user.value = await api.me()
          needsSetup.value = false
          return
        } catch {
          forget()
        }
      }
      // 没有可用会话 —— 判断是不是"从未安装过"。
      // 这里用一个必然会失败的请求去探：零用户时 bootstrap 端点本身是开的，
      // 只有它返回 403 才说明已经装好了。
      needsSetup.value = await probeUninstalled()
    } finally {
      checking.value = false
    }
  }

  async function probeUninstalled(): Promise<boolean> {
    try {
      // 用一个非法口令去撞：口令不合策略会 400，说明端点开着（= 零用户）；
      // 已经装好则直接 403。两个响应都"失败"，但含义完全不同。
      await api.bootstrapAdmin({ username: '__probe__', password: '' })
      return true
    } catch (e) {
      if (e instanceof api.ApiError) {
        if (e.status === 403) return false
        if (e.status === 400) return true
      }
      // 连不上服务端：既不能断言装过、也不能断言没装，保守按"已安装"处理，
      // 让用户去登录页（那里会有明确的"连不上"提示）。
      return false
    }
  }

  async function setup(username: string, password: string): Promise<boolean> {
    error.value = ''
    try {
      await api.bootstrapAdmin({ username, password })
      needsSetup.value = false
      return await doLogin(username, password)
    } catch (e) {
      error.value = describe(e)
      return false
    }
  }

  async function doLogin(username: string, password: string): Promise<boolean> {
    error.value = ''
    approvalId.value = null
    try {
      const r = await api.login({
        username,
        password,
        device_name: deviceName(),
        device_kind: deviceKind(),
        platform: platform(),
      })
      adopt(r.token, r.user)
      return true
    } catch (e) {
      if (api.isApprovalRequired(e)) {
        // 不是失败，是"等待家长批准" —— 必须让用户看得见，
        // 否则孩子的体验是"密码明明对的就是进不去"。
        const detail = (e as api.ApiError).detail as unknown as api.ApprovalRequired
        approvalId.value = detail.approval_id ?? null
        error.value = '已向家长发出请求，请家长在设置里批准后重试'
        return false
      }
      error.value = describe(e)
      return false
    }
  }

  async function signOut(all = false): Promise<void> {
    try {
      if (all) await api.logoutAll()
      else await api.logout()
    } catch {
      /* 服务端已经不知道这个会话了，本地照清 */
    }
    forget()
  }

  function describe(e: unknown): string {
    if (e instanceof api.ApiError) {
      switch (e.detail.code) {
        case 'INVALID_CREDENTIAL':
          return '用户名或口令不对'
        case 'ACCOUNT_LOCKED':
          return '失败次数过多，账户已临时锁定，请稍后再试'
        case 'ACCOUNT_DISABLED':
          return '该账户已被停用'
        case 'APPROVAL_REQUIRED':
          return '该设备正在等待家长批准'
        case 'USERNAME_TAKEN':
          return '这个用户名已经有人用了'
        default:
          return e.detail.message || `请求失败（${e.status}）`
      }
    }
    return String(e)
  }

  return {
    token,
    user,
    needsSetup,
    checking,
    error,
    approvalId,
    isAuthed,
    role,
    isChild,
    isAdmin,
    bootstrap,
    setup,
    login: doLogin,
    signOut,
    forget,
  }
})

// ── 设备标识 ────────────────────────────────────────────────
//
// 设备名用于"登出所有设备"列表里让人认出哪一台。
// 用 `navigator.userAgent` 拼一个可读的名字，而不是随机串 ——
// 随机串在列表里等于没有信息。

function platform(): string {
  const ua = navigator.userAgent
  if (ua.includes('Windows')) return 'windows'
  if (ua.includes('Mac OS')) return 'macos'
  if (ua.includes('Android')) return 'android'
  if (ua.includes('iPhone') || ua.includes('iPad')) return 'ios'
  if (ua.includes('Linux')) return 'linux'
  return 'web'
}

function deviceKind(): 'web' | 'desktop' | 'mobile' | 'tv' {
  // Tauri 会在 window 上留一个内部标记。
  const w = window as unknown as { __TAURI_INTERNALS__?: unknown }
  if (w.__TAURI_INTERNALS__) return 'desktop'
  const ua = navigator.userAgent
  if (/iPhone|iPad|Android|Mobile/.test(ua)) return 'mobile'
  if (/TV|CrKey|AFT/.test(ua)) return 'tv'
  return 'web'
}

function deviceName(): string {
  const kind = deviceKind()
  const p = platform()
  const label: Record<string, string> = {
    windows: 'Windows',
    macos: 'macOS',
    linux: 'Linux',
    android: 'Android',
    ios: 'iOS',
    web: '浏览器',
  }
  const prefix = { desktop: '桌面端', mobile: '手机', tv: '电视', web: '浏览器' }[kind]
  return `${prefix} · ${label[p] ?? p}`
}
