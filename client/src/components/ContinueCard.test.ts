import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ContinueCard from './ContinueCard.vue'
import type { ResumePoint } from '../api/client'

const sampleResume: ResumePoint = {
  item_id: 'item-100',
  file_id: 'file-100',
  session_id: 'sess-100',
  title: '流浪地球 2',
  position_ms: 3600000, // 60分钟
  duration_ms: 7200000, // 120分钟
  kind: 'movie',
  strategy: 'direct',
  updated_at: '2026-01-01T00:00:00Z',
}

describe('ContinueCard', () => {
  it('正确渲染标题与计算的剩余时间', () => {
    const w = mount(ContinueCard, {
      props: {
        resume: sampleResume,
      },
      global: {
        stubs: {
          RouterLink: {
            template: '<a :href="to.name"><slot /></a>',
            props: ['to'],
          },
        },
      },
    })

    expect(w.text()).toContain('流浪地球 2')
    expect(w.text()).toContain('剩余 60 分钟')
    expect(w.text()).toContain('已观看 50%')
  })

  it('自定义 subtitle 覆盖计算文本', () => {
    const w = mount(ContinueCard, {
      props: {
        resume: sampleResume,
        subtitle: '第 3 集 · 剩余 15 分钟',
      },
      global: {
        stubs: {
          RouterLink: {
            template: '<a><slot /></a>',
          },
        },
      },
    })

    expect(w.text()).toContain('第 3 集 · 剩余 15 分钟')
  })

  it('图片加载失败降级为首字', async () => {
    const w = mount(ContinueCard, {
      props: {
        resume: sampleResume,
      },
      global: {
        stubs: {
          RouterLink: {
            template: '<a><slot /></a>',
          },
        },
      },
    })

    const img = w.find('img')
    expect(img.exists()).toBe(true)
    await img.trigger('error')

    expect(w.text()).toContain('流')
  })
})
