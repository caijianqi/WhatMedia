/**
 * 我的收藏页。
 *
 * 钉死两条：
 *   · 空的时候要给出"去哪收藏"的下一步，而不是一句"暂无数据"。
 *   · 列表直接消费服务端 join 出来的标题 —— 不再为每个 id 拉一次详情。
 *     这一条靠断言"只发了一次请求"来守：哪天有人改成逐个拉，
 *     20 个收藏就会变成 21 次请求。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

vi.mock('../api/client', () => ({
  ApiError: class ApiError extends Error {},
  listMyFavorites: vi.fn(),
}))

vi.mock('vue-router', async (importOriginal) => {
  const actual = await importOriginal<typeof import('vue-router')>()
  return { ...actual, useRouter: () => ({ push: vi.fn() }) }
})

import FavoritesView from './FavoritesView.vue'
import * as api from '../api/client'

const listMock = vi.mocked(api.listMyFavorites)

function favorites(titles: string[]) {
  return {
    total: titles.length,
    data: titles.map((t, i) => ({
      item_id: `i${i}`,
      kind: 'movie' as const,
      title: t,
      year: 2020 + i,
      added_at: '',
    })),
  }
}

async function mountView() {
  const w = mount(FavoritesView, {
    global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
  })
  await flushPromises()
  return w
}

beforeEach(() => {
  listMock.mockReset()
  listMock.mockResolvedValue(favorites([]) as never)
})

describe('FavoritesView', () => {
  it('只发一次请求拿到全部收藏（不逐个拉详情）', async () => {
    listMock.mockResolvedValue(favorites(['电影一', '电影二', '电影三']) as never)
    await mountView()
    expect(listMock).toHaveBeenCalledTimes(1)
  })

  it('列出收藏的标题', async () => {
    listMock.mockResolvedValue(favorites(['沙丘']) as never)
    const w = await mountView()
    expect(w.text()).toContain('沙丘')
  })

  it('空收藏时告诉用户去哪里收藏', async () => {
    const w = await mountView()
    expect(w.text()).toContain('还没有收藏')
    // 必须指出"详情页的心形按钮"，否则用户不知道这个功能在哪。
    expect(w.text()).toContain('心形')
  })

  it('请求失败时可以重试', async () => {
    listMock.mockRejectedValueOnce(new Error('boom'))
    const w = await mountView()
    expect(w.text()).toContain('boom')

    listMock.mockResolvedValue(favorites(['沙丘']) as never)
    await w.get('button').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('沙丘')
  })

  it('渲染 PosterCard 海报卡片并支持就地取消收藏', async () => {
    listMock.mockResolvedValue(favorites(['沙丘']) as never)
    const w = await mountView()
    expect(w.text()).toContain('已收藏 1 部')

    const card = w.findComponent({ name: 'PosterCard' })
    expect(card.exists()).toBe(true)

    // 触发卡片取消收藏事件
    card.vm.$emit('favorite-change', false)
    await flushPromises()

    // 列表就地清除该条目并展示空状态
    expect(w.text()).toContain('还没有收藏任何东西')
  })
})
