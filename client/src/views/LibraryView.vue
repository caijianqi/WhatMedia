<script setup lang="ts">
/**
 * 媒体库列表页（LibraryView）。
 * 1:1 遵循 ui.html 原生规范：
 *   1. 页面 Header：标题 + 副标题 + 重新整理刷新按钮（.page-header）
 *   2. 工具栏：原生分类 Chips + 排序选择器（.select）+ 网格/列表双态开关（.view-toggle）
 *   3. 统计状态行：找到 X 项内容 + 清除筛选按钮（.section-head）
 *   4. 多态视图渲染：
 *      - 网格视图：标准 6 列海报网格（.library-grid）
 *      - 列表视图：单列条目横排清单（.library-grid.list-view）
 *   5. 原生空状态（.empty-state）与分页器集成
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import PosterCard from '../components/PosterCard.vue'
import ContinueCard from '../components/ContinueCard.vue'
import EmptyState from '../components/EmptyState.vue'
import { toast } from '../stores/toast'
import * as api from '../api/client'
import type { ItemSort, KindCount, MediaKind, ResumePoint } from '../api/client'

const route = useRoute()
const router = useRouter()

const items = ref<api.ItemSummary[]>([])
const rawSearchItems = ref<api.ItemSummary[]>([])
const rawResumeItems = ref<ResumePoint[]>([])
const total = ref(0)
const loading = ref(false)
const error = ref<string | null>(null)
const kinds = ref<KindCount[]>([])

/** 网格 / 列表视图。默认网格；若存储过则读取。 */
const view = ref<'grid' | 'list'>('grid')

const isContinueMode = computed(() => route.query.filter === 'continue')
const searchQuery = computed(() => ((route.query.q as string) || '').trim())

const pageTitle = computed(() => {
  if (isContinueMode.value) return '继续观看'
  if (searchQuery.value) return '搜索结果'
  return '媒体库'
})

const pageSubtitle = computed(() => {
  if (isContinueMode.value) return '接着上次的进度，精彩不必重新寻找。'
  if (searchQuery.value) return `包含关键词「${searchQuery.value}」的媒体条目。`
  return '统一浏览与筛选你的个人影视收藏。'
})

const kind = computed<MediaKind | ''>(() => (route.query.kind as MediaKind) ?? '')
const sort = computed<ItemSort>(() => (route.query.sort as ItemSort) ?? 'added_at')
const order = computed<'asc' | 'desc'>(() => (route.query.order as 'asc' | 'desc') ?? 'desc')
const page = computed(() => Number(route.query.page ?? 1))
const pageSize = 48

const pageCount = computed(() => Math.max(1, Math.ceil(total.value / pageSize)))

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

/** 动态统计搜索命中结果中实际存在的分类 */
const searchKindCounts = computed(() => {
  const map = new Map<MediaKind, number>()
  for (const it of rawSearchItems.value) {
    map.set(it.kind, (map.get(it.kind) ?? 0) + 1)
  }
  return Array.from(map.entries()).map(([k, count]) => ({
    kind: k,
    count,
  }))
})

/** 动态统计继续观看中实际存在的分类 */
const continueKindCounts = computed(() => {
  const map = new Map<MediaKind, number>()
  for (const it of rawResumeItems.value) {
    map.set(it.kind, (map.get(it.kind) ?? 0) + 1)
  }
  return Array.from(map.entries()).map(([k, count]) => ({
    kind: k,
    count,
  }))
})

/** 页签：搜索或继续观看模式下根据实际结果动态生成，常规浏览展示全库 */
const tabs = computed<Array<{ value: MediaKind | ''; label: string }>>(() => {
  if (searchQuery.value) {
    const list = searchKindCounts.value
    const totalCount = rawSearchItems.value.length
    return [
      { value: '', label: `全部 ${totalCount}` },
      ...list.map((k) => ({
        value: k.kind,
        label: `${KIND_LABEL[k.kind] ?? k.kind} ${k.count}`,
      })),
    ]
  }

  if (isContinueMode.value) {
    const list = continueKindCounts.value
    const totalCount = rawResumeItems.value.length
    return [
      { value: '', label: `全部 ${totalCount}` },
      ...list.map((k) => ({
        value: k.kind,
        label: `${KIND_LABEL[k.kind] ?? k.kind} ${k.count}`,
      })),
    ]
  }

  return [
    { value: '', label: '全部' },
    ...kinds.value.map((k) => ({
      value: k.kind,
      label: `${KIND_LABEL[k.kind] ?? k.kind} ${k.count}`,
    })),
  ]
})

const displayedItems = computed(() => {
  if (searchQuery.value) {
    return kind.value
      ? rawSearchItems.value.filter((i) => i.kind === kind.value)
      : rawSearchItems.value
  }
  return items.value
})

const displayedResumeItems = computed(() => {
  return kind.value
    ? rawResumeItems.value.filter((r) => r.kind === kind.value)
    : rawResumeItems.value
})

const displayedTotal = computed(() => {
  if (isContinueMode.value) {
    return displayedResumeItems.value.length
  }
  if (searchQuery.value) {
    return displayedItems.value.length
  }
  return total.value
})

async function loadKinds(): Promise<void> {
  try {
    kinds.value = await api.mediaKinds()
  } catch {
    kinds.value = []
  }
}

async function load(): Promise<void> {
  loading.value = true
  error.value = null
  try {
    if (isContinueMode.value) {
      const all = await api.resumeList(100)
      rawResumeItems.value = all
      items.value = []
      rawSearchItems.value = []
    } else if (searchQuery.value) {
      const p = await api.search(searchQuery.value, {
        page: 1,
        page_size: 200,
      })
      rawSearchItems.value = p.data
      items.value = p.data
      total.value = p.page.total
      rawResumeItems.value = []
    } else {
      const p = await api.listMedia({
        kind: kind.value || undefined,
        sort: sort.value,
        order: order.value,
        page: page.value,
        page_size: pageSize,
      })
      items.value = p.data
      total.value = p.page.total
      rawSearchItems.value = []
      rawResumeItems.value = []
    }
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

async function refresh(): Promise<void> {
  await load()
  toast(isContinueMode.value ? '播放记录已刷新' : '媒体库已刷新', { type: 'info' })
}

async function onRemoveResume(target: ResumePoint): Promise<void> {
  rawResumeItems.value = rawResumeItems.value.filter(
    (it) => it.session_id !== target.session_id && it.item_id !== target.item_id,
  )
  try {
    if (target.session_id) {
      await api.stopSession(target.session_id)
    }
    toast('已移除观看记录', { type: 'info' })
  } catch {
    /* 静默忽略 */
  }
}

function setKind(val: MediaKind | ''): void {
  const query: Record<string, string | undefined> = { ...route.query, page: undefined }
  if (val) query.kind = val
  else delete query.kind
  void router.push({ name: 'library', query: query as Record<string, string> })
}

function onSortChange(e: Event): void {
  const val = (e.target as HTMLSelectElement).value
  void router.push({
    name: 'library',
    query: { ...route.query, sort: val, page: undefined },
  })
}

async function setView(v: 'grid' | 'list'): Promise<void> {
  view.value = v
  try {
    await api.putSetting('ui.library_view', v)
  } catch {
    /* 允许离线失败 */
  }
}

const hasFilter = computed(
  () => !!route.query.kind || !!route.query.sort || !!route.query.filter || !!route.query.q,
)

function clearFilter(): void {
  void router.push({ name: 'library' })
}

onMounted(async () => {
  await loadKinds()
  await load()
  try {
    const s = await api.getSettings()
    const saved = s.entries.find((e) => e.key === 'ui.library_view')?.value
    if (saved === 'grid' || saved === 'list') view.value = saved
  } catch {
    /* 默认网格 */
  }
})

watch(
  () => [
    route.query.kind,
    route.query.sort,
    route.query.order,
    route.query.page,
    route.query.filter,
    route.query.q,
  ],
  () => void load(),
)
</script>

<template>
  <div>
    <!-- 1. 页面 Header -->
    <div class="page-header">
      <div>
        <h1>{{ pageTitle }}</h1>
        <p>{{ pageSubtitle }}</p>
      </div>
      <button
        type="button"
        class="btn btn-secondary"
        title="重新整理"
        @click="refresh"
      >
        ⟳ 重新整理
      </button>
    </div>

    <!-- 2. 原生工具栏 Toolbar -->
    <div class="toolbar">
      <!-- 分类 Chips -->
      <div class="chips">
        <button
          v-for="t in tabs"
          :key="t.value || 'all'"
          type="button"
          class="chip"
          :class="{ active: t.value === kind }"
          @click="setKind(t.value)"
        >
          {{ t.label }}
        </button>
      </div>

      <span class="toolbar-spacer"></span>

      <!-- 排序下拉与视图切换（仅普通模式显示） -->
      <template v-if="!isContinueMode">
        <label style="font-size:11px;color:var(--muted)">排序</label>
        <select
          class="select"
          aria-label="排序方式"
          :value="sort"
          @change="onSortChange"
        >
          <option value="added_at">添加时间</option>
          <option value="title">名称</option>
          <option value="rating">评分</option>
          <option value="year">年份</option>
        </select>

        <!-- 网格 / 列表视图双态切换 -->
        <div class="view-toggle">
          <button
            type="button"
            :class="{ active: view === 'grid' }"
            aria-label="网格视图"
            @click="setView('grid')"
          >
            <svg viewBox="0 0 24 24">
              <rect x="3" y="3" width="7" height="7" rx="1" />
              <rect x="14" y="3" width="7" height="7" rx="1" />
              <rect x="3" y="14" width="7" height="7" rx="1" />
              <rect x="14" y="14" width="7" height="7" rx="1" />
            </svg>
          </button>
          <button
            type="button"
            :class="{ active: view === 'list' }"
            aria-label="列表视图"
            @click="setView('list')"
          >
            <svg viewBox="0 0 24 24">
              <path d="M9 5h12M9 12h12M9 19h12M3 5h.01M3 12h.01M3 19h.01" />
            </svg>
          </button>
        </div>
      </template>
    </div>

    <!-- 3. 统计状态行 -->
    <div class="section-head" style="margin-bottom:15px">
      <div style="font-size:11px;color:var(--muted)">
        找到 <strong style="color:#e9eeff">{{ displayedTotal }}</strong> 项内容
      </div>
      <button
        v-if="hasFilter"
        type="button"
        class="text-btn"
        @click="clearFilter"
      >
        清除筛选
      </button>
    </div>

    <!-- 错误反馈 -->
    <p v-if="error" style="color:#f87171;font-size:13px">{{ error }}</p>

    <!-- 4.1 继续观看模式渲染 -->
    <div v-else-if="isContinueMode">
      <div v-if="loading" class="continue-grid">
        <div v-for="n in 6" :key="n" class="skeleton-continue" />
      </div>
      <div v-else-if="displayedResumeItems.length > 0" class="continue-grid">
        <ContinueCard
          v-for="r in displayedResumeItems"
          :key="r.session_id || r.item_id"
          :resume="r"
          @remove="onRemoveResume"
        />
      </div>
      <EmptyState
        v-else
        kind="empty"
        action="浏览全部影片"
        @action="$router.push({ name: 'library' })"
      />
    </div>

    <!-- 4.2 普通媒体库双态视图展现 -->
    <div v-else>
      <!-- 加载骨架 -->
      <div v-if="loading" class="library-grid">
        <div v-for="n in 12" :key="n" class="skeleton-poster" />
      </div>

      <div
        v-else-if="displayedItems.length > 0"
        :class="['library-grid', view === 'list' ? 'list-view' : '']"
      >
        <template v-if="view === 'grid'">
          <PosterCard v-for="it in displayedItems" :key="it.id" :item="it" />
        </template>

        <!-- 原生列表视图条目 -->
        <template v-else>
          <article
            v-for="it in displayedItems"
            :key="it.id"
            class="poster-card list-row"
            tabindex="0"
            @click="$router.push({ name: 'item', params: { id: it.id } })"
            @keydown.enter="$router.push({ name: 'item', params: { id: it.id } })"
          >
            <div class="poster-image">
              <img
                :src="`/api/v1/items/${it.id}/images/poster`"
                :alt="it.title"
                loading="lazy"
              />
            </div>
            <div class="poster-info">
              <div class="poster-name">{{ it.title }}</div>
            </div>
            <div class="list-meta">
              {{ KIND_LABEL[it.kind] || '影片' }} · {{ it.year ?? '—' }} · <span class="tag">已入库</span>
            </div>
          </article>
        </template>
      </div>

      <!-- 5. 原生空状态 -->
      <template v-else>
        <EmptyState
          :kind="hasFilter ? 'none' : 'empty'"
          :action="hasFilter ? '清除筛选' : undefined"
          :alt="hasFilter ? undefined : '去设置媒体库'"
          @action="clearFilter"
          @alt="$router.push({ name: 'settings', query: { tab: 'storage' } })"
        />
      </template>

      <!-- 分页器（如总页数大于 1） -->
      <div v-if="!searchQuery && !isContinueMode && pageCount > 1 && !loading" class="pagination-bar">
        <button
          type="button"
          class="btn btn-secondary"
          :disabled="page <= 1"
          @click="$router.push({ query: { ...route.query, page: String(page - 1) } })"
        >
          上一页
        </button>
        <span style="font-size:12px;color:var(--muted)">第 {{ page }} / {{ pageCount }} 页</span>
        <button
          type="button"
          class="btn btn-secondary"
          :disabled="page >= pageCount"
          @click="$router.push({ query: { ...route.query, page: String(page + 1) } })"
        >
          下一页
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.continue-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 16px;
  margin-top: 6px;
}

.skeleton-continue {
  height: 112px;
  border-radius: 12px;
  background: linear-gradient(90deg, #162033 25%, #1e2c47 50%, #162033 75%);
  background-size: 200% 100%;
  animation: shimmer 1.5s infinite;
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

.list-row {
  cursor: pointer;
  outline: none;
}
.list-row:hover {
  background: #152238;
}
.list-meta {
  grid-column: 2;
  grid-row: 2;
  font-size: 11px;
  color: var(--muted);
  display: flex;
  align-items: center;
  gap: 8px;
}

.pagination-bar {
  display: flex;
  justify-content: center;
  align-items: center;
  gap: 16px;
  margin-top: 36px;
}
</style>
