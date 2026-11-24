import { onBeforeUnmount, onMounted, ref, type Ref } from 'vue'
import { useRouter } from 'vue-router'
import {
  FocusMemory,
  moveFocus,
  recoverFocus,
  initialFocus,
  parseRemoteKeyEvent,
  type Direction,
  type Focusable,
} from '../lib/tv'
import { useToastStore } from '../stores/toast'

export interface UseTvFocusOptions {
  /** 是否启用。默认 true */
  enabled?: Ref<boolean> | boolean
  /** 发生返回键时的自定义处理。返回 true 表示已消费，不再走默认 router.back() */
  onBack?: () => boolean | void
  /** 发生菜单键时的自定义处理 (如打开 TV 大屏搜索或操作抽屉) */
  onMenu?: () => void
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

  let lastMainElement: HTMLElement | null = null
  let lastSidebarElement: HTMLElement | null = null
  let lastBackPressTime = 0

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
    // TV 3米视距居中平滑对齐，预留四周边缘安全区
    el.scrollIntoView({ block: 'center', inline: 'nearest', behavior: 'smooth' })
  }

  function handleDirection(dir: Direction): boolean {
    const modal = getActiveModalScope()
    const scope = modal || document.body
    const { elements, items } = collectFocusables(scope)
    if (items.length === 0) return false

    isTvNavigating.value = true

    // 查找当前焦点项
    let currentIdx = -1
    const active = document.activeElement as HTMLElement | null
    if (active) {
      currentIdx = elements.indexOf(active)
      if (currentIdx < 0) {
        const parentFocusable = active.closest<HTMLElement>(FOCUSABLE_SELECTOR)
        if (parentFocusable) {
          currentIdx = elements.indexOf(parentFocusable)
        }
      }
    }

    // ── 智能跨区路由 (Zone Navigation: Sidebar ⇄ Main) ──
    if (!modal && active) {
      const inSidebar = !!active.closest('.sidebar, [data-tv-zone="sidebar"]')
      const inMain = !!active.closest('.main, [data-tv-zone="main"]')

      if (inMain) {
        lastMainElement = active
        if (dir === 'left') {
          // 检查在主内容区内部，左侧是否还有可聚焦候选项
          const mainEl = active.closest<HTMLElement>('.main, [data-tv-zone="main"]')
          const mainFocusables = mainEl ? collectFocusables(mainEl) : null
          let hasLeftCandidateInMain = false
          if (mainFocusables) {
            const curMainIdx = mainFocusables.elements.indexOf(active)
            if (curMainIdx >= 0) {
              const nextMainIdx = moveFocus(mainFocusables.items, curMainIdx, 'left', { memory })
              if (nextMainIdx !== curMainIdx) {
                hasLeftCandidateInMain = true
              }
            }
          }

          if (!hasLeftCandidateInMain) {
            // 已达主内容区最左列边界，跨区穿越进入侧边栏
            const sidebar = document.querySelector<HTMLElement>('.sidebar, [data-tv-zone="sidebar"]')
            if (sidebar) {
              const { elements: sbElements } = collectFocusables(sidebar)
              if (sbElements.length > 0) {
                const target =
                  (lastSidebarElement && isElementVisible(lastSidebarElement) && sbElements.includes(lastSidebarElement)
                    ? lastSidebarElement
                    : null) ||
                  sidebar.querySelector<HTMLElement>('a.active') ||
                  sbElements[0]
                if (target) {
                  applyFocus(target)
                  return true
                }
              }
            }
          }
        }
      } else if (inSidebar) {
        lastSidebarElement = active
        if (dir === 'right') {
          // 从侧栏向右切入主内容区：优先恢复离开前聚焦的卡片，无缝保持浏览高度
          const main = document.querySelector<HTMLElement>('.main, [data-tv-zone="main"]')
          if (main) {
            if (lastMainElement && isElementVisible(lastMainElement) && main.contains(lastMainElement)) {
              applyFocus(lastMainElement)
              return true
            }
            const { elements: mainElements, items: mainItems } = collectFocusables(main)
            if (mainElements.length > 0) {
              const idx = initialFocus(mainItems)
              if (mainElements[idx]) {
                applyFocus(mainElements[idx]!)
                return true
              }
            }
          }
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

    active.click()
    return true
  }

  function handleBack(): boolean {
    if (options.onBack) {
      const handled = options.onBack()
      if (handled) return true
    }

    // 1. 优先关闭激活的模态窗并还焦
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

    // 2. 次级页面回退路由
    const currentRouteName = router?.currentRoute?.value?.name
    const isAtRoot = currentRouteName === 'home' || !router || window.history.length <= 1

    if (!isAtRoot && router) {
      router.back()
      return true
    }

    // 3. 根路由（首页）双击返回防误触保护
    const now = Date.now()
    if (now - lastBackPressTime < 2000) {
      try {
        const tauri = (window as any).__TAURI_INTERNALS__
        if (tauri) {
          void tauri.invoke('plugin:process|exit')
        }
      } catch {
        /* 忽略非原生桌面/移动壳环境 */
      }
      return true
    } else {
      lastBackPressTime = now
      try {
        const toast = useToastStore()
        toast.show('再按一次退出 WhatMedia', { type: 'info', duration: 2000 })
      } catch {
        /* 忽略单元测试无 pinia 上下文 */
      }
      return true
    }
  }

  function onKeyDown(e: KeyboardEvent) {
    if (!isEnabled()) return

    const target = e.target as HTMLElement | null
    const isTyping =
      target &&
      /^(INPUT|TEXTAREA)$/.test(target.tagName) &&
      !['checkbox', 'radio', 'range', 'button', 'submit'].includes((target as HTMLInputElement).type)

    const rk = parseRemoteKeyEvent(e)
    if (rk) {
      switch (rk) {
        case 'up':
          e.preventDefault()
          handleDirection('up')
          break
        case 'down':
          e.preventDefault()
          handleDirection('down')
          break
        case 'left':
          if (!isTyping) {
            e.preventDefault()
            handleDirection('left')
          }
          break
        case 'right':
          if (!isTyping) {
            e.preventDefault()
            handleDirection('right')
          }
          break
        case 'select':
          if (!isTyping) {
            e.preventDefault()
            handleSelect()
          }
          break
        case 'back':
          if (!isTyping) {
            e.preventDefault()
            handleBack()
          }
          break
        case 'menu':
          if (options.onMenu) {
            e.preventDefault()
            options.onMenu()
          }
          break
        default:
          break
      }
      return
    }

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
