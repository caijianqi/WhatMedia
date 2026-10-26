import { onBeforeUnmount, onMounted, ref, type Ref } from 'vue'
import { useRouter } from 'vue-router'
import {
  FocusMemory,
  moveFocus,
  recoverFocus,
  initialFocus,
  type Direction,
  type Focusable,
} from '../lib/tv'

export interface UseTvFocusOptions {
  /** 是否启用。默认 true */
  enabled?: Ref<boolean> | boolean
  /** 发生返回键时的自定义处理。返回 true 表示已消费，不再走默认 router.back() */
  onBack?: () => boolean | void
}

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex="0"]',
  '[data-tv-focus]',
].join(', ')

/** 检查元素是否真正可见且可交互 */
export function isElementVisible(el: HTMLElement): boolean {
  if (!el.isConnected) return false
  const rect = el.getBoundingClientRect()
  if (rect.width <= 0 || rect.height <= 0) return false
  const style = window.getComputedStyle(el)
  if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') {
    return false
  }
  return true
}

/** 查找当前激活的模态弹层容器 */
export function getActiveModalScope(): HTMLElement | null {
  const modals = document.querySelectorAll<HTMLElement>(
    '.vod-modal-backdrop, .modal-backdrop, [role="dialog"], .global-modal',
  )
  for (let i = modals.length - 1; i >= 0; i--) {
    const modal = modals[i]
    if (modal && isElementVisible(modal)) {
      return modal
    }
  }
  return null
}

/** 收集容器内的所有可聚焦元素并转换为 Focusable 格式 */
export function collectFocusables(root: ParentNode = document): {
  elements: HTMLElement[]
  items: Focusable[]
} {
  const all = Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR))
  const elements: HTMLElement[] = []
  const items: Focusable[] = []

  for (let i = 0; i < all.length; i++) {
    const el = all[i]
    if (!el || !isElementVisible(el)) continue
    // 如果当前在根作用域下且页面有弹层，过滤掉弹层外的元素
    const rect = el.getBoundingClientRect()
    elements.push(el)
    items.push({
      id: el.id || `tv-focus-${items.length}`,
      rect: {
        x: rect.left,
        y: rect.top,
        w: rect.width,
        h: rect.height,
      },
      disabled: el.hasAttribute('disabled'),
    })
  }

  return { elements, items }
}

export function useTvFocus(options: UseTvFocusOptions = {}) {
  const router = useRouter()
  const memory = new FocusMemory()
  const lastIndex = ref(0)
  const isTvNavigating = ref(false)

  const isEnabled = () => {
    if (typeof options.enabled === 'boolean') return options.enabled
    if (options.enabled && 'value' in options.enabled) return options.enabled.value
    return true
  }

  function applyFocus(el: HTMLElement) {
    document.querySelectorAll('.tv-focused').forEach((node) => {
      if (node !== el) node.classList.remove('tv-focused')
    })
    el.classList.add('tv-focused')
    el.focus({ preventScroll: true })
    el.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'smooth' })
  }

  function handleDirection(dir: Direction): boolean {
    const modal = getActiveModalScope()
    const scope = modal || document.body
    const { elements, items } = collectFocusables(scope)
    if (items.length === 0) return false

    isTvNavigating.value = true

    // 查找当前焦点索引
    let currentIdx = -1
    const active = document.activeElement as HTMLElement | null
    if (active) {
      currentIdx = elements.indexOf(active)
      if (currentIdx < 0) {
        // 可能聚焦在子节点上，往上寻找
        const parentFocusable = active.closest<HTMLElement>(FOCUSABLE_SELECTOR)
        if (parentFocusable) {
          currentIdx = elements.indexOf(parentFocusable)
        }
      }
    }

    let nextIdx: number
    if (currentIdx < 0) {
      // 焦点流失或首次移动
      nextIdx = recoverFocus(items, lastIndex.value)
    } else {
      nextIdx = moveFocus(items, currentIdx, dir, { memory })
    }

    if (nextIdx >= 0 && nextIdx < elements.length) {
      lastIndex.value = nextIdx
      memory.remember(nextIdx, items)
      applyFocus(elements[nextIdx]!)
      return true
    }
    return false
  }

  function handleSelect(): boolean {
    const active = document.activeElement as HTMLElement | null
    if (!active || active === document.body) {
      // 若当前没有聚焦项，尝试聚焦第一项
      const modal = getActiveModalScope()
      const { elements, items } = collectFocusables(modal || document.body)
      if (items.length > 0) {
        const idx = initialFocus(items)
        if (elements[idx]) {
          applyFocus(elements[idx]!)
          return true
        }
      }
      return false
    }

    // 触发点击
    active.click()
    return true
  }

  function handleBack(): boolean {
    if (options.onBack) {
      const handled = options.onBack()
      if (handled) return true
    }

    // 检查是否有打开的模态窗，优先关闭它
    const modal = getActiveModalScope()
    if (modal) {
      const closeBtn = modal.querySelector<HTMLElement>(
        '.close-btn, .btn-close, [data-modal-close], button[title="关闭"]',
      )
      if (closeBtn) {
        closeBtn.click()
        return true
      }
    }

    // 无弹层时，回退路由
    if (router && window.history.length > 1) {
      router.back()
      return true
    }
    return false
  }

  function onKeyDown(e: KeyboardEvent) {
    if (!isEnabled()) return

    // 如果焦点在真正的输入框中打字，放行左右输入，但支持上下或回车
    const target = e.target as HTMLElement | null
    const isTyping =
      target &&
      /^(INPUT|TEXTAREA)$/.test(target.tagName) &&
      !['checkbox', 'radio', 'range', 'button', 'submit'].includes((target as HTMLInputElement).type)

    switch (e.key) {
      case 'ArrowUp':
        e.preventDefault()
        handleDirection('up')
        break
      case 'ArrowDown':
        e.preventDefault()
        handleDirection('down')
        break
      case 'ArrowLeft':
        if (!isTyping) {
          e.preventDefault()
          handleDirection('left')
        }
        break
      case 'ArrowRight':
        if (!isTyping) {
          e.preventDefault()
          handleDirection('right')
        }
        break
      case 'Enter':
        if (!isTyping) {
          e.preventDefault()
          handleSelect()
        }
        break
      case 'Escape':
      case 'Backspace':
        if (!isTyping) {
          e.preventDefault()
          handleBack()
        }
        break
      default:
        break
    }
  }

  function resetFocus() {
    setTimeout(() => {
      if (!isEnabled()) return
      const modal = getActiveModalScope()
      const { elements, items } = collectFocusables(modal || document.body)
      if (items.length > 0) {
        const idx = initialFocus(items)
        if (elements[idx]) {
          lastIndex.value = idx
          applyFocus(elements[idx]!)
        }
      }
    }, 120)
  }

  onMounted(() => {
    window.addEventListener('keydown', onKeyDown)
  })

  onBeforeUnmount(() => {
    window.removeEventListener('keydown', onKeyDown)
    document.querySelectorAll('.tv-focused').forEach((el) => el.classList.remove('tv-focused'))
  })

  return {
    handleDirection,
    handleSelect,
    handleBack,
    resetFocus,
    isTvNavigating,
  }
}
