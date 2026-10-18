<script setup lang="ts">
/**
 * 全局浮动 Toast 组件。
 *
 * 挂载在应用顶层，监听 useToastStore 的变化。
 * 纯深色毛玻璃背景 + 柔和边框 + 底部居中上浮过渡，
 * 与原型中的 `.toast` 视觉一致。
 */
import { useToastStore } from '../stores/toast'

const store = useToastStore()
</script>

<template>
  <div
    class="pointer-events-none fixed bottom-7 left-1/2 z-50 -translate-x-1/2"
    aria-live="polite"
    role="status"
  >
    <Transition
      enter-active-class="transition duration-200 ease-smooth"
      enter-from-class="opacity-0 translate-y-3 scale-95"
      enter-to-class="opacity-100 translate-y-0 scale-100"
      leave-active-class="transition duration-150 ease-smooth"
      leave-from-class="opacity-100 translate-y-0 scale-100"
      leave-to-class="opacity-0 translate-y-2 scale-95"
    >
      <div
        v-if="store.current"
        :key="store.current.id"
        class="pointer-events-auto flex items-center gap-2.5 rounded-xl border border-white/10 bg-[#1d2941]/95 px-4 py-2.5 text-xs font-medium text-[#eef2ff] shadow-panel backdrop-blur-md"
      >
        <!-- 图标依类型微调 -->
        <span
          v-if="store.current.type === 'success'"
          class="flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-good/20 text-good"
        >
          <svg viewBox="0 0 24 24" class="h-3 w-3 stroke-current" fill="none" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <path d="M20 6 9 17l-5-5" />
          </svg>
        </span>
        <span
          v-else-if="store.current.type === 'warn'"
          class="flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-warn/20 text-warn"
        >
          <svg viewBox="0 0 24 24" class="h-3 w-3 stroke-current" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="9" />
            <path d="M12 8v4M12 16h.01" />
          </svg>
        </span>
        <span
          v-else-if="store.current.type === 'error'"
          class="flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-danger/20 text-danger"
        >
          <svg viewBox="0 0 24 24" class="h-3 w-3 stroke-current" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="9" />
            <path d="m15 9-6 6M9 9l6 6" />
          </svg>
        </span>

        <span>{{ store.current.message }}</span>
      </div>
    </Transition>
  </div>
</template>
