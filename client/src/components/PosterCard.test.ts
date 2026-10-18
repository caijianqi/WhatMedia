/**
 * 海报卡片的渲染与交互测试。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import PosterCard from './PosterCard.vue'
import type { ItemSummary } from '../api/client'
import * as api from '../api/client'

vi.mock('../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof api>()
  return {
    ...actual,
    setFavorite: vi.fn(),
  }
})

const item: ItemSummary = {
  id: 'abc',
  kind: 'movie',
  title: '失控玩家',
  year: 2021,
  added_at: '2026-01-01T00:00:00Z',
}

function mountCard(
  props: Partial<{
    item: ItemSummary
    progress: number
    quality: string
    isFavorite: boolean
  }> = {},
) {
  return mount(PosterCard, {
    props: { item, ...props } as never,
    global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
  })
}

describe('PosterCard', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('渲染标题与年份', () => {
    const w = mountCard()
    expect(w.text()).toContain('失控玩家')
    expect(w.text()).toContain('2021')
  })

  it('默认尝试加载海报', () => {
    const w = mountCard()
    const img = w.find('img')
    expect(img.exists()).toBe(true)
    expect(img.attributes('src')).toBe('/api/v1/items/abc/images/poster')
  })

  it('海报加载失败时降级成标题首字，而不是留一片灰', () => {
    const w = mountCard()
    const img = w.find('img')
    img.trigger('error')
    expect(w.text()).toContain('失')
  })

  it('有进度时画出进度条，百分比是整数', async () => {
    const w = mountCard({ progress: 0.375 })
    const bar = w.find('[role="progressbar"]')
    expect(bar.exists()).toBe(true)
    expect(bar.attributes('aria-valuenow')).toBe('38')
  })

  it('没有进度时不画进度条', () => {
    const w = mountCard()
    expect(w.find('[role="progressbar"]').exists()).toBe(false)
  })

  it('传 quality 渲染品质角标', () => {
    const w = mountCard({ quality: '4K HDR' })
    expect(w.text()).toContain('4K HDR')
    expect(w.find('.badge-quality').exists()).toBe(true)
  })

  it('点击收藏按钮切换状态并调用 api 与 emit 事件', async () => {
    vi.mocked(api.setFavorite).mockResolvedValueOnce({
      item_id: 'abc',
      favorite: true,
      count: 1,
    })

    const w = mountCard({ isFavorite: false })
    const btn = w.find('button[aria-label="添加收藏"]')
    expect(btn.exists()).toBe(true)

    await btn.trigger('click')
    expect(api.setFavorite).toHaveBeenCalledWith('abc', true)
    expect(w.emitted('favorite-change')).toBeTruthy()
    expect(w.emitted('favorite-change')![0]).toEqual([true])
  })
})
