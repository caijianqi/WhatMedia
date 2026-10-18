/**
 * AI 助手会话（P7）。
 *
 * 这一层存在的唯一理由：**让"预览必须先于执行"成为类型系统之外的一道闸。**
 *
 * 服务端已经保证了危险工具必须走 Plan→Preview→Confirm→Execute，
 * 但前端如果随手写——拿到回复发现有 `pending` 就自动调用一次 confirm——
 * 那道闸就等于没设：用户什么都没看清，库已经被扫了一遍。
 *
 * 所以这里把两件事做成硬约束：
 *
 *   1. `confirm()` 只有在当前轮次确实存在 `pending` 时才发请求；
 *      否则直接抛错，**一个字节都不会发到服务端**。
 *   2. `reject()` 把本轮标记为 rejected，之后再调 `confirm()` 同样抛错。
 *
 * 一个反直觉的取舍：**工单处理完之后本地不清掉 pending。**
 * 清掉最干净，但界面会瞬间把"我刚刚执行/取消了什么"抹掉——
 * 而这件事恰恰是用户最需要留痕的。所以门闩只挂在 `phase` 上：
 * `phase !== 'awaitingConfirmation'` 时 `confirm()` 一律抛错，
 * pending 留在那里纯粹是给用户看的。
 *
 * 状态机与 UI 框架无关（不 import vue），因此能在 vitest 里完整跑一遍。
 */

import * as api from '../api/client'

// ─────────────────────────────────────────────────────────────
// 类型
// ─────────────────────────────────────────────────────────────

export type Phase =
  | 'idle'
  | 'asking'
  | 'answered'
  /** 有工单待确认。**这是唯一允许调用 confirm() 的状态。** */
  | 'awaitingConfirmation'
  | 'confirming'
  | 'rejected'
  | 'failed'

/** 一次待确认动作。字段与服务端 `PendingActionView` 一一对应。 */
export interface PendingAction {
  id: string
  token: string
  tool: string
  risk: string
  summary: string
  lines: string[]
  warnings: string[]
  secondsLeft: number
  /** 到期时刻（本地时钟）。倒计时用；到点后服务端会拒，前端提前禁用按钮。 */
  expiresAt: number
}

export interface Turn {
  question: string
  answer: string
  agent: string
  /** 走了规则引擎兜底。UI 上要标出来，别让人以为模型真懂。 */
  degraded: boolean
  tools: string[]
  pending: PendingAction | null
  /** confirmed：已执行；rejected：用户取消；failed：执行出错。 */
  status: 'answered' | 'awaiting' | 'confirmed' | 'rejected' | 'failed'
  error: string | null
}

export interface AssistantState {
  phase: Phase
  turns: Turn[]
  error: string | null
}

export interface Actor {
  id: string
  role: api.AiRole
}

/**
 * 传输层抽象。
 *
 * 抽出来不是为好看：确认流程的每条约束都必须能在不启服务端的前提下验证，
 * 否则"未经预览不得执行"就只是一句注释。
 */
export interface AiTransport {
  chat(message: string, actor: Actor): Promise<api.AiChatReply>
  confirm(token: string, actor: Actor): Promise<api.AiChatReply>
}

/** 真实传输：走 HTTP。 */
export const httpTransport: AiTransport = {
  chat: (message, actor) => api.askAi(message, actor),
  confirm: (token, actor) => api.confirmAi(token, actor),
}

// ─────────────────────────────────────────────────────────────
// 会话
// ─────────────────────────────────────────────────────────────

function toPending(v: api.PendingActionView): PendingAction {
  return {
    id: v.id,
    token: v.token,
    tool: v.tool,
    risk: v.risk,
    summary: v.summary,
    lines: v.lines,
    warnings: v.warnings,
    secondsLeft: v.expires_in_secs,
    expiresAt: Date.now() + v.expires_in_secs * 1000,
  }
}

function messageOf(e: unknown): string {
  if (e instanceof api.ApiError) {
    return e.isNetwork ? '连不上服务端' : e.detail.message
  }
  return e instanceof Error ? e.message : String(e)
}

export class AssistantSession {
  private listeners = new Set<(s: AssistantState) => void>()
  private state: AssistantState = { phase: 'idle', turns: [], error: null }

  constructor(
    private readonly transport: AiTransport,
    private actor: Actor,
  ) {}

  get snapshot(): AssistantState {
    return this.state
  }

  get hasPending(): boolean {
    return this.state.phase === 'awaitingConfirmation'
  }

  subscribe(fn: (s: AssistantState) => void): () => void {
    this.listeners.add(fn)
    fn(this.state)
    return () => this.listeners.delete(fn)
  }

  setActor(actor: Actor): void {
    this.actor = actor
  }

  private emit(): void {
    for (const fn of this.listeners) fn(this.state)
  }

  private patch(p: Partial<AssistantState>): void {
    this.state = { ...this.state, ...p }
    this.emit()
  }

  private patchTurn(i: number, p: Partial<Turn>): void {
    const turns = this.state.turns.slice()
    turns[i] = { ...turns[i]!, ...p }
    this.patch({ turns })
  }

  /** 提问。危险工具会停在 `awaitingConfirmation`，不会自己往下走。 */
  async ask(text: string): Promise<void> {
    const question = text.trim()
    if (!question) {
      this.patch({ error: '请输入问题' })
      return
    }
    this.patch({ phase: 'asking', error: null, turns: [...this.state.turns, {
      question,
      answer: '',
      agent: '',
      degraded: false,
      tools: [],
      pending: null,
      status: 'answered',
      error: null,
    }] })
    const i = this.state.turns.length - 1

    try {
      const reply = await this.transport.chat(question, this.actor)
      this.patchTurn(i, {
        answer: reply.answer,
        agent: reply.agent,
        degraded: reply.degraded,
        tools: reply.traces.map((t) => t.tool),
        pending: reply.pending ? toPending(reply.pending) : null,
        status: reply.pending ? 'awaiting' : 'answered',
      })
      this.patch({ phase: reply.pending ? 'awaitingConfirmation' : 'answered' })
    } catch (e) {
      this.patchTurn(i, { status: 'failed', error: messageOf(e) })
      this.patch({ phase: 'failed', error: messageOf(e) })
    }
  }

  /**
   * 确认执行当前工单。
   *
   * **前置条件即约束**：没有待确认工单时抛错并直接返回，
   * 不发出任何网络请求。这条是"预览必须先于执行"在这里的落点。
   */
  async confirm(): Promise<void> {
    const i = this.state.turns.length - 1
    const turn = this.state.turns[i]
    const pending = turn?.pending
    if (!pending || this.state.phase !== 'awaitingConfirmation') {
      throw new Error('没有待确认的操作，或它已经处理过了')
    }
    if (pending.expiresAt <= Date.now()) {
      const msg = '确认已过期，请重新发起'
      // 本轮一并标 failed：否则界面上只剩一块"待确认"却没有任何按钮可点。
      this.patchTurn(i, { status: 'failed', error: msg })
      this.patch({ phase: 'failed', error: msg })
      return
    }

    this.patch({ phase: 'confirming' })
    try {
      const reply = await this.transport.confirm(pending.token, this.actor)
      this.patchTurn(i, {
        answer: reply.answer,
        agent: reply.agent,
        degraded: reply.degraded,
        tools: reply.traces.map((t) => t.tool),
        // 令牌在服务端取走即销毁，本地**保留**这条工单：
        // 用户得看得见"刚刚执行了什么"。按钮由 `status` 控制，不靠 pending 是否存在。
        status: 'confirmed',
        error: null,
      })
      this.patch({ phase: 'answered' })
    } catch (e) {
      this.patchTurn(i, { status: 'failed', error: messageOf(e) })
      this.patch({ phase: 'failed', error: messageOf(e) })
    }
  }

  /** 取消当前工单。取消之后 `confirm()` 必定抛错。 */
  reject(): void {
    if (this.state.phase !== 'awaitingConfirmation') return
    const i = this.state.turns.length - 1
    // 与 confirm() 同一策略：保留 pending，让"取消了什么"留在屏幕上。
    this.patchTurn(i, { status: 'rejected' })
    this.patch({ phase: 'rejected' })
  }

  reset(): void {
    this.patch({ phase: 'idle', turns: [], error: null })
  }
}
