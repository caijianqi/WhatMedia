import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { toast, useToastStore } from './toast'

describe('toast store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
  })

  it('初始状态下没有 toast', () => {
    const store = useToastStore()
    expect(store.current).toBeNull()
  })

  it('调用 show 产生 toast 并在指定时间后自动消除', () => {
    const store = useToastStore()
    store.show('操作成功', { type: 'success', duration: 1500 })

    expect(store.current).not.toBeNull()
    expect(store.current?.message).toBe('操作成功')
    expect(store.current?.type).toBe('success')

    vi.advanceTimersByTime(1499)
    expect(store.current).not.toBeNull()

    vi.advanceTimersByTime(2)
    expect(store.current).toBeNull()
  })

  it('连续调用 show 覆盖前一个 toast 并刷新计时', () => {
    const store = useToastStore()
    store.show('第一条', { duration: 1000 })
    vi.advanceTimersByTime(500)

    store.show('第二条', { duration: 1000 })
    expect(store.current?.message).toBe('第二条')

    vi.advanceTimersByTime(600)
    // 距离第二条只过了 600ms，不该消失
    expect(store.current?.message).toBe('第二条')

    vi.advanceTimersByTime(500)
    expect(store.current).toBeNull()
  })

  it('dismiss 能立即清除', () => {
    const store = useToastStore()
    store.show('测试')
    expect(store.current).not.toBeNull()
    store.dismiss()
    expect(store.current).toBeNull()
  })

  it('便利函数 toast() 能正常触发 store', () => {
    const store = useToastStore()
    toast('通过便利函数弹出', { type: 'warn' })
    expect(store.current?.message).toBe('通过便利函数弹出')
    expect(store.current?.type).toBe('warn')
  })
})
