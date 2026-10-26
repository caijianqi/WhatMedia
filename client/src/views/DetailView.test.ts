/**
 * 详情页。
 *
 * 重点测两件"数据来了才看得出对不对"的事：
 *   · 演职员按服务端分好的 cast / crew 渲染，幕后按职能归并
 *   · 只有"这一集属于某部剧"时才渲染季列表；电影不该出现"剧集"两个字
 *
 * 顺带钉死一条：**附属数据（演职员 / 剧集）取失败不能让详情页打不开**。
 * 没刮削过的库里这两个端点就是空的，那是常态不是故障。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import DetailView from './DetailView.vue'

// 详情页从路由拿 id。`useRoute()` 读的是注入的 router，
// 而 `mocks: { $route }` 对它无效（那是 Options API 的路径），
// 所以直接把 useRoute 换成固定值 —— 这里测的是数据渲染，不是路由。
vi.mock('vue-router', async (importOriginal) => {
  const actual = await importOriginal<typeof import('vue-router')>()
  return {
    ...actual,
    useRoute: () => ({ params: { id: 'i1' }, fullPath: '/item/i1' }),
    useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  }
})

const item = {
  id: 'i1',
  kind: 'episode',
  title: '剧集一',
  added_at: '',
  year: 2021,
  rating: 7.8,
  runtime_ms: 2_700_000,
  overview: '一段简介',
  genres: ['剧情', '悬疑'],
  versions: [],
}

const credits = {
  cast: [
    { person: '张三', role: 'actor', character: '主角', sort_order: 0 },
    { person: '李四', role: 'actor', character: '反派', sort_order: 1 },
  ],
  crew: [
    { person: '王五', role: 'director', character: null, sort_order: 0 },
    { person: '赵六', role: 'writer', character: null, sort_order: 1 },
    { person: '钱七', role: 'writer', character: null, sort_order: 2 },
  ],
}

function stubFetch(handlers: Array<[string, () => unknown]>) {
  globalThis.fetch = vi.fn((url: string | URL | Request) => {
    const u = String(url)
    for (const [needle, h] of handlers) {
      if (u.includes(needle)) {
        const ok = !u.includes('__fail__')
        return Promise.resolve({
          ok,
          status: ok ? 200 : 500,
          text: () =>
            Promise.resolve(
              ok
                ? JSON.stringify(h())
                : '{"error":{"code":"X","message":"boom"}}',
            ),
        } as Response)
      }
    }
    return Promise.resolve({ ok: true, status: 200, text: () => Promise.resolve('{}') } as Response)
  }) as unknown as typeof fetch
}

function mountDetail() {
  return mount(DetailView, {
    global: {
      plugins: [createPinia()],
      stubs: { RouterLink: { template: '<a><slot /></a>' } },
    },
  })
}

beforeEach(() => {
  vi.restoreAllMocks()
})

describe('DetailView', () => {
  it('渲染演职员，幕后按职能归并成一行', async () => {
    stubFetch([
      ['/playback/decide', () => ({ strategy: 'direct_play', reason: '能直出', reasons: [] })],
      ['/playback/resume', () => []],
      ['/credits', () => credits],
      ['/series', () => null],
      ['/items/', () => item],
    ])

    const w = mountDetail()
    await flushPromises()

    expect(w.text()).toContain('演职员')
    expect(w.text()).toContain('张三')
    expect(w.text()).toContain('主角')
    // 导演 / 编剧各占一行，两个编剧用顿号连起来。
    expect(w.text()).toContain('导演')
    expect(w.text()).toContain('王五')
    expect(w.text()).toContain('赵六、钱七')
  })

  it('条目属于某部剧时才渲染季列表', async () => {
    stubFetch([
      ['/playback/decide', () => ({ strategy: 'direct_play', reason: '能直出', reasons: [] })],
      ['/playback/resume', () => []],
      ['/credits', () => ({ cast: [], crew: [] })],
      // '/seasons' 必须排在 '/series' 前面：`/api/v1/series/s1/seasons`
      // 这个 URL 里也含 "/series"，按前缀匹配会被上一行截走。
      ['/seasons', () => [{ id: 'sa', number: 1, episode_count: 2 }]],
      ['/series', () => ({ id: 's1', title: '剧集一', year: 2021 })],
      ['/episodes', () => ({ data: [], page: { total: 0, page: 1, page_size: 500 } })],
      ['/items/', () => item],
    ])

    const w = mountDetail()
    await flushPromises()

    expect(w.text()).toContain('剧集')
    expect(w.text()).toContain('第 1 季')
  })

  it('演职员取失败时详情页照常打开，只是没有那一块', async () => {
    stubFetch([
      ['/playback/decide', () => ({ strategy: 'direct_play', reason: '能直出', reasons: [] })],
      ['/playback/resume', () => []],
      ['/credits__fail__', () => credits],
      ['/series__fail__', () => null],
      ['/items/', () => item],
    ])

    const w = mountDetail()
    await flushPromises()

    expect(w.text()).toContain('剧集一')
    expect(w.text()).toContain('一段简介')
    expect(w.text()).not.toContain('演职员')
  })

  it('类型 chip 来自详情里的 genres', async () => {
    stubFetch([
      ['/playback/decide', () => ({ strategy: 'direct_play', reason: '能直出', reasons: [] })],
      ['/playback/resume', () => []],
      ['/credits', () => ({ cast: [], crew: [] })],
      ['/series', () => null],
      ['/items/', () => item],
    ])

    const w = mountDetail()
    await flushPromises()

    expect(w.text()).toContain('剧情')
    expect(w.text()).toContain('悬疑')
  })
})
