<script setup lang="ts">
/**
 * 电视直播 (Live TV / IPTV)。
 *
 * 视觉与交互规范全面对齐点播播放器 (PlayerView.vue)：
 * 1. 采用全屏沉浸画幅 (.player-shell)，深邃纯黑画幅底色与无缝自适应视口。
 * 2. 顶部自隐式导航条 (.player-topbar)：返回按钮、选台栏快捷唤醒、频道名称与线路徽章。
 * 3. 中央状态层对齐：暂停时居中发光大播放圆钮 (.player-center-play + .big-play)、缓冲中极光旋转圆环 (.player-buffering-layer)、网络重连气泡 (.player-recovering-hint)、统一错误卡片 (.player-error-layer)。
 * 4. 底部暗夜渐变悬浮控制台 (.player-bottom-panel)：矢量 SVG 按钮 (.ctl-btn)、音量滑块、线路切换下拉 (.ctl-select)、画中画与全屏，无时间轴/进度条 (No seekbar)。
 * 5. 2.5 秒指针静止自动隐藏机制 (armHideTimer)。
 * 6. 左侧边缘感应自动滑出选台抽屉 (.tv-edge-sensor + .tv-channel-drawer)，选台即切。
 */
import { computed, onBeforeUnmount, onMounted, ref, shallowRef } from 'vue'
import { RouterLink, useRouter } from 'vue-router'
import Hls from 'hls.js'
import * as api from '../api/client'
import type { LiveChannel } from '../api/client'

const router = useRouter()

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

const shell = ref<HTMLElement | null>(null)
const video = ref<HTMLVideoElement | null>(null)
const hls = shallowRef<Hls | null>(null)
const starting = ref(false)
const streamNotice = ref<string | null>(null)
const streamFailed = ref(false)
const retryCount = ref(0)
const isDirectStream = ref(false)

export interface ChannelGroupNode {
  name: string
  channels: AggregatedChannel[]
}

const searchQuery = ref('')
const expandedGroups = ref<Set<string>>(new Set())

// ── 频道收藏系统 ──────────────────────────────────────────
const FAVORITES_STORAGE_KEY = 'whatmedia_live_favorites'
const LAST_CHANNEL_STORAGE_KEY = 'whatmedia_last_live_channel_key'

const favoriteKeys = ref<Set<string>>(new Set())

function loadFavorites(): void {
  try {
    const raw = localStorage.getItem(FAVORITES_STORAGE_KEY)
    if (raw) {
      const arr = JSON.parse(raw)
      if (Array.isArray(arr)) {
        favoriteKeys.value = new Set(arr)
      }
    }
  } catch {
    // 忽略 localStorage 错误
  }
}

function saveFavorites(): void {
  try {
    localStorage.setItem(
      FAVORITES_STORAGE_KEY,
      JSON.stringify(Array.from(favoriteKeys.value)),
    )
  } catch {
    // 忽略
  }
}

function isFavorite(key: string): boolean {
  return favoriteKeys.value.has(key)
}

function toggleFavorite(ch: AggregatedChannel, e?: Event): void {
  if (e) e.stopPropagation()
  const next = new Set(favoriteKeys.value)
  if (next.has(ch.key)) {
    next.delete(ch.key)
  } else {
    next.add(ch.key)
  }
  favoriteKeys.value = next
  saveFavorites()
}

const groupedChannels = computed<ChannelGroupNode[]>(() => {
  const map = new Map<string, AggregatedChannel[]>()
  const q = searchQuery.value.trim().toLowerCase()
  const favChannels: AggregatedChannel[] = []

  for (const c of aggregatedChannels.value) {
    if (isFavorite(c.key)) {
      favChannels.push(c)
    }
    if (q) {
      const match =
        c.name.toLowerCase().includes(q) ||
        (c.group && c.group.toLowerCase().includes(q)) ||
        (c.now_playing && c.now_playing.toLowerCase().includes(q))
      if (!match) continue
    }
    const grp = c.group?.trim() || '未分类'
    let list = map.get(grp)
    if (!list) {
      list = []
      map.set(grp, list)
    }
    list.push(c)
  }

  const res: ChannelGroupNode[] = []

  // 1. 若有收藏的频道，将「⭐ 我的收藏」分组置于最顶端
  if (favChannels.length > 0) {
    const filteredFavs = q
      ? favChannels.filter(
          (c) =>
            c.name.toLowerCase().includes(q) ||
            (c.group && c.group.toLowerCase().includes(q)) ||
            (c.now_playing && c.now_playing.toLowerCase().includes(q)),
        )
      : favChannels
    if (filteredFavs.length > 0) {
      res.push({
        name: '⭐ 我的收藏',
        channels: filteredFavs,
      })
    }
  }

  // 2. 常规频道分组
  for (const [name, list] of map.entries()) {
    res.push({ name, channels: list })
  }
  return res
})

// 平铺列表供上一台/下一台或全局索引使用
const visibleChannels = computed(() => {
  const q = searchQuery.value.trim().toLowerCase()
  if (!q) return aggregatedChannels.value
  return aggregatedChannels.value.filter(
    (c) =>
      c.name.toLowerCase().includes(q) ||
      (c.group && c.group.toLowerCase().includes(q)) ||
      (c.now_playing && c.now_playing.toLowerCase().includes(q)),
  )
})

function toggleGroup(name: string): void {
  const next = new Set(expandedGroups.value)
  if (next.has(name)) {
    next.delete(name)
  } else {
    next.add(name)
  }
  expandedGroups.value = next
}

function expandAll(): void {
  expandedGroups.value = new Set(groupedChannels.value.map((g) => g.name))
}

function collapseAll(): void {
  expandedGroups.value = new Set()
}

function isGroupExpanded(name: string): boolean {
  if (searchQuery.value.trim()) return true
  return expandedGroups.value.has(name)
}

async function reload(): Promise<void> {
  loading.value = true
  error.value = null
  try {
    channels.value = await api.listLiveChannels()
    loadFavorites()

    if (aggregatedChannels.value.length > 0) {
      // 需求 3：记录并恢复上次播放频道
      let target = aggregatedChannels.value[0]
      let savedKey: string | null = null
      try {
        savedKey = localStorage.getItem(LAST_CHANNEL_STORAGE_KEY)
      } catch {
        // 忽略
      }
      if (savedKey) {
        const found = aggregatedChannels.value.find((c) => c.key === savedKey)
        if (found) {
          target = found
        }
      }

      // 需求 1：进入电视直播后，默认除播放的那个频道所在分组展开，其他全部折叠
      const targetGroup = target.group || '未分类'
      expandedGroups.value = new Set([targetGroup])

      if (!currentChannel.value || currentChannel.value.key !== target.key) {
        void playChannel(target, 0)
      }
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

// ── 播放器显隐与控制系统 (对齐 PlayerView) ─────────────────
const controlsVisible = ref(true)
let hideTimer: number | undefined
const isFullscreen = ref(false)
const isPaused = ref(false)
const isMuted = ref(false)
const volume = ref(1)

const isDrawerOpen = ref(false)
const showOsd = ref(false)
let osdTimer: ReturnType<typeof setTimeout> | null = null
let drawerTimer: ReturnType<typeof setTimeout> | null = null

function armHideTimer(): void {
  if (hideTimer) window.clearTimeout(hideTimer)
  controlsVisible.value = true
  hideTimer = window.setTimeout(() => {
    if (!isPaused.value) {
      controlsVisible.value = false
    }
  }, 2500)
}

function onPointerMove(): void {
  armHideTimer()
}

function onPointerLeave(): void {
  if (!isPaused.value) {
    controlsVisible.value = false
  }
}

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

function goBack(): void {
  if (router && typeof router.back === 'function') {
    if (window.history.length > 1) {
      router.back()
    } else {
      void router.push('/')
    }
  } else {
    window.history.back()
  }
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
  armHideTimer()
}

function onVolumeInput(e: Event): void {
  const v = Number((e.target as HTMLInputElement).value)
  volume.value = v
  if (video.value) {
    video.value.volume = v
    video.value.muted = v === 0
    isMuted.value = v === 0
  }
  armHideTimer()
}

function toggleMute(): void {
  if (!video.value) return
  video.value.muted = !video.value.muted
  isMuted.value = video.value.muted
  if (!video.value.muted && volume.value === 0) {
    volume.value = 0.5
    video.value.volume = 0.5
  }
  armHideTimer()
}

function toggleFullscreen(): void {
  const el = shell.value
  if (!el) return
  if (!document.fullscreenElement) {
    el.requestFullscreen().catch(() => undefined)
  } else {
    document.exitFullscreen().catch(() => undefined)
  }
}

function onFullscreenChange(): void {
  isFullscreen.value = !!document.fullscreenElement
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
  if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.tagName === 'SELECT')) {
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
      e.preventDefault()
      toggleDrawer()
      break
    case ' ':
      e.preventDefault()
      togglePlay()
      break
    case 'Escape':
      if (isDrawerOpen.value) {
        isDrawerOpen.value = false
      }
      break
  }
  armHideTimer()
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
  try {
    localStorage.setItem(LAST_CHANNEL_STORAGE_KEY, ch.key)
  } catch {
    // 忽略
  }
  if (ch.group) {
    expandedGroups.value = new Set([...expandedGroups.value, ch.group])
  }
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
    isDirectStream.value = !!(p as any).direct
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
    isPaused.value = false
    triggerOsd()
  } catch (e) {
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
    armHideTimer()
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
  armHideTimer()
})

onBeforeUnmount(async () => {
  teardown()
  window.removeEventListener('keydown', handleKeyDown)
  document.removeEventListener('fullscreenchange', onFullscreenChange)
  if (hideTimer) window.clearTimeout(hideTimer)
  if (osdTimer) clearTimeout(osdTimer)
  if (drawerTimer) clearTimeout(drawerTimer)
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
    ref="shell"
    class="player-shell live-player-shell"
    :class="{ 'is-fullscreen': isFullscreen }"
    @pointermove="onPointerMove"
    @pointerleave="onPointerLeave"
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
          <button type="button" class="drawer-close-btn" title="关闭选台列表" @click="isDrawerOpen = false">✕</button>
        </div>
        <!-- 搜索框 -->
        <div class="drawer-search-wrap">
          <input
            v-model="searchQuery"
            class="drawer-search-input"
            placeholder="快速搜索频道…"
          />
          <button
            v-if="searchQuery"
            type="button"
            class="drawer-search-clear"
            title="清空搜索"
            @click="searchQuery = ''"
          >
            ✕
          </button>
        </div>
        <!-- 树形工具栏：统计与折叠操作 -->
        <div class="drawer-tree-toolbar">
          <span class="tree-total-info">
            {{ groupedChannels.length }} 分类 · {{ visibleChannels.length }} 频道
          </span>
          <div class="tree-actions">
            <button type="button" class="tree-action-btn" title="展开所有分类" @click="expandAll">全部展开</button>
            <span class="tree-action-sep">/</span>
            <button type="button" class="tree-action-btn" title="收起所有分类" @click="collapseAll">全部折叠</button>
          </div>
        </div>
      </div>

      <!-- 树形分组频道列表 (Tree View) -->
      <div class="drawer-channels-tree">
        <div v-if="!groupedChannels.length" class="drawer-empty-hint">
          无匹配频道
        </div>
        <div
          v-for="grp in groupedChannels"
          :key="grp.name"
          class="drawer-tree-group"
          :class="{ 'is-expanded': isGroupExpanded(grp.name) }"
        >
          <!-- 一级树节点：分组头部 (点击展开/折叠) -->
          <button
            type="button"
            class="tree-group-header"
            :class="{ 'has-active-channel': currentChannel?.group === grp.name }"
            @click="toggleGroup(grp.name)"
          >
            <span class="tree-arrow" :class="{ 'is-open': isGroupExpanded(grp.name) }">
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path d="m9 18 6-6-6-6" />
              </svg>
            </span>
            <span class="tree-group-name">{{ grp.name }}</span>
            <span class="tree-group-count">{{ grp.channels.length }}</span>
          </button>

          <!-- 二级树节点：频道列表 -->
          <div v-show="isGroupExpanded(grp.name)" class="tree-channels-sublist">
            <div
              v-for="(c, idx) in grp.channels"
              :key="c.key"
              class="drawer-channel-row"
              :class="{ active: currentChannel?.key === c.key }"
            >
              <button
                type="button"
                class="drawer-channel-btn"
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
              <button
                type="button"
                class="ch-fav-btn"
                :class="{ 'is-fav': isFavorite(c.key) }"
                :title="isFavorite(c.key) ? '取消收藏' : '加入收藏'"
                data-test="channel-fav-btn"
                @click.stop="toggleFavorite(c, $event)"
              >
                <svg viewBox="0 0 24 24" class="fav-star-icon" fill="currentColor">
                  <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
                </svg>
              </button>
            </div>
          </div>
        </div>
      </div>

      <div class="drawer-footer">
        <span>💡 快捷键：<strong>↑/↓</strong> 换台 · <strong>←/→</strong> 线路 · <strong>空格</strong> 暂停 · <strong>F</strong> 全屏</span>
      </div>
    </aside>

    <!-- 3. 未配置直播源时的居中空状态 -->
    <div
      v-if="!loading && !channels.length"
      class="player-empty-fullscreen"
      data-test="empty"
    >
      <div class="player-empty-card">
        <div class="empty-icon-badge">📺</div>
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

    <!-- 4. 电视主播放画幅 (全面对齐 PlayerView.vue) -->
    <template v-else>
      <!-- 视频视口 -->
      <div
        class="player-stage-viewport"
        data-test="player"
        @click="togglePlay"
      >
        <video
          ref="video"
          class="tv-video-el"
          playsinline
          autoplay
          @play="isPaused = false"
          @pause="isPaused = true"
        />
      </div>

      <!-- 顶部操作条 (.player-topbar) -->
      <div
        class="player-topbar"
        :class="{ 'is-hidden': !controlsVisible }"
      >
        <div class="topbar-left">
          <button
            type="button"
            class="player-back-btn"
            title="返回"
            @click="goBack"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M15 6l-6 6 6 6" />
            </svg>
            <span>返回</span>
          </button>

          <button
            type="button"
            class="player-channel-list-btn"
            title="查看频道列表"
            @click="toggleDrawer"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="2" y="7" width="20" height="15" rx="2" ry="2" />
              <polyline points="17 2 12 7 7 2" />
            </svg>
            <span>频道列表</span>
          </button>

          <div v-if="currentChannel" class="topbar-channel-meta">
            <span class="topbar-ch-name">{{ currentChannel.name }}</span>
            <button
              type="button"
              class="topbar-fav-btn"
              :class="{ 'is-fav': isFavorite(currentChannel.key) }"
              :title="isFavorite(currentChannel.key) ? '取消收藏' : '加入收藏'"
              data-test="topbar-fav-btn"
              @click.stop="toggleFavorite(currentChannel, $event)"
            >
              <svg viewBox="0 0 24 24" class="fav-star-icon" fill="currentColor">
                <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
              </svg>
            </button>
            <span v-if="current?.now_playing" class="topbar-now-playing">{{ current.now_playing }}</span>
          </div>
        </div>

        <div class="topbar-right">
          <div v-if="currentChannel" class="player-strategy-badge">
            <span v-if="isDirectStream" class="direct-stream-tag" title="当前频道免转码极速直出">⚡ 直连</span>
            线路 {{ activeRouteIndex + 1 }} / {{ currentChannel.routes.length }}
          </div>
          <div v-if="currentChannel?.group" class="player-group-badge">
            {{ currentChannel.group }}
          </div>
          <RouterLink
            to="/settings?tab=live"
            class="player-topbar-btn"
            title="配置直播源"
            data-test="link-settings"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
            </svg>
          </RouterLink>
        </div>
      </div>

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

      <!-- 网络重连/自愈浮层 (PlayerView 规范) -->
      <div
        v-if="streamNotice"
        class="player-recovering-hint"
      >
        <p class="recovering-bubble">
          <svg
            viewBox="0 0 24 24"
            class="spin-icon"
            fill="none"
            stroke="currentColor"
            stroke-width="2.2"
            stroke-linecap="round"
            aria-hidden="true"
          >
            <path d="M20 12a8 8 0 1 1-2.3-5.6M20 4v4h-4" />
          </svg>
          <span>{{ streamNotice }}</span>
        </p>
      </div>

      <!-- 缓冲动画指示器 (PlayerView 规范) -->
      <div
        v-else-if="starting"
        class="player-buffering-layer"
      >
        <div class="player-spinner" />
      </div>

      <!-- 严重错误提示 (PlayerView 规范) -->
      <div
        v-if="streamFailed"
        class="player-error-layer"
        @click.stop
      >
        <div class="error-icon-box">
          <svg
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <path d="M12 9v4M12 17h.01M10.3 3.9 2.4 18a1.8 1.8 0 0 0 1.6 2.7h15.9a1.8 1.8 0 0 0 1.6-2.7L13.7 3.9a1.8 1.8 0 0 0-3.4 0z" />
          </svg>
        </div>
        <p class="error-message">直播信号连接受阻</p>
        <p class="error-reasons">
          {{ error || '当前源站响应超时或未能拉取到直播流分片。' }}
        </p>
        <div class="error-actions">
          <button
            type="button"
            class="btn btn-secondary"
            @click="() => currentChannel && playChannel(currentChannel, activeRouteIndex)"
          >
            重试当前线路
          </button>
          <button
            type="button"
            class="btn btn-primary"
            @click="playNext"
          >
            播放下一频道
          </button>
        </div>
      </div>

      <!-- 居中暂停发光大播放圆钮 (PlayerView 规范) -->
      <div
        v-if="isPaused && !starting && !streamFailed"
        class="player-center-play"
        :class="{ 'is-hidden': !controlsVisible }"
      >
        <button
          type="button"
          class="big-play"
          aria-label="播放"
          title="播放 (空格)"
          @click.stop="togglePlay"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="m8 5 12 7-12 7z" />
          </svg>
        </button>
      </div>

      <!-- 底部悬浮控制台 (PlayerView 规范，无进度条) -->
      <div
        class="player-bottom-panel"
        :class="{ 'is-hidden': !controlsVisible }"
        @click.stop
      >
        <div class="player-ctl-row">
          <!-- 播放 / 暂停 -->
          <button
            type="button"
            class="ctl-btn"
            :aria-label="isPaused ? '播放' : '暂停'"
            :title="isPaused ? '播放 (空格)' : '暂停 (空格)'"
            @click="togglePlay"
          >
            <svg v-if="!isPaused" viewBox="0 0 24 24" aria-hidden="true">
              <path d="M9 5h2.5v14H9zM12.5 5H15v14h-2.5z" />
            </svg>
            <svg v-else viewBox="0 0 24 24" aria-hidden="true">
              <path d="M8 5.5 19 12 8 18.5z" />
            </svg>
          </button>

          <!-- 上一台 -->
          <button
            type="button"
            class="ctl-btn"
            aria-label="上一台"
            title="上一台 (↑)"
            @click="playPrev"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <polygon points="19 20 9 12 19 4 19 20" />
              <line x1="5" y1="19" x2="5" y2="5" />
            </svg>
          </button>

          <!-- 下一台 -->
          <button
            type="button"
            class="ctl-btn"
            aria-label="下一台"
            title="下一台 (↓)"
            @click="playNext"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <polygon points="5 4 15 12 5 20 5 4" />
              <line x1="19" y1="5" x2="19" y2="19" />
            </svg>
          </button>

          <!-- 停止播放 -->
          <button
            type="button"
            class="ctl-btn stop-btn"
            aria-label="停止播放"
            title="停止播放 (关闭流)"
            data-test="stop"
            @click="stopCurrent"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <rect x="6" y="6" width="12" height="12" rx="2" />
            </svg>
          </button>

          <!-- 当前频道信息 -->
          <div class="player-live-title-box">
            <span class="live-dot" />
            <span class="live-ch-name">{{ currentChannel?.name }}</span>
            <span v-if="current?.now_playing" class="live-ch-program">{{ current.now_playing }}</span>
          </div>

          <!-- 音量控制 -->
          <div class="player-volume-group">
            <button
              type="button"
              class="ctl-btn"
              :aria-label="isMuted ? '取消静音' : '静音'"
              :title="isMuted ? '取消静音 (M)' : '静音 (M)'"
              @click="toggleMute"
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
              >
                <path d="M11 5 6.5 9H3v6h3.5L11 19z" />
                <template v-if="!isMuted">
                  <path d="M15.5 9a4 4 0 0 1 0 6" />
                  <path d="M18 6.5a8 8 0 0 1 0 11" />
                </template>
                <template v-else>
                  <path d="M16 10l4 4M20 10l-4 4" />
                </template>
              </svg>
            </button>
            <input
              type="range"
              min="0"
              max="1"
              step="0.01"
              :value="isMuted ? 0 : volume"
              class="player-range volume-slider"
              aria-label="音量"
              @input="onVolumeInput"
            />
          </div>

          <div class="ctl-spacer" />

          <!-- 线路切换下拉选择器 (PlayerView .ctl-select 风格) -->
          <select
            v-if="currentChannel && currentChannel.routes.length > 1"
            class="ctl-select"
            :value="activeRouteIndex"
            title="切换当前频道线路"
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

          <!-- 画中画 -->
          <button
            type="button"
            class="ctl-btn"
            title="画中画"
            aria-label="画中画"
            @click="togglePip"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
              <rect x="11" y="9" width="9" height="6" rx="1" ry="1" />
            </svg>
          </button>

          <!-- 全屏切换 -->
          <button
            type="button"
            class="ctl-btn"
            :title="isFullscreen ? '退出全屏 (F)' : '全屏 (F)'"
            :aria-label="isFullscreen ? '退出全屏' : '全屏'"
            @click="toggleFullscreen"
          >
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" />
            </svg>
          </button>

          <!-- 直达设置页配置直播源 -->
          <RouterLink
            to="/settings?tab=live"
            class="ctl-btn"
            title="前往系统设置管理直播源"
            data-test="link-settings"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
            </svg>
          </RouterLink>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
/* ── 1. 全屏沉浸画幅 (完全对齐 PlayerView.vue) ────────── */
.player-shell {
  position: fixed;
  inset: 0;
  z-index: 50;
  display: flex;
  flex-direction: column;
  background: #000;
  user-select: none;
  overflow: hidden;
}

.player-stage-viewport {
  position: relative;
  flex: 1;
  width: 100%;
  height: 100%;
  overflow: hidden;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #000;
}

.tv-video-el {
  width: 100%;
  height: 100%;
  object-fit: contain;
  background: #000;
  display: block;
}

/* ── 2. 左侧边缘感应与滑出抽屉 ────────────────────────── */
.tv-edge-sensor {
  position: absolute;
  top: 0;
  left: 0;
  width: 50px;
  height: 100%;
  z-index: 30;
  pointer-events: auto;
  cursor: pointer;
}

.tv-channel-drawer {
  position: absolute;
  top: 0;
  left: 0;
  bottom: 0;
  width: 330px;
  max-width: 85vw;
  background: rgba(10, 16, 31, 0.94);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  border-right: 1px solid rgba(255, 255, 255, 0.12);
  box-shadow: 16px 0 48px rgba(0, 0, 0, 0.85);
  display: flex;
  flex-direction: column;
  z-index: 35;
  transform: translateX(-100%);
  transition: transform 0.28s cubic-bezier(0.16, 1, 0.3, 1);
  pointer-events: auto;
}

.tv-channel-drawer.is-open {
  transform: translateX(0);
}

.drawer-header {
  padding: 16px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  display: flex;
  flex-direction: column;
  gap: 12px;
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
  width: 28px;
  height: 28px;
  border-radius: 50%;
  border: 1px solid rgba(255, 255, 255, 0.15);
  background: rgba(255, 255, 255, 0.06);
  color: #98a6c2;
  cursor: pointer;
  display: grid;
  place-items: center;
  font-size: 13px;
  transition: 0.18s ease;
}

.drawer-close-btn:hover {
  background: rgba(255, 255, 255, 0.15);
  color: #fff;
}

.drawer-search-wrap {
  position: relative;
}

.drawer-search-input {
  width: 100%;
  box-sizing: border-box;
  padding: 8px 12px;
  background: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 8px;
  color: #fff;
  font-size: 13px;
  outline: none;
  transition: 0.18s ease;
}

.drawer-search-input:focus {
  border-color: #795bff;
  background: rgba(255, 255, 255, 0.1);
}

.drawer-search-clear {
  position: absolute;
  right: 8px;
  top: 50%;
  transform: translateY(-50%);
  width: 20px;
  height: 20px;
  border-radius: 50%;
  border: none;
  background: rgba(255, 255, 255, 0.15);
  color: #94a3b8;
  cursor: pointer;
  display: grid;
  place-items: center;
  font-size: 11px;
  transition: 0.15s ease;
}
.drawer-search-clear:hover {
  background: rgba(255, 255, 255, 0.25);
  color: #fff;
}

/* 树形工具栏 */
.drawer-tree-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
  color: #64748b;
  padding: 0 2px;
}

.tree-total-info {
  color: #94a3b8;
}

.tree-actions {
  display: flex;
  align-items: center;
  gap: 4px;
}

.tree-action-btn {
  background: transparent;
  border: none;
  color: #38bdf8;
  font-size: 11px;
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 4px;
  transition: 0.15s ease;
}
.tree-action-btn:hover {
  color: #7dd3fc;
  background: rgba(56, 189, 248, 0.1);
}

.tree-action-sep {
  color: #475569;
}

/* 树形分组主体 */
.drawer-channels-tree {
  flex: 1;
  overflow-y: auto;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.drawer-tree-group {
  display: flex;
  flex-direction: column;
  border-radius: 8px;
  transition: background 0.18s ease;
}

/* 一级分组头 */
.tree-group-header {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 10px;
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(255, 255, 255, 0.06);
  color: #e2e8f0;
  cursor: pointer;
  text-align: left;
  transition: 0.18s ease;
}
.tree-group-header:hover {
  background: rgba(255, 255, 255, 0.08);
  border-color: rgba(255, 255, 255, 0.12);
  color: #fff;
}
.tree-group-header.has-active-channel {
  border-color: rgba(121, 91, 255, 0.35);
  background: rgba(121, 91, 255, 0.08);
}
.tree-group-header.has-active-channel .tree-group-name {
  color: #c4b5fd;
  font-weight: 700;
}

.tree-arrow {
  display: grid;
  place-items: center;
  width: 16px;
  height: 16px;
  color: #94a3b8;
  transition: transform 0.2s cubic-bezier(0.16, 1, 0.3, 1);
}
.tree-arrow svg {
  width: 14px;
  height: 14px;
  fill: none;
  stroke: currentColor;
  stroke-width: 2.2;
  stroke-linecap: round;
  stroke-linejoin: round;
}
.tree-arrow.is-open {
  transform: rotate(90deg);
  color: #795bff;
}

.tree-group-name {
  flex: 1;
  font-size: 13px;
  font-weight: 600;
  letter-spacing: 0.2px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tree-group-count {
  font-size: 11px;
  padding: 1px 7px;
  border-radius: 9999px;
  background: rgba(255, 255, 255, 0.08);
  color: #94a3b8;
  font-weight: 600;
}

/* 二级频道子列表 */
.tree-channels-sublist {
  display: flex;
  flex-direction: column;
  gap: 3px;
  margin-top: 4px;
  margin-left: 12px;
  padding-left: 8px;
  border-left: 1px dashed rgba(255, 255, 255, 0.12);
}

.drawer-empty-hint {
  padding: 32px 16px;
  text-align: center;
  color: #64748b;
  font-size: 13px;
}

.drawer-channel-row {
  display: flex;
  align-items: center;
  width: 100%;
  border-radius: 8px;
  border: 1px solid transparent;
  transition: 0.18s ease;
  padding-right: 6px;
}

.drawer-channel-row:hover {
  background: rgba(255, 255, 255, 0.07);
}

.drawer-channel-row.active {
  background: rgba(121, 91, 255, 0.18);
  border-color: rgba(121, 91, 255, 0.4);
}

.drawer-channel-btn {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: 1;
  min-width: 0;
  padding: 10px 10px 10px 12px;
  background: transparent;
  border: none;
  color: #cbd5e1;
  cursor: pointer;
  text-align: left;
  transition: 0.18s ease;
}

.drawer-channel-row.active .drawer-channel-btn {
  color: #fff;
}

.drawer-ch-index {
  font-family: monospace;
  font-size: 12px;
  color: #64748b;
  font-weight: 600;
}

.drawer-ch-info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.drawer-ch-title {
  font-size: 13px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.drawer-ch-now-playing {
  font-size: 11px;
  color: #94a3b8;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.drawer-ch-routes {
  font-size: 11px;
  padding: 2px 6px;
  border-radius: 4px;
  background: rgba(255, 255, 255, 0.08);
  color: #94a3b8;
}

.drawer-playing-tag {
  font-size: 11px;
  color: #38bdf8;
  font-weight: 600;
}

.ch-fav-btn {
  background: transparent;
  border: none;
  padding: 4px;
  border-radius: 6px;
  color: #64748b;
  cursor: pointer;
  display: grid;
  place-items: center;
  transition: all 0.15s ease;
  opacity: 0.6;
}

.ch-fav-btn:hover {
  color: #fbbf24;
  transform: scale(1.15);
  opacity: 1;
}

.ch-fav-btn.is-fav {
  color: #fbbf24;
  opacity: 1;
}

.topbar-fav-btn {
  background: transparent;
  border: none;
  padding: 4px 6px;
  border-radius: 6px;
  color: rgba(255, 255, 255, 0.4);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  transition: all 0.15s ease;
}

.topbar-fav-btn:hover {
  color: #fbbf24;
  transform: scale(1.18);
}

.topbar-fav-btn.is-fav {
  color: #fbbf24;
}

.fav-star-icon {
  width: 14px;
  height: 14px;
}

.topbar-fav-btn .fav-star-icon {
  width: 16px;
  height: 16px;
}

.drawer-footer {
  padding: 12px 16px;
  border-top: 1px solid rgba(255, 255, 255, 0.08);
  font-size: 11px;
  color: #64748b;
  background: rgba(5, 8, 15, 0.5);
}

.drawer-footer strong {
  color: #cbd5e1;
}

/* ── 3. 顶部操作条 (完全对齐 PlayerView.vue) ──────────── */
.player-topbar {
  position: absolute;
  inset-inline: 0;
  top: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px 42px;
  background: linear-gradient(180deg, rgba(0, 0, 0, 0.85) 0%, transparent 100%);
  transition: opacity 0.25s ease;
  z-index: 10;
  pointer-events: auto;
}

.player-topbar.is-hidden {
  opacity: 0;
  pointer-events: none;
}

.topbar-left {
  display: flex;
  align-items: center;
  gap: 12px;
}

.player-back-btn,
.player-channel-list-btn {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  border-radius: 9999px;
  border: 1px solid rgba(255, 255, 255, 0.15);
  background: rgba(10, 16, 31, 0.7);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  padding: 7px 16px 7px 12px;
  font-size: 13px;
  font-weight: 600;
  color: #dbe4f6;
  text-decoration: none;
  cursor: pointer;
  transition: 0.18s ease;
}

.player-back-btn:hover,
.player-channel-list-btn:hover {
  background: rgba(22, 33, 58, 0.85);
  border-color: rgba(255, 255, 255, 0.3);
  color: #fff;
  transform: translateY(-1px);
}

.player-back-btn svg,
.player-channel-list-btn svg {
  width: 16px;
  height: 16px;
  stroke: currentColor;
  fill: none;
  stroke-width: 2.2;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.topbar-channel-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-left: 6px;
}

.topbar-ch-name {
  font-size: 15px;
  font-weight: 700;
  color: #fff;
  text-shadow: 0 1px 4px rgba(0, 0, 0, 0.8);
}

.topbar-now-playing {
  font-size: 12px;
  color: #94a3b8;
  max-width: 240px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.topbar-right {
  display: flex;
  align-items: center;
  gap: 10px;
}

.player-strategy-badge,
.player-group-badge {
  font-size: 11px;
  padding: 5px 12px;
  border-radius: 9999px;
  background: rgba(10, 16, 31, 0.7);
  border: 1px solid rgba(255, 255, 255, 0.15);
  color: #98a6c2;
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
}

.player-group-badge {
  color: #a5b4fc;
  border-color: rgba(165, 180, 252, 0.25);
}

.player-topbar-btn {
  display: grid;
  place-items: center;
  width: 34px;
  height: 34px;
  border-radius: 50%;
  border: 1px solid rgba(255, 255, 255, 0.15);
  background: rgba(10, 16, 31, 0.7);
  color: #dbe4f6;
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  text-decoration: none;
  transition: 0.18s ease;
}

.player-topbar-btn:hover {
  background: rgba(22, 33, 58, 0.85);
  border-color: rgba(255, 255, 255, 0.3);
  color: #fff;
  transform: translateY(-1px);
}

.player-topbar-btn svg {
  width: 16px;
  height: 16px;
}

/* ── 4. 换台电视 OSD 横幅 ─────────────────────────────── */
.tv-osd-banner {
  position: absolute;
  top: 72px;
  left: 24px;
  right: 24px;
  max-width: 760px;
  background: rgba(8, 12, 22, 0.9);
  backdrop-filter: blur(20px);
  -webkit-backdrop-filter: blur(20px);
  border: 1px solid rgba(255, 255, 255, 0.14);
  border-radius: 14px;
  padding: 14px 20px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  box-shadow: 0 12px 36px rgba(0, 0, 0, 0.6);
  z-index: 8;
  pointer-events: auto;
}

.osd-left {
  display: flex;
  align-items: center;
  gap: 14px;
}

.osd-badge {
  background: linear-gradient(135deg, #795bff, #6bd2ff);
  color: #fff;
  font-size: 13px;
  font-weight: 800;
  padding: 6px 12px;
  border-radius: 8px;
  box-shadow: 0 2px 8px rgba(121, 91, 255, 0.4);
}

.osd-content {
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.osd-title-row {
  display: flex;
  align-items: center;
  gap: 10px;
}

.osd-name {
  font-size: 18px;
  font-weight: 700;
  color: #fff;
}

.osd-group-pill {
  font-size: 11px;
  padding: 2px 8px;
  border-radius: 9999px;
  background: rgba(255, 255, 255, 0.1);
  color: #94a3b8;
}

.osd-meta-row {
  display: flex;
  align-items: center;
  gap: 12px;
  font-size: 12px;
}

.osd-route-label {
  color: #38bdf8;
  font-weight: 600;
}

.osd-now-playing {
  color: #cbd5e1;
}

.osd-right {
  display: none;
}
@media (min-width: 768px) {
  .osd-right {
    display: block;
    font-size: 11px;
    color: #64748b;
  }
}

.osd-fade-enter-active,
.osd-fade-leave-active {
  transition: opacity 0.3s ease, transform 0.3s ease;
}
.osd-fade-enter-from,
.osd-fade-leave-to {
  opacity: 0;
  transform: translateY(-8px);
}

/* ── 5. 中央状态浮层 (对齐 PlayerView.vue) ─────────────── */
.player-recovering-hint,
.player-buffering-layer {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  pointer-events: none;
  z-index: 4;
}

.recovering-bubble {
  display: flex;
  align-items: center;
  gap: 8px;
  border-radius: 9999px;
  border: 1px solid rgba(251, 191, 36, 0.3);
  background: rgba(0, 0, 0, 0.8);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  padding: 8px 18px;
  font-size: 13px;
  color: #fef08a;
}

.spin-icon {
  width: 16px;
  height: 16px;
  animation: spin 1s linear infinite;
}

@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

.player-spinner {
  width: 48px;
  height: 48px;
  border-radius: 50%;
  border: 3px solid rgba(255, 255, 255, 0.15);
  border-top-color: #987fff;
  animation: spin 0.8s linear infinite;
}

/* 严重错误卡片 */
.player-error-layer {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 16px;
  background: rgba(0, 0, 0, 0.92);
  backdrop-filter: blur(14px);
  -webkit-backdrop-filter: blur(14px);
  padding: 24px;
  text-align: center;
  z-index: 6;
  pointer-events: auto;
}

.error-icon-box {
  width: 52px;
  height: 52px;
  border-radius: 16px;
  display: grid;
  place-items: center;
  border: 1px solid rgba(239, 68, 68, 0.35);
  background: rgba(127, 29, 29, 0.5);
  color: #f87171;
}

.error-icon-box svg {
  width: 26px;
  height: 26px;
}

.error-message {
  font-size: 17px;
  font-weight: 600;
  color: #fca5a5;
  margin: 0;
}

.error-reasons {
  max-width: 440px;
  font-size: 13px;
  line-height: 1.7;
  color: #94a3b8;
  margin: 0;
}

.error-actions {
  display: flex;
  gap: 12px;
  margin-top: 8px;
}

.btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 8px 18px;
  border-radius: 9999px;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  transition: 0.18s ease;
  text-decoration: none;
}

.btn-secondary {
  border: 1px solid rgba(255, 255, 255, 0.18);
  background: rgba(255, 255, 255, 0.08);
  color: #e2e8f0;
}
.btn-secondary:hover {
  background: rgba(255, 255, 255, 0.16);
  color: #fff;
}

.btn-primary {
  border: 1px solid rgba(121, 91, 255, 0.4);
  background: linear-gradient(135deg, #795bff, #6bd2ff);
  color: #fff;
}
.btn-primary:hover {
  filter: brightness(1.1);
  transform: translateY(-1px);
}

/* 居中大播放圆钮 (PlayerView 规范) */
.player-center-play {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  pointer-events: none;
  z-index: 4;
  transition: opacity 0.25s ease;
}

.player-center-play.is-hidden {
  opacity: 0;
}

.big-play {
  pointer-events: auto;
  width: 68px;
  height: 68px;
  border-radius: 50%;
  border: 2px solid rgba(255, 255, 255, 0.25);
  background: rgba(10, 16, 31, 0.75);
  backdrop-filter: blur(16px);
  -webkit-backdrop-filter: blur(16px);
  color: #fff;
  display: grid;
  place-items: center;
  cursor: pointer;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6), 0 0 24px rgba(121, 91, 255, 0.35);
  transition: transform 0.2s ease, background 0.2s ease, border-color 0.2s ease;
}

.big-play:hover {
  transform: scale(1.1);
  background: rgba(22, 33, 58, 0.9);
  border-color: rgba(255, 255, 255, 0.4);
}

.big-play svg {
  width: 30px;
  height: 30px;
  fill: currentColor;
  margin-left: 3px;
}

/* ── 6. 底部悬浮控制台 (PlayerView 规范) ─────────────── */
.player-bottom-panel {
  position: absolute;
  inset-inline: 0;
  bottom: 0;
  background: linear-gradient(0deg, rgba(5, 8, 15, 0.98) 0%, rgba(5, 8, 15, 0.75) 60%, transparent 100%);
  padding: 38px 20px 18px;
  transition: opacity 0.25s ease;
  z-index: 10;
  pointer-events: auto;
}

.player-bottom-panel.is-hidden {
  opacity: 0;
  pointer-events: none;
}

.player-ctl-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 12px;
}

.ctl-btn {
  display: grid;
  place-items: center;
  width: 38px;
  height: 38px;
  border-radius: 50%;
  border: 1px solid rgba(255, 255, 255, 0.12);
  background: rgba(255, 255, 255, 0.08);
  color: #eaf0ff;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  cursor: pointer;
  transition: 0.18s ease;
  padding: 0;
  text-decoration: none;
}

.ctl-btn:hover {
  border-color: rgba(255, 255, 255, 0.28);
  background: rgba(255, 255, 255, 0.18);
  color: #fff;
  transform: translateY(-1px);
}

.ctl-btn svg {
  width: 18px;
  height: 18px;
  fill: currentColor;
}

.ctl-btn.stop-btn svg {
  fill: currentColor;
}

.player-live-title-box {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-left: 4px;
}

.live-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #ef4444;
  box-shadow: 0 0 8px #ef4444;
  animation: pulse 1.8s infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 1; transform: scale(1); }
  50% { opacity: 0.4; transform: scale(0.85); }
}

.live-ch-name {
  font-size: 14px;
  font-weight: 700;
  color: #fff;
}

.live-ch-program {
  font-size: 12px;
  color: #94a3b8;
  max-width: 200px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.player-volume-group {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-left: 6px;
}

.volume-slider {
  width: 78px;
}

.ctl-spacer {
  flex: 1;
}

/* 范围滑动条 (PlayerView 规范) */
.player-range {
  height: 14px;
  cursor: pointer;
}
.player-range::-webkit-slider-runnable-track {
  height: 4px;
  border-radius: 9999px;
  background: rgba(255, 255, 255, 0.2);
}
.player-range::-webkit-slider-thumb {
  -webkit-appearance: none;
  height: 13px;
  width: 13px;
  margin-top: -4.5px;
  border-radius: 9999px;
  background: #ffffff;
  box-shadow: 0 1px 5px rgba(0, 0, 0, 0.7);
  transition: transform 0.15s ease;
}
.player-range:hover::-webkit-slider-thumb {
  transform: scale(1.25);
}
.player-range::-moz-range-track {
  height: 4px;
  border-radius: 9999px;
  background: rgba(255, 255, 255, 0.2);
}
.player-range::-moz-range-thumb {
  height: 13px;
  width: 13px;
  border: none;
  border-radius: 9999px;
  background: #ffffff;
}

/* 下拉菜单 (PlayerView 规范) */
.ctl-select {
  cursor: pointer;
  appearance: none;
  border-radius: 9999px;
  border: 1px solid rgba(255, 255, 255, 0.12);
  background: rgba(255, 255, 255, 0.08);
  padding: 7px 28px 7px 14px;
  font-size: 12px;
  font-weight: 500;
  color: #eaf0ff;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  transition: 0.18s ease;
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%2394a3b8' stroke-width='2.4' stroke-linecap='round'%3E%3Cpath d='M6 9l6 6 6-6'/%3E%3C/svg%3E");
  background-repeat: no-repeat;
  background-position: right 8px center;
  background-size: 13px;
}
.ctl-select:hover {
  border-color: rgba(255, 255, 255, 0.28);
  background-color: rgba(255, 255, 255, 0.18);
}
.ctl-select option {
  background-color: #111a2c;
  color: #fff;
}

/* ── 7. 空状态居中全屏卡片 ───────────────────────────── */
.player-empty-fullscreen {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 24px;
  background: radial-gradient(circle at center, #111b2f 0%, #05080f 100%);
  z-index: 20;
}

.player-empty-card {
  max-width: 480px;
  text-align: center;
  padding: 40px 32px;
  border-radius: 20px;
  border: 1px solid rgba(255, 255, 255, 0.1);
  background: rgba(10, 16, 31, 0.85);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  box-shadow: 0 24px 64px rgba(0, 0, 0, 0.6);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 16px;
}

.empty-icon-badge {
  font-size: 42px;
  width: 76px;
  height: 76px;
  display: grid;
  place-items: center;
  border-radius: 20px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid rgba(255, 255, 255, 0.1);
}

.player-empty-card h2 {
  font-size: 20px;
  font-weight: 700;
  color: #fff;
  margin: 0;
}

.player-empty-card p {
  font-size: 14px;
  line-height: 1.6;
  color: #94a3b8;
  margin: 0;
}

.btn-go-settings {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 10px 24px;
  border-radius: 9999px;
  background: linear-gradient(135deg, #795bff, #6bd2ff);
  color: #fff;
  font-size: 14px;
  font-weight: 600;
  text-decoration: none;
  box-shadow: 0 4px 16px rgba(121, 91, 255, 0.35);
  transition: transform 0.18s ease, filter 0.18s ease;
}

.btn-go-settings:hover {
  transform: translateY(-2px);
  filter: brightness(1.1);
}

.direct-stream-tag {
  background: rgba(16, 185, 129, 0.2);
  color: #34d399;
  border: 1px solid rgba(16, 185, 129, 0.4);
  font-size: 11px;
  font-weight: 600;
  padding: 1px 6px;
  border-radius: 4px;
  margin-right: 6px;
  display: inline-flex;
  align-items: center;
}
</style>
