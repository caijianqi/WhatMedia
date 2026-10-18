import { beforeEach, describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import GlobalToast from './GlobalToast.vue'
import { useToastStore } from '../stores/toast'

describe('GlobalToast', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('初始无 toast 时不渲染内容', () => {
    const w = mount(GlobalToast)
    expect(w.text()).toBe('')
  })

  it('有 toast 时渲染文案与正确类型图标', async () => {
    const store = useToastStore()
    const w = mount(GlobalToast)

    store.show('设置已更新', { type: 'success' })
    await w.vm.$nextTick()

    expect(w.text()).toContain('设置已更新')
    expect(w.find('svg').exists()).toBe(true)
  })
})
