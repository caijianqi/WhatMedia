import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import TvSearchModal from './TvSearchModal.vue'

// Mock api
vi.mock('../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/client')>()
  return {
    ...actual,
    resolveMediaUrl: vi.fn().mockReturnValue('https://example.com/poster.jpg'),
    searchAggregate: vi.fn().mockResolvedValue({
      local: {
        data: [{ id: 'm1', title: '流浪地球', year: 2019, kind: 'movie', added_at: '2026-01-01' }],
        pagination: { page: 1, page_size: 20, total: 1, total_pages: 1 },
      },
      remote: [{
        id: 'v1',
        title: '狂飙',
        clean_title: '狂飙',
        routes: [{ source_id: 'lz', source_name: '量子资源', play_from: 'lz', episodes: [] }],
      }],
    }),
  }
})

// Mock router
vi.mock('vue-router', () => ({
  useRouter: () => ({
    push: vi.fn(),
  }),
}))

describe('TvSearchModal.vue', () => {
  it('渲染大屏虚拟遥控键盘并正确输入首字母', async () => {
    const wrapper = mount(TvSearchModal)

    // 检查键盘按键存在（36 个按键 + 2 个操作按钮）
    const keys = wrapper.findAll('.tv-key-btn')
    expect(keys.length).toBe(36)

    // 点击 'K' 和 'B'
    const btnK = keys.find((k) => k.text() === 'K')
    const btnB = keys.find((k) => k.text() === 'B')
    expect(btnK).toBeDefined()
    expect(btnB).toBeDefined()

    await btnK!.trigger('click')
    await btnB!.trigger('click')

    // 检查输入栏展示 "KB"
    expect(wrapper.find('.search-text').text()).toBe('KB')

    // 点击退格
    const delBtn = wrapper.find('.del-btn')
    await delBtn.trigger('click')
    expect(wrapper.find('.search-text').text()).toBe('K')

    // 点击清空
    const clearBtn = wrapper.findAll('.tv-key-action-btn')[0]!
    await clearBtn.trigger('click')
    expect(wrapper.find('.search-placeholder').exists()).toBe(true)
  })

  it('点击关闭按钮触发 close 事件', async () => {
    const wrapper = mount(TvSearchModal)
    const closeBtn = wrapper.find('.close-btn')
    await closeBtn.trigger('click')

    expect(wrapper.emitted('close')).toBeTruthy()
  })
})
