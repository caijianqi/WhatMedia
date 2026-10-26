/**
 * 媒体库页面的多态视图与筛选交互测试。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import LibraryView from './LibraryView.vue'
import * as api from '../api/client'

const mockRouter = {
  push: vi.fn(),
}

let mockQuery: Record<string, string> = {}

vi.mock('vue-router', () => ({
  useRoute: () => ({ query: mockQuery }),
  useRouter: () => mockRouter,
  RouterLink: {
    template: '<a><slot /></a>',
  },
}))

vi.mock('../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof api>()
  return {
    ...actual,
    listMedia: vi.fn(),
    mediaKinds: vi.fn(),
    getSettings: vi.fn(),
    putSetting: vi.fn(),
    resumeList: vi.fn(),
    stopSession: vi.fn(),
  }
})

const sampleMedia: api.ItemSummary[] = [
  { id: '1', title: '测试影片一', kind: 'movie', year: 2023, rating: 8.3, added_at: '2026-01-01' },
  { id: '2', title: '测试剧集一', kind: 'series', year: 2024, rating: 8.0, added_at: '2026-01-02' },
]

function mountLibrary() {
  return mount(LibraryView, {
    global: {
      stubs: {
        RouterLink: { template: '<a><slot /></a>' },
      },
    },
  })
}

describe('LibraryView', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    mockQuery = {}
    vi.clearAllMocks()

    vi.mocked(api.mediaKinds).mockResolvedValue([
      { kind: 'movie', count: 12 },
      { kind: 'series', count: 8 },
    ])
    vi.mocked(api.getSettings).mockResolvedValue({ entries: [], rejected: [] })
    vi.mocked(api.listMedia).mockResolvedValue({
      data: sampleMedia,
      page: { total: 2, page: 1, page_size: 48 },
    })
  })

  it('进入页面拉取媒体数据并展示统计与海报', async () => {
    const w = mountLibrary()
    await flushPromises()

    expect(w.text()).toContain('测试影片一')
    expect(w.text()).toContain('测试剧集一')
    expect(w.text()).toContain('找到 2 项内容')
  })

  it('能够切换到列表视图并渲染列表条目', async () => {
    const w = mountLibrary()
    await flushPromises()

    const listBtn = w.find('button[aria-label="列表视图"]')
    expect(listBtn.exists()).toBe(true)

    await listBtn.trigger('click')
    await flushPromises()

    // 列表视图中包含 "已入库" 标签
    expect(w.text()).toContain('已入库')
    expect(api.putSetting).toHaveBeenCalledWith('ui.library_view', 'list')
  })

  it('有筛选条件时展示清除筛选按钮并可触发重置', async () => {
    mockQuery = { kind: 'movie' }
    const w = mountLibrary()
    await flushPromises()

    const clearBtn = w.findAll('button').find((b) => b.text().includes('清除筛选'))
    expect(clearBtn).toBeTruthy()

    await clearBtn!.trigger('click')
    expect(mockRouter.push).toHaveBeenCalledWith({ name: 'library' })
  })

  it('点击重新整理按钮重新请求数据', async () => {
    const w = mountLibrary()
    await flushPromises()

    const refreshBtn = w.find('button[title="重新整理"]')
    expect(refreshBtn.exists()).toBe(true)

    await refreshBtn.trigger('click')
    expect(api.listMedia).toHaveBeenCalledTimes(2)
  })

  it('无内容时渲染 EmptyState', async () => {
    vi.mocked(api.listMedia).mockResolvedValueOnce({
      data: [],
      page: { total: 0, page: 1, page_size: 48 },
    })

    const w = mountLibrary()
    await flushPromises()

    expect(w.text()).toContain('找到 0 项内容')
    expect(w.findComponent({ name: 'EmptyState' }).exists()).toBe(true)
  })

  it('当 filter=continue 时拉取真实续播列表而非展示全部媒体', async () => {
    mockQuery = { filter: 'continue' }
    vi.mocked(api.resumeList).mockResolvedValueOnce([
      {
        item_id: 'res-1',
        file_id: 'file-1',
        session_id: 'sess-1',
        title: '流浪地球 2 观看中',
        position_ms: 100000,
        duration_ms: 200000,
        kind: 'movie',
        strategy: 'direct',
        updated_at: '2026-01-01T00:00:00Z',
      },
    ])

    const w = mountLibrary()
    await flushPromises()

    expect(api.resumeList).toHaveBeenCalledWith(100)
    expect(w.text()).toContain('继续观看')
    expect(w.text()).toContain('流浪地球 2 观看中')
    expect(w.findComponent({ name: 'ContinueCard' }).exists()).toBe(true)
  })
})
