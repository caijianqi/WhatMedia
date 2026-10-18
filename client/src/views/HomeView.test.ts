/**
 * 首页的渲染测试。
 *
 * 验证四件**只看编译通过看不出来的**事：
 *   1. 真的会按端点去取数，取回来真的渲染成海报
 *   2. 分类 tab 来自数据（`mixed` 这类也要能显示），不是硬编码清单
 *   3. Hero 的某一条详情取失败，只影响那一条，不整块消失
 *   4. 空库时显示的是"怎么开始"的引导，而不是一片空白
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import HomeView from './HomeView.vue'
import HeroCarousel from '../components/HeroCarousel.vue'

const page = (data: unknown[]) => ({
  data,
  page: { total: data.length, page: 1, page_size: 24 },
})

const movie = { id: 'm1', kind: 'movie', title: '电影一', year: 2020, rating: 8.4, added_at: '' }
const episode = { id: 'e1', kind: 'episode', title: '剧集一', year: 2021, rating: 7.1, added_at: '' }

const detail = (id: string) => ({
  id,
  kind: 'movie',
  title: '电影一',
  added_at: '',
  overview: '一段简介',
  genres: ['动作', '科幻'],
  versions: [],
})

/** 按 URL 片段给不同的响应。**顺序敏感**：先写的先匹配。 */
function stubFetch(handlers: Array<[string, () => unknown]>) {
  globalThis.fetch = vi.fn((url: string | URL | Request) => {
    const u = String(url)
    for (const [needle, h] of handlers) {
      if (u.includes(needle)) {
        try {
          const body = h()
          return Promise.resolve({
            ok: true,
            status: 200,
            text: () => Promise.resolve(JSON.stringify(body)),
          } as Response)
        } catch {
          // 模拟 500：request() 会把它变成抛错，调用方各自降级。
          return Promise.resolve({
            ok: false,
            status: 500,
            text: () => Promise.resolve('{"error":{"code":"X","message":"boom"}}'),
          } as Response)
        }
      }
    }
    return Promise.resolve({
      ok: true,
      status: 200,
      text: () => Promise.resolve('{}'),
    } as Response)
  }) as unknown as typeof fetch
}

/** 空库。 */
const emptyHandlers: Array<[string, () => unknown]> = [
  ['/media/kinds', () => []],
  ['/playback/resume', () => []],
  ['/media', () => page([])],
  ['/items/', () => detail('m1')],
]

function mountHome() {
  return mount(HomeView, {
    // 首页要读布局 store（列数 / 是否有指针），所以必须有 pinia。
    global: {
      plugins: [createPinia()],
      stubs: { RouterLink: { template: '<a><slot /></a>' } },
    },
  })
}

beforeEach(() => {
  vi.restoreAllMocks()
})

describe('HomeView', () => {
  it('取回来的条目会渲染成海报行', async () => {
    stubFetch([
      ['/media/kinds', () => [{ kind: 'movie', count: 1 }]],
      ['/playback/resume', () => []],
      ['/items/', () => detail('m1')],
      ['/media', () => page([movie])],
    ])

    const w = mountHome()
    await flushPromises()

    expect(w.text()).toContain('电影一')
    expect(w.text()).toContain('最近添加')
    expect(w.text()).toContain('高分推荐')
  })

  it('分类 tab 由数据决定：只有库里有的类型才会出现', async () => {
    stubFetch([
      ['/media/kinds', () => [{ kind: 'movie', count: 3 }]],
      ['/playback/resume', () => []],
      ['/items/', () => detail('m1')],
      ['/media', () => page([movie])],
    ])

    const w = mountHome()
    await flushPromises()

    expect(w.text()).toContain('全部')
    expect(w.text()).toContain('电影 3')
    // 库里没有剧集，就不该有剧集 tab —— 点进去永远空的 tab 比没有更糟。
    expect(w.text()).not.toContain('剧集 0')
  })

  it('Hero 的某一条详情取失败，只丢那一条的简介，标题还在', async () => {
    stubFetch([
      ['/media/kinds', () => [{ kind: 'movie', count: 1 }]],
      ['/playback/resume', () => []],
      ['/items/', () => detail('m1')],
      ['/media', () => page([movie])],
    ])

    const w = mountHome()
    await flushPromises()
    expect(w.text()).toContain('一段简介')

    // 第二轮：详情 500。Hero 仍要渲染（标题还在），只是没有简介。
    stubFetch([
      ['/media/kinds', () => [{ kind: 'movie', count: 1 }]],
      ['/playback/resume', () => []],
      ['/items/', () => { throw new Error('boom') }],
      ['/media', () => page([movie])],
    ])
    const w2 = mountHome()
    await flushPromises()
    expect(w2.text()).toContain('电影一')
    expect(w2.text()).not.toContain('一段简介')
  })

  it('空库时呈现接入指引而不是空白，且不凭空冒出任何影片', async () => {
    stubFetch(emptyHandlers)

    const w = mountHome()
    await flushPromises()

    expect(w.text()).toContain('你的家庭影院')
    expect(w.text()).toContain('连接与设置媒体库')

    // 反向断言。库是空的却显示出片名，曾经真的发生过：
    // 首页在接口返回空数组时回退到一份写死的演示数据集，
    // 于是"没扫过片"和"扫到了 23 部"在界面上长得一模一样。
    expect(w.findComponent(HeroCarousel).exists()).toBe(false)
    expect(w.text()).not.toContain('电影一')
    expect(w.text()).not.toContain('剧集一')
  })

  it('有续播记录时显示"继续观看"', async () => {
    stubFetch([
      ['/media/kinds', () => []],
      [
        '/playback/resume',
        () => [
          {
            item_id: 'm1',
            title: '电影一',
            kind: 'movie',
            position_ms: 600_000,
            duration_ms: 1_200_000,
          },
        ],
      ],
      ['/items/', () => detail('m1')],
      ['/media', () => page([movie])],
    ])

    const w = mountHome()
    await flushPromises()

    expect(w.text()).toContain('继续观看')
  })

  it('切到某个类型后，海报行只请求该类型', async () => {
    const urls: string[] = []
    stubFetch([
      ['/media/kinds', () => [{ kind: 'episode', count: 2 }]],
      ['/playback/resume', () => []],
      ['/items/', () => detail('e1')],
      ['/media', () => page([episode])],
    ])
    const raw = globalThis.fetch
    globalThis.fetch = vi.fn((url: string | URL | Request) => {
      urls.push(String(url))
      return (raw as (u: unknown) => Promise<Response>)(url)
    }) as unknown as typeof fetch

    const w = mountHome()
    await flushPromises()

    const tab = w.findAll('button').find((b) => b.text().includes('剧集'))
    expect(tab).toBeTruthy()
    await tab!.trigger('click')
    await flushPromises()

    expect(urls.some((u) => u.includes('kind=episode'))).toBe(true)
  })
})
