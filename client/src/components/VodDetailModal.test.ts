import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import VodDetailModal from './VodDetailModal.vue'
import type { AggregatedVodItem } from '../api/client'

describe('VodDetailModal.vue', () => {
  const mockVod: AggregatedVodItem = {
    id: 'vod_123',
    title: '繁花',
    clean_title: '繁花',
    year: 2023,
    type_name: '国产剧',
    remark: '30集全',
    routes: [
      {
        source_id: 'src_lz',
        source_name: '量子资源 [lzm3u8]',
        play_from: 'lzm3u8',
        episodes: [
          { name: '第01集', play_url: 'https://lz.test/1.m3u8', index: 1 },
          { name: '第02集', play_url: 'https://lz.test/2.m3u8', index: 2 },
        ],
      },
      {
        source_id: 'src_ff',
        source_name: '非凡资源 [ffm3u8]',
        play_from: 'ffm3u8',
        episodes: [
          { name: '第01集', play_url: 'https://ff.test/1.m3u8', index: 1 },
        ],
      },
    ],
  }

  it('renders correctly when open with vod item', () => {
    const wrapper = mount(VodDetailModal, {
      props: {
        open: true,
        item: mockVod,
      },
    })

    expect(wrapper.text()).toContain('繁花')
    expect(wrapper.text()).toContain('30集全')
    expect(wrapper.text()).toContain('量子资源 [lzm3u8]')
    expect(wrapper.text()).toContain('非凡资源 [ffm3u8]')
    expect(wrapper.findAll('.episode-btn').length).toBe(2)
  })

  it('emits play event when clicking an episode', async () => {
    const wrapper = mount(VodDetailModal, {
      props: {
        open: true,
        item: mockVod,
      },
    })

    const epButtons = wrapper.findAll('.episode-btn')
    await epButtons[0].trigger('click')

    const playEvents = wrapper.emitted('play')
    expect(playEvents).toBeTruthy()
    expect(playEvents![0][0]).toEqual({
      item: mockVod,
      route: mockVod.routes[0],
      episode: mockVod.routes[0].episodes[0],
    })
  })

  it('switches routes and updates episode list', async () => {
    const wrapper = mount(VodDetailModal, {
      props: {
        open: true,
        item: mockVod,
      },
    })

    const routePills = wrapper.findAll('.route-pill')
    await routePills[1].trigger('click') // 切换到非凡资源

    expect(wrapper.findAll('.episode-btn').length).toBe(1)
  })
})
