<script setup lang="ts">
/**
 * 首页（HomeView）。
 * 1:1 遵循 ui.html 原生规范：
 *   1. 顶部 Hero 轮播（大标题、双层遮罩、眉毛标签与操作按钮）
 *   2. 数据驱动的分类 Tabs（仅显示库中实际存在的分类）
 *   3. 继续观看分节（3 列横向卡片，带剩余时间与双色滑轨）
 *   4. 最近添加与为你发现（5 列海报行，带原生副标题与查看全部快速跳转）
 *   5. 按类型浏览（4 列 GenreCard 氛围卡网格）
 *   6. 底部家庭影院定制说明条（.section-note）
 */
import { computed, onMounted, ref, watch } from 'vue'
import HeroCarousel, { type HeroSlide } from '../components/HeroCarousel.vue'
import MediaRow from '../components/MediaRow.vue'
import ContinueCard from '../components/ContinueCard.vue'
import GenreCard from '../components/GenreCard.vue'
import * as api from '../api/client'
import type { ItemSummary, KindCount, MediaKind, ResumePoint } from '../api/client'

const KIND_LABELS: Record<string, string> = {
  movie: '电影',
  series: '剧集',
  episode: '剧集',
  anime: '动画',
  documentary: '纪录片',
  variety: '综艺',
  concert: '演唱会',
  music: '音乐',
  music_video: 'MV',
  short_video: '短片',
  home_video: '家庭影像',
  season: '季',
  mixed: '其他',
}

type KindTab = '' | MediaKind

const kinds = ref<KindCount[]>([])
const activeKind = ref<KindTab>('')

const tabs = computed<Array<{ kind: KindTab; label: string }>>(() => [
  { kind: '', label: '全部' },
  ...kinds.value.map((k) => ({
    kind: k.kind,
    label: `${KIND_LABELS[k.kind] ?? k.kind} ${k.count}`,
  })),
])

// ── Hero ────────────────────────────────────────────────────
const slides = ref<HeroSlide[]>([])
const heroLoading = ref(true)

// ── 继续观看 ────────────────────────────────────────────────
const resume = ref<ResumePoint[]>([])
const resumeLoading = ref(true)
const resumeError = ref<string | null>(null)

// ── 横向行：最近添加 / 高分推荐 ──────────────────────────────
interface Row {
  key: string
  title: string
  subtitle: string
  viewAllTo: { name: string; query?: Record<string, string> }
  viewAllText: string
  items: ItemSummary[]
  loading: boolean
  error: string | null
}
const rows = ref<Row[]>([
  {
    key: 'recent',
    title: '最近添加',
    subtitle: '新入库的影片，一眼就能看到',
    viewAllTo: { name: 'library', query: { sort: 'added_at' } },
    viewAllText: '查看全部',
    items: [],
    loading: true,
    error: null,
  },
  {
    key: 'top',
    title: '为你发现',
    subtitle: '高分推荐 · 更多值得放进片单的内容',
    viewAllTo: { name: 'library', query: { sort: 'rating' } },
    viewAllText: '浏览媒体库',
    items: [],
    loading: true,
    error: null,
  },
])

const kindFilter = computed<MediaKind | undefined>(
  () => (activeKind.value === '' ? undefined : activeKind.value),
)

async function loadKinds(): Promise<void> {
  try {
    kinds.value = await api.mediaKinds()
  } catch {
    kinds.value = []
  }
}

async function loadHero(): Promise<void> {
  heroLoading.value = true
  try {
    const page = await api.listMedia({
      sort: 'rating',
      order: 'desc',
      page_size: 5,
      kind: kindFilter.value,
    })
    const picked = page.data.slice(0, 5)
    const details = await Promise.all(
      picked.map((it) => api.getItem(it.id).catch(() => null)),
    )
    slides.value = picked.map((it, i) => ({
      id: it.id,
      title: it.title,
      year: it.year,
      rating: it.rating,
      kind: it.kind,
      overview: details[i]?.overview ?? null,
      genres: details[i]?.genres ?? [],
    }))
  } catch {
    slides.value = []
  } finally {
    heroLoading.value = false
  }
}

async function loadResume(): Promise<void> {
  resumeLoading.value = true
  resumeError.value = null
  try {
    const all = await api.resumeList(24)
    resume.value = kindFilter.value ? all.filter((r) => r.kind === kindFilter.value) : all
  } catch (e) {
    resumeError.value = e instanceof Error ? e.message : String(e)
  } finally {
    resumeLoading.value = false
  }
}

async function loadRow(row: Row, sort: 'added_at' | 'rating'): Promise<void> {
  row.loading = true
  row.error = null
  try {
    const page = await api.listMedia({
      sort,
      order: 'desc',
      page_size: 24,
      kind: kindFilter.value,
    })
    row.items = page.data
  } catch (e) {
    row.error = e instanceof Error ? e.message : String(e)
  } finally {
    row.loading = false
  }
}

async function reloadAll(): Promise<void> {
  await Promise.all([
    loadHero(),
    loadResume(),
    loadRow(rows.value[0]!, 'added_at'),
    loadRow(rows.value[1]!, 'rating'),
  ])
}

onMounted(async () => {
  await loadKinds()
  await reloadAll()
})

watch(activeKind, () => void reloadAll())

// 类型氛围卡固定推荐（真实跳转到对应分类筛选，1:1 对齐 ui.html）
const genreCards = [
  {
    title: '电影',
    subtitle: '大银幕体验 · 影片收藏',
    bg: 'https://images.unsplash.com/photo-1462331940025-496dfbfc7564?auto=format&fit=crop&w=550&q=75',
    to: { name: 'library', query: { kind: 'movie' } },
  },
  {
    title: '电视剧',
    subtitle: '追剧进度 · 按季选集',
    bg: 'https://images.unsplash.com/photo-1533929736458-ca588d08c8be?auto=format&fit=crop&w=550&q=75',
    to: { name: 'library', query: { kind: 'series' } },
  },
  {
    title: '纪录片',
    subtitle: '自然、人文与探索',
    bg: 'https://images.unsplash.com/photo-1448375240586-882707db888b?auto=format&fit=crop&w=550&q=75',
    to: { name: 'library', query: { kind: 'documentary' } },
  },
  {
    title: '探索全部',
    subtitle: '浏览个人媒体收藏',
    bg: 'https://images.unsplash.com/photo-1519608487953-e999c86e7455?auto=format&fit=crop&w=550&q=75',
    to: { name: 'library' },
  },
]
</script>

<template>
  <div class="home-container">
    <!-- Hero 轮播大横幅（紧随 82px 顶栏，margin-top: 0，首屏 1:1 对齐 ui.html） -->
    <div v-if="heroLoading && !slides.length" class="hero-skeleton" />
    <HeroCarousel v-else-if="slides.length" :slides="slides" />

    <!-- 分类 Tab Chips（紧随 Hero 之后，提供按类型快速过滤，不破坏顶部沉浸视觉） -->
    <div v-if="tabs.length > 1" class="home-filter-strip">
      <div class="chips">
        <button
          v-for="t in tabs"
          :key="t.kind || 'all'"
          type="button"
          class="chip"
          :class="{ active: t.kind === activeKind }"
          :aria-pressed="t.kind === activeKind"
          @click="activeKind = t.kind"
        >
          {{ t.label }}
        </button>
      </div>
    </div>

    <!-- 分节一：继续观看 (Continue Watching) -->
    <section v-if="resume.length > 0 || resumeLoading" class="section">
      <div class="section-head">
        <div>
          <h2 class="section-title">继续观看</h2>
          <p class="section-subtitle">接着上次的进度，精彩不必重新寻找</p>
        </div>
        <RouterLink
          :to="{ name: 'library', query: { filter: 'continue' } }"
          class="text-btn"
        >
          <span>查看全部</span>
          <svg viewBox="0 0 24 24">
            <path d="M5 12h14m-6-6 6 6-6 6" />
          </svg>
        </RouterLink>
      </div>

      <p v-if="resumeError && !resume.length" style="color:#f87171;font-size:13px">{{ resumeError }}</p>

      <!-- 横向宽卡片 3 列网格 -->
      <div class="continue-grid">
        <ContinueCard
          v-for="r in resume.slice(0, 3)"
          :key="r.session_id || r.item_id"
          :resume="r"
        />
      </div>
    </section>

    <!-- 分节二 & 分节三：最近添加 & 为你发现/高分推荐（海报行） -->
    <MediaRow
      v-for="row in rows"
      :key="row.key"
      :title="row.title"
      :subtitle="row.subtitle"
      :view-all-to="row.viewAllTo"
      :view-all-text="row.viewAllText"
      :items="row.items"
      :loading="row.loading && !row.items.length"
      :error="row.error && !row.items.length ? row.error : null"
    />

    <!-- 分节四：按类型浏览 (Genre Grid) -->
    <section class="section">
      <div class="section-head">
        <div>
          <h2 class="section-title">按类型浏览</h2>
          <p class="section-subtitle">从喜欢的类型开始探索</p>
        </div>
      </div>
      <div class="genre-grid">
        <GenreCard
          v-for="g in genreCards"
          :key="g.title"
          :title="g.title"
          :subtitle="g.subtitle"
          :bg-image="g.bg"
          :to="g.to"
        />
      </div>
    </section>

    <!-- 分节五：底部家庭影院定制说明条 (Section Note) -->
    <div class="section-note">
      <div style="display:flex;align-items:center;gap:14px">
        <span class="note-icon">
          <svg viewBox="0 0 24 24">
            <rect x="4" y="4" width="16" height="16" rx="3" />
            <path d="M8 12h8M12 8v8" />
          </svg>
        </span>
        <div>
          <strong>你的家庭影院，按你的方式组织</strong>
          <p>统一管理本地媒体、观看记录与收藏。连接真实媒体服务后，这里会显示你的实际内容。</p>
        </div>
      </div>
      <RouterLink :to="{ name: 'settings', query: { tab: 'storage' } }" class="btn btn-secondary">
        <span>连接与设置媒体库</span>
        <svg viewBox="0 0 24 24">
          <path d="M5 12h14m-6-6 6 6-6 6" />
        </svg>
      </RouterLink>
    </div>
  </div>
</template>

<style scoped>
.home-container {
  min-width: 0;
}

.home-filter-strip {
  margin-top: 24px;
}

.hero-skeleton {
  min-height: 365px;
  border-radius: 22px;
  background: linear-gradient(90deg, #152441 25%, #1d3157 50%, #152441 75%);
  background-size: 200% 100%;
  animation: shimmer 1.5s infinite;
}

.continue-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 15px;
}

@media (max-width: 1250px) {
  .continue-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
@media (max-width: 620px) {
  .continue-grid {
    grid-template-columns: 1fr;
  }
}

.continue-skeleton {
  height: 98px;
  border-radius: 14px;
  background: linear-gradient(90deg, #162033 25%, #1e2c47 50%, #162033 75%);
  background-size: 200% 100%;
  animation: shimmer 1.5s infinite;
}

@keyframes shimmer {
  0% { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}
</style>
