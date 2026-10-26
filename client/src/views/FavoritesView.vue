<script setup lang="ts">
/**
 * 我的收藏（FavoritesView）。
 * 遵循 ui.html 原生规范：
 * - .page-header 与 .section-title
 * - 标准 6 列 .library-grid PosterCard 海报卡片网格
 * - 支持按类型 Chips 筛选与海报微交互直接取消收藏
 * - 纯净原生空状态 .empty-state
 */
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import * as api from '../api/client'
import PosterCard from '../components/PosterCard.vue'
import EmptyState from '../components/EmptyState.vue'

const router = useRouter()
const rows = ref<api.FavoriteRow[]>([])
const loading = ref(true)
const failed = ref<string | null>(null)
const selectedKind = ref<string>('')

async function load(): Promise<void> {
  loading.value = true
  failed.value = null
  try {
    const r = await api.listMyFavorites(1, 100)
    rows.value = r.data
  } catch (e) {
    failed.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

onMounted(load)

const KIND_LABEL: Record<string, string> = {
  movie: '电影',
  series: '电视剧',
  episode: '单集',
  anime: '动画',
  documentary: '纪录片',
  variety: '综艺',
  music: '音乐',
  musicvideo: 'MV',
  audiobook: '有声书',
  photo: '照片',
  livetv: '直播',
  mixed: '混合',
  other: '其它',
}

const kindTabs = computed(() => {
  const counts: Record<string, number> = {}
  for (const r of rows.value) {
    counts[r.kind] = (counts[r.kind] ?? 0) + 1
  }
  const tabs = [{ value: '', label: `全部 ${rows.value.length}` }]
  for (const [k, count] of Object.entries(counts)) {
    tabs.push({ value: k, label: `${KIND_LABEL[k] ?? k} ${count}` })
  }
  return tabs
})

const filteredItems = computed<api.ItemSummary[]>(() => {
  const list = selectedKind.value
    ? rows.value.filter((r) => r.kind === selectedKind.value)
    : rows.value
  return list.map((r) => ({
    id: r.item_id,
    kind: r.kind as api.MediaKind,
    title: r.title,
    original_title: null,
    year: r.year,
    rating: (r as unknown as { rating?: number }).rating ?? null,
    runtime_ms: null,
    added_at: r.added_at,
  }))
})

function onFavoriteChange(itemId: string, isFav: boolean): void {
  if (!isFav) {
    rows.value = rows.value.filter((r) => r.item_id !== itemId)
  }
}
</script>

<template>
  <div>
    <div class="page-header">
      <div>
        <h1>我的收藏</h1>
        <p>收藏是每人一份的 —— 包含你标记心形的所有本地与云端影视资产。</p>
      </div>
      <div v-if="rows.length > 0" class="fav-count-badge">
        已收藏 <strong>{{ rows.length }}</strong> 部
      </div>
    </div>

    <!-- 类型筛选 Tabs（收藏有内容且包含多种类型时显示） -->
    <div v-if="rows.length > 0 && kindTabs.length > 2" class="toolbar">
      <div class="chips">
        <button
          v-for="t in kindTabs"
          :key="t.value || 'all'"
          type="button"
          class="chip"
          :class="{ active: t.value === selectedKind }"
          @click="selectedKind = t.value"
        >
          {{ t.label }}
        </button>
      </div>
    </div>

    <!-- 加载骨架 -->
    <div v-if="loading" class="library-grid">
      <div v-for="n in 6" :key="n" class="skeleton-poster" />
    </div>

    <EmptyState
      v-else-if="failed"
      kind="error"
      :hint="failed ?? ''"
      action="重试"
      @action="load"
    />

    <EmptyState
      v-else-if="rows.length === 0"
      kind="empty"
      title="还没有收藏任何东西"
      hint="在影片详情页点那颗心形按钮，它就会出现在这里。"
      alt="去逛媒体库"
      @alt="router.push({ name: 'library' })"
    />

    <!-- 标准 6 列海报网格展示收藏，支持海报微交互直接取消收藏 -->
    <div v-else class="library-grid">
      <PosterCard
        v-for="it in filteredItems"
        :key="it.id"
        :item="it"
        :is-favorite="true"
        @favorite-change="onFavoriteChange(it.id, $event)"
      />
    </div>
  </div>
</template>

<style scoped>
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

.fav-count-badge {
  font-size: 12px;
  color: var(--muted);
  background: rgba(255, 255, 255, 0.04);
  padding: 6px 14px;
  border-radius: 20px;
  border: 1px solid var(--line);
}
.fav-count-badge strong {
  color: #c084fc;
}
</style>
