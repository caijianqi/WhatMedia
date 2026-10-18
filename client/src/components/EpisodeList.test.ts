/**
 * 剧集季 / 集列表。
 *
 * 测的都是"看代码看不出对不对"的地方：
 *   · 季 tab 上的集数是真的（来自服务端，不是前端瞎猜）
 *   · 切季会真的重新去取那一季的集
 *   · 没拆出季时给的是说明文字，不是一片空白
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import EpisodeList from './EpisodeList.vue'

const seasons = [
  { id: 'sa', number: 1, episode_count: 2 },
  { id: 'sb', number: 2, episode_count: 3 },
]

const eps = (seasonId: string) => ({
  data:
    seasonId === 'sa'
      ? [
          { id: 'e1', item_id: 'i1', series_id: 's1', series_title: '剧', season_number: 1, episode_number: 1, title: '第一集' },
          { id: 'e2', item_id: 'i2', series_id: 's1', series_title: '剧', season_number: 1, episode_number: 2, title: null },
        ]
      : [
          { id: 'e3', item_id: 'i3', series_id: 's1', series_title: '剧', season_number: 2, episode_number: 1, title: '第二季第一集' },
        ],
  page: { total: 2, page: 1, page_size: 500 },
})

function stubFetch(handlers: Array<[string, () => unknown]>) {
  globalThis.fetch = vi.fn((url: string | URL | Request) => {
    const u = String(url)
    for (const [needle, h] of handlers) {
      if (u.includes(needle)) {
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () => Promise.resolve(JSON.stringify(h())),
        } as Response)
      }
    }
    return Promise.resolve({ ok: true, status: 200, text: () => Promise.resolve('{}') } as Response)
  }) as unknown as typeof fetch
}

/** 记录请求过的 URL，用来断言"切季真的重新取数了"。 */
function spyUrls(handlers: Array<[string, () => unknown]>): string[] {
  const urls: string[] = []
  const inner: typeof fetch = ((url: string | URL | Request) => {
    const u = String(url)
    urls.push(u)
    for (const [needle, h] of handlers) {
      if (u.includes(needle)) {
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () => Promise.resolve(JSON.stringify(h())),
        } as Response)
      }
    }
    return Promise.resolve({ ok: true, status: 200, text: () => Promise.resolve('{}') } as Response)
  }) as unknown as typeof fetch
  globalThis.fetch = vi.fn(inner) as unknown as typeof fetch
  return urls
}

function mountList() {
  return mount(EpisodeList, {
    props: { seriesId: 's1' },
    global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
  })
}

beforeEach(() => {
  vi.restoreAllMocks()
})

describe('EpisodeList', () => {
  it('季 tab 上带真实集数，且默认落在第一季', async () => {
    stubFetch([
      ['/seasons', () => seasons],
      ['/episodes', () => eps('sa')],
    ])

    const w = mountList()
    await flushPromises()

    expect(w.text()).toContain('第 1 季')
    expect(w.text()).toContain('2 集')
    expect(w.text()).toContain('第一集')
  })

  it('没有标题的集用"第 N 集"兜底，不显示空白', async () => {
    stubFetch([
      ['/seasons', () => seasons],
      ['/episodes', () => eps('sa')],
    ])

    const w = mountList()
    await flushPromises()

    expect(w.text()).toContain('第 2 集')
  })

  it('切季会真的重新请求那一季的集', async () => {
    const urls = spyUrls([
      ['/seasons', () => seasons],
      ['/episodes', () => eps('sa')],
    ])

    const w = mountList()
    await flushPromises()

    // 第二季的响应：这里换掉 handler，验证请求确实带着新的 season_id 发出去。
    const urls2 = spyUrls([
      ['/seasons', () => seasons],
      ['/episodes', () => eps('sb')],
    ])
    const btn = w.findAll('button').find((b) => b.text().includes('第 2 季'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    await flushPromises()

    expect(urls.some((u) => u.includes('season_id=sa'))).toBe(true)
    expect(urls2.some((u) => u.includes('season_id=sb'))).toBe(true)
  })

  it('没拆出季时给说明文字而不是空白', async () => {
    stubFetch([
      ['/seasons', () => []],
      ['/episodes', () => ({ data: [], page: { total: 0, page: 1, page_size: 500 } })],
    ])

    const w = mountList()
    await flushPromises()

    expect(w.text()).toContain('还没有拆出季')
    expect(w.find('ol').exists()).toBe(false)
  })
})
