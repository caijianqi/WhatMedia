/**
 * 首页的渲染测试。
 *
 * 验证三件事，都是**只看编译通过看不出来的**：
 *   1. 真的会按端点去取数，取回来真的渲染成卡片
 *   2. 某一行失败不影响其它行（刮削挂了不该让整个首页空白）
 *   3. 空库时显示的是"还没有内容"而不是一片空白
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import HomeView from './HomeView.vue'

const page = (data: unknown[]) => ({
  data,
  page: { total: data.length, page: 1, page_size: 18 },
})

const movie = { id: 'm1', kind: 'movie', title: '电影一', year: 2020, added_at: '' }
const series = { id: 's1', kind: 'series', title: '剧集一', year: 2021, added_at: '' }

/** 按 URL 前缀给不同的响应。 */
function stubFetch(handlers: Record<string, () => unknown>) {
  globalThis.fetch = vi.fn((url: string | URL | Request) => {
    const u = String(url)
    for (const [prefix, h] of Object.entries(handlers)) {
      if (u.includes(prefix)) {
        return Promise.resolve({ ok: true, status: 200, text: () => Promise.resolve(JSON.stringify(h())) } as Response)
      }
    }
    return Promise.resolve({ ok: true, status: 200, text: () => Promise.resolve('{}') } as Response)
  }) as unknown as typeof fetch
}

function mountHome() {
  return mount(HomeView, {
    global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
  })
}

beforeEach(() => {
  vi.restoreAllMocks()
})

describe('HomeView', () => {
  it('取回来的条目会渲染出来', async () => {
    stubFetch({
      '/playback/resume': () => [],
      'kind=series': () => page([series]),
      '/media': () => page([movie]),
      '/movies': () => page([movie]),
      '/series': () => page([series]),
    })

    const w = mountHome()
    await flushPromises()

    expect(w.text()).toContain('电影一')
    expect(w.text()).toContain('剧集一')
    expect(w.text()).not.toContain('还没有内容')
  })

  it('某一行失败只影响那一行，其它行照常显示', async () => {
    stubFetch({
      '/playback/resume': () => [],
      'kind=series': () => page([series]),
      '/media': () => page([movie]),
      '/movies': () => {
        throw new Error('boom')
      },
      '/series': () => page([series]),
    })

    const w = mountHome()
    await flushPromises()

    expect(w.text()).toContain('电影一')
    expect(w.text()).toContain('剧集一')
  })

  it('空库显示"还没有内容"而不是空白', async () => {
    stubFetch({
      '/playback/resume': () => [],
      'kind=series': () => page([]),
      '/media': () => page([]),
      '/movies': () => page([]),
      '/series': () => page([]),
    })

    const w = mountHome()
    await flushPromises()

    expect(w.text()).toContain('还没有内容')
  })

  it('有续播记录时显示"继续观看"', async () => {
    stubFetch({
      '/playback/resume': () => [
        { item_id: 'm1', title: '电影一', kind: 'movie', position_ms: 600_000, duration_ms: 1_200_000 },
      ],
      'kind=series': () => page([series]),
      '/media': () => page([movie]),
      '/movies': () => page([movie]),
      '/series': () => page([series]),
    })

    const w = mountHome()
    await flushPromises()

    expect(w.text()).toContain('继续观看')
  })
})
