/**
 * 电视直播页单元测试。
 *
 * 验证：
 * 1. 同名频道的多线路聚合展示（避免重名频道刷屏）。
 * 2. 线路切换与起播调用。
 * 3. 预设列表包含推荐的 GuoVin TXT / M3U 订阅源。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

vi.mock('vue-router', async (importOriginal) => {
  const actual = await importOriginal<typeof import('vue-router')>()
  return {
    ...actual,
    useRouter: () => ({
      push: vi.fn(),
      back: vi.fn(),
    }),
  }
})

vi.mock('../api/client', () => ({
  ApiError: class ApiError extends Error {},
  listLiveChannels: vi.fn(),
  listLiveSources: vi.fn(),
  playLiveChannel: vi.fn(),
  stopLiveChannel: vi.fn(),
  createLiveSource: vi.fn(),
  syncLiveSource: vi.fn(),
  deleteLiveSource: vi.fn(),
}))

vi.mock('hls.js', () => {
  return {
    default: class MockHls {
      static isSupported() {
        return true
      }
      static Events = { ERROR: 'hlsError' }
      static ErrorTypes = { NETWORK_ERROR: 'networkError', MEDIA_ERROR: 'mediaError' }
      loadSource = vi.fn()
      attachMedia = vi.fn()
      on = vi.fn()
      destroy = vi.fn()
      startLoad = vi.fn()
      recoverMediaError = vi.fn()
    },
  }
})

import LiveView from './LiveView.vue'
import * as api from '../api/client'

const listChannelsMock = vi.mocked(api.listLiveChannels)
const listSourcesMock = vi.mocked(api.listLiveSources)
const playLiveChannelMock = vi.mocked(api.playLiveChannel)
const stopLiveChannelMock = vi.mocked(api.stopLiveChannel)

const mockRawChannels = [
  {
    id: 'cctv1-l1',
    name: 'CCTV-1',
    group: '📺央视频道',
    logo_url: null,
    dvr_window_secs: 0,
    now_playing: '新闻联播',
    viewers: 12,
  },
  {
    id: 'cctv1-l2',
    name: 'CCTV-1',
    group: '📺央视频道',
    logo_url: null,
    dvr_window_secs: 0,
    now_playing: '新闻联播',
    viewers: 8,
  },
  {
    id: 'cctv1-l3',
    name: 'CCTV-1',
    group: '📺央视频道',
    logo_url: null,
    dvr_window_secs: 0,
    now_playing: '新闻联播',
    viewers: 5,
  },
  {
    id: 'hn-l1',
    name: '湖南卫视',
    group: '📡卫视频道',
    logo_url: null,
    dvr_window_secs: 0,
    now_playing: null,
    viewers: 20,
  },
]

async function mountView() {
  const w = mount(LiveView, {
    global: {
      stubs: {
        RouterLink: {
          template: '<a :href="to"><slot /></a>',
          props: ['to'],
        },
      },
    },
  })
  await flushPromises()
  return w
}

beforeEach(() => {
  vi.clearAllMocks()
  localStorage.clear()
  listChannelsMock.mockResolvedValue(mockRawChannels as never)
  listSourcesMock.mockResolvedValue([] as never)
  stopLiveChannelMock.mockResolvedValue({ ok: true } as never)
  playLiveChannelMock.mockResolvedValue({
    job_id: 'job-1',
    playlist_url: '/api/v1/hls/job-1/master.m3u8',
    live: true,
    dvr_window_secs: 1800,
  } as never)
  window.HTMLMediaElement.prototype.play = vi.fn().mockResolvedValue(undefined)
  window.HTMLMediaElement.prototype.pause = vi.fn()
})

describe('LiveView', () => {
  it('智能聚合同名频道，避免多个 CCTV-1 刷屏并标明线路数量', async () => {
    const w = await mountView()

    const cards = w.findAll('[data-test="channel"]')
    // 3条 CCTV-1 + 1条 湖南卫视 应当聚合成 2 个频道卡片
    expect(cards).toHaveLength(2)

    const text = w.text()
    expect(text).toContain('CCTV-1')
    expect(text).toContain('湖南卫视')
    expect(text).toContain('3 线路')
  })

  it('页面加载自动起播首个频道并支持多线路', async () => {
    const w = await mountView()
    // 默认使用第一条线路起播
    expect(playLiveChannelMock).toHaveBeenCalledWith('cctv1-l1')
    expect(w.find('[data-test="player"]').exists()).toBe(true)
    expect(w.text()).toContain('线路 1 / 3')
  })

  it('顶部或浮层提供直达设置页的配置直播源入口', async () => {
    const w = await mountView()
    const link = w.find('[data-test="link-settings"]')
    expect(link.exists()).toBe(true)
    expect(link.attributes('href')).toBe('/settings?tab=live')
  })

  it('未配置任何频道时展示空状态与前往设置引导', async () => {
    listChannelsMock.mockResolvedValueOnce([])
    const w = await mountView()
    const empty = w.find('[data-test="empty"]')
    expect(empty.exists()).toBe(true)
    expect(empty.text()).toContain('尚未配置任何电视频道')
    expect(empty.text()).toContain('前往「设置 - 电视直播」配置订阅源')
  })

  it('支持鼠标移动到左侧感应区呼出选台栏与换台', async () => {
    const w = await mountView()
    const sensor = w.find('.tv-edge-sensor')
    expect(sensor.exists()).toBe(true)

    // 鼠标移入左侧感应区呼出选台抽屉
    await sensor.trigger('mouseenter')
    await flushPromises()

    const drawer = w.find('[data-test="channel-drawer"]')
    expect(drawer.exists()).toBe(true)
    expect(drawer.classes()).toContain('is-open')
    expect(w.text()).toContain('📺 频道列表')

    // 在抽屉中点击切换到湖南卫视
    const hnBtn = w.findAll('.drawer-channel-btn').find((b) => b.text().includes('湖南卫视'))
    expect(hnBtn).toBeDefined()
    await hnBtn!.trigger('click')
    await flushPromises()

    expect(playLiveChannelMock).toHaveBeenCalledWith('hn-l1')
  })

  it('底部控制台支持停止播放与释放流资源', async () => {
    const w = await mountView()
    const stopBtn = w.find('[data-test="stop"]')
    expect(stopBtn.exists()).toBe(true)

    await stopBtn.trigger('click')
    await flushPromises()

    expect(stopLiveChannelMock).toHaveBeenCalledWith('cctv1-l1')
  })

  it('底部控制台多线路下拉切换可秒切线路', async () => {
    const w = await mountView()
    const select = w.find('select.ctl-select')
    expect(select.exists()).toBe(true)

    // CCTV-1 有 3 条线路，切换到线路 2
    await select.setValue('1')
    await flushPromises()

    expect(playLiveChannelMock).toHaveBeenCalledWith('cctv1-l2')
  })

  it('频道抽屉采用树形分组展示，默认仅展开当前播中分组，其他折叠', async () => {
    const w = await mountView()
    // 呼出抽屉
    await w.find('.tv-edge-sensor').trigger('mouseenter')
    await flushPromises()

    // 检查树形分组一级节点 (央视频道 + 卫视频道)
    const groupHeaders = w.findAll('.tree-group-header')
    expect(groupHeaders).toHaveLength(2)
    expect(groupHeaders[0].text()).toContain('📺央视频道')
    expect(groupHeaders[1].text()).toContain('📡卫视频道')

    // 默认仅播放中的央视频道子列表展开，卫视频道折叠
    expect(w.findAll('.tree-channels-sublist')[0].attributes('style') || '').not.toContain('display: none')
    expect(w.findAll('.tree-channels-sublist')[1].attributes('style') || '').toContain('display: none')

    // 点击全部折叠
    const collapseBtn = w.findAll('.tree-action-btn').find((b) => b.text().includes('全部折叠'))
    expect(collapseBtn).toBeDefined()
    await collapseBtn!.trigger('click')
    await flushPromises()

    // 两个分组的子列表均隐藏 (style 包含 display: none)
    expect(w.findAll('.tree-channels-sublist')[0].attributes('style')).toContain('display: none')
    expect(w.findAll('.tree-channels-sublist')[1].attributes('style')).toContain('display: none')

    // 单独点击展开第一个分组 (央视频道)
    await w.findAll('.tree-group-header')[0].trigger('click')
    await flushPromises()
    expect(w.findAll('.drawer-tree-group')[0].classes()).toContain('is-expanded')
    expect(w.findAll('.tree-channels-sublist')[0].attributes('style')).not.toContain('display: none')
    expect(w.findAll('.tree-channels-sublist')[1].attributes('style')).toContain('display: none')

    // 测试搜索：输入“湖南”，自动过滤并展开匹配的卫视频道
    const searchInput = w.find('.drawer-search-input')
    await searchInput.setValue('湖南')
    await flushPromises()

    const drawerTree = w.find('.drawer-channels-tree')
    const filteredGroups = drawerTree.findAll('.tree-group-header')
    expect(filteredGroups).toHaveLength(1)
    expect(filteredGroups[0].text()).toContain('📡卫视频道')
    expect(drawerTree.text()).toContain('湖南卫视')
    expect(drawerTree.text()).not.toContain('CCTV-1')
  })

  it('支持频道收藏并置顶展示「⭐ 我的收藏」分组', async () => {
    const w = await mountView()
    // 呼出抽屉
    await w.find('.tv-edge-sensor').trigger('mouseenter')
    await flushPromises()

    // 点击当前顶部导航栏上的收藏按钮
    const topFavBtn = w.find('[data-test="topbar-fav-btn"]')
    expect(topFavBtn.exists()).toBe(true)
    await topFavBtn.trigger('click')
    await flushPromises()

    expect(topFavBtn.classes()).toContain('is-fav')

    // 检查抽屉中置顶出现了「⭐ 我的收藏」
    expect(w.text()).toContain('⭐ 我的收藏')

    // 再次点击取消收藏
    await topFavBtn.trigger('click')
    await flushPromises()

    expect(topFavBtn.classes()).not.toContain('is-fav')
    expect(w.text()).not.toContain('⭐ 我的收藏')
  })

  it('记录上次播放频道，刷新或重新进入时自动起播历史频道', async () => {
    // 模拟上次播放的是湖南卫视
    localStorage.setItem('whatmedia_last_live_channel_key', '📡卫视频道__湖南卫视')

    const w = await mountView()
    // 自动起播湖南卫视
    expect(playLiveChannelMock).toHaveBeenCalledWith('hn-l1')
    expect(w.text()).toContain('湖南卫视')

    // 并且默认展开的是卫视频道，央视频道折叠
    await w.find('.tv-edge-sensor').trigger('mouseenter')
    await flushPromises()

    const sublists = w.findAll('.tree-channels-sublist')
    expect(sublists[0].attributes('style') || '').toContain('display: none') // 央视频道折叠
    expect(sublists[1].attributes('style') || '').not.toContain('display: none')  // 卫视频道展开
  })
})


