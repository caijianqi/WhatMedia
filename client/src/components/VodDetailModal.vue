<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { AggregatedVodItem, VodEpisode, VodRoute } from '../api/client'

const props = defineProps<{
  open: boolean
  item: AggregatedVodItem | null
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'play', payload: { item: AggregatedVodItem; route: VodRoute; episode: VodEpisode }): void
}>()

const activeRouteIndex = ref(0)

watch(
  () => props.item,
  () => {
    activeRouteIndex.value = 0
  },
)

const activeRoute = computed<VodRoute | null>(() => {
  if (!props.item || !props.item.routes.length) return null
  return props.item.routes[activeRouteIndex.value] ?? props.item.routes[0]
})

function onPlay(episode: VodEpisode): void {
  if (!props.item || !activeRoute.value) return
  emit('play', {
    item: props.item,
    route: activeRoute.value,
    episode,
  })
}

function close(): void {
  emit('close')
}
</script>

<template>
  <div v-if="open && item" class="vod-modal-backdrop" @click.self="close">
    <div class="vod-modal card">
      <!-- 弹窗顶栏 -->
      <div class="vod-modal-header">
        <div class="vod-header-badge">
          <span class="chip-online">✦ 云端聚合在线播放</span>
          <span v-if="item.year" class="chip">{{ item.year }}</span>
          <span v-if="item.type_name" class="chip">{{ item.type_name }}</span>
        </div>
        <button type="button" class="icon-btn close-btn" title="关闭" @click="close">
          ✕
        </button>
      </div>

      <!-- 弹窗主体内容 -->
      <div class="vod-modal-body">
        <!-- 左侧海报 -->
        <div class="vod-poster-wrap">
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

        <!-- 右侧元数据 -->
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
        </div>
      </div>

      <!-- 线路选择面板 (MoonTV 风格) -->
      <div class="vod-routes-section">
        <div class="routes-header">
          <span class="section-label">可用线路 ({{ item.routes.length }} 条)</span>
          <span class="routes-tip">若当前线路无法播放，可切换下方其他线路</span>
        </div>

        <div class="routes-pill-list">
          <button
            v-for="(r, idx) in item.routes"
            :key="idx"
            type="button"
            class="route-pill"
            :class="{ active: activeRouteIndex === idx }"
            @click="activeRouteIndex = idx"
          >
            <span>{{ r.source_name }}</span>
            <span class="route-count">({{ r.episodes.length }}集)</span>
          </button>
        </div>
      </div>

      <!-- 选集列表 -->
      <div v-if="activeRoute" class="vod-episodes-section">
        <div class="episodes-header">
          <span class="section-label">选集列表</span>
        </div>

        <div class="episodes-grid">
          <button
            v-for="ep in activeRoute.episodes"
            :key="ep.index"
            type="button"
            class="episode-btn"
            @click="onPlay(ep)"
          >
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
  max-width: 820px;
  max-height: 88vh;
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
  padding: 18px 24px;
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
  max-height: 240px;
}

.vod-poster-wrap {
  width: 140px;
  height: 200px;
  flex-shrink: 0;
  border-radius: 10px;
  overflow: hidden;
  background: #1e1b2e;
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
  -webkit-line-clamp: 4;
}

.vod-routes-section {
  padding: 16px 24px;
  background: rgba(0, 0, 0, 0.25);
  border-top: 1px solid rgba(255, 255, 255, 0.05);
}

.routes-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 12px;
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
  padding: 6px 14px;
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

.route-count {
  font-size: 11px;
  opacity: 0.8;
}

.vod-episodes-section {
  padding: 16px 24px 24px;
  overflow-y: auto;
  max-height: 240px;
}

.episodes-header {
  margin-bottom: 12px;
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
.episode-btn:active {
  transform: translateY(0);
}
</style>
