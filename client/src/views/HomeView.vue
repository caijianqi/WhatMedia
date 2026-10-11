<script setup lang="ts">
/**
 * 首页。
 *
 * 三块内容：继续观看 / 最近添加 / 按类型分的行。
 * 每块**独立加载、独立报错** —— 刮削服务挂了不该导致"最近添加"也出不来。
 * 所以是三个并行的 fetch，各自有自己的 loading 和 error 状态。
 */
import { onMounted, ref } from 'vue'
import PosterCard from '../components/PosterCard.vue'
import * as api from '../api/client'
import type { ItemSummary, ResumePoint } from '../api/client'

interface Row {
  title: string
  items: ItemSummary[]
  loading: boolean
  error: string | null
}

const resume = ref<ResumePoint[]>([])
const resumeError = ref<string | null>(null)

const rows = ref<Row[]>([
  { title: '最近添加', items: [], loading: true, error: null },
  { title: '电影', items: [], loading: true, error: null },
  { title: '剧集', items: [], loading: true, error: null },
])

async function loadResume() {
  try {
    resume.value = await api.resumeList(12)
  } catch (e) {
    resumeError.value = e instanceof Error ? e.message : String(e)
  }
}

async function loadRow(i: number, kind: 'all' | 'movie' | 'series') {
  const row = rows.value[i]!
  row.loading = true
  try {
    // 三行都走 `listMedia`：它返回的是 `ItemSummary`，海报墙能直接吃。
    // `/series` 返回的 `SeriesSummary` 没有 kind / added_at，喂给海报卡片会缺字段 ——
    // 那个端点是给"剧集列表"页用的，不是给首页海报行用的。
    const q: api.ItemQuery = { sort: 'added_at', order: 'desc', page_size: 18 }
    if (kind !== 'all') q.kind = kind
    row.items = (await api.listMedia(q)).data
  } catch (e) {
    row.error = e instanceof Error ? e.message : String(e)
  } finally {
    row.loading = false
  }
}

onMounted(() => {
  void loadResume()
  void loadRow(0, 'all')
  void loadRow(1, 'movie')
  void loadRow(2, 'series')
})

function progressOf(r: ResumePoint): number {
  const d = r.duration_ms ?? 0
  return d > 0 ? r.position_ms / d : 0
}
</script>

<template>
  <div class="space-y-10">
    <!-- 继续观看 -->
    <section v-if="resume.length > 0">
      <h2 class="mb-3 text-lg font-semibold text-slate-100">继续观看</h2>
      <div class="grid grid-cols-2 gap-4 sm:grid-cols-4 lg:grid-cols-6 xl:grid-cols-8">
        <PosterCard
          v-for="r in resume"
          :key="r.item_id"
          :item="{
            id: r.item_id,
            kind: r.kind,
            title: r.title,
            added_at: '',
          }"
          :progress="progressOf(r)"
        />
      </div>
    </section>
    <p v-else-if="resumeError" class="text-sm text-red-400">继续观看加载失败：{{ resumeError }}</p>

    <!-- 各分类行 -->
    <section v-for="row in rows" :key="row.title">
      <h2 class="mb-3 text-lg font-semibold text-slate-100">{{ row.title }}</h2>

      <p v-if="row.error" class="text-sm text-red-400">{{ row.error }}</p>
      <div
        v-else-if="row.loading"
        class="grid grid-cols-2 gap-4 sm:grid-cols-4 lg:grid-cols-6 xl:grid-cols-8"
      >
        <div
          v-for="n in 6"
          :key="n"
          class="aspect-[2/3] animate-pulse rounded-lg bg-ink-850"
        />
      </div>
      <p v-else-if="row.items.length === 0" class="text-sm text-slate-500">还没有内容</p>
      <div v-else class="grid grid-cols-2 gap-4 sm:grid-cols-4 lg:grid-cols-6 xl:grid-cols-8">
        <PosterCard v-for="it in row.items" :key="it.id" :item="it" />
      </div>
    </section>
  </div>
</template>
