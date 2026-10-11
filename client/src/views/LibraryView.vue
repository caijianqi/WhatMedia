<script setup lang="ts">
/**
 * 媒体库列表页。
 *
 * 筛选条件写进 URL 的 query，而不是只存组件状态。
 * 理由：用户会分享"我筛到哪了"的链接，也会用浏览器后退回到上一组筛选。
 * 只存组件状态的话，后退键会把整个列表重置 —— 这是很常见但很烦人的 bug。
 */
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import PosterCard from '../components/PosterCard.vue'
import * as api from '../api/client'
import type { ItemSort, MediaKind } from '../api/client'

const route = useRoute()
const router = useRouter()

const items = ref<api.ItemSummary[]>([])
const total = ref(0)
const loading = ref(false)
const error = ref<string | null>(null)

const kind = computed<MediaKind | ''>(() => (route.query.kind as MediaKind) ?? '')
const sort = computed<ItemSort>(() => (route.query.sort as ItemSort) ?? 'added_at')
const order = computed<'asc' | 'desc'>(() => (route.query.order as 'asc' | 'desc') ?? 'desc')
const page = computed(() => Number(route.query.page ?? 1))
const pageSize = 48

const pageCount = computed(() => Math.max(1, Math.ceil(total.value / pageSize)))

async function load() {
  loading.value = true
  error.value = null
  try {
    const p = await api.listMedia({
      kind: kind.value || undefined,
      sort: sort.value,
      order: order.value,
      page: page.value,
      page_size: pageSize,
    })
    items.value = p.data
    total.value = p.page.total
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

function update(patch: Record<string, string | number | undefined>) {
  void router.push({ query: { ...route.query, ...patch, page: 1 } })
}

watch(() => route.query, load, { immediate: true, deep: true })

const kinds: { value: MediaKind | ''; label: string }[] = [
  { value: '', label: '全部' },
  { value: 'movie', label: '电影' },
  { value: 'series', label: '剧集' },
  { value: 'anime', label: '动画' },
  { value: 'documentary', label: '纪录片' },
  { value: 'variety', label: '综艺' },
]

const sorts: { value: ItemSort; label: string }[] = [
  { value: 'added_at', label: '添加时间' },
  { value: 'title', label: '标题' },
  { value: 'year', label: '年份' },
  { value: 'rating', label: '评分' },
]
</script>

<template>
  <div class="space-y-6">
    <div class="flex flex-wrap items-center gap-3">
      <div class="flex flex-wrap gap-1">
        <button
          v-for="k in kinds"
          :key="k.value"
          class="focus-ring rounded-full px-3 py-1.5 text-sm transition"
          :class="
            kind === k.value
              ? 'bg-brand text-white'
              : 'bg-ink-850 text-slate-300 hover:bg-ink-800'
          "
          @click="update({ kind: k.value || undefined })"
        >
          {{ k.label }}
        </button>
      </div>

      <div class="ml-auto flex items-center gap-2">
        <select
          class="focus-ring rounded bg-ink-850 px-2 py-1.5 text-sm text-slate-200"
          :value="sort"
          @change="update({ sort: ($event.target as HTMLSelectElement).value })"
        >
          <option v-for="s in sorts" :key="s.value" :value="s.value">{{ s.label }}</option>
        </select>
        <button
          class="focus-ring rounded bg-ink-850 px-2 py-1.5 text-sm text-slate-200 hover:bg-ink-800"
          :title="order === 'desc' ? '当前：降序' : '当前：升序'"
          @click="update({ order: order === 'desc' ? 'asc' : 'desc' })"
        >
          {{ order === 'desc' ? '↓' : '↑' }}
        </button>
      </div>
    </div>

    <p v-if="error" class="rounded bg-red-950/60 px-3 py-2 text-sm text-red-300">
      {{ error }}
    </p>

    <div
      v-if="loading"
      class="grid grid-cols-2 gap-4 sm:grid-cols-4 lg:grid-cols-6 xl:grid-cols-8"
    >
      <div v-for="n in 12" :key="n" class="aspect-[2/3] animate-pulse rounded-lg bg-ink-850" />
    </div>
    <p v-else-if="items.length === 0" class="text-sm text-slate-500">没有匹配的条目</p>
    <div v-else class="grid grid-cols-2 gap-4 sm:grid-cols-4 lg:grid-cols-6 xl:grid-cols-8">
      <PosterCard v-for="it in items" :key="it.id" :item="it" />
    </div>

    <div v-if="pageCount > 1" class="flex items-center justify-center gap-3 pt-2">
      <button
        class="focus-ring rounded bg-ink-850 px-3 py-1.5 text-sm disabled:opacity-40"
        :disabled="page <= 1"
        @click="update({ page: page - 1 })"
      >
        上一页
      </button>
      <span class="text-sm text-slate-400">{{ page }} / {{ pageCount }}（共 {{ total }}）</span>
      <button
        class="focus-ring rounded bg-ink-850 px-3 py-1.5 text-sm disabled:opacity-40"
        :disabled="page >= pageCount"
        @click="update({ page: page + 1 })"
      >
        下一页
      </button>
    </div>
  </div>
</template>
