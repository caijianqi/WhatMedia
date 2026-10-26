<script setup lang="ts">
/**
 * 全网多源影视聚合独立模块（AggregatorView）。
 * 深度结合 MoonTV 采集源规范：
 *   1. 独立全网并发搜索与关键词检索
 *   2. 热播/最新采集漫游发现流（无需搜索也能浏览热播大片与连载剧）
 *   3. 采集源健康状态监测与快捷配置入口
 *   4. 一键添加到媒体库（虚拟云端资产化）
 *   5. 多线路无缝试看与选集播放
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import * as api from '../api/client'
import type { AggregatedVodItem, VodEpisode, VodRoute, VodSourceConfig } from '../api/client'
import { toast } from '../stores/toast'
import VodDetailModal from '../components/VodDetailModal.vue'

const route = useRoute()
const router = useRouter()

// ── 搜索状态 ────────────────────────────────────────────────
const keyword = ref('')
const searching = ref(false)
const searchResults = ref<AggregatedVodItem[]>([])
const hasSearched = ref(false)

// ── 热播发现流 ──────────────────────────────────────────────
const hotLoading = ref(false)
const hotItems = ref<AggregatedVodItem[]>([])

// ── 采集源状态 ──────────────────────────────────────────────
const sources = ref<VodSourceConfig[]>([])
const sourcesLoading = ref(false)
const sourcesModalOpen = ref(false)

// ── 分类筛选 ────────────────────────────────────────────────
type CategoryFilter = '' | 'movie' | 'series' | 'anime' | 'variety'
const activeCategory = ref<CategoryFilter>('')

const categories: Array<{ key: CategoryFilter; label: string }> = [
  { key: '', label: '全部' },
  { key: 'movie', label: '电影' },
  { key: 'series', label: '剧集' },
  { key: 'anime', label: '动漫' },
  { key: 'variety', label: '综艺' },
]

// ── 选集与详情弹窗 ──────────────────────────────────────────
const selectedVod = ref<AggregatedVodItem | null>(null)
const detailModalOpen = ref(false)

// ── 媒体库入库状态追踪 ──────────────────────────────────────
const importedTitles = ref<Set<string>>(new Set())
const importingTitles = ref<Set<string>>(new Set())

const activeSourcesCount = computed(() => sources.value.filter((s) => s.is_active).length)

// ── 过滤后条目列表 ──────────────────────────────────────────
const displayItems = computed(() => {
  const list = hasSearched.value ? searchResults.value : hotItems.value
  if (!activeCategory.value) return list

  return list.filter((item) => {
    const type = (item.type_name || '').toLowerCase()
    if (activeCategory.value === 'movie') return type.includes('电影') || type.includes('片')
    if (activeCategory.value === 'series') return type.includes('剧') || type.includes('电视')
    if (activeCategory.value === 'anime') return type.includes('动漫') || type.includes('动画')
    if (activeCategory.value === 'variety') return type.includes('综艺')
    return true
  })
})

async function loadSources(): Promise<void> {
  sourcesLoading.value = true
  try {
    sources.value = await api.listVodSources()
  } catch {
    sources.value = []
  } finally {
    sourcesLoading.value = false
  }
}

async function loadHotFeed(): Promise<void> {
  hotLoading.value = true
  try {
    hotItems.value = await api.getHotVodItems()
  } catch {
    hotItems.value = []
  } finally {
    hotLoading.value = false
  }
}

async function runSearch(q?: string): Promise<void> {
  const query = (q ?? keyword.value).trim()
  if (!query) {
    hasSearched.value = false
    searchResults.value = []
    return
  }

  searching.value = true
  hasSearched.value = true

  try {
    const res = await api.searchAggregate(query, { page_size: 40 })
    searchResults.value = res.remote
    // 异步排查入库状态
    void checkBatchImported(res.remote)
  } catch {
    searchResults.value = []
    toast('全网检索失败，请检查采集源网络连接', { type: 'error' })
  } finally {
    searching.value = false
  }
}

async function checkBatchImported(items: AggregatedVodItem[]): Promise<void> {
  for (const item of items) {
    if (importedTitles.value.has(item.title)) continue
    try {
      const res = await api.checkVodImported(item.title)
      if (res.imported) {
        importedTitles.value.add(item.title)
      }
    } catch {
      /* 静默 */
    }
  }
}

function onSearchSubmit(): void {
  const q = keyword.value.trim()
  void router.replace({ query: q ? { q } : {} })
  void runSearch(q)
}

function clearSearch(): void {
  keyword.value = ''
  hasSearched.value = false
  searchResults.value = []
  void router.replace({ query: {} })
  if (hotItems.value.length === 0) {
    void loadHotFeed()
  }
}

function openVodDetail(item: AggregatedVodItem): void {
  selectedVod.value = item
  detailModalOpen.value = true
}

async function handleImport(item: AggregatedVodItem): Promise<void> {
  if (importingTitles.value.has(item.title) || importedTitles.value.has(item.title)) return

  importingTitles.value.add(item.title)
  try {
    const res = await api.importVodItem(item)
    importedTitles.value.add(item.title)
    toast(res.is_new ? `《${item.title}》已成功添加到媒体库` : `《${item.title}》已在媒体库中`, {
      type: 'info',
    })
  } catch {
    toast(`添加《${item.title}》失败，请稍后重试`, { type: 'error' })
  } finally {
    importingTitles.value.delete(item.title)
  }
}

function onEpisodeSelect(_payload: {
  item: AggregatedVodItem
  route: VodRoute
  episode: VodEpisode
}): void {
  // 弹窗内已进行测试试看播放，不跳出大厅
}

function onFullscreenPlayEpisode(payload: {
  item: AggregatedVodItem
  route: VodRoute
  episode: VodEpisode
}): void {
  detailModalOpen.value = false
  void router.push({
    name: 'play',
    params: { id: 'vod' },
    query: {
      vod_url: payload.episode.play_url,
      vod_title: `${payload.item.title} - ${payload.episode.name}`,
      vod_source: payload.route.source_name,
    },
  })
}

onMounted(() => {
  void loadSources()
  const qParam = route.query.q as string | undefined
  if (qParam && qParam.trim()) {
    keyword.value = qParam.trim()
    void runSearch(qParam.trim())
  } else {
    void loadHotFeed()
  }
})

watch(
  () => route.query.q,
  (newQ) => {
    if (typeof newQ === 'string' && newQ.trim() && newQ !== keyword.value) {
      keyword.value = newQ.trim()
      void runSearch(newQ.trim())
    }
  },
)
</script>

<template>
  <main class="aggregator-page">
    <!-- 顶部 Banner 与专属搜索区 -->
    <header class="aggregator-header">
      <div class="header-content">
        <div class="eyebrow">
          <span class="chip-live">✦ 多源在线聚合</span>
          <span class="source-status-badge" @click="sourcesModalOpen = true">
            <span class="status-dot"></span>
            {{ activeSourcesCount }}/{{ sources.length }} 采集线路在线
          </span>
        </div>
        <h1 class="page-title">全网影视聚合大厅</h1>
        <p class="page-subtitle">
          并发采集全网 MacCMS 资源站，海量热播剧集与院线大片，支持多线路智能秒播与一键虚拟入库。
        </p>

        <!-- 聚合搜索输入框 -->
        <form class="aggregator-searchbox card" @submit.prevent="onSearchSubmit">
          <svg class="search-icon" viewBox="0 0 24 24">
            <circle cx="10.8" cy="10.8" r="6.8" />
            <path d="m16 16 4.5 4.5" />
          </svg>
          <input
            v-model="keyword"
            type="search"
            placeholder="搜索全网电影、电视剧、动漫、演员、导演..."
            autocomplete="off"
            aria-label="搜索全网影视"
          />
          <button v-if="keyword" type="button" class="btn-clear" title="清空" @click="clearSearch">
            ✕
          </button>
          <button type="submit" class="btn btn-primary btn-search" :disabled="searching">
            {{ searching ? '全网并发检索中…' : '搜索' }}
          </button>
        </form>
      </div>
    </header>

    <!-- 分类标签与状态栏 -->
    <section class="filter-bar">
      <div class="category-tabs">
        <button
          v-for="cat in categories"
          :key="cat.key"
          type="button"
          class="tab-btn"
          :class="{ active: activeCategory === cat.key }"
          @click="activeCategory = cat.key"
        >
          {{ cat.label }}
        </button>
      </div>

      <div class="filter-actions">
        <span v-if="hasSearched" class="search-stats">
          找到 <strong>{{ searchResults.length }}</strong> 部全网影视
        </span>
        <span v-else class="search-stats">
          全网今日热播 / 最新更新 ({{ hotItems.length }})
        </span>

        <button
          v-if="hasSearched"
          type="button"
          class="btn-text reset-btn"
          @click="clearSearch"
        >
          返回热播推荐
        </button>
      </div>
    </section>

    <!-- 内容展示网格 -->
    <section class="content-section">
      <!-- 加载中状态 -->
      <div v-if="searching || hotLoading" class="loading-state card">
        <div class="spinner"></div>
        <p class="loading-text">
          {{ searching ? '正在向全网多个采集站并发检索...' : '正在同步全网最新热播内容...' }}
        </p>
      </div>

      <!-- 空结果状态 -->
      <div
        v-else-if="displayItems.length === 0"
        class="empty-state card"
      >
        <div class="empty-icon">🌐</div>
        <h3>{{ hasSearched ? `未检索到关于「${keyword}」的影视资源` : '暂无推荐影视数据' }}</h3>
        <p v-if="sources.length === 0" class="empty-tip">
          当前尚未配置任何采集源。请前往设置页导入采集站配置。
        </p>
        <p v-else class="empty-tip">
          可以尝试简化搜索关键词，或在右上角检查采集线路状态。
        </p>
        <RouterLink
          v-if="sources.length === 0"
          :to="{ name: 'settings' }"
          class="btn btn-primary"
        >
          前往配置采集源
        </RouterLink>
      </div>

      <!-- 影视海报卡片网格 -->
      <div v-else class="vod-grid">
        <article
          v-for="item in displayItems"
          :key="item.id"
          class="vod-card card"
        >
          <!-- 封面图 -->
          <div class="poster-wrap" @click="openVodDetail(item)">
            <img
              v-if="item.poster"
              :src="item.poster"
              :alt="item.title"
              class="poster-img"
              loading="lazy"
            />
            <div v-else class="poster-placeholder">
              <span>{{ item.title.slice(0, 2) }}</span>
            </div>

            <div class="poster-badge-top">
              <span class="badge-routes">{{ item.routes.length }} 线路</span>
              <span v-if="item.year" class="badge-year">{{ item.year }}</span>
            </div>

            <div v-if="item.remark" class="poster-badge-bottom">
              {{ item.remark }}
            </div>

            <!-- 悬浮播放微交互 -->
            <div class="poster-overlay">
              <button type="button" class="play-overlay-btn" aria-label="查看与试看">
                <svg viewBox="0 0 24 24">
                  <path d="m8 5 12 7-12 7z" />
                </svg>
              </button>
            </div>
          </div>

          <!-- 底部详情与操作 -->
          <div class="vod-card-body">
            <h3 class="vod-card-title" :title="item.title" @click="openVodDetail(item)">
              {{ item.title }}
            </h3>
            <p v-if="item.type_name" class="vod-card-type">{{ item.type_name }}</p>

            <div class="vod-card-actions">
              <button
                type="button"
                class="btn-card-import"
                :class="{ 'is-imported': importedTitles.has(item.title) }"
                :disabled="importingTitles.has(item.title)"
                @click.stop="handleImport(item)"
              >
                <span v-if="importingTitles.has(item.title)">入库中…</span>
                <span v-else-if="importedTitles.has(item.title)">✓ 已在库</span>
                <span v-else>+ 入库</span>
              </button>

              <button
                type="button"
                class="btn-card-play"
                @click.stop="openVodDetail(item)"
              >
                选集播放
              </button>
            </div>
          </div>
        </article>
      </div>
    </section>

    <!-- 选集详情弹窗 -->
    <VodDetailModal
      :open="detailModalOpen"
      :item="selectedVod"
      @close="detailModalOpen = false"
      @play="onEpisodeSelect"
      @fullscreen-play="onFullscreenPlayEpisode"
    />

    <!-- 采集源状态侧拉/浮层 -->
    <div
      v-if="sourcesModalOpen"
      class="sources-backdrop"
      @click.self="sourcesModalOpen = false"
    >
      <div class="sources-modal card">
        <div class="sources-modal-header">
          <h3>采集线路健康状态 ({{ sources.length }})</h3>
          <button type="button" class="btn-clear" @click="sourcesModalOpen = false">✕</button>
        </div>
        <div class="sources-list">
          <div
            v-for="src in sources"
            :key="src.id"
            class="source-row"
          >
            <div class="source-info">
              <span class="source-status-light" :class="{ active: src.is_active }"></span>
              <span class="source-name">{{ src.name }}</span>
              <span class="source-timeout">{{ src.timeout_ms }}ms</span>
            </div>
            <span class="source-state-tag" :class="{ 'tag-active': src.is_active }">
              {{ src.is_active ? '启用中' : '已停用' }}
            </span>
          </div>
        </div>
        <div class="sources-modal-footer">
          <RouterLink :to="{ name: 'settings' }" class="btn btn-secondary btn-full">
            管理与导入采集源
          </RouterLink>
        </div>
      </div>
    </div>
  </main>
</template>

<style scoped>
.aggregator-page {
  padding: 32px 36px 64px;
  max-width: 1600px;
  margin: 0 auto;
}

.aggregator-header {
  margin-bottom: 28px;
}

.eyebrow {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 12px;
}

.chip-live {
  background: rgba(168, 85, 247, 0.2);
  color: #c084fc;
  font-size: 11px;
  font-weight: 700;
  padding: 3px 8px;
  border-radius: 9999px;
  border: 1px solid rgba(168, 85, 247, 0.4);
}

.source-status-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: var(--muted);
  background: rgba(255, 255, 255, 0.05);
  padding: 3px 10px;
  border-radius: 9999px;
  cursor: pointer;
  transition: all 0.2s;
}

.source-status-badge:hover {
  background: rgba(255, 255, 255, 0.1);
  color: var(--text);
}

.status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #22c55e;
  box-shadow: 0 0 6px rgba(34, 197, 94, 0.6);
}

.page-title {
  font-size: 28px;
  font-weight: 800;
  color: var(--text);
  margin: 0 0 6px;
  letter-spacing: -0.5px;
}

.page-subtitle {
  font-size: 14px;
  color: var(--muted);
  margin: 0 0 20px;
  max-width: 720px;
  line-height: 1.5;
}

.aggregator-searchbox {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 12px 8px 18px;
  background: rgba(20, 27, 45, 0.85);
  border: 1px solid rgba(255, 255, 255, 0.1);
  border-radius: 14px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.25);
  max-width: 840px;
}

.search-icon {
  width: 20px;
  height: 20px;
  stroke: var(--muted);
  fill: none;
  stroke-width: 2;
  flex-shrink: 0;
}

.aggregator-searchbox input {
  flex: 1;
  background: transparent;
  border: 0;
  outline: none;
  color: #fff;
  font-size: 15px;
}

.btn-clear {
  background: transparent;
  border: 0;
  color: var(--muted);
  cursor: pointer;
  font-size: 14px;
  padding: 4px;
}

.btn-clear:hover {
  color: #fff;
}

.btn-search {
  padding: 10px 24px;
  font-weight: 700;
  border-radius: 10px;
}

.filter-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 24px;
  padding-bottom: 12px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.06);
}

.category-tabs {
  display: flex;
  gap: 8px;
}

.tab-btn {
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(255, 255, 255, 0.08);
  color: var(--muted);
  padding: 6px 14px;
  border-radius: 8px;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.15s ease;
}

.tab-btn:hover {
  color: #fff;
  background: rgba(255, 255, 255, 0.08);
}

.tab-btn.active {
  background: var(--accent);
  color: #fff;
  border-color: var(--accent);
}

.filter-actions {
  display: flex;
  align-items: center;
  gap: 16px;
}

.search-stats {
  font-size: 12px;
  color: var(--muted);
}

.search-stats strong {
  color: var(--accent);
}

.btn-text {
  background: transparent;
  border: 0;
  color: var(--accent);
  font-size: 12px;
  cursor: pointer;
  padding: 0;
}

.btn-text:hover {
  text-decoration: underline;
}

.vod-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
  gap: 20px;
}

.vod-card {
  display: flex;
  flex-direction: column;
  overflow: hidden;
  border-radius: 12px;
  background: rgba(22, 30, 49, 0.65);
  border: 1px solid rgba(255, 255, 255, 0.06);
  transition: transform 0.2s, box-shadow 0.2s, border-color 0.2s;
}

.vod-card:hover {
  transform: translateY(-4px);
  border-color: rgba(168, 85, 247, 0.4);
  box-shadow: 0 12px 28px rgba(0, 0, 0, 0.35);
}

.poster-wrap {
  position: relative;
  aspect-ratio: 2 / 2.85;
  background: #111827;
  overflow: hidden;
  cursor: pointer;
}

.poster-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  transition: transform 0.35s ease;
}

.vod-card:hover .poster-img {
  transform: scale(1.05);
}

.poster-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 28px;
  font-weight: 800;
  color: var(--muted);
  background: linear-gradient(135deg, #1e293b, #0f172a);
}

.poster-badge-top {
  position: absolute;
  top: 8px;
  left: 8px;
  right: 8px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  z-index: 2;
}

.badge-routes {
  background: rgba(14, 165, 233, 0.85);
  color: #fff;
  font-size: 10px;
  font-weight: 700;
  padding: 2px 6px;
  border-radius: 4px;
  backdrop-filter: blur(4px);
}

.badge-year {
  background: rgba(0, 0, 0, 0.6);
  color: #e2e8f0;
  font-size: 10px;
  font-weight: 600;
  padding: 2px 6px;
  border-radius: 4px;
}

.poster-badge-bottom {
  position: absolute;
  bottom: 0;
  left: 0;
  right: 0;
  padding: 16px 8px 6px;
  background: linear-gradient(transparent, rgba(0, 0, 0, 0.8));
  font-size: 11px;
  color: #f1f5f9;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  z-index: 2;
}

.poster-overlay {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  opacity: 0;
  transition: opacity 0.2s ease;
  z-index: 3;
}

.vod-card:hover .poster-overlay {
  opacity: 1;
}

.play-overlay-btn {
  width: 44px;
  height: 44px;
  border-radius: 50%;
  background: var(--accent);
  color: #fff;
  border: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  box-shadow: 0 4px 14px rgba(168, 85, 247, 0.6);
  transform: scale(0.9);
  transition: transform 0.2s ease;
}

.play-overlay-btn svg {
  width: 22px;
  height: 22px;
  fill: currentColor;
  margin-left: 2px;
}

.vod-card:hover .play-overlay-btn {
  transform: scale(1);
}

.vod-card-body {
  padding: 12px;
  display: flex;
  flex-direction: column;
  flex: 1;
}

.vod-card-title {
  font-size: 14px;
  font-weight: 700;
  color: var(--text);
  margin: 0 0 4px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  cursor: pointer;
}

.vod-card-title:hover {
  color: var(--accent);
}

.vod-card-type {
  font-size: 11px;
  color: var(--muted);
  margin: 0 0 10px;
}

.vod-card-actions {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
  margin-top: auto;
}

.btn-card-import {
  padding: 6px 8px;
  border-radius: 6px;
  font-size: 11px;
  font-weight: 600;
  background: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.1);
  color: var(--text);
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
}

.btn-card-import:hover:not(:disabled) {
  background: rgba(168, 85, 247, 0.2);
  border-color: rgba(168, 85, 247, 0.4);
  color: #fff;
}

.btn-card-import.is-imported {
  background: rgba(34, 197, 94, 0.15);
  color: #4ade80;
  border-color: rgba(34, 197, 94, 0.3);
}

.btn-card-play {
  padding: 6px 8px;
  border-radius: 6px;
  font-size: 11px;
  font-weight: 600;
  background: var(--accent);
  border: 0;
  color: #fff;
  cursor: pointer;
  white-space: nowrap;
  transition: opacity 0.15s ease;
}

.btn-card-play:hover {
  opacity: 0.9;
}

.loading-state,
.empty-state {
  padding: 64px 24px;
  text-align: center;
  border-radius: 16px;
  background: rgba(20, 27, 45, 0.5);
  border: 1px solid rgba(255, 255, 255, 0.05);
}

.spinner {
  width: 36px;
  height: 36px;
  border: 3px solid rgba(255, 255, 255, 0.1);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
  margin: 0 auto 16px;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

.loading-text {
  font-size: 14px;
  color: var(--muted);
  margin: 0;
}

.empty-icon {
  font-size: 48px;
  margin-bottom: 12px;
}

.empty-state h3 {
  font-size: 18px;
  color: var(--text);
  margin: 0 0 8px;
}

.empty-tip {
  font-size: 13px;
  color: var(--muted);
  max-width: 480px;
  margin: 0 auto 20px;
}

/* 采集源浮层 */
.sources-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.65);
  backdrop-filter: blur(4px);
  z-index: 50;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
}

.sources-modal {
  width: 100%;
  max-width: 480px;
  background: #172138;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 16px;
  overflow: hidden;
  box-shadow: 0 20px 40px rgba(0, 0, 0, 0.5);
}

.sources-modal-header {
  padding: 16px 20px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
}

.sources-modal-header h3 {
  margin: 0;
  font-size: 16px;
  color: #fff;
}

.sources-list {
  padding: 12px 20px;
  max-height: 320px;
  overflow-y: auto;
}

.source-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 0;
  border-bottom: 1px solid rgba(255, 255, 255, 0.04);
}

.source-row:last-child {
  border-bottom: 0;
}

.source-info {
  display: flex;
  align-items: center;
  gap: 8px;
}

.source-status-light {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #ef4444;
}

.source-status-light.active {
  background: #22c55e;
}

.source-name {
  font-size: 13px;
  color: #fff;
  font-weight: 600;
}

.source-timeout {
  font-size: 11px;
  color: var(--muted);
}

.source-state-tag {
  font-size: 11px;
  padding: 2px 6px;
  border-radius: 4px;
  background: rgba(255, 255, 255, 0.08);
  color: var(--muted);
}

.source-state-tag.tag-active {
  background: rgba(34, 197, 94, 0.15);
  color: #4ade80;
}

.sources-modal-footer {
  padding: 12px 20px;
  border-top: 1px solid rgba(255, 255, 255, 0.08);
}

.btn-full {
  width: 100%;
  text-align: center;
  display: block;
}

@media (max-width: 768px) {
  .aggregator-page {
    padding: 16px 16px 48px;
  }
  .vod-grid {
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 12px;
  }
  .page-title {
    font-size: 22px;
  }
}
</style>
