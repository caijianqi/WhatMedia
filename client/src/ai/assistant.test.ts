/**
 * AI 助手会话测试（P7）。
 *
 * 这些测试守的不是渲染，而是**权限语义**：
 *
 *   · 危险工具必须停在待确认，不能自己往下走
 *   · 没有工单时 confirm() 一个字节都不该发到服务端
 *   · 取消之后 confirm() 必须失败
 *   · 执行成功后工单即销毁，不能点第二次
 *
 * 任何一条被改坏，用户可能在没有看清预览的情况下把库扫了一遍。
 * 这类 bug 不会报错，只会"莫名其妙地动了我的文件"，所以必须在测试里钉死。
 */

import { describe, expect, it, vi } from 'vitest'
import * as api from '../api/client'
import { AssistantSession, type AiTransport, type Actor } from './assistant'

const actor: Actor = { id: 'u1', role: 'adult' }

function reply(over: Partial<api.AiChatReply> = {}): api.AiChatReply {
  return {
    answer: '好的',
    agent: 'media',
    pending: null,
    traces: [],
    degraded: false,
    ...over,
  }
}

const pendingView: api.PendingActionView = {
  id: 'act-1',
  token: 'tok-1',
  tool: 'scan_library',
  risk: 'write',
  summary: '扫描媒体库「电影」',
  lines: ['电影：全量扫描（增量）'],
  warnings: ['扫描期间请保持磁盘在线'],
  expires_in_secs: 600,
}

/** 记录调用次数的假传输层。 */
function fake(over: Partial<AiTransport> = {}) {
  const chat = vi.fn(async () => reply())
  const confirm = vi.fn(async (_token: string, _actor: api.AiActor) =>
    reply({ answer: '已开始扫描' }),
  )
  return {
    transport: { chat, confirm, ...over } as AiTransport,
    chat,
    confirm,
  }
}

function session(t: AiTransport): AssistantSession {
  return new AssistantSession(t, actor)
}

describe('只读提问', () => {
  it('问完即答，不需要确认', async () => {
    const { transport } = fake({ chat: async () => reply({ answer: '找到 3 部' }) })
    const s = session(transport)
    await s.ask('有哪些科幻片')
    expect(s.snapshot.phase).toBe('answered')
    expect(s.snapshot.turns[0]?.answer).toBe('找到 3 部')
    expect(s.hasPending).toBe(false)
  })

  it('空问题不发请求', async () => {
    const { transport, chat } = fake()
    const s = session(transport)
    await s.ask('   ')
    expect(chat).not.toHaveBeenCalled()
    expect(s.snapshot.error).toBe('请输入问题')
  })

  it('规则引擎兜底时 degraded 为 true', async () => {
    const { transport } = fake({ chat: async () => reply({ degraded: true }) })
    const s = session(transport)
    await s.ask('找出没有海报的')
    expect(s.snapshot.turns[0]?.degraded).toBe(true)
  })

  it('请求失败进 failed 状态而不是抛出', async () => {
    const { transport } = fake({
      chat: async () => {
        throw new Error('AI 未启用')
      },
    })
    const s = session(transport)
    await s.ask('找片子')
    expect(s.snapshot.phase).toBe('failed')
    expect(s.snapshot.error).toBe('AI 未启用')
    expect(s.snapshot.turns[0]?.status).toBe('failed')
  })
})

describe('危险操作的两步确认', () => {
  it('预览必须先于执行：拿到工单后停在待确认', async () => {
    const { transport, confirm } = fake({ chat: async () => reply({ pending: pendingView }) })
    const s = session(transport)
    await s.ask('扫描一下媒体库')

    expect(s.snapshot.phase).toBe('awaitingConfirmation')
    expect(s.hasPending).toBe(true)
    expect(s.snapshot.turns[0]?.pending?.token).toBe('tok-1')
    // 关键：预览阶段一次 confirm 都没发
    expect(confirm).not.toHaveBeenCalled()
  })

  it('确认后才执行，且带上同一张令牌', async () => {
    const { transport, confirm } = fake({ chat: async () => reply({ pending: pendingView }) })
    const s = session(transport)
    await s.ask('扫描一下媒体库')
    await s.confirm()

    expect(confirm).toHaveBeenCalledTimes(1)
    expect(confirm.mock.calls[0]?.[0]).toBe('tok-1')
    expect(s.snapshot.turns[0]?.status).toBe('confirmed')
    expect(s.snapshot.phase).toBe('answered')
  })

  it('没有工单时 confirm 不发任何请求', async () => {
    const { transport, confirm } = fake()
    const s = session(transport)
    await s.ask('有哪些科幻片')

    await expect(s.confirm()).rejects.toThrow(/没有待确认/)
    expect(confirm).not.toHaveBeenCalled()
  })

  it('从未提问就 confirm 也不发请求', async () => {
    const { transport, confirm } = fake()
    const s = session(transport)
    await expect(s.confirm()).rejects.toThrow(/没有待确认/)
    expect(confirm).not.toHaveBeenCalled()
  })

  it('拒绝之后不可执行', async () => {
    const { transport, confirm } = fake({ chat: async () => reply({ pending: pendingView }) })
    const s = session(transport)
    await s.ask('扫描一下媒体库')
    s.reject()

    expect(s.snapshot.phase).toBe('rejected')
    expect(s.snapshot.turns[0]?.status).toBe('rejected')
    // 工单留在本地供界面留痕，"不能执行"靠 phase 挡，不靠把 pending 抹掉。
    expect(s.snapshot.turns[0]?.pending).not.toBeNull()

    await expect(s.confirm()).rejects.toThrow(/没有待确认/)
    expect(confirm).not.toHaveBeenCalled()
  })

  it('执行成功后不能点第二次', async () => {
    const { transport, confirm } = fake({ chat: async () => reply({ pending: pendingView }) })
    const s = session(transport)
    await s.ask('扫描一下媒体库')
    await s.confirm()

    expect(s.snapshot.turns[0]?.status).toBe('confirmed')
    expect(s.snapshot.phase).toBe('answered')

    await expect(s.confirm()).rejects.toThrow(/没有待确认/)
    expect(confirm).toHaveBeenCalledTimes(1)
  })

  it('工单过期后 confirm 不发请求，只提示重来', async () => {
    const { transport, confirm } = fake({
      chat: async () => reply({ pending: { ...pendingView, expires_in_secs: -1 } }),
    })
    const s = session(transport)
    await s.ask('扫描一下媒体库')
    await s.confirm()

    expect(confirm).not.toHaveBeenCalled()
    expect(s.snapshot.phase).toBe('failed')
    expect(s.snapshot.error).toContain('过期')
  })

  it('执行失败时本轮标为 failed 并保留原因', async () => {
    const { transport } = fake({
      chat: async () => reply({ pending: pendingView }),
      confirm: async () => {
        throw new Error('令牌已被使用')
      },
    })
    const s = session(transport)
    await s.ask('扫描一下媒体库')
    await s.confirm()

    expect(s.snapshot.phase).toBe('failed')
    expect(s.snapshot.turns[0]?.error).toBe('令牌已被使用')
  })
})

describe('多轮会话', () => {
  it('新一轮提问会清掉上一轮的待确认状态', async () => {
    const { transport } = fake({ chat: async () => reply({ pending: pendingView }) })
    const s = session(transport)
    await s.ask('扫描一下媒体库')
    await s.ask('有哪些科幻片')

    expect(s.snapshot.turns).toHaveLength(2)
    expect(s.snapshot.turns[0]?.status).toBe('awaiting')
    expect(s.snapshot.phase).toBe('awaitingConfirmation')
  })

  it('订阅者能收到状态变化', async () => {
    const { transport } = fake()
    const s = session(transport)
    const seen: string[] = []
    s.subscribe((st) => seen.push(st.phase))
    await s.ask('有哪些科幻片')
    expect(seen).toContain('asking')
    expect(seen[seen.length - 1]).toBe('answered')
  })
})
