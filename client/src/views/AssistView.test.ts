/**
 * AI 助手页的渲染测试（P7）。
 *
 * 验证的是**服务端那道闸在界面上真的看得见**：
 *
 *   1. 危险工具返回的工单会渲染成一块带摘要的面板，而不是一句"好的"
 *   2. 没有待确认工单时，确认按钮根本不存在（不是置灰，是不渲染）
 *   3. 点取消之后确认按钮消失
 *
 * `assistant.test.ts` 已经证明逻辑层不会乱发请求；这份测试证明的是
 * 用户在界面上看得到自己在确认什么。只测一端不够——逻辑对了但按钮
 * 常年置灰（或常年可点），是同一类事故。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils'
import AssistView from './AssistView.vue'

const ticket = {
  id: 'act-1',
  token: 'tok-1',
  tool: 'scan_library',
  risk: 'write',
  summary: '扫描媒体库「电影」',
  lines: ['电影：全量扫描（增量）'],
  warnings: ['扫描期间请保持磁盘在线'],
  expires_in_secs: 600,
}

const readOnlyReply = {
  answer: '找到 3 部',
  agent: 'media',
  pending: null,
  traces: [{ tool: 'search_media', ok: true, summary: '3 部' }],
  degraded: false,
}

const pendingReply = {
  answer: '需要先你确认',
  agent: 'library',
  pending: ticket,
  traces: [{ tool: 'scan_library', ok: false, summary: '等待用户确认' }],
  degraded: false,
}

const toolsReply = {
  data: [
    { name: 'search_media', description: '搜索媒体', risk: 'read_only', params: [] },
  ],
}

/** `/ai/tools` 固定返回工具清单，其余一律返回 `chatReply`。 */
function stubFetch(chatReply: unknown): void {
  globalThis.fetch = vi.fn((url: string | URL | Request) => {
    const u = String(url)
    const body = u.includes('/ai/tools') ? toolsReply : chatReply
    return Promise.resolve({
      ok: true,
      status: 200,
      text: () => Promise.resolve(JSON.stringify(body)),
    } as Response)
  }) as unknown as typeof fetch
}

async function boot(chatReply: unknown): Promise<VueWrapper> {
  stubFetch(chatReply)
  const w = mount(AssistView)
  await flushPromises()
  return w
}

async function ask(w: VueWrapper, text: string): Promise<void> {
  await w.find('[data-test="input"]').setValue(text)
  await w.find('form').trigger('submit')
  await flushPromises()
}

let w: VueWrapper | null = null

describe('AssistView', () => {
  beforeEach(() => {
    w?.unmount()
    w = null
  })

  it('展示服务端给出的工具清单', async () => {
    w = await boot(readOnlyReply)
    expect(w.text()).toContain('search_media')
  })

  it('只读回答不渲染确认按钮', async () => {
    w = await boot(readOnlyReply)
    await ask(w, '有哪些科幻片')
    expect(w.text()).toContain('找到 3 部')
    expect(w.find('[data-test="confirm"]').exists()).toBe(false)
  })

  it('危险工具把工单整块渲染出来，并给出唯一的执行入口', async () => {
    w = await boot(pendingReply)
    await ask(w, '扫描一下媒体库')

    expect(w.text()).toContain('扫描媒体库「电影」')
    expect(w.text()).toContain('电影：全量扫描（增量）')
    expect(w.text()).toContain('扫描期间请保持磁盘在线')
    expect(w.find('[data-test="confirm"]').exists()).toBe(true)
  })

  it('取消之后确认按钮消失', async () => {
    w = await boot(pendingReply)
    await ask(w, '扫描一下媒体库')
    expect(w.find('[data-test="confirm"]').exists()).toBe(true)

    await w.find('[data-test="reject"]').trigger('click')
    await flushPromises()

    expect(w.find('[data-test="confirm"]').exists()).toBe(false)
    expect(w.text()).toContain('已取消')
  })

  it('降级回答会标出离线模式', async () => {
    w = await boot({ ...readOnlyReply, degraded: true })
    await ask(w, '找出没有海报的')
    expect(w.find('[data-test="degraded"]').exists()).toBe(true)
  })
})
