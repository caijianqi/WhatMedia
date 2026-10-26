/**
 * 全网多源影视聚合独立模块（AggregatorView）单测。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { flushPromises, mount } from '@vue/test-utils'

vi.mock('../api/client', () => ({
  listVodSources: vi.fn(),
  getHotVodItems: vi.fn(),
  searchAggregate: vi.fn(),
  checkVodImported: vi.fn(),
  importVodItem: vi.fn(),
  getVodStreamProxyUrl: vi.fn((url: string) => `/proxy?url=${url}`),
}))

vi.mock('vue-router', async (importOriginal) => {
  const actual = await importOriginal<typeof import('vue-router')>()
  return {
    ...actual,
    useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
    useRoute: () => ({ name: 'aggregator', query: {} }),
  }
})

import AggregatorView from './AggregatorView.vue'
import * as api from '../api/client'
import type { AggregatedVodItem } from '../api/client'

const mockListSources = vi.mocked(api.listVodSources)
const mockGetHot = vi.mocked(api.getHotVodItems)
const mockSearch = vi.mocked(api.searchAggregate)
const mockCheckImported = vi.mocked(api.checkVodImported)
const mockImportVod = vi.mocked(api.importVodItem)

function mockVodItem(id: string, title: string, type_name = '科幻片'): AggregatedVodItem {
  return {
    id,
    title,
    clean_title: title.toLowerCase(),
    sub_title: undefined,
    year: 2024,
    poster: 'https://example.com/poster.jpg',
    type_name,
    remark: 'HD国语',
    actor: '主演',
    director: '导演',
    content: '简介',
    routes: [
      {
        source_id: 'lz',
        source_name: '量子资源',
        play_from: 'lzm3u8',
        episodes: [{ name: '正片', play_url: 'https://example.com/play.m3u8', index: 1 }],
      },
    ],
  }
}

async function mountView() {
  const w = mount(AggregatorView, {
    global: {
      stubs: {
        RouterLink: { template: '<a><slot /></a>' },
        VodDetailModal: { template: '<div class="vod-detail-modal-stub" />' },
      },
    },
  })
  await flushPromises()
  return w
}

beforeEach(() => {
  setActivePinia(createPinia())
  mockListSources.mockReset()
  mockGetHot.mockReset()
  mockSearch.mockReset()
  mockCheckImported.mockReset()
  mockImportVod.mockReset()

  mockListSources.mockResolvedValue([
    { id: 'lz', name: '量子资源', api_url: 'https://lz.com', is_active: true, weight: 10, timeout_ms: 2000 },
  ])
  mockGetHot.mockResolvedValue([
    mockVodItem('1', '黑神话：悟空', '动漫'),
    mockVodItem('2', '流浪地球 2', '科幻片'),
  ])
  mockCheckImported.mockResolvedValue({ imported: false, item_id: null })
  mockImportVod.mockResolvedValue({ item_id: 'item-uuid-1', library_id: 'lib-uuid', title: '流浪地球 2', is_new: true })
})

describe('AggregatorView', () => {
  it('默认加载全网热播流与采集源列表', async () => {
    const w = await mountView()
    expect(mockListSources).toHaveBeenCalled()
    expect(mockGetHot).toHaveBeenCalled()
    expect(w.text()).toContain('全网影视聚合大厅')
    expect(w.text()).toContain('黑神话：悟空')
    expect(w.text()).toContain('流浪地球 2')
  })

  it('输入关键词触发全网并发检索', async () => {
    mockSearch.mockResolvedValue({
      local: { data: [], page: { total: 0, page: 1, page_size: 10 } },
      remote: [mockVodItem('3', '三体', '国产剧')],
    })
    const w = await mountView()
    const input = w.get('input[type="search"]')
    await input.setValue('三体')
    await w.get('form').trigger('submit')
    await flushPromises()

    expect(mockSearch).toHaveBeenCalledWith('三体', expect.anything())
    expect(w.text()).toContain('三体')
  })

  it('点击入库按钮调用 importVodItem 并更新按钮状态', async () => {
    const w = await mountView()
    const importBtn = w.findAll('.btn-card-import')[1]!
    expect(importBtn.text()).toContain('+ 入库')

    await importBtn.trigger('click')
    await flushPromises()

    expect(mockImportVod).toHaveBeenCalledWith(expect.objectContaining({ title: '流浪地球 2' }))
    expect(importBtn.text()).toContain('✓ 已在库')
  })

  it('按分类标签过滤显示内容', async () => {
    const w = await mountView()
    expect(w.text()).toContain('黑神话：悟空')
    expect(w.text()).toContain('流浪地球 2')

    // 切换到"电影"
    const movieTab = w.findAll('.tab-btn').find((b) => b.text() === '电影')
    await movieTab?.trigger('click')
    await flushPromises()

    expect(w.text()).toContain('流浪地球 2')
    expect(w.text()).not.toContain('黑神话：悟空')
  })
})
