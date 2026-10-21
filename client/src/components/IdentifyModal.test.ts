import { describe, expect, it, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import IdentifyModal from './IdentifyModal.vue'

describe('IdentifyModal', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  it('正确解析候选条目的 poster 并支持点击海报放大高清预览', async () => {
    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/metadata/candidates')) {
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () =>
            Promise.resolve(
              JSON.stringify([
                {
                  provider: 'tmdb',
                  provider_id: '106449',
                  title: '凡人修仙传',
                  original_title: '凡人修仙传',
                  year: 2020,
                  overview: '天才少年韩立修仙之路',
                  poster: 'https://image.tmdb.org/t/p/w500/fanren_poster.jpg',
                  score: 1.0,
                  confidence: 'auto',
                },
                {
                  provider: 'tmdb',
                  provider_id: '243224',
                  title: '凡人修仙传 真人版',
                  original_title: '凡人修仙传',
                  year: 2025,
                  overview: '凡人真人影视剧',
                  poster_url: 'https://image.tmdb.org/t/p/w500/fanren_live.jpg',
                  score: 0.83,
                  confidence: 'review',
                },
              ]),
            ),
        })
      }
      return Promise.resolve({
        ok: true,
        status: 200,
        text: () => Promise.resolve('{}'),
      })
    })

    const wrapper = mount(IdentifyModal, {
      props: {
        open: true,
        itemId: 'item-123',
        initialTitle: '凡人修仙传',
        initialYear: 2020,
      },
      attachTo: document.body,
    })

    await flushPromises()

    const cards = wrapper.findAll('.candidate-card')
    expect(cards.length).toBe(2)

    // 缩略图通过代理
    const poster1 = cards[0].find('.cand-poster')
    expect(poster1.exists()).toBe(true)
    const img1 = poster1.find('img')
    expect(img1.exists()).toBe(true)
    expect(img1.attributes('src')).toContain('/api/v1/metadata/image_proxy?url=')

    // 初始状态下无大图预览浮层
    expect(document.querySelector('.lightbox-backdrop')).toBeNull()

    // 点击第一张海报放大
    await poster1.trigger('click')
    await flushPromises()

    // 验证 Lightbox 浮层展开
    const lightbox = document.querySelector('.lightbox-backdrop')
    expect(lightbox).not.toBeNull()

    // 验证大图将 /w500/ 升级为了 /original/ 高清档位
    const bigImg = lightbox?.querySelector('.lightbox-img')
    expect(bigImg?.getAttribute('src')).toContain(encodeURIComponent('https://image.tmdb.org/t/p/original/fanren_poster.jpg'))

    // 验证标题与高清原图标注
    expect(lightbox?.querySelector('.lightbox-title')?.textContent).toBe('凡人修仙传')
    expect(lightbox?.querySelector('.lightbox-hd-tag')?.textContent).toBe('高清原图')

    // 点击关闭按钮关闭大图
    const closeBtn = lightbox?.querySelector('.lightbox-close') as HTMLElement
    closeBtn?.click()
    await flushPromises()

    // 验证 Lightbox 已关闭
    expect(document.querySelector('.lightbox-backdrop')).toBeNull()
  })
})
