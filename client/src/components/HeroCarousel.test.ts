/**
 * 首页 Hero 轮播。
 *
 * 三条值得钉死的规则（都是"改坏了不会报错、只会变难用"的那类）：
 *   · 鼠标停在上面时不自动切换 —— 用户正读简介时画面换了会被当成 bug
 *   · 上一张在 index=0 时要绕到最后一张，不能变空白
 *   · 背景图加载失败要降级成品牌色渐变，而不是留一块黑
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import HeroCarousel, { type HeroSlide } from './HeroCarousel.vue'

const slides: HeroSlide[] = [
  { id: 'a', title: '第一张', year: 2020, rating: 8.2, kind: 'movie', overview: '简介一', genres: ['动作'] },
  { id: 'b', title: '第二张', year: 2021, rating: 7.4, kind: 'episode', overview: '简介二', genres: [] },
]

/**
 * 自动播放依赖定时器。用假定时器而不是真的等 7 秒：
 * 等真时间的测试既慢又不稳（CI 机器一卡就 flaky）。
 */
function mountHero(list: HeroSlide[] = slides, interval = 7000) {
  return mount(HeroCarousel, {
    props: { slides: list, interval },
    global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
  })
}

beforeEach(() => {
  vi.restoreAllMocks()
  vi.useFakeTimers()
})

describe('HeroCarousel', () => {
  it('显示第一张：标题 / 年份 / 评分 / 类型 / 简介', () => {
    const w = mountHero()
    expect(w.text()).toContain('第一张')
    expect(w.text()).toContain('2020')
    expect(w.text()).toContain('8.2')
    expect(w.text()).toContain('电影')
    expect(w.text()).toContain('简介一')
  })

  it('到点自动切到下一张', async () => {
    const w = mountHero()
    vi.advanceTimersByTime(7001)
    await flushPromises()
    expect(w.text()).toContain('第二张')
  })

  it('鼠标悬停时暂停自动切换', async () => {
    const w = mountHero()
    await w.trigger('mouseenter')
    vi.advanceTimersByTime(21000)
    await flushPromises()
    // 还停在第一张 —— 用户正看着的内容不能自己跑掉。
    expect(w.text()).toContain('第一张')
    expect(w.text()).not.toContain('第二张')
  })

  it('在第一张点"上一张"绕到最后一张，而不是空白', async () => {
    const w = mountHero()
    const prev = w.findAll('button').find((b) => b.attributes('aria-label') === '上一张')
    expect(prev).toBeTruthy()
    await prev!.trigger('click')
    expect(w.text()).toContain('第二张')
  })

  it('背景图加载失败时降级成渐变，不留黑块', async () => {
    const w = mountHero()
    const img = w.find('img')
    expect(img.exists()).toBe(true)
    await img.trigger('error')
    await flushPromises()

    // img 被移除、降级块出现，但标题仍在 —— 图挂了不等于这条内容没了。
    expect(w.find('img').exists()).toBe(false)
    expect(w.text()).toContain('第一张')
  })

  it('只有一张时不画切换箭头', () => {
    const w = mountHero([slides[0]!])
    expect(
      w.findAll('button').some((b) => b.attributes('aria-label') === '上一张'),
    ).toBe(false)
  })
})
