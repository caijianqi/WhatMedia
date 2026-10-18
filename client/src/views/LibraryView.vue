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
import EmptyState from '../components/EmptyState.vue'
import { toast } from '../stores/toast'
import * as api from '../api/client'
import type { ItemSort, KindCount, MediaKind } from '../api/client'

const route = useRoute()
const router = useRouter()

const items = ref<api.ItemSummary[]>([])
const total = ref(0)
const loading = ref(false)
const error = ref<string | null>(null)
const kinds = ref<KindCount[]>([])

/** 网格 / 列表视图。默认网格；若存储过则读取。 */
const view = ref<'grid' | 'list'>('grid')

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

const tabs = computed<Array<{ value: MediaKind | ''; label: string }>>(() => [
  { value: '', label: '全部' },
  ...kinds.value.map((k) => ({
    value: k.kind,
    label: `${KIND_LABEL[k.kind] ?? k.kind} ${k.count}`,
  })),
])

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

async function refresh(): Promise<void> {
  await load()
  toast('媒体库已刷新', { type: 'info' })
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

const hasFilter = computed(() => !!route.query.kind || !!route.query.sort)

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
  () => [route.query.kind, route.query.sort, route.query.order, route.query.page],
  () => void load(),
)
</script>

<template>
  <div>
    <!-- 1. 页面 Header -->
    <div class="page-header">
      <div>
        <h1>媒体库</h1>
        <p>统一浏览与筛选你的个人影视收藏。</p>
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

      <!-- 排序下拉 -->
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
    </div>

    <!-- 3. 统计状态行 -->
    <div class="section-head" style="margin-bottom:15px">
      <div style="font-size:11px;color:var(--muted)">
        找到 <strong style="color:#e9eeff">{{ total }}</strong> 项内容
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

    <!-- 加载骨架 -->
    <div v-else-if="loading" class="library-grid">
      <div v-for="n in 12" :key="n" class="skeleton-poster" />
    </div>

    <!-- 4. 双态视图展现 -->
    <div
      v-else-if="items.length > 0"
      :class="['library-grid', view === 'list' ? 'list-view' : '']"
    >
      <template v-if="view === 'grid'">
        <PosterCard v-for="it in items" :key="it.id" :item="it" />
      </template>

      <!-- 原生列表视图条目 -->
      <template v-else>
        <article
          v-for="it in items"
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
      <!--
        "库里根本没有东西"和"被筛掉了"是两件事：前者要去扫描，后者只要清筛选。
        合成一句"暂无数据"，用户就不知道下一步该干什么。
      -->
      <EmptyState
        :kind="hasFilter ? 'none' : 'empty'"
        :action="hasFilter ? '清除筛选' : undefined"
        :alt="hasFilter ? undefined : '去设置媒体库'"
        @action="clearFilter"
        @alt="$router.push({ name: 'settings', query: { tab: 'storage' } })"
      />
    </template>

    <!-- 分页器（如总页数大于 1） -->
    <div v-if="pageCount > 1 && !loading" class="pagination-bar">
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
