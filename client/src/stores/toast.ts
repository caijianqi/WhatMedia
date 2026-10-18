/**
 * 全局轻量 Toast 提示系统。
 *
 * 对应原型中的 toast 机制：
 * 当用户执行收藏/取消收藏、修改设置、调整音量/进度、复制链接等操作时，
 * 在屏幕底部中央浮现轻量提示，随后自动淡出，避免阻塞用户主操作流程。
 */
import { defineStore } from 'pinia'
import { ref } from 'vue'

export type ToastType = 'info' | 'success' | 'warn' | 'error'

export interface ToastItem {
  id: number
  message: string
  type: ToastType
  duration: number
}

let nextId = 1

export const useToastStore = defineStore('toast', () => {
  const current = ref<ToastItem | null>(null)
  let timer: ReturnType<typeof setTimeout> | null = null

  function show(message: string, options?: { type?: ToastType; duration?: number }): void {
    const type = options?.type ?? 'info'
    const duration = options?.duration ?? 2300

    if (timer) {
      clearTimeout(timer)
      timer = null
    }

    current.value = {
      id: nextId++,
      message,
      type,
      duration,
    }

    if (duration > 0) {
      timer = setTimeout(() => {
        dismiss()
      }, duration)
    }
  }

  function dismiss(): void {
    if (timer) {
      clearTimeout(timer)
      timer = null
    }
    current.value = null
  }

  return {
    current,
    show,
    dismiss,
  }
})

/**
 * 方便在组件或事件处理中直接调用的便捷函数。
 */
export function toast(message: string, options?: { type?: ToastType; duration?: number }): void {
  const store = useToastStore()
  store.show(message, options)
}
