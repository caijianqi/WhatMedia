<script setup lang="ts">
/**
 * 海报卡片。
 *
 * 没有海报是常态而不是异常 —— 刚扫完还没刮削的库里可能一大半都没有。
 * 所以降级显示的是**标题首字 + 年份**，而不是一张灰色的占位图：
 * 灰色方块一片的时候，用户根本分不出哪个是哪个。
 */
import { computed, ref, watch } from 'vue'
import type { ItemSummary } from '../api/client'

const props = defineProps<{
  item: ItemSummary
  /** 0..1。有值时画一条进度条。 */
  progress?: number
}>()

const failed = ref(false)

// 换条目时要重置失败标记，否则新条目会沿用上一个的降级状态。
watch(
  () => props.item.id,
  () => {
    failed.value = false
  },
)

const posterUrl = computed(() => `/api/v1/items/${props.item.id}/images/poster`)

const initial = computed(() => {
  const t = props.item.title.trim()
  return t ? [...t][0]!.toUpperCase() : '?'
})

const pct = computed(() => Math.round(Math.min(1, Math.max(0, props.progress ?? 0)) * 100))
</script>

<template>
  <RouterLink
    :to="{ name: 'item', params: { id: item.id } }"
    class="focus-ring group block overflow-hidden rounded-lg bg-ink-900 border border-ink-800 transition hover:border-brand/60"
  >
    <div class="relative aspect-[2/3] w-full overflow-hidden bg-ink-800">
      <img
        v-if="!failed"
        :src="posterUrl"
        :alt="item.title"
        loading="lazy"
        class="h-full w-full object-cover transition duration-200 group-hover:scale-[1.03]"
        @error="failed = true"
      />
      <div
        v-else
        class="flex h-full w-full items-center justify-center bg-gradient-to-br from-ink-700 to-ink-850 text-4xl font-semibold text-slate-400"
      >
        {{ initial }}
      </div>

      <!-- 进度条放在海报底部而不是卡片外面：外面那条会和下一行挤在一起 -->
      <div
        v-if="pct > 0"
        class="absolute inset-x-0 bottom-0 h-1 bg-black/60"
        role="progressbar"
        :aria-valuenow="pct"
        aria-valuemin="0"
        aria-valuemax="100"
      >
        <div class="h-full bg-brand" :style="{ width: pct + '%' }" />
      </div>
    </div>

    <div class="p-2">
      <div class="truncate text-sm font-medium text-slate-100 group-hover:text-white">
        {{ item.title }}
      </div>
      <div class="mt-0.5 text-xs text-slate-500">
        {{ item.year ?? '—' }}
      </div>
    </div>
  </RouterLink>
</template>
