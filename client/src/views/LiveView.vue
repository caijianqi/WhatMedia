<script setup lang="ts">
/**
 * 电视直播 (Live TV / IPTV)。
 *
 * 遵循与点播页的差异与工程约束：
 * 1. 采用 HLS 直播边缘流播放，支持起播重试与网络抖动自愈。
 * 2. 增强流可用性检测与容灾处理：播放失败时支持多线路无感秒切或一键重试。
 * 3. 离开页面或换台时显式释放服务端拉流转封装引用计数 (stop)。
 * 4. 纯粹沉浸大屏交互：订阅源增删改查统一在「设置 - 电视直播」中管理。
 */
import { computed, onBeforeUnmount, onMounted, ref, shallowRef } from 'vue'
import { RouterLink } from 'vue-router'
import Hls from 'hls.js'
import * as api from '../api/client'
import type { LiveChannel } from '../api/client'

const channels = ref<LiveChannel[]>([])
const loading = ref(true)
const error = ref<string | null>(null)

// ── 多线路聚合频道模型 ────────────────────────────────────
export interface ChannelRoute {
  channelId: string
  routeIndex: number // 1, 2, 3...
  name: string
}

export interface AggregatedChannel {
  key: string
  id: string
  name: string
  group: string
  logo_url?: string | null
  dvr_window_secs: number
  now_playing?: string | null
  viewers: number
  routes: ChannelRoute[]
}

const aggregatedChannels = computed<AggregatedChannel[]>(() => {
  const map = new Map<string, AggregatedChannel>()
  for (const c of channels.value) {
    const grp = c.group?.trim() || '未分类'
    const name = c.name.trim()
    const key = `${grp}__${name}`
    let item = map.get(key)
    if (!item) {
      item = {
        key,
        id: c.id,
        name,
        group: grp,
        logo_url: c.logo_url,
        dvr_window_secs: c.dvr_window_secs,
        now_playing: c.now_playing,
        viewers: c.viewers,
        routes: [],
      }
      map.set(key, item)
    }
    const routeIndex = item.routes.length + 1
    item.routes.push({
      channelId: c.id,
      routeIndex,
      name: `线路 ${routeIndex}`,
    })
  }
  return Array.from(map.values())
})

// ── 播放与容灾自愈状态机 ──────────────────────────────────
const currentChannel = ref<AggregatedChannel | null>(null)
const activeRouteIndex = ref(0)
const currentPlayingId = ref<string | null>(null)

// 兼容旧模板引用的 current (类型为 LiveChannel | null)
const current = computed<LiveChannel | null>(() => {
  if (!currentChannel.value) return null
  const route = currentChannel.value.routes[activeRouteIndex.value] || currentChannel.value.routes[0]
  const raw = channels.value.find((c) => c.id === route?.channelId)
  return raw || {
    id: currentChannel.value.id,
    name: currentChannel.value.name,
    group: currentChannel.value.group,
    logo_url: currentChannel.value.logo_url ?? null,
    dvr_window_secs: currentChannel.value.dvr_window_secs,
    now_playing: currentChannel.value.now_playing ?? null,
    viewers: currentChannel.value.viewers,
  }
})

const video = ref<HTMLVideoElement | null>(null)
const hls = shallowRef<Hls | null>(null)
const starting = ref(false)
const streamNotice = ref<string | null>(null)
const streamFailed = ref(false)
const retryCount = ref(0)

const searchQuery = ref('')

const groups = computed(() => {
  const set = new Set<string>()
  for (const c of aggregatedChannels.value) {
    if (c.group) set.add(c.group)
  }
  return ['全部', ...Array.from(set)]
})
const activeGroup = ref('全部')

const visibleChannels = computed(() => {
  let list = aggregatedChannels.value
  if (activeGroup.value !== '全部') {
    list = list.filter((c) => c.group === activeGroup.value)
  }
  const q = searchQuery.value.trim().toLowerCase()
  if (q) {
    list = list.filter(
      (c) =>
        c.name.toLowerCase().includes(q) ||
        (c.group && c.group.toLowerCase().includes(q)) ||
        (c.now_playing && c.now_playing.toLowerCase().includes(q)),
    )
  }
  return list
})

async function reload(): Promise<void> {
  loading.value = true
  error.value = null
  try {
    channels.value = await api.listLiveChannels()
    if (!currentChannel.value && aggregatedChannels.value.length > 0) {
      void playChannel(aggregatedChannels.value[0], 0)
    }
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

function teardown(): void {
  if (hls.value) {
    hls.value.destroy()
    hls.value = null
  }
  if (video.value) video.value.removeAttribute('src')
}

// ── 电视交互系统 (左侧边缘感应侧栏 & OSD & 全屏 & 遥控快捷键) ──
const playerContainer = ref<HTMLDivElement | null>(null)
const isDrawerOpen = ref(false)
const showOsd = ref(false)
const isFullscreen = ref(false)
const isPaused = ref(false)
const isMuted = ref(false)
const isControlsVisible = ref(false)

let osdTimer: ReturnType<typeof setTimeout> | null = null
let drawerTimer: ReturnType<typeof setTimeout> | null = null
let controlsTimer: ReturnType<typeof setTimeout> | null = null

function triggerOsd(): void {
  showOsd.value = true
  if (osdTimer) clearTimeout(osdTimer)
  osdTimer = setTimeout(() => {
    showOsd.value = false
  }, 3500)
}

function openDrawer(): void {
  if (drawerTimer) {
    clearTimeout(drawerTimer)
    drawerTimer = null
  }
  isDrawerOpen.value = true
}

function cancelDrawerTimer(): void {
  if (drawerTimer) {
    clearTimeout(drawerTimer)
    drawerTimer = null
  }
}

function scheduleDrawerClose(): void {
  if (drawerTimer) clearTimeout(drawerTimer)
  drawerTimer = setTimeout(() => {
    isDrawerOpen.value = false
  }, 400)
}

function toggleDrawer(): void {
  if (isDrawerOpen.value) {
    isDrawerOpen.value = false
  } else {
    openDrawer()
  }
}

function handleMouseMove(): void {
  isControlsVisible.value = true
  if (controlsTimer) clearTimeout(controlsTimer)
  controlsTimer = setTimeout(() => {
    isControlsVisible.value = false
  }, 3000)
}

function handleMouseLeave(): void {
  isControlsVisible.value = false
}

function togglePlay(): void {
  if (!video.value) return
  if (video.value.paused) {
    video.value.play().catch(() => undefined)
    isPaused.value = false
  } else {
    video.value.pause()
    isPaused.value = true
  }
}

function toggleMute(): void {
  if (!video.value) return
  video.value.muted = !video.value.muted
  isMuted.value = video.value.muted
}

function toggleFullscreen(): void {
  const el = playerContainer.value
  if (!el) return
  if (!document.fullscreenElement) {
    el.requestFullscreen().catch(() => undefined)
  } else {
    document.exitFullscreen().catch(() => undefined)
  }
}

async function togglePip(): Promise<void> {
  const el = video.value
  if (!el) return
  if (document.pictureInPictureElement) {
    await document.exitPictureInPicture().catch(() => undefined)
  } else if (document.pictureInPictureEnabled) {
    await el.requestPictureInPicture().catch(() => undefined)
  }
}

function handleKeyDown(e: KeyboardEvent): void {
  const target = e.target as HTMLElement
  if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) {
    if (e.key === 'Escape') target.blur()
    return
  }

  switch (e.key) {
    case 'ArrowUp':
      e.preventDefault()
      playPrev()
      break
    case 'ArrowDown':
      e.preventDefault()
      playNext()
      break
    case 'ArrowLeft':
      e.preventDefault()
      if (!isDrawerOpen.value) {
        openDrawer()
      } else if (currentChannel.value && currentChannel.value.routes.length > 1) {
        const prevIdx =
          (activeRouteIndex.value - 1 + currentChannel.value.routes.length) %
          currentChannel.value.routes.length
        void switchRoute(prevIdx)
      }
      break
    case 'ArrowRight':
      e.preventDefault()
      if (isDrawerOpen.value) {
        isDrawerOpen.value = false
      } else if (currentChannel.value && currentChannel.value.routes.length > 1) {
        const nextIdx = (activeRouteIndex.value + 1) % currentChannel.value.routes.length
        void switchRoute(nextIdx)
      }
      break
    case 'm':
    case 'M':
      toggleMute()
      break
    case 'f':
    case 'F':
      toggleFullscreen()
      break
    case 'Enter':
    case ' ':
      e.preventDefault()
      toggleDrawer()
      break
    case 'Escape':
      if (isDrawerOpen.value) {
        isDrawerOpen.value = false
      }
      break
  }
}

function onFullscreenChange(): void {
  isFullscreen.value = !!document.fullscreenElement
}

/** 播放聚合频道（支持指定备用线路） */
async function playChannel(ch: AggregatedChannel, routeIndex = 0): Promise<void> {
  // 换台或换线路：释放上一个频道的引用
  if (currentPlayingId.value) {
    teardown()
    try {
      await api.stopLiveChannel(currentPlayingId.value)
    } catch {
      // 忽略停止异常
    }
  }
  starting.value = true
  error.value = null
  streamNotice.value = null
  streamFailed.value = false
  retryCount.value = 0

  currentChannel.value = ch
  activeRouteIndex.value = Math.max(0, Math.min(routeIndex, ch.routes.length - 1))
  const route = ch.routes[activeRouteIndex.value] || ch.routes[0]
  if (!route) {
    starting.value = false
    error.value = '该频道没有可用的播放线路'
    return
  }
  currentPlayingId.value = route.channelId

  try {
    const p = await api.playLiveChannel(route.channelId)
    await new Promise((r) => requestAnimationFrame(r))
    const el = video.value
    if (!el) return

    if (Hls.isSupported()) {
      const h = new Hls({
        liveSyncDurationCount: 3,
        manifestLoadingRetryDelay: 600,
        manifestLoadingMaxRetry: 4,
        levelLoadingMaxRetry: 4,
      })
      h.loadSource(p.playlist_url)
      h.attachMedia(el)
      h.on(Hls.Events.ERROR, (_e, data) => {
        if (data.fatal) {
          switch (data.type) {
            case Hls.ErrorTypes.NETWORK_ERROR:
              // 当前线路网络波动：优先尝试本地自愈
              if (retryCount.value < 2) {
                retryCount.value++
                streamNotice.value = `正在重新连接线路 ${activeRouteIndex.value + 1} (${retryCount.value}/2)…`
                h.startLoad()
              } else if (currentChannel.value && activeRouteIndex.value + 1 < currentChannel.value.routes.length) {
                // 线路不可达！自动秒切下一条备用线路
                const nextIdx = activeRouteIndex.value + 1
                const nextRoute = currentChannel.value.routes[nextIdx]
                streamNotice.value = `线路 ${activeRouteIndex.value + 1} 信号受阻，已自动为您切至线路 ${nextRoute.routeIndex} (${nextRoute.routeIndex}/${currentChannel.value.routes.length})…`
                void playChannel(currentChannel.value, nextIdx)
              } else {
                streamFailed.value = true
                error.value = '当前频道所有备用线路均无法连接，建议切换频道或刷新订阅'
                h.destroy()
              }
              break
            case Hls.ErrorTypes.MEDIA_ERROR:
              h.recoverMediaError()
              break
            default:
              if (currentChannel.value && activeRouteIndex.value + 1 < currentChannel.value.routes.length) {
                const nextIdx = activeRouteIndex.value + 1
                const nextRoute = currentChannel.value.routes[nextIdx]
                streamNotice.value = `播放异常，正在自动切至备用线路 ${nextRoute.routeIndex}…`
                void playChannel(currentChannel.value, nextIdx)
              } else {
                streamFailed.value = true
                error.value = '播放遇到不可恢复的异常，当前源可能已下线'
                h.destroy()
              }
              break
          }
        }
      })
      hls.value = h
    } else if (el.canPlayType('application/vnd.apple.mpegurl')) {
      el.src = p.playlist_url
    } else {
      error.value = '当前浏览器不支持 HLS 直播协议'
      return
    }
    await el.play().catch(() => undefined)
    triggerOsd()
  } catch (e) {
    // 若拉流 API 报错，尝试自动切下一条线路
    if (currentChannel.value && activeRouteIndex.value + 1 < currentChannel.value.routes.length) {
      const nextIdx = activeRouteIndex.value + 1
      streamNotice.value = `线路 ${activeRouteIndex.value + 1} 启动失败，正在为您切换备用线路 ${nextIdx + 1}…`
      void playChannel(currentChannel.value, nextIdx)
    } else {
      streamFailed.value = true
      error.value = e instanceof Error ? e.message : String(e)
    }
  } finally {
    starting.value = false
  }
}

/** 手动切换线路 */
async function switchRoute(index: number): Promise<void> {
  if (!currentChannel.value) return
  if (index === activeRouteIndex.value) return
  if (index < 0 || index >= currentChannel.value.routes.length) return
  await playChannel(currentChannel.value, index)
}

async function stopCurrent(): Promise<void> {
  if (currentPlayingId.value) {
    try {
      await api.stopLiveChannel(currentPlayingId.value)
    } catch {
      // 忽略停止异常
    }
  }
  teardown()
  currentChannel.value = null
  currentPlayingId.value = null
  streamFailed.value = false
  streamNotice.value = null
}

function playNext(): void {
  if (!currentChannel.value || visibleChannels.value.length === 0) return
  const idx = visibleChannels.value.findIndex((c) => c.key === currentChannel.value?.key)
  const nextIdx = (idx + 1) % visibleChannels.value.length
  const next = visibleChannels.value[nextIdx]
  if (next) void playChannel(next, 0)
}

function playPrev(): void {
  if (!currentChannel.value || visibleChannels.value.length === 0) return
  const idx = visibleChannels.value.findIndex((c) => c.key === currentChannel.value?.key)
  const prevIdx = (idx - 1 + visibleChannels.value.length) % visibleChannels.value.length
  const prev = visibleChannels.value[prevIdx]
  if (prev) void playChannel(prev, 0)
}

onMounted(() => {
  void reload()
  window.addEventListener('keydown', handleKeyDown)
  document.addEventListener('fullscreenchange', onFullscreenChange)
})

onBeforeUnmount(async () => {
  teardown()
  window.removeEventListener('keydown', handleKeyDown)
  document.removeEventListener('fullscreenchange', onFullscreenChange)
  if (osdTimer) clearTimeout(osdTimer)
  if (currentPlayingId.value) {
    try {
      await api.stopLiveChannel(currentPlayingId.value)
    } catch {
      // 忽略停止异常
    }
  }
})
</script>

<template>
  <div
    ref="playerContainer"
    class="tv-cinema-container"
    :class="{ 'is-fullscreen': isFullscreen }"
    @mousemove="handleMouseMove"
    @mouseleave="handleMouseLeave"
  >
    <!-- 1. 左侧边缘感应触发区域 (透明感应区，鼠标移入滑出选台栏) -->
    <div
      class="tv-edge-sensor"
      title="移动到此处查看频道列表"
      @mouseenter="openDrawer"
    />

    <!-- 2. 左侧悬浮滑出频道选台侧栏 (Channel Drawer) -->
    <aside
      class="tv-channel-drawer"
      :class="{ 'is-open': isDrawerOpen }"
      data-test="channel-drawer"
      @mouseenter="cancelDrawerTimer"
      @mouseleave="scheduleDrawerClose"
    >
      <div class="drawer-header">
        <div class="drawer-title-row">
          <div class="drawer-title">📺 频道列表</div>
          <button type="button" class="drawer-close-btn" @click="isDrawerOpen = false">✕</button>
        </div>
        <!-- 搜索框 -->
        <div class="drawer-search-wrap">
          <input
            v-model="searchQuery"
            class="drawer-search-input"
            placeholder="快速搜索频道…"
          />
        </div>
        <!-- 分类药丸横向滚动 -->
        <div v-if="groups.length > 1" class="drawer-groups-bar">
          <button
            v-for="g in groups"
            :key="g"
            type="button"
            class="drawer-group-chip"
            :class="{ active: activeGroup === g }"
            @click="activeGroup = g"
          >
            {{ g }}
          </button>
        </div>
      </div>

      <!-- 频道纵向列表 -->
      <div class="drawer-channels-list">
        <div v-if="!visibleChannels.length" class="drawer-empty-hint">
          无匹配频道
        </div>
        <button
          v-for="(c, idx) in visibleChannels"
          :key="c.key"
          type="button"
          class="drawer-channel-btn"
          :class="{ active: currentChannel?.key === c.key }"
          data-test="channel"
          @click="playChannel(c, 0); isDrawerOpen = false"
        >
          <span class="drawer-ch-index">{{ String(idx + 1).padStart(2, '0') }}</span>
          <div class="drawer-ch-info">
            <span class="drawer-ch-title">{{ c.name }}</span>
            <span v-if="c.now_playing" class="drawer-ch-now-playing">{{ c.now_playing }}</span>
          </div>
          <span v-if="c.routes.length > 1" class="drawer-ch-routes">{{ c.routes.length }} 线路</span>
          <span v-if="currentChannel?.key === c.key" class="drawer-playing-tag">● 播中</span>
        </button>
      </div>

      <div class="drawer-footer">
        <span>💡 快捷键：<strong>↑/↓</strong> 换台 · <strong>←/→</strong> 线路 · <strong>F</strong> 全屏</span>
      </div>
    </aside>

    <!-- 3. 未配置直播源时的居中空状态 -->
    <div
      v-if="!loading && !channels.length"
      class="tv-empty-fullscreen"
      data-test="empty"
    >
      <div class="tv-empty-content">
        <div class="empty-icon">📺</div>
        <h2>尚未配置任何电视频道</h2>
        <p>电视直播订阅源已统一在系统设置中管理。请前往设置添加 M3U 播放列表或 TXT 频道列表。</p>
        <RouterLink
          to="/settings?tab=live"
          class="btn-go-settings"
          data-test="link-settings"
        >
          ⚙️ 前往「设置 - 电视直播」配置订阅源
        </RouterLink>
      </div>
    </div>

    <!-- 4. 电视主播放画幅 -->
    <main
      v-else
      class="tv-screen-stage"
      data-test="player"
      @click="togglePlay"
    >
      <!-- 纯净视频元素：无 controls，无原生进度条 (Seekbar) -->
      <video
        ref="video"
        class="tv-video-el"
        playsinline
        autoplay
        @play="isPaused = false"
        @pause="isPaused = true"
      />

      <!-- 换台电视 OSD 半透明横幅（3.5 秒自动淡出） -->
      <transition name="osd-fade">
        <div v-if="showOsd && currentChannel" class="tv-osd-banner" @click.stop>
          <div class="osd-left">
            <div class="osd-badge">TV</div>
            <div class="osd-content">
              <div class="osd-title-row">
                <span class="osd-name">{{ currentChannel.name }}</span>
                <span v-if="currentChannel.group" class="osd-group-pill">{{ currentChannel.group }}</span>
              </div>
              <div class="osd-meta-row">
                <span class="osd-route-label">
                  线路 {{ activeRouteIndex + 1 }} / {{ currentChannel.routes.length }}
                </span>
                <span v-if="current?.now_playing" class="osd-now-playing">
                  正在播出：{{ current.now_playing }}
                </span>
              </div>
            </div>
          </div>
          <div class="osd-right">
            <span class="osd-shortcut-hint">快捷操作：↑/↓ 换台 · ← 频道列表 · 鼠标靠左滑出节目单</span>
          </div>
        </div>
      </transition>

      <!-- 起播中加载浮层 -->
      <div v-if="starting" class="player-overlay loading-overlay">
        <div class="spinner" />
        <span>正在连接信号流并转封装…</span>
      </div>

      <!-- 播放中断或信号受阻容灾卡片 -->
      <div v-if="streamFailed" class="player-overlay error-overlay" @click.stop>
        <div class="fail-icon">⚠️</div>
        <div class="fail-title">直播信号连接受阻</div>
        <p class="fail-desc">
          {{ error || '当前源站响应超时或未能拉取到直播流分片。' }}
        </p>
        <div class="fail-actions">
          <button type="button" class="btn-fail-action" @click="() => currentChannel && playChannel(currentChannel, activeRouteIndex)">
            🔄 重试当前线路
          </button>
          <button type="button" class="btn-fail-action primary" @click="playNext">
            ⏭️ 播放下一频道
          </button>
        </div>
      </div>

      <!-- 5. 悬浮极简无进度条控制浮层（鼠标移动显示，静止自动隐藏，无 Seekbar） -->
      <div
        class="tv-bottom-bar"
        :class="{ 'bar-visible': isControlsVisible || isPaused }"
        @click.stop
      >
        <div class="bar-left">
          <button
            type="button"
            class="tv-btn-action"
            title="播放 / 暂停 (空格)"
            @click="togglePlay"
          >
            {{ isPaused ? '▶ 播放' : '⏸ 暂停' }}
          </button>
          <button
            type="button"
            class="tv-btn-action"
            title="上一台 (↑)"
            @click="playPrev"
          >
            ⏮️ 上一台
          </button>
          <button
            type="button"
            class="tv-btn-action"
            title="下一台 (↓)"
            @click="playNext"
          >
            ⏭️ 下一台
          </button>
          <button
            type="button"
            class="tv-btn-action stop"
            title="停止播放 (关闭流)"
            data-test="stop"
            @click="stopCurrent"
          >
            ⏹ 停止
          </button>
          <span class="tv-bar-channel-name">{{ currentChannel?.name }}</span>
          <span v-if="current?.now_playing" class="tv-bar-program">{{ current.now_playing }}</span>
        </div>

        <div class="bar-right">
          <!-- 线路切换 -->
          <div v-if="currentChannel && currentChannel.routes.length > 1" class="tv-route-select-wrap">
            <select
              :value="activeRouteIndex"
              class="tv-route-select"
              @change="switchRoute(Number(($event.target as HTMLSelectElement).value))"
            >
              <option
                v-for="(r, idx) in currentChannel.routes"
                :key="r.channelId"
                :value="idx"
              >
                {{ r.name }}
              </option>
            </select>
          </div>

          <!-- 静音控制 -->
          <button
            type="button"
            class="tv-icon-btn"
            :title="isMuted ? '取消静音 (M)' : '静音 (M)'"
            @click="toggleMute"
          >
            {{ isMuted ? '🔇' : '🔊' }}
          </button>

          <!-- 画中画 -->
          <button
            type="button"
            class="tv-icon-btn"
            title="画中画"
            @click="togglePip"
          >
            ⛶
          </button>

          <!-- 全屏 -->
          <button
            type="button"
            class="tv-icon-btn"
            title="全屏 (F)"
            @click="toggleFullscreen"
          >
            {{ isFullscreen ? '⤓' : '⤢' }}
          </button>

          <!-- 直达设置 -->
          <RouterLink
            to="/settings?tab=live"
            class="tv-icon-btn"
            title="配置直播源"
            data-test="link-settings"
          >
            ⚙️
          </RouterLink>
        </div>
      </div>
    </main>
  </div>
</template>

<style scoped>
/* ── 电视纯净全视口容器 ────────────────────────────────── */
.tv-cinema-container {
  position: relative;
  width: 100%;
  height: calc(100vh - 80px);
  min-height: 520px;
  background: #000;
  border-radius: 14px;
  overflow: hidden;
  box-shadow: 0 16px 50px rgba(0, 0, 0, 0.7);
  user-select: none;
}

.tv-cinema-container.is-fullscreen {
  height: 100vh;
  border-radius: 0;
  box-shadow: none;
}

/* ── 1. 左侧边缘感应触发区 ──────────────────────────────── */
.tv-edge-sensor {
  position: absolute;
  top: 0;
  left: 0;
  bottom: 0;
  width: 60px;
  z-index: 45;
  cursor: pointer;
  background: transparent;
}

/* ── 2. 左侧滑出的选台侧栏 ──────────────────────────────── */
.tv-channel-drawer {
  position: absolute;
  top: 0;
  left: 0;
  bottom: 0;
  width: 330px;
  max-width: 85vw;
  background: rgba(11, 17, 32, 0.94);
  backdrop-filter: blur(24px);
  border-right: 1px solid rgba(255, 255, 255, 0.1);
  box-shadow: 12px 0 40px rgba(0, 0, 0, 0.65);
  display: flex;
  flex-direction: column;
  z-index: 50;
  transform: translateX(-100%);
  transition: transform 0.28s cubic-bezier(0.16, 1, 0.3, 1);
}

.tv-channel-drawer.is-open {
  transform: translateX(0);
}

.drawer-header {
  padding: 16px 18px 12px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.drawer-title-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.drawer-title {
  font-size: 15px;
  font-weight: 700;
  color: #fff;
  letter-spacing: 0.5px;
}

.drawer-close-btn {
  background: transparent;
  border: none;
  color: var(--muted);
  font-size: 16px;
  cursor: pointer;
  padding: 2px 6px;
  border-radius: 4px;
}

.drawer-close-btn:hover {
  color: #fff;
  background: rgba(255, 255, 255, 0.1);
}

.drawer-search-wrap {
  position: relative;
}

.drawer-search-input {
  width: 100%;
  padding: 7px 12px;
  border-radius: 8px;
  border: 1px solid rgba(255, 255, 255, 0.12);
  background: rgba(0, 0, 0, 0.4);
  color: #fff;
  font-size: 12px;
  outline: none;
  transition: border-color 0.18s ease;
}

.drawer-search-input:focus {
  border-color: var(--cyan);
}

.drawer-groups-bar {
  display: flex;
  gap: 6px;
  overflow-x: auto;
  padding-bottom: 2px;
  scrollbar-width: none;
}

.drawer-groups-bar::-webkit-scrollbar {
  display: none;
}

.drawer-group-chip {
  padding: 3px 10px;
  border-radius: 999px;
  border: 1px solid rgba(255, 255, 255, 0.1);
  background: rgba(255, 255, 255, 0.06);
  color: var(--muted);
  font-size: 11px;
  white-space: nowrap;
  cursor: pointer;
  transition: all 0.15s ease;
}

.drawer-group-chip:hover {
  color: #fff;
  background: rgba(255, 255, 255, 0.12);
}

.drawer-group-chip.active {
  background: var(--cyan);
  border-color: var(--cyan);
  color: #0c1322;
  font-weight: 700;
}

.drawer-channels-list {
  flex: 1;
  overflow-y: auto;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.drawer-channel-btn {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 12px;
  border-radius: 8px;
  border: 1px solid transparent;
  background: transparent;
  color: #e2e8f0;
  text-align: left;
  cursor: pointer;
  transition: all 0.15s ease;
}

.drawer-channel-btn:hover {
  background: rgba(255, 255, 255, 0.08);
  border-color: rgba(255, 255, 255, 0.15);
}

.drawer-channel-btn.active {
  background: linear-gradient(90deg, rgba(87, 200, 245, 0.22), rgba(121, 91, 255, 0.18));
  border-color: rgba(87, 200, 245, 0.45);
  color: #fff;
}

.drawer-ch-index {
  font-family: var(--mono);
  font-size: 11px;
  color: var(--muted);
  font-weight: 600;
  min-width: 20px;
}

.drawer-ch-info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.drawer-ch-title {
  font-size: 13.5px;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.drawer-ch-now-playing {
  font-size: 11px;
  color: var(--muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.drawer-ch-routes {
  font-size: 10px;
  padding: 1px 5px;
  border-radius: 4px;
  background: rgba(121, 91, 255, 0.25);
  color: #c4b5fd;
  white-space: nowrap;
}

.drawer-playing-tag {
  font-size: 11px;
  color: var(--cyan);
  font-weight: 700;
  white-space: nowrap;
}

.drawer-empty-hint {
  padding: 24px;
  text-align: center;
  color: var(--muted);
  font-size: 12px;
}

.drawer-footer {
  padding: 10px 14px;
  border-top: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(0, 0, 0, 0.3);
  font-size: 11px;
  color: var(--muted);
}

.drawer-footer strong {
  color: var(--cyan);
}

/* ── 3. 空状态 ─────────────────────────────────────────── */
.tv-empty-fullscreen {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #090e1a;
  z-index: 10;
}

.tv-empty-content {
  text-align: center;
  max-width: 440px;
  padding: 32px 24px;
}

.empty-icon {
  font-size: 48px;
  margin-bottom: 12px;
}

.tv-empty-content h2 {
  font-size: 20px;
  font-weight: 700;
  color: #fff;
  margin: 0 0 8px;
}

.tv-empty-content p {
  font-size: 13px;
  color: var(--muted);
  line-height: 1.5;
  margin: 0 0 20px;
}

.btn-go-settings {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 9px 20px;
  background: linear-gradient(135deg, var(--accent), var(--cyan));
  color: #fff;
  border-radius: 8px;
  text-decoration: none;
  font-size: 13px;
  font-weight: 600;
  transition: opacity 0.15s ease;
}

.btn-go-settings:hover {
  opacity: 0.9;
}

/* ── 4. 电视主播放视口 ─────────────────────────────────── */
.tv-screen-stage {
  position: relative;
  width: 100%;
  height: 100%;
  background: #000;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: default;
}

.tv-video-el {
  width: 100%;
  height: 100%;
  object-fit: contain;
  background: #000;
}

/* 换台电视 OSD 半透明横幅 */
.tv-osd-banner {
  position: absolute;
  top: 24px;
  left: 28px;
  right: 28px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 20px;
  background: rgba(10, 16, 28, 0.85);
  backdrop-filter: blur(16px);
  border: 1px solid rgba(255, 255, 255, 0.15);
  border-radius: 12px;
  box-shadow: 0 8px 30px rgba(0, 0, 0, 0.6);
  z-index: 30;
  pointer-events: none;
}

.osd-left {
  display: flex;
  align-items: center;
  gap: 14px;
}

.osd-badge {
  font-size: 13px;
  font-weight: 800;
  letter-spacing: 1px;
  padding: 5px 9px;
  border-radius: 6px;
  background: var(--cyan);
  color: #0c1322;
}

.osd-content {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.osd-title-row {
  display: flex;
  align-items: center;
  gap: 10px;
}

.osd-name {
  font-size: 18px;
  font-weight: 800;
  color: #fff;
  letter-spacing: -0.3px;
}

.osd-group-pill {
  font-size: 11px;
  padding: 2px 7px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.12);
  color: #e2e8f0;
}

.osd-meta-row {
  display: flex;
  align-items: center;
  gap: 12px;
  font-size: 12px;
}

.osd-route-label {
  color: var(--cyan);
  font-weight: 600;
}

.osd-now-playing {
  color: var(--muted);
}

.osd-right {
  display: flex;
  align-items: center;
}

.osd-shortcut-hint {
  font-size: 11.5px;
  color: var(--muted);
}

.osd-fade-enter-active,
.osd-fade-leave-active {
  transition: all 0.35s ease;
}

.osd-fade-enter-from,
.osd-fade-leave-to {
  opacity: 0;
  transform: translateY(-12px);
}

/* 浮层态 */
.player-overlay {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  background: rgba(8, 13, 23, 0.88);
  backdrop-filter: blur(10px);
  padding: 24px;
  text-align: center;
  z-index: 25;
}

.spinner {
  width: 40px;
  height: 40px;
  border: 3px solid rgba(87, 200, 245, 0.2);
  border-top-color: var(--cyan);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.fail-icon {
  font-size: 36px;
}

.fail-title {
  font-size: 17px;
  font-weight: 700;
  color: #fff;
}

.fail-desc {
  font-size: 12.5px;
  color: var(--muted);
  max-width: 420px;
  line-height: 1.5;
  margin: 0;
}

.fail-actions {
  display: flex;
  gap: 10px;
  margin-top: 10px;
}

.btn-fail-action {
  padding: 7px 16px;
  border-radius: 8px;
  border: 1px solid rgba(255, 255, 255, 0.15);
  background: rgba(255, 255, 255, 0.1);
  color: #fff;
  font-size: 12.5px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.btn-fail-action.primary {
  background: var(--cyan);
  color: #0c1322;
  border-color: var(--cyan);
  font-weight: 700;
}

/* ── 5. 悬浮极简无进度条控制条 ──────────────────────────── */
.tv-bottom-bar {
  position: absolute;
  left: 20px;
  right: 20px;
  bottom: 20px;
  padding: 10px 16px;
  background: rgba(10, 16, 28, 0.85);
  backdrop-filter: blur(20px);
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 12px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  z-index: 35;
  opacity: 0;
  transform: translateY(12px);
  pointer-events: none;
  transition: all 0.25s ease;
}

.tv-bottom-bar.bar-visible {
  opacity: 1;
  transform: translateY(0);
  pointer-events: auto;
}

.bar-left {
  display: flex;
  align-items: center;
  gap: 10px;
}

.tv-btn-action {
  padding: 6px 12px;
  border-radius: 7px;
  border: 1px solid rgba(255, 255, 255, 0.12);
  background: rgba(255, 255, 255, 0.08);
  color: #fff;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
}

.tv-btn-action:hover {
  background: rgba(255, 255, 255, 0.16);
  border-color: var(--cyan);
}

.tv-bar-channel-name {
  font-size: 14px;
  font-weight: 700;
  color: #fff;
  margin-left: 4px;
}

.tv-bar-program {
  font-size: 12px;
  color: var(--muted);
  max-width: 240px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.bar-right {
  display: flex;
  align-items: center;
  gap: 8px;
}

.tv-route-select-wrap {
  position: relative;
}

.tv-route-select {
  padding: 5px 8px;
  border-radius: 6px;
  border: 1px solid rgba(255, 255, 255, 0.15);
  background: rgba(0, 0, 0, 0.5);
  color: var(--cyan);
  font-size: 11.5px;
  font-weight: 600;
  outline: none;
  cursor: pointer;
}

.tv-icon-btn {
  width: 32px;
  height: 32px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 7px;
  border: 1px solid rgba(255, 255, 255, 0.12);
  background: rgba(255, 255, 255, 0.08);
  color: #fff;
  font-size: 13px;
  cursor: pointer;
  text-decoration: none;
  transition: all 0.15s ease;
}

.tv-icon-btn:hover {
  background: rgba(255, 255, 255, 0.18);
  border-color: var(--cyan);
}
</style>
