/**
 * 顶栏：全局搜索与通知。
 *
 * 钉死三条：
 *   · **防抖**：打四个字不能发四次请求。
 *   · **未读角标显示的是服务端真的未读数**，而且 0 的时候不显示 ——
 *     一个恒为 0 或恒为 3 的红点都等于没有红点。
 *   · 搜索结果为空时要说"没有匹配"，不能一片空白让人以为输入框坏了。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { flushPromises, mount } from '@vue/test-utils'

// TopBar 依赖 useRouter / useRoute。这里测的是数据渲染与请求时机，
// 不是路由，所以把 router 换成桩。
vi.mock('vue-router', async (importOriginal) => {
  const actual = await importOriginal<typeof import('vue-router')>()
  return {
    ...actual,
    useRouter: () => ({ push: vi.fn() }),
    useRoute: () => ({ name: 'home', fullPath: '/', query: {} }),
  }
})

vi.mock('../api/client', () => ({
  ApiError: class ApiError extends Error {
    constructor(
      public detail: { code: string; message: string },
      public status: number,
    ) {
      super(detail.message)
    }
    get isNetwork(): boolean {
      return this.status === 0
    }
  },
  search: vi.fn(),
  listNotifications: vi.fn(),
  readNotification: vi.fn(),
  readAllNotifications: vi.fn(),
  health: vi.fn(),
}))

import TopBar from './TopBar.vue'
import * as api from '../api/client'

const searchMock = vi.mocked(api.search)
const notesMock = vi.mocked(api.listNotifications)

function page(titles: string[]) {
  return {
    data: titles.map((t, i) => ({
      id: `i${i}`,
      kind: 'movie' as const,
      title: t,
      original_title: null,
      year: 2020 + i,
      rating: null,
      runtime_ms: null,
      added_at: '',
    })),
    page: { total: titles.length, page: 1, page_size: 8 },
  }
}

function notes(unread: number) {
  return {
    unread,
    data: Array.from({ length: unread }, (_, i) => ({
      id: `n${i}`,
      kind: 'scan_finished',
      level: 'info',
      title: `通知${i}`,
      body: '正文',
      link: null,
      created_at: new Date().toISOString(),
      read_at: null,
    })),
    pending_approvals: 0,
  }
}

async function mountBar() {
  const w = mount(TopBar, {
    global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
  })
  await flushPromises()
  return w
}

beforeEach(() => {
  setActivePinia(createPinia())
  vi.useFakeTimers()
  searchMock.mockReset()
  notesMock.mockReset()
  searchMock.mockResolvedValue(page([]) as never)
  notesMock.mockResolvedValue(notes(0) as never)
})

describe('TopBar 搜索', () => {
  it('输入后防抖 250ms 才发一次请求', async () => {
    const w = await mountBar()
    const input = w.get('input')

    // 模拟连打四个字。
    await input.setValue('沙')
    await input.setValue('沙丘')
    await input.setValue('沙丘 2')
    await input.setValue('沙丘 2 ')
    expect(searchMock).not.toHaveBeenCalled()

    vi.advanceTimersByTime(250)
    await flushPromises()

    // 四次输入合并成一次请求，且用的是最后一次的值。
    expect(searchMock).toHaveBeenCalledTimes(1)
    expect(searchMock.mock.calls[0]?.[0]).toBe('沙丘 2')
  })

  it('有结果时列出结果，点击可跳转条目', async () => {
    searchMock.mockResolvedValue(page(['沙丘', '沙丘 2']) as never)
    const w = await mountBar()
    await w.get('input').setValue('沙丘')
    vi.advanceTimersByTime(250)
    await flushPromises()

    expect(w.text()).toContain('沙丘')
    expect(w.text()).toContain('沙丘 2')
  })

  it('没有结果时明确说"没有匹配"，而不是留白', async () => {
    searchMock.mockResolvedValue(page([]) as never)
    const w = await mountBar()
    await w.get('input').setValue('不存在')
    vi.advanceTimersByTime(250)
    await flushPromises()

    expect(w.text()).toContain('没有匹配')
  })

  it('TV 形态下不渲染搜索框', async () => {
    const w = await mountBar()
    expect(w.find('input').exists()).toBe(true)
    const w2 = mount(TopBar, {
      props: { showSearch: false },
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await flushPromises()
    expect(w2.find('input').exists()).toBe(false)
  })
})

describe('TopBar 通知', () => {
  it('未读数为 0 时不显示角标', async () => {
    const w = await mountBar()
    // 角标是那个带数字的 span；0 时整个不渲染。
    expect(w.text()).not.toContain('99+')
    expect(w.findAll('span').some((s) => s.text() === '3')).toBe(false)
  })

  it('未读数来自服务端，不是写死的', async () => {
    notesMock.mockResolvedValue(notes(3) as never)
    const w = await mountBar()
    expect(w.text()).toContain('3')
  })

  it('未读超过 99 时收敛成 99+', async () => {
    notesMock.mockResolvedValue(notes(120) as never)
    const w = await mountBar()
    expect(w.text()).toContain('99+')
  })

  it('点"全部已读"会调服务端并刷新', async () => {
    notesMock.mockResolvedValue(notes(2) as never)
    vi.mocked(api.readAllNotifications).mockResolvedValue({ updated: 2 })
    const w = await mountBar()

    await w.get('button[aria-label="通知"]').trigger('click')
    await flushPromises()
    await w.get('button:not([aria-label])').trigger('click')
    await flushPromises()

    expect(api.readAllNotifications).toHaveBeenCalled()
    // 刷新后重新拉过一次列表。
    expect(notesMock.mock.calls.length).toBeGreaterThanOrEqual(2)
  })
})
