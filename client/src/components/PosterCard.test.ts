/**
 * 海报卡片的渲染测试。
 *
 * 覆盖的是**降级路径**而不是正常路径：刮削没跑完的库里大片条目都没有海报，
 * 那条路径上的表现比"有海报时显示图片"重要得多 ——
 * 满屏灰色占位方块时，用户根本分不出哪个是哪个。
 */

import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import PosterCard from './PosterCard.vue'
import type { ItemSummary } from '../api/client'

const item: ItemSummary = {
  id: 'abc',
  kind: 'movie',
  title: '失控玩家',
  year: 2021,
  added_at: '2026-01-01T00:00:00Z',
}

function mountCard(props: Partial<{ item: ItemSummary; progress: number }> = {}) {
  return mount(PosterCard, {
    props: { item, ...props } as never,
    global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
  })
}

describe('PosterCard', () => {
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
    // jsdom 不会真的去请求图片，手动触发 error。
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
})
