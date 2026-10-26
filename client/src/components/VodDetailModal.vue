<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import Hls from 'hls.js'
import type { AggregatedVodItem, VodEpisode, VodRoute } from '../api/client'
import * as api from '../api/client'
import { toast } from '../stores/toast'

const props = defineProps<{
  open: boolean
  item: AggregatedVodItem | null
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'play', payload: { item: AggregatedVodItem; route: VodRoute; episode: VodEpisode }): void
  (e: 'fullscreen-play', payload: { item: AggregatedVodItem; route: VodRoute; episode: VodEpisode }): void
}>()

const activeRouteIndex = ref(0)
const selectedSourceIds = ref<string[]>([])
const isImported = ref(false)
const importing = ref(false)

// 弹窗内测试试看状态
const testEpisode = ref<VodEpisode | null>(null)
const testVideoRef = ref<HTMLVideoElement | null>(null)
const isTesting = ref(false)
const testError = ref<string | null>(null)
let hlsInstance: Hls | null = null

watch(
  () => props.item,
  (newItem) => {
    stopTestPlayer()
    activeRouteIndex.value = 0
    isImported.value = false
    selectedSourceIds.value = newItem ? newItem.routes.map((r) => r.source_id) : []
    if (newItem?.title) {
      void api.checkVodImported(newItem.title).then((r) => {
        isImported.value = r.imported
      }).catch(() => {})
    }
  },
  { immediate: true },
)

watch(
  () => props.open,
  (val) => {
    if (!val) {
      stopTestPlayer()
    }
  },
)

onBeforeUnmount(() => {
  stopTestPlayer()
})

const activeRoute = computed<VodRoute | null>(() => {
  if (!props.item || !props.item.routes.length) return null
  return props.item.routes[activeRouteIndex.value] ?? props.item.routes[0]
})

function stopTestPlayer(): void {
  if (hlsInstance) {
    hlsInstance.destroy()
    hlsInstance = null
  }
  if (testVideoRef.value) {
    testVideoRef.value.pause()
    testVideoRef.value.removeAttribute('src')
    testVideoRef.value.load()
  }
  isTesting.value = false
  testEpisode.value = null
  testError.value = null
}

async function playEpisodeTest(ep: VodEpisode): Promise<void> {
  if (!props.item || !activeRoute.value) return
  testEpisode.value = ep
  isTesting.value = true
  testError.value = null

  // 同时也触发 emit('play') 告知外界当前选中的测试集（用于保持状态兼容）
  emit('play', {
    item: props.item,
    route: activeRoute.value,
    episode: ep,
  })

  await nextTick()
  const video = testVideoRef.value
  if (!video) return

  if (hlsInstance) {
    hlsInstance.destroy()
    hlsInstance = null
  }

  let streamUrl = ep.play_url
  let hasFallenBack = false

  if (Hls.isSupported() && (streamUrl.includes('.m3u8') || streamUrl.startsWith('http'))) {
    const hls = new Hls({
      enableWorker: true,
      lowLatencyMode: true,
      maxBufferLength: 30,
    })
    hls.loadSource(streamUrl)
    hls.attachMedia(video)
    hls.on(Hls.Events.ERROR, (_, data) => {
      if (data.fatal) {
        if (!hasFallenBack && (streamUrl.startsWith('http://') || streamUrl.startsWith('https://'))) {
          hasFallenBack = true
          streamUrl = api.getVodStreamProxyUrl(ep.play_url, activeRoute.value?.source_id || '')
          hls.loadSource(streamUrl)
          return
        }
        testError.value = '该分集测试加载失败，建议切换上方其他可用线路试播'
      }
    })
    hlsInstance = hls
  } else {
    video.src = streamUrl
  }
  try {
    const playPromise = video.play()
    if (playPromise && typeof playPromise.catch === 'function') {
      playPromise.catch(() => {})
    }
  } catch {
    /* 忽略环境不支持的播放异常 */
  }
}

function onSwitchRoute(idx: number): void {
  activeRouteIndex.value = idx
  // 如果当前正在测试播放，无缝在新线路上找同集测试播放
  if (isTesting.value && testEpisode.value && props.item) {
    const newRoute = props.item.routes[idx]
    if (newRoute) {
      const match =
        newRoute.episodes.find(
          (e) => e.index === testEpisode.value?.index || e.name === testEpisode.value?.name,
        ) || newRoute.episodes[0]
      if (match) {
        void playEpisodeTest(match)
      }
    }
  }
}

function toggleRouteSelection(sourceId: string, event: Event): void {
  event.stopPropagation()
  const idx = selectedSourceIds.value.indexOf(sourceId)
  if (idx >= 0) {
    if (selectedSourceIds.value.length <= 1) {
      toast('至少需要保留一条可用线路', { type: 'warn' })
      return
    }
    selectedSourceIds.value.splice(idx, 1)
  } else {
    selectedSourceIds.value.push(sourceId)
  }
}

async function onImport(mode: 'all' | 'current' = 'current'): Promise<void> {
  if (!props.item || importing.value) return
  importing.value = true
  try {
    const targetSourceIds =
      mode === 'current' && activeRoute.value
        ? [activeRoute.value.source_id]
        : selectedSourceIds.value
    const defaultSourceId = activeRoute.value?.source_id
    const res = await api.importVodItem(props.item, undefined, {
      selectedSourceIds: targetSourceIds,
      defaultSourceId,
    })
    isImported.value = true
    const routeName = activeRoute.value?.source_name ?? '首选线路'
    toast(
      res.is_new
        ? `《${props.item.title}》已成功入库（首选：${mode === 'current' ? routeName : '所选多线路'}）`
        : `《${props.item.title}》已在媒体库中`,
      { type: 'info' },
    )
  } catch {
    toast('添加失败，请稍后重试', { type: 'error' })
  } finally {
    importing.value = false
  }
}

function onFullscreenPlay(): void {
  if (!props.item || !activeRoute.value) return
  const ep = testEpisode.value ?? activeRoute.value.episodes[0]
  if (!ep) return
  stopTestPlayer()
  emit('fullscreen-play', {
    item: props.item,
    route: activeRoute.value,
    episode: ep,
  })
}

function close(): void {
  stopTestPlayer()
  emit('close')
}
</script>

<template>
  <div v-if="open && item" class="vod-modal-backdrop" @click.self="close">
    <div class="vod-modal card">
      <!-- 弹窗顶栏 -->
      <div class="vod-modal-header">
        <div class="vod-header-badge">
          <span class="chip-online">✦ 云端聚合在线试看</span>
          <span v-if="item.year" class="chip">{{ item.year }}</span>
          <span v-if="item.type_name" class="chip">{{ item.type_name }}</span>
        </div>
        <div class="vod-header-actions">
          <button
            type="button"
            class="header-action-btn"
            title="进入全屏沉浸播放页"
            @click="onFullscreenPlay"
          >
            <span>全屏播放</span>
          </button>
          <button type="button" class="icon-btn close-btn" title="关闭" @click="close">
            ✕
          </button>
        </div>
      </div>

      <!-- 弹窗主体内容 -->
      <div class="vod-modal-body">
        <!-- 左侧媒体视口：未试看显示海报，点击选集后原地切换为测试播放器 -->
        <div class="vod-media-pane">
          <div v-if="isTesting" class="vod-test-player-wrap">
            <video
              ref="testVideoRef"
              class="vod-test-video"
              controls
              playsinline
              autoplay
            ></video>
            <div class="test-player-bar">
              <span class="test-badge">✦ 测试播放中: {{ testEpisode?.name }}</span>
              <button type="button" class="btn-stop-test" title="退出测试" @click="stopTestPlayer">✕</button>
            </div>
            <div v-if="testError" class="test-error-overlay">
              <span>{{ testError }}</span>
            </div>
          </div>

          <div v-else class="vod-poster-wrap">
            <img
              v-if="item.poster"
              :src="item.poster"
              :alt="item.title"
              class="vod-poster"
              loading="lazy"
            />
            <div v-else class="vod-poster-placeholder">
              <span>{{ item.title.slice(0, 2) }}</span>
            </div>
          </div>
        </div>

        <!-- 右侧元数据与入库操作 -->
        <div class="vod-meta-wrap">
          <h2 class="vod-title">{{ item.title }}</h2>
          <p v-if="item.remark" class="vod-remark">{{ item.remark }}</p>

          <div class="vod-details">
            <p v-if="item.director" class="vod-detail-row">
              <span class="detail-label">导演：</span>
              <span class="detail-val">{{ item.director }}</span>
            </p>
            <p v-if="item.actor" class="vod-detail-row">
              <span class="detail-label">主演：</span>
              <span class="detail-val">{{ item.actor }}</span>
            </p>
          </div>

          <p v-if="item.content" class="vod-content">
            {{ item.content }}
          </p>

          <!-- 线路入库操作栏：可单选当前线路入库，或多线路入库 -->
          <div class="vod-actions">
            <button
              type="button"
              class="btn-import btn-import-primary"
              :class="{ 'btn-imported': isImported }"
              :disabled="importing"
              :title="`仅将当前选中的【${activeRoute?.source_name}】入库到媒体库`"
              @click="onImport('current')"
            >
              <span v-if="importing">正在入库…</span>
              <span v-else-if="isImported">✓ 已在媒体库</span>
              <span v-else>+ 仅入库当前线路 ({{ activeRoute?.source_name }})</span>
            </button>

            <button
              v-if="item.routes.length > 1 && !isImported"
              type="button"
              class="btn-import btn-import-sub"
              :disabled="importing"
              :title="`将勾选的 ${selectedSourceIds.length} 条可用线路全部入库`"
              @click="onImport('all')"
            >
              + 入库全部可用 ({{ selectedSourceIds.length }}条)
            </button>
          </div>
        </div>
      </div>

      <!-- 线路选择面板 (可自选入库线路与测试线路) -->
      <div class="vod-routes-section">
        <div class="routes-header">
          <span class="section-label">可用线路 ({{ item.routes.length }} 条)</span>
          <span class="routes-tip">点击切换测试线路，勾选框用于选择要入库的线路</span>
        </div>

        <div class="routes-pill-list">
          <div
            v-for="(r, idx) in item.routes"
            :key="idx"
            class="route-pill"
            :class="{ active: activeRouteIndex === idx }"
            @click="onSwitchRoute(idx)"
          >
            <input
              type="checkbox"
              class="route-checkbox"
              :checked="selectedSourceIds.includes(r.source_id)"
              :title="selectedSourceIds.includes(r.source_id) ? '已选定入库此线路' : '点击勾选此线路入库'"
              @click="toggleRouteSelection(r.source_id, $event)"
            />
            <span>{{ r.source_name }}</span>
            <span class="route-count">({{ r.episodes.length }}集)</span>
            <span v-if="activeRouteIndex === idx" class="route-cur-badge">首选</span>
          </div>
        </div>
      </div>

      <!-- 选集列表：点击在弹窗内测试播放 -->
      <div v-if="activeRoute" class="vod-episodes-section">
        <div class="episodes-header">
          <span class="section-label">选集列表</span>
          <span class="episodes-tip">（点击分集将在弹窗内进行测试播放）</span>
        </div>

        <div class="episodes-grid">
          <button
            v-for="ep in activeRoute.episodes"
            :key="ep.index"
            type="button"
            class="episode-btn"
            :class="{ 'is-testing-ep': isTesting && testEpisode?.index === ep.index }"
            @click="playEpisodeTest(ep)"
          >
            <span v-if="isTesting && testEpisode?.index === ep.index" class="test-pulse">▶ </span>
            {{ ep.name }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.vod-modal-backdrop {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(0, 0, 0, 0.75);
  backdrop-filter: blur(8px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
}

.vod-modal {
  width: 100%;
  max-width: 860px;
  max-height: 90vh;
  background: #14121f;
  border: 1px solid rgba(168, 85, 247, 0.25);
  border-radius: 16px;
  box-shadow: 0 20px 50px rgba(0, 0, 0, 0.6);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.vod-modal-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 24px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
}

.vod-header-badge {
  display: flex;
  gap: 8px;
  align-items: center;
}

.chip-online {
  background: rgba(245, 158, 11, 0.18);
  color: #fbbf24;
  font-size: 11px;
  padding: 3px 8px;
  border-radius: 999px;
  font-weight: 600;
  border: 1px solid rgba(245, 158, 11, 0.35);
}

.vod-header-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.header-action-btn {
  background: rgba(124, 58, 237, 0.2);
  border: 1px solid rgba(124, 58, 237, 0.4);
  color: #c4b5fd;
  font-size: 12px;
  padding: 5px 12px;
  border-radius: 6px;
  cursor: pointer;
  transition: all 0.2s;
}
.header-action-btn:hover {
  background: rgba(124, 58, 237, 0.4);
  color: #fff;
}

.close-btn {
  font-size: 16px;
  width: 32px;
  height: 32px;
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(255, 255, 255, 0.06);
  color: #aaa;
  cursor: pointer;
  border: none;
}
.close-btn:hover {
  background: rgba(255, 255, 255, 0.12);
  color: #fff;
}

.vod-modal-body {
  display: flex;
  gap: 24px;
  padding: 20px 24px;
  overflow-y: auto;
  max-height: 260px;
}

.vod-media-pane {
  width: 280px;
  height: 180px;
  flex-shrink: 0;
  border-radius: 10px;
  overflow: hidden;
  background: #1e1b2e;
  position: relative;
}

.vod-poster-wrap {
  width: 100%;
  height: 100%;
}

.vod-poster {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.vod-poster-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  background: linear-gradient(135deg, #2b1f47, #181426);
  color: rgba(255, 255, 255, 0.4);
  font-weight: 700;
  font-size: 24px;
}

.vod-test-player-wrap {
  width: 100%;
  height: 100%;
  position: relative;
  background: #000;
  display: flex;
  align-items: center;
  justify-content: center;
}

.vod-test-video {
  width: 100%;
  height: 100%;
  object-fit: contain;
}

.test-player-bar {
  position: absolute;
  top: 6px;
  left: 6px;
  right: 6px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  pointer-events: none;
}

.test-badge {
  background: rgba(0, 0, 0, 0.7);
  color: #38bdf8;
  font-size: 11px;
  padding: 2px 8px;
  border-radius: 4px;
  border: 1px solid rgba(56, 189, 248, 0.4);
}

.btn-stop-test {
  pointer-events: auto;
  background: rgba(0, 0, 0, 0.7);
  color: #fff;
  border: none;
  border-radius: 4px;
  width: 22px;
  height: 22px;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 12px;
}
.btn-stop-test:hover {
  background: rgba(239, 68, 68, 0.8);
}

.test-error-overlay {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.85);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 12px;
  text-align: center;
  color: #f87171;
  font-size: 12px;
}

.vod-meta-wrap {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.vod-title {
  margin: 0;
  font-size: 20px;
  font-weight: 700;
  color: #f3f4f6;
}

.vod-remark {
  margin: 0;
  font-size: 13px;
  color: #a78bfa;
}

.vod-details {
  font-size: 12px;
  color: #9ca3af;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.vod-detail-row {
  margin: 0;
}
.detail-label {
  color: #6b7280;
}

.vod-content {
  margin: 0;
  font-size: 12px;
  color: #9ca3af;
  line-height: 1.6;
  overflow: hidden;
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 3;
}

.vod-routes-section {
  padding: 14px 24px;
  background: rgba(0, 0, 0, 0.25);
  border-top: 1px solid rgba(255, 255, 255, 0.05);
}

.routes-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 10px;
}

.section-label {
  font-size: 13px;
  font-weight: 650;
  color: #e5e7eb;
}

.routes-tip {
  font-size: 11px;
  color: #9ca3af;
}

.routes-pill-list {
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
}

.route-pill {
  padding: 6px 12px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.1);
  color: #d1d5db;
  font-size: 12px;
  cursor: pointer;
  display: flex;
  gap: 6px;
  align-items: center;
  transition: all 0.2s ease;
  user-select: none;
}
.route-pill:hover {
  background: rgba(168, 85, 247, 0.15);
  border-color: rgba(168, 85, 247, 0.35);
  color: #fff;
}
.route-pill.active {
  background: #7c3aed;
  border-color: #9333ea;
  color: #fff;
  font-weight: 600;
  box-shadow: 0 0 14px rgba(124, 58, 237, 0.45);
}

.route-checkbox {
  cursor: pointer;
  accent-color: #a855f7;
}

.route-cur-badge {
  font-size: 10px;
  background: rgba(255, 255, 255, 0.2);
  padding: 1px 6px;
  border-radius: 4px;
}

.route-count {
  font-size: 11px;
  opacity: 0.8;
}

.vod-episodes-section {
  padding: 14px 24px 22px;
  overflow-y: auto;
  max-height: 240px;
}

.episodes-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 10px;
}

.episodes-tip {
  font-size: 11px;
  color: #9ca3af;
}

.episodes-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(88px, 1fr));
  gap: 8px;
}

.episode-btn {
  padding: 9px 8px;
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid rgba(255, 255, 255, 0.08);
  color: #e5e7eb;
  font-size: 12px;
  cursor: pointer;
  text-align: center;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  transition: all 0.15s ease;
}
.episode-btn:hover {
  background: rgba(168, 85, 247, 0.2);
  border-color: rgba(168, 85, 247, 0.4);
  color: #fff;
  transform: translateY(-1px);
}
.episode-btn.is-testing-ep {
  background: rgba(56, 189, 248, 0.2);
  border-color: #38bdf8;
  color: #38bdf8;
  font-weight: 700;
  box-shadow: 0 0 10px rgba(56, 189, 248, 0.35);
}

.test-pulse {
  animation: pulse 1s infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}

.vod-actions {
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
  align-items: center;
  margin-top: 10px;
}

.btn-import {
  padding: 7px 14px;
  border: 0;
  border-radius: 6px;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  transition: all 0.2s;
}

.btn-import-primary {
  background: var(--accent);
  color: #fff;
}
.btn-import-primary:hover:not(:disabled) {
  opacity: 0.9;
  transform: translateY(-1px);
}

.btn-import-sub {
  background: rgba(255, 255, 255, 0.08);
  color: #e5e7eb;
  border: 1px solid rgba(255, 255, 255, 0.15);
}
.btn-import-sub:hover:not(:disabled) {
  background: rgba(255, 255, 255, 0.14);
}

.btn-import:disabled {
  opacity: 0.7;
  cursor: not-allowed;
}

.btn-import.btn-imported {
  background: rgba(34, 197, 94, 0.2);
  color: #4ade80;
  border: 1px solid rgba(34, 197, 94, 0.4);
}

/* 手机移动端响应式优化 */
@media (max-width: 640px) {
  .vod-modal-backdrop {
    padding: 8px;
    align-items: flex-end;
  }
  .vod-modal {
    max-height: 94vh;
    border-radius: 16px 16px 8px 8px;
  }
  .vod-modal-header {
    padding: 12px 14px;
  }
  .vod-modal-body {
    flex-direction: column;
    gap: 12px;
    padding: 12px 14px;
    max-height: 280px;
  }
  .vod-media-pane {
    width: 100%;
    height: 170px;
  }
  .vod-routes-section {
    padding: 10px 14px;
  }
  .vod-episodes-section {
    padding: 10px 14px 16px;
    max-height: 200px;
  }
  .episodes-grid {
    grid-template-columns: repeat(auto-fill, minmax(64px, 1fr));
    gap: 6px;
  }
  .episode-btn {
    padding: 8px 4px;
    font-size: 11px;
    min-height: 38px;
  }
  .vod-actions {
    flex-direction: column;
    width: 100%;
    margin-top: 8px;
  }
  .btn-import {
    width: 100%;
    justify-content: center;
    min-height: 40px;
  }
}
</style>
