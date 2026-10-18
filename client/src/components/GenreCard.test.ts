import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import GenreCard from './GenreCard.vue'

describe('GenreCard', () => {
  it('正确渲染标题、副标题与背景图', () => {
    const w = mount(GenreCard, {
      props: {
        title: '电影',
        subtitle: '大银幕体验 · 影片收藏',
        bgImage: 'https://example.com/poster.jpg',
        to: { name: 'library', query: { kind: 'movie' } },
      },
      global: {
        stubs: {
          RouterLink: {
            template: '<a :data-to="JSON.stringify(to)"><slot /></a>',
            props: ['to'],
          },
        },
      },
    })

    expect(w.text()).toContain('电影')
    expect(w.text()).toContain('大银幕体验 · 影片收藏')
    const bgDiv = w.find('div[style*="https://example.com/poster.jpg"]')
    expect(bgDiv.exists()).toBe(true)
  })

  it('无 bgImage 时渲染降级渐变底', () => {
    const w = mount(GenreCard, {
      props: {
        title: '纪录片',
        subtitle: '自然、人文与探索',
        to: { name: 'library', query: { kind: 'documentary' } },
      },
      global: {
        stubs: {
          RouterLink: {
            template: '<a><slot /></a>',
            props: ['to'],
          },
        },
      },
    })

    expect(w.text()).toContain('纪录片')
    expect(w.find('.bg-gradient-to-br').exists()).toBe(true)
  })
})
