<script setup lang="ts">
/**
 * 横向海报行（MediaRow）。
 * 1:1 遵循 ui.html 原生规范：
 * - 采用 .section、.section-head、.section-title、.section-subtitle、.text-btn
 * - 原生 5 列 poster-row 网格与平滑横向滚动支持
 * - 悬停时右上角优雅左右滚动箭头控制
 */
import { computed, ref } from 'vue'
import PosterCard from './PosterCard.vue'
import { useLayoutStore } from '../stores/layout'
import type { ItemSummary } from '../api/client'
import type { RouteLocationRaw } from 'vue-router'

const props = defineProps<{
  title: string
  subtitle?: string
  viewAllTo?: RouteLocationRaw
  viewAllText?: string
  items: ItemSummary[]
  loading?: boolean
  error?: string | null
  progressOf?: (item: ItemSummary) => number
}>()

const layout = useLayoutStore()
const rail = ref<HTMLElement | null>(null)

const canScroll = computed(() => props.items.length > 0 && !props.loading)

function scrollBy(dir: -1 | 1): void {
  const el = rail.value
  if (!el) return
  const step = 200 * Math.max(1, Math.floor(el.clientWidth / 200) - 1)
  el.scrollBy({ left: dir * step, behavior: 'smooth' })
}
</script>

<template>
  <section class="section group-row">
    <div class="section-head">
      <div>
        <h2 class="section-title">{{ title }}</h2>
        <p v-if="subtitle" class="section-subtitle">{{ subtitle }}</p>
      </div>

      <div style="display:flex;align-items:center;gap:10px">
        <RouterLink
          v-if="viewAllTo"
          :to="viewAllTo"
          class="text-btn"
        >
          <span>{{ viewAllText || '查看全部' }}</span>
          <svg viewBox="0 0 24 24">
            <path d="M5 12h14m-6-6 6 6-6 6" />
          </svg>
        </RouterLink>

        <!-- 左右滚动微按钮（桌面有鼠标时悬停显示） -->
        <div v-if="canScroll && layout.hasPointer" class="row-scroll-arrows">
          <button
            type="button"
            class="scroll-arrow-btn"
            :aria-label="`${title}：向左滚动`"
            @click="scrollBy(-1)"
          >
            <svg viewBox="0 0 24 24">
              <path d="M15 18l-6-6 6-6" />
            </svg>
          </button>
          <button
            type="button"
            class="scroll-arrow-btn"
            :aria-label="`${title}：向右滚动`"
            @click="scrollBy(1)"
          >
            <svg viewBox="0 0 24 24">
              <path d="M9 18l6-6-6-6" />
            </svg>
          </button>
        </div>
      </div>
    </div>

    <p v-if="error" style="color:#f87171;font-size:13px">{{ error }}</p>

    <!-- 加载骨架 -->
    <div v-else-if="loading" class="poster-row">
      <div v-for="n in 5" :key="n" class="skeleton-poster" />
    </div>

    <p v-else-if="items.length === 0" style="color:var(--muted);font-size:13px">还没有内容</p>

    <!-- 原生横向滚动海报行 -->
    <div
      v-else
      ref="rail"
      class="media-rail"
    >
      <div
        v-for="it in items"
        :key="it.id"
        class="media-rail-item"
      >
        <PosterCard :item="it" :progress="progressOf ? progressOf(it) : undefined" />
      </div>
    </div>
  </section>
</template>

<style scoped>
.group-row {
  position: relative;
}

.row-scroll-arrows {
  display: flex;
  gap: 4px;
  opacity: 0;
  transition: opacity 0.18s ease;
}
.group-row:hover .row-scroll-arrows {
  opacity: 1;
}

.scroll-arrow-btn {
  width: 28px;
  height: 28px;
  border-radius: 7px;
  border: 1px solid var(--line);
  background: var(--panel2);
  color: var(--muted);
  display: grid;
  place-items: center;
  cursor: pointer;
  transition: all 0.15s ease;
}
.scroll-arrow-btn:hover {
  background: var(--panel3);
  color: #fff;
  border-color: rgba(154, 131, 255, 0.4);
}
.scroll-arrow-btn svg {
  width: 14px;
  height: 14px;
  stroke: currentColor;
  fill: none;
  stroke-width: 2;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.media-rail {
  display: flex;
  gap: 15px;
  overflow-x: auto;
  scroll-behavior: smooth;
  padding-bottom: 4px;
  scrollbar-width: none;
  -ms-overflow-style: none;
}
.media-rail::-webkit-scrollbar {
  display: none;
}

.media-rail-item {
  flex: 0 0 calc((100% - 60px) / 5);
  min-width: 140px;
}

@media (max-width: 1250px) {
  .media-rail {
    gap: 11px;
  }
  .media-rail-item {
    flex: 0 0 calc((100% - 44px) / 4);
  }
}
@media (max-width: 620px) {
  .media-rail {
    gap: 14px;
  }
  .media-rail-item {
    flex: 0 0 calc((100% - 14px) / 2);
  }
}

.skeleton-poster {
  aspect-ratio: 2 / 2.72;
  border-radius: 13px;
  background: linear-gradient(90deg, #162033 25%, #1e2c47 50%, #162033 75%);
  background-size: 200% 100%;
  animation: shimmer 1.5s infinite;
}

@keyframes shimmer {
  0% { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}
</style>
