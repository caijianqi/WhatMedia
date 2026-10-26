<script setup lang="ts">
/**
 * 播放器页。
 *
 * 两条原则：
 *   1. **菜单跟着能力走**。`canSwitchAudioTrack` 为 false 时不画音轨菜单 ——
 *      给一个点了没反应的按钮比不给更糟。同理画质菜单在没有多码率时也不出现。
 *   2. **页面卸载前必须补发进度**。`pagehide` 时普通 fetch 大概率发不出去，
 *      所以走 `sendBeacon`。漏了这一步，用户关掉标签页后进度就停在十秒前。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { usePlayerStore } from '../stores/player'
import { useLayoutStore } from '../stores/layout'
import { toast } from '../stores/toast'
import * as api from '../api/client'
import {
  TV_CONTROLS_TIMEOUT_MS,
  FocusMemory,
  moveFocus,
  type RemoteKeyName,
  type Direction,
  type Focusable,
} from '../lib/tv'
import { isElementVisible } from '../composables/useTvFocus'
import { seekSecondsForDrag, volumeDeltaForDrag } from '../lib/platform'

const route = useRoute()
const router = useRouter()
const store = usePlayerStore()
const layout = useLayoutStore()

const itemId = computed(() => String(route.params.id))
const startMs = computed(() => {
  const v = Number(route.query.start ?? 0)
  if (v > 0 && v < 10000) return v * 1000
  return v
})
const isVod = computed(() => !!route.query.vod_url || String(route.params.id) === 'vod')
const vodTitle = computed(() => (route.query.vod_title as string) || '云端在线播放')
const vodSource = computed(() => (route.query.vod_source as string) || '多源聚合')

/** TV 形态。TV 上没有指针，也没有"鼠标不动就隐藏控制条"这回事。 */
const isTv = computed(() => layout.isTv)
const isHandheld = computed(() => layout.isHandheld)

const shell = ref<HTMLElement | null>(null)
const stage = ref<HTMLElement | null>(null)

/** 控制条自动隐藏。鼠标不动 2.5 秒后淡出，避免挡住画面。 */
const controlsVisible = ref(true)
let hideTimer: number | undefined

const seeking = ref(false)
const dragValue = ref(0)

/**
 * 是否处于全屏。
 *
 * 模板里访问不了 `document`（Vue 模板只暴露有限的全局对象），所以要显式建 ref，
 * 并且监听 `fullscreenchange` —— 用户按 Esc 退出时不是我们的代码改的状态。
 */
const isFullscreen = ref(false)
function onFullscreenChange() {
  isFullscreen.value = !!document.fullscreenElement
  if (isFullscreen.value) {
    if (layout.isHandheld && window.screen?.orientation && 'lock' in window.screen.orientation) {
      try {
        void (window.screen.orientation as any).lock('landscape').catch(() => {})
      } catch {
        // 部分浏览器不支持 orientation.lock，静默放行
      }
    }
  } else {
    if (window.screen?.orientation && 'unlock' in window.screen.orientation) {
      try {
        window.screen.orientation.unlock()
      } catch {
        // 静默放行
      }
    }
  }
}

const position = computed(() => (seeking.value ? dragValue.value : store.snapshot.position))
const duration = computed(() => store.snapshot.duration || 0)

const playedPct = computed(() => {
  if (!duration.value || duration.value <= 0) return 0
  return Math.min(100, Math.max(0, (position.value / duration.value) * 100))
})

const rates = [1.0, 1.25, 1.5, 2.0, 0.75]
const currentRate = ref(1.0)

function cycleRate(): void {
  const nextIndex = (rates.indexOf(currentRate.value) + 1) % rates.length
  currentRate.value = rates[nextIndex]!
  void store.setRate(currentRate.value)
  toast(`播放速度：${currentRate.value}×`, { type: 'info' })
  armHideTimer()
}

function skip(deltaSeconds: number): void {
  const target = Math.max(0, Math.min(duration.value, position.value + deltaSeconds))
  void store.seek(target)
  armHideTimer()
}

const isPlaying = computed(() => store.snapshot.state === 'playing')

const mediaTitle = ref('')
const isCloudVodItem = ref(false)

// ── 选集与自动连播系统 ──────────────────────────────────────
export interface PlayableEpisode {
  id: string
  name: string
  url?: string
}

const episodeList = ref<PlayableEpisode[]>([])
const currentEpisodeId = ref<string>('')
const showEpisodeDrawer = ref(false)
const autoPlayCountdown = ref<number | null>(null)
let countdownTimer: number | undefined
const vodBaseTitle = ref('')

const currentEpisodeIndex = computed(() =>
  episodeList.value.findIndex(
    (e) => e.id === currentEpisodeId.value || (e.url && e.url === currentEpisodeId.value),
  ),
)

const nextEpisode = computed<PlayableEpisode | null>(() => {
  const idx = currentEpisodeIndex.value
  if (idx >= 0 && idx + 1 < episodeList.value.length) {
    return episodeList.value[idx + 1] ?? null
  }
  return null
})

const prevEpisode = computed<PlayableEpisode | null>(() => {
  const idx = currentEpisodeIndex.value
  if (idx > 0) {
    return episodeList.value[idx - 1] ?? null
  }
  return null
})

function isCurrentEpisode(ep: PlayableEpisode): boolean {
  return ep.id === currentEpisodeId.value || (!!ep.url && ep.url === currentEpisodeId.value)
}

function toggleEpisodeDrawer() {
  showEpisodeDrawer.value = !showEpisodeDrawer.value
  armHideTimer()
}

function cancelAutoPlay() {
  if (countdownTimer) {
    clearInterval(countdownTimer)
    countdownTimer = undefined
  }
  autoPlayCountdown.value = null
}

function startAutoPlayCountdown() {
  cancelAutoPlay()
  if (!nextEpisode.value) return
  autoPlayCountdown.value = 5
  countdownTimer = window.setInterval(() => {
    if (autoPlayCountdown.value === null) return
    if (autoPlayCountdown.value <= 1) {
      cancelAutoPlay()
      if (nextEpisode.value) {
        void switchEpisode(nextEpisode.value)
      }
    } else {
      autoPlayCountdown.value -= 1
    }
  }, 1000)
}

async function switchEpisode(ep: PlayableEpisode) {
  cancelAutoPlay()
  showEpisodeDrawer.value = false
  currentEpisodeId.value = ep.url || ep.id
  if (!stage.value) return

  if (isVod.value && ep.url) {
    mediaTitle.value = vodBaseTitle.value ? `${vodBaseTitle.value} - ${ep.name}` : ep.name
    await store.openVodUrl(
      { container: stage.value, fullscreenTarget: shell.value ?? undefined },
      ep.url,
      {
        startMs: 0,
        autoplay: true,
      },
    )
  } else {
    mediaTitle.value = ep.name
    await store.open(
      { container: stage.value, fullscreenTarget: shell.value ?? undefined },
      ep.id,
      {
        startMs: 0,
        autoplay: true,
      },
    )
  }
  armHideTimer()
}

async function loadEpisodes(): Promise<void> {
  if (isVod.value) {
    const rawVodUrl = (route.query.vod_url as string) || ''
    currentEpisodeId.value = rawVodUrl
    try {
      const cached = sessionStorage.getItem('current_vod_episodes')
      if (cached) {
        const parsed = JSON.parse(cached)
        vodBaseTitle.value = parsed.item_title || ''
        if (Array.isArray(parsed.episodes) && parsed.episodes.length > 0) {
          episodeList.value = parsed.episodes.map((e: any, idx: number) => ({
            id: e.play_url || String(idx),
            name: e.name || `第 ${idx + 1} 集`,
            url: e.play_url,
          }))
        }
      }
    } catch {}
  } else {
    currentEpisodeId.value = itemId.value
    try {
      const item = await api.getItem(itemId.value).catch(() => null)
      if (item) {
        // 如果是剧集或单集，反查剧集列表
        const seriesRef = await api.itemSeries(itemId.value).catch(() => null)
        const seriesId = seriesRef?.id || (item.kind === 'series' ? item.id : null)
        if (seriesId) {
          const epPage = await api.listEpisodes({ series_id: seriesId, page_size: 500 }).catch(() => null)
          if (epPage && epPage.data.length > 0) {
            episodeList.value = epPage.data.map((e) => ({
              id: e.item_id,
              name: e.title || `第 ${e.episode_number} 集`,
            }))
            return
          }
        }

        // 尝试从文件版本分集匹配
        const defaultVer = item.versions?.find((v) => v.is_default) ?? item.versions?.[0]
        if (defaultVer && defaultVer.files.length > 1) {
          episodeList.value = defaultVer.files.map((f, idx) => ({
            id: f.id,
            name: `第 ${idx + 1} 集`,
            url: f.path,
          }))
        }
      }
    } catch {}
  }
}

// 监听播放结束事件，若有下一集自动开启倒计时连播
watch(
  () => store.snapshot.state,
  (state: string) => {
    if (state === 'ended' && nextEpisode.value) {
      startAutoPlayCountdown()
    } else if (state !== 'ended') {
      cancelAutoPlay()
    }
  },
)

// ── 生命周期 ────────────────────────────────────────────────

onMounted(async () => {
  if (!stage.value) return
  if (isVod.value) {
    const rawVodUrl = route.query.vod_url as string
    if (rawVodUrl) {
      await store.openVodUrl(
        { container: stage.value, fullscreenTarget: shell.value ?? undefined },
        rawVodUrl,
        {
          startMs: startMs.value,
          autoplay: true,
        },
      )
    }
  } else {
    try {
      const itemDetail = await api.getItem(itemId.value).catch(() => null)
      const targetEpisodeNum = route.query.episode ? Number(route.query.episode) : null

      const defaultVersion =
        itemDetail?.versions?.find((v) => v.is_default) ?? itemDetail?.versions?.[0]
      const files = defaultVersion?.files ?? []

      // 智能匹配分集文件（优先根据 fingerprint 结尾的 _集数，其次按索引定位）
      const matchedFile = targetEpisodeNum
        ? files.find((f) => (f as any).file_fingerprint?.endsWith(`_${targetEpisodeNum}`)) ??
          files[targetEpisodeNum - 1]
        : files[0]

      const targetFile = matchedFile ?? files[0]

      if (itemDetail) {
        let title = itemDetail.title || ''
        if (targetEpisodeNum) {
          title += ` 第 ${String(targetEpisodeNum).padStart(2, '0')} 集`
        }
        mediaTitle.value = title
      }

      const isCloudVod =
        (itemDetail as any)?.provider === 'vod_aggregate' ||
        targetFile?.storage_provider === 'cloud_vod' ||
        targetFile?.path?.startsWith('http://') ||
        targetFile?.path?.startsWith('https://')

      isCloudVodItem.value = isCloudVod

      if (isCloudVod && targetFile?.path) {
        await store.openVodUrl(
          { container: stage.value, fullscreenTarget: shell.value ?? undefined },
          targetFile.path,
          {
            startMs: startMs.value,
            autoplay: true,
            itemId: itemId.value,
            fileId: targetFile.id,
          },
        )
      } else {
        await store.open(
          { container: stage.value, fullscreenTarget: shell.value ?? undefined },
          itemId.value,
          {
            startMs: startMs.value,
            autoplay: true,
          },
        )
      }
    } catch {
      await store.open(
        { container: stage.value, fullscreenTarget: shell.value ?? undefined },
        itemId.value,
        {
          startMs: startMs.value,
          autoplay: true,
        },
      )
    }
  }
  void loadEpisodes()
  window.addEventListener('keydown', onKeyDown)
  window.addEventListener('pagehide', onPageHide)
  document.addEventListener('fullscreenchange', onFullscreenChange)
  // 网络切换：WiFi ↔ 4G。不监听的话断网后播放器就死在那里了。
  window.addEventListener('online', onOnline)
  window.addEventListener('offline', onOffline)
  armHideTimer()
})

onBeforeUnmount(() => {
  cancelAutoPlay()
  window.removeEventListener('keydown', onKeyDown)
  window.removeEventListener('pagehide', onPageHide)
  document.removeEventListener('fullscreenchange', onFullscreenChange)
  window.removeEventListener('online', onOnline)
  window.removeEventListener('offline', onOffline)
  if (hideTimer) window.clearTimeout(hideTimer)
  if (window.screen?.orientation && 'unlock' in window.screen.orientation) {
    try {
      window.screen.orientation.unlock()
    } catch {}
  }
  void store.close()
})

function onOnline() {
  store.onNetworkChange(true)
}

function onOffline() {
  store.onNetworkChange(false)
}

function onPageHide() {
  store.flushBeforeUnload()
}

// ── 控制条显隐与焦点管理 ────────────────────────────────────

const tvPlayerMemory = new FocusMemory()
const currentFocusIdx = ref(-1)

function collectPlayerFocusables(): { elements: HTMLElement[]; items: Focusable[] } {
  if (!shell.value) return { elements: [], items: [] }

  // 选集抽屉打开时，TV 焦点完全限制在抽屉内
  if (showEpisodeDrawer.value) {
    const selector = '.drawer-close, .drawer-ep-btn'
    const all = Array.from(shell.value.querySelectorAll<HTMLElement>(selector))
    const elements: HTMLElement[] = []
    const items: Focusable[] = []
    for (let i = 0; i < all.length; i++) {
      const el = all[i]
      if (!el || !isElementVisible(el)) continue
      const rect = el.getBoundingClientRect()
      elements.push(el)
      items.push({
        id: el.id || `p-drawer-${items.length}`,
        rect: { x: rect.left, y: rect.top, w: rect.width, h: rect.height },
        disabled: el.hasAttribute('disabled'),
      })
    }
    return { elements, items }
  }

  if (!controlsVisible.value) return { elements: [], items: [] }
  const selector =
    '.player-back-btn, .big-play, .timeline-slider, .ctl-btn, .ctl-select, .speed-btn'
  const all = Array.from(shell.value.querySelectorAll<HTMLElement>(selector))
  const elements: HTMLElement[] = []
  const items: Focusable[] = []
  for (let i = 0; i < all.length; i++) {
    const el = all[i]
    if (!el || !isElementVisible(el)) continue
    const rect = el.getBoundingClientRect()
    elements.push(el)
    items.push({
      id: el.id || `p-focus-${items.length}`,
      rect: { x: rect.left, y: rect.top, w: rect.width, h: rect.height },
      disabled: el.hasAttribute('disabled'),
    })
  }
  return { elements, items }
}

function applyPlayerFocus(el: HTMLElement) {
  shell.value?.querySelectorAll('.tv-focused').forEach((node) => {
    if (node !== el) node.classList.remove('tv-focused')
  })
  el.classList.add('tv-focused')
  el.focus({ preventScroll: true })
}

function clearPlayerFocus() {
  shell.value?.querySelectorAll('.tv-focused').forEach((node) => node.classList.remove('tv-focused'))
  if (document.activeElement && shell.value?.contains(document.activeElement)) {
    (document.activeElement as HTMLElement).blur()
  }
  currentFocusIdx.value = -1
}

function movePlayerFocus(dir: Direction): boolean {
  const { elements, items } = collectPlayerFocusables()
  if (items.length === 0) return false

  let curIdx = currentFocusIdx.value
  const active = document.activeElement as HTMLElement | null
  if (active && shell.value?.contains(active)) {
    const found = elements.indexOf(active)
    if (found >= 0) curIdx = found
  }

  let nextIdx: number
  if (curIdx < 0 || curIdx >= elements.length) {
    // 默认聚焦到底部播放按钮或第一个控制按钮
    const playBtnIdx = elements.findIndex((el) => {
      const label = el.getAttribute('aria-label') || ''
      return label.includes('播放') || label.includes('暂停')
    })
    nextIdx = playBtnIdx >= 0 ? playBtnIdx : 0
  } else {
    nextIdx = moveFocus(items, curIdx, dir, { memory: tvPlayerMemory })
  }

  if (nextIdx >= 0 && nextIdx < elements.length) {
    currentFocusIdx.value = nextIdx
    tvPlayerMemory.remember(nextIdx, items)
    applyPlayerFocus(elements[nextIdx]!)
    return true
  }
  return false
}

function armHideTimer() {
  if (hideTimer) window.clearTimeout(hideTimer)
  controlsVisible.value = true
  const delay = isTv.value ? TV_CONTROLS_TIMEOUT_MS : 2500
  hideTimer = window.setTimeout(() => {
    if (isPlaying.value) {
      controlsVisible.value = false
      clearPlayerFocus()
    }
  }, delay)
}

function onPointerMove() {
  armHideTimer()
}

// ── 遥控器（TV）────────────────────────────────────────────
function tvKeyName(e: KeyboardEvent): RemoteKeyName | null {
  switch (e.key) {
    case 'ArrowUp':
      return 'up'
    case 'ArrowDown':
      return 'down'
    case 'ArrowLeft':
      return 'left'
    case 'ArrowRight':
      return 'right'
    case 'Enter':
      return 'select'
    case 'Escape':
    case 'Backspace':
      return 'back'
    case 'MediaPlayPause':
      return 'toggle'
    case 'MediaTrackNext':
    case 'PageDown':
      return 'next'
    case 'MediaTrackPrevious':
    case 'PageUp':
      return 'previous'
    default:
      return null
  }
}

function onRemoteKey(name: RemoteKeyName): void {
  // 1. 返回键：优先关闭选集抽屉或连播倒计时，其次收起控制条，最后才退出播放
  if (name === 'back') {
    if (showEpisodeDrawer.value) {
      showEpisodeDrawer.value = false
      return
    }
    if (autoPlayCountdown.value !== null) {
      cancelAutoPlay()
      return
    }
    if (controlsVisible.value) {
      controlsVisible.value = false
      clearPlayerFocus()
      return
    }
    if (isFullscreen.value) {
      void store.setFullscreen(false)
    } else {
      void router.back()
    }
    return
  }

  // 2. 媒体键与切集
  if (name === 'next') {
    if (nextEpisode.value) {
      void switchEpisode(nextEpisode.value)
    } else {
      skip(10)
    }
    armHideTimer()
    return
  }
  if (name === 'previous') {
    if (prevEpisode.value) {
      void switchEpisode(prevEpisode.value)
    } else {
      skip(-10)
    }
    armHideTimer()
    return
  }
  if (name === 'toggle') {
    togglePlay()
    armHideTimer()
    return
  }
  if (name === 'seek-forward') {
    skip(10)
    armHideTimer()
    return
  }
  if (name === 'seek-backward') {
    skip(-10)
    armHideTimer()
    return
  }

  // 3. 菜单键
  if (name === 'menu') {
    if (!controlsVisible.value) {
      armHideTimer()
      movePlayerFocus('down')
    } else {
      controlsVisible.value = false
      clearPlayerFocus()
    }
    return
  }

  // 4. 选择/确认键（OK）
  if (name === 'select') {
    if (!controlsVisible.value) {
      togglePlay()
      armHideTimer()
      return
    }

    const active = document.activeElement as HTMLElement | null
    if (active && shell.value?.contains(active) && active !== document.body) {
      active.click()
    } else {
      togglePlay()
    }
    armHideTimer()
    return
  }

  // 5. 方向键（Up / Down / Left / Right）
  if (name === 'left' || name === 'right') {
    if (!controlsVisible.value) {
      // 控制条未显示时直接快退/快进 10 秒并唤醒控制条查看进度
      skip(name === 'left' ? -10 : 10)
      armHideTimer()
      return
    }

    const active = document.activeElement as HTMLElement | null
    if (active && active.classList.contains('timeline-slider')) {
      skip(name === 'left' ? -5 : 5)
      armHideTimer()
      return
    }

    const moved = movePlayerFocus(name)
    if (!moved) {
      skip(name === 'left' ? -10 : 10)
    }
    armHideTimer()
    return
  }

  if (name === 'up' || name === 'down') {
    if (!controlsVisible.value) {
      const delta = name === 'up' ? 0.05 : -0.05
      const newVol = Math.min(1, Math.max(0, store.snapshot.volume + delta))
      void store.setVolume(newVol)
      gestureHint.value = `音量 ${Math.round(newVol * 100)}%`
      setTimeout(() => {
        gestureHint.value = null
      }, 1500)
      armHideTimer()
      return
    }

    movePlayerFocus(name)
    armHideTimer()
    return
  }

  armHideTimer()
}

// ── 触摸手势（手机）────────────────────────────────────────
//
// 手机上"拖动进度条"这个动作精度不够 —— 手指比鼠标粗，
// 且屏幕窄，进度条只有几百像素，拖 1 像素就是好几秒。
// 所以手机上用**整屏横向拖动**做 seek：拖一半屏幕 ≈ 跳到中间。
//
// 纵向拖动做音量：手机上系统音量键往往被映射成铃声，不是媒体音量。
let gestureStart: { x: number; y: number; at: number } | null = null
const gestureHint = ref<string | null>(null)

function onTouchStart(e: TouchEvent) {
  if (!isHandheld.value || e.touches.length !== 1) return
  const t = e.touches[0]!
  gestureStart = { x: t.clientX, y: t.clientY, at: store.snapshot.position }
}

function onTouchMove(e: TouchEvent) {
  if (!gestureStart || e.touches.length !== 1) return
  const t = e.touches[0]!
  const dx = (t.clientX - gestureStart.x) / window.innerWidth
  const dy = (t.clientY - gestureStart.y) / window.innerHeight
  // 横向与纵向判别：谁幅度大算谁，避免斜着拖时两个都生效。
  if (Math.abs(dx) > Math.abs(dy)) {
    const target = Math.max(0, gestureStart.at + seekSecondsForDrag(dx, duration.value))
    gestureHint.value = `快进到 ${fmt(target)}`
  } else {
    const delta = volumeDeltaForDrag(-dy)
    const v = Math.min(1, Math.max(0, store.snapshot.volume + delta))
    gestureHint.value = `音量 ${Math.round(v * 100)}%`
  }
}

function onTouchEnd(e: TouchEvent) {
  if (!gestureStart) return
  const start = gestureStart
  gestureStart = null
  const changed = e.changedTouches[0]
  if (!changed) return
  const dx = (changed.clientX - start.x) / window.innerWidth
  const dy = (changed.clientY - start.y) / window.innerHeight
  if (Math.abs(dx) > Math.abs(dy)) {
    const target = Math.max(0, start.at + seekSecondsForDrag(dx, duration.value))
    void store.seek(target)
  } else {
    void store.setVolume(Math.min(1, Math.max(0, store.snapshot.volume + volumeDeltaForDrag(-dy))))
  }
  gestureHint.value = null
  armHideTimer()
}

// ── 交互 ────────────────────────────────────────────────────

function togglePlay() {
  void (isPlaying.value ? store.pause() : store.play())
  armHideTimer()
}

function onSeekInput(e: Event) {
  dragValue.value = Number((e.target as HTMLInputElement).value)
}

function commitSeek() {
  void store.seek(dragValue.value)
  seeking.value = false
  armHideTimer()
}

function onKeyDown(e: KeyboardEvent) {
  // 输入框里打字时不该触发播放控制。
  const t = e.target as HTMLElement | null
  if (t && /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName)) return

  // TV 上优先走遥控器映射 —— 它的按键语义与桌面键盘不同
  // （比如 TV 上左右是移动焦点，而桌面上是快进快退）。
  if (isTv.value) {
    const name = tvKeyName(e)
    if (name) {
      e.preventDefault()
      onRemoteKey(name)
      return
    }
  }

  switch (e.key) {
    case ' ':
      e.preventDefault()
      togglePlay()
      break
    case 'ArrowLeft':
      void store.seek(Math.max(0, store.snapshot.position - 10))
      break
    case 'ArrowRight':
      void store.seek(store.snapshot.position + 10)
      break
    case 'ArrowUp':
      e.preventDefault()
      void store.setVolume(Math.min(1, store.snapshot.volume + 0.05))
      break
    case 'ArrowDown':
      e.preventDefault()
      void store.setVolume(Math.max(0, store.snapshot.volume - 0.05))
      break
    case 'm':
      void store.setMuted(!store.snapshot.muted)
      break
    case 'f':
      void store.setFullscreen(!isFullscreen.value)
      break
    default:
      return
  }
  armHideTimer()
}

// ── 显示辅助 ────────────────────────────────────────────────

function fmt(sec: number): string {
  const s = Math.max(0, Math.floor(sec))
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const v = s % 60
  const pad = (n: number) => String(n).padStart(2, '0')
  return h > 0 ? `${h}:${pad(m)}:${pad(v)}` : `${m}:${pad(v)}`
}

const bufferedPct = computed(() => {
  const d = duration.value
  return d > 0 ? Math.min(100, (store.snapshot.buffered / d) * 100) : 0
})
</script>

<template>
  <div
    ref="shell"
    class="player-shell"
    @pointermove="onPointerMove"
    @touchstart.passive="onTouchStart"
    @touchmove.passive="onTouchMove"
    @touchend="onTouchEnd"
  >
    <!-- 视频渲染舞台 -->
    <div ref="stage" class="player-stage-viewport" @click="togglePlay" />

    <!-- 顶部操作条 -->
    <div
      class="player-topbar"
      :class="{ 'is-hidden': !controlsVisible }"
    >
      <RouterLink
        :to="isVod ? { name: 'home' } : { name: 'item', params: { id: itemId } }"
        class="player-back-btn"
        :title="isVod ? '返回首页' : '返回详情'"
      >
        <svg
          viewBox="0 0 24 24"
          aria-hidden="true"
        >
          <path d="M15 6l-6 6 6 6" />
        </svg>
        <span>返回</span>
      </RouterLink>

      <div v-if="isVod || mediaTitle" class="player-vod-info">
        <span class="player-strategy-badge vod-badge">
          {{ isVod ? `云端聚合 · ${vodSource}` : isCloudVodItem ? '全网聚合线路' : (store.strategy || '直出播放') }}
        </span>
        <span class="vod-play-title">{{ isVod ? vodTitle : mediaTitle }}</span>
      </div>
      <div v-else-if="store.strategy" class="player-strategy-badge">
        {{ store.strategy }}
      </div>
    </div>

    <!-- 网络重连浮层 -->
    <div
      v-if="store.recovering || store.netNote"
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
        <span>{{ store.netNote ?? '正在重连…' }}</span>
      </p>
    </div>

    <!-- 手势提示（移动端） -->
    <div
      v-if="gestureHint"
      class="player-gesture-tip"
    >
      <p class="gesture-bubble">
        {{ gestureHint }}
      </p>
    </div>

    <!-- 严重错误提示 -->
    <div
      v-if="store.error"
      class="player-error-layer"
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
      <p class="error-message">{{ store.error }}</p>
      <p v-if="store.reasons.length" class="error-reasons">
        {{ store.reasons.map((r) => r.message).join('；') }}
      </p>
      <RouterLink
        :to="isVod ? { name: 'home' } : { name: 'item', params: { id: itemId } }"
        class="btn btn-secondary"
      >
        {{ isVod ? '返回首页' : '返回详情' }}
      </RouterLink>
    </div>

    <!-- 缓冲动画指示器 -->
    <div
      v-else-if="store.snapshot.state === 'buffering'"
      class="player-buffering-layer"
    >
      <div class="player-spinner" />
    </div>

    <!-- 居中暂停发光大播放圆钮（ui.html 规范） -->
    <div
      v-if="!isPlaying && !store.error && !store.recovering && store.snapshot.state !== 'buffering'"
      class="player-center-play"
      :class="{ 'is-hidden': !controlsVisible }"
    >
      <button
        type="button"
        class="big-play"
        aria-label="播放"
        title="播放"
        @click="togglePlay"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="m8 5 12 7-12 7z" />
        </svg>
      </button>
    </div>

    <!-- 底部悬浮控制台 -->
    <div
      class="player-bottom-panel"
      :class="{ 'is-hidden': !controlsVisible }"
    >
      <!-- 双色发光进度滑轨 -->
      <div class="timeline-wrapper">
        <div class="timeline-bar">
          <div
            class="timeline-buffered"
            :style="{ width: bufferedPct + '%' }"
          />
          <div
            class="timeline-played"
            :style="{ width: playedPct + '%' }"
          />
        </div>
        <input
          type="range"
          min="0"
          :max="duration || 0"
          step="0.1"
          :value="position"
          class="player-range timeline-slider"
          aria-label="播放进度"
          @pointerdown="seeking = true"
          @input="onSeekInput"
          @change="commitSeek"
        />
      </div>

      <!-- 控制按钮与元数据条 -->
      <div class="player-ctl-row">
        <!-- 播放 / 暂停 -->
        <button
          type="button"
          class="ctl-btn"
          :aria-label="isPlaying ? '暂停' : '播放'"
          :title="isPlaying ? '暂停' : '播放'"
          @click="togglePlay"
        >
          <svg v-if="isPlaying" viewBox="0 0 24 24" aria-hidden="true">
            <path d="M9 5h2.5v14H9zM12.5 5H15v14h-2.5z" />
          </svg>
          <svg v-else viewBox="0 0 24 24" aria-hidden="true">
            <path d="M8 5.5 19 12 8 18.5z" />
          </svg>
        </button>

        <!-- 快退 10 秒 -->
        <button
          type="button"
          class="ctl-btn"
          aria-label="后退 10 秒"
          title="后退 10 秒"
          @click="skip(-10)"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 8v4l3 3M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
            <path d="M3 3v5h5" />
          </svg>
        </button>

        <!-- 快进 10 秒 -->
        <button
          type="button"
          class="ctl-btn"
          aria-label="快进 10 秒"
          title="快进 10 秒"
          @click="skip(10)"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 8v4l-3 3M21 12a9 9 0 1 1-9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
            <path d="M21 3v5h-5" />
          </svg>
        </button>

        <!-- 时间显示 -->
        <div class="player-time-display">
          <span>{{ fmt(position) }}</span>
          <span class="sep">/</span>
          <span class="dur">{{ fmt(duration) }}</span>
        </div>

        <!-- 音量控制 -->
        <div class="player-volume-group">
          <button
            type="button"
            class="ctl-btn"
            :aria-label="store.snapshot.muted ? '取消静音' : '静音'"
            @click="store.setMuted(!store.snapshot.muted)"
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
              <template v-if="!store.snapshot.muted">
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
            :value="store.snapshot.volume"
            class="player-range volume-slider"
            aria-label="音量"
            @input="store.setVolume(Number(($event.target as HTMLInputElement).value))"
          />
        </div>

        <div class="ctl-spacer" />


        <!-- 音轨菜单 -->
        <select
          v-if="store.canSwitchAudio && store.snapshot.audioTracks.length > 0"
          class="ctl-select"
          :value="store.snapshot.currentAudioTrackId ?? ''"
          @change="store.snapshot.currentAudioTrackId = ($event.target as HTMLSelectElement).value"
        >
          <option v-for="t in store.snapshot.audioTracks" :key="t.id" :value="t.id">
            {{ t.label }}
          </option>
        </select>

        <!-- 画质菜单 -->
        <select
          v-if="store.hasQualities"
          class="ctl-select"
          :value="store.snapshot.currentQualityId ?? ''"
          @change="store.selectQuality(($event.target as HTMLSelectElement).value)"
        >
          <option v-for="q in store.snapshot.qualities" :key="q.id" :value="q.id">
            {{ q.label }}
          </option>
        </select>

        <!-- 选集按钮（多集时可见） -->
        <button
          v-if="episodeList.length > 1"
          type="button"
          class="ctl-btn episode-toggle-btn"
          title="切换选集"
          aria-label="切换选集"
          @click="toggleEpisodeDrawer"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M4 6h16M4 12h16M4 18h16" />
          </svg>
          <span class="ep-btn-label">选集</span>
        </button>

        <!-- 播放倍速 -->
        <button
          type="button"
          class="ctl-btn speed-btn"
          :title="`播放速度：${currentRate}×`"
          aria-label="切换播放速度"
          @click="cycleRate"
        >
          {{ currentRate }}×
        </button>

        <!-- 全屏切换 -->
        <button
          type="button"
          class="ctl-btn"
          aria-label="全屏"
          title="全屏"
          @click="store.setFullscreen(!isFullscreen)"
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
      </div>
    </div>

    <!-- 自动连播下一集倒计时浮层 -->
    <div
      v-if="autoPlayCountdown !== null && nextEpisode"
      class="player-countdown-overlay card"
    >
      <div class="countdown-badge">✦ 即将连播下一集</div>
      <div class="countdown-title">{{ nextEpisode.name }}</div>
      <div class="countdown-btn-group">
        <button
          type="button"
          class="btn btn-primary btn-sm"
          @click="switchEpisode(nextEpisode)"
        >
          立即播放 ({{ autoPlayCountdown }}s)
        </button>
        <button
          type="button"
          class="btn btn-secondary btn-sm"
          @click="cancelAutoPlay"
        >
          取消
        </button>
      </div>
    </div>

    <!-- 侧滑选集抽屉 -->
    <div
      v-if="showEpisodeDrawer"
      class="player-drawer-backdrop"
      @click.self="showEpisodeDrawer = false"
    >
      <aside class="player-drawer card">
        <div class="drawer-header">
          <div>
            <h3 class="drawer-title">选集列表</h3>
            <span class="drawer-count">共 {{ episodeList.length }} 集</span>
          </div>
          <button
            type="button"
            class="icon-btn drawer-close"
            title="关闭选集列表"
            aria-label="关闭选集列表"
            @click="showEpisodeDrawer = false"
          >
            ✕
          </button>
        </div>
        <div class="drawer-grid">
          <button
            v-for="ep in episodeList"
            :key="ep.id"
            type="button"
            class="drawer-ep-btn"
            :class="{ 'is-active': isCurrentEpisode(ep) }"
            :title="`播放 ${ep.name}`"
            :aria-label="`播放 ${ep.name}`"
            @click="switchEpisode(ep)"
          >
            <span>{{ ep.name }}</span>
            <span v-if="isCurrentEpisode(ep)" class="playing-indicator">▶ 正在播放</span>
          </button>
        </div>
      </aside>
    </div>
  </div>
</template>

<style scoped>
.player-shell {
  position: fixed;
  inset: 0;
  z-index: 50;
  display: flex;
  flex-direction: column;
  background: #000;
  user-select: none;
}

.player-stage-viewport {
  position: relative;
  flex: 1;
  overflow: hidden;
  cursor: pointer;
}

/* 顶部操作条 */
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
  z-index: 5;
}
.player-topbar.is-hidden {
  opacity: 0;
  pointer-events: none;
}
.player-back-btn {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  border-radius: 9999px;
  border: 1px solid rgba(255, 255, 255, 0.15);
  background: rgba(10, 16, 31, 0.7);
  backdrop-filter: blur(12px);
  padding: 7px 16px 7px 12px;
  font-size: 13px;
  font-weight: 600;
  color: #dbe4f6;
  text-decoration: none;
  transition: 0.18s ease;
}
.player-back-btn:hover {
  background: rgba(22, 33, 58, 0.85);
  border-color: rgba(255, 255, 255, 0.3);
  color: #fff;
  transform: translateY(-1px);
}
.player-back-btn svg {
  width: 16px;
  height: 16px;
  stroke: currentColor;
  fill: none;
  stroke-width: 2.2;
  stroke-linecap: round;
  stroke-linejoin: round;
}
.player-strategy-badge {
  font-size: 11px;
  padding: 5px 12px;
  border-radius: 9999px;
  background: rgba(10, 16, 31, 0.7);
  border: 1px solid rgba(255, 255, 255, 0.15);
  color: #98a6c2;
  backdrop-filter: blur(12px);
}

/* 状态浮层 */
.player-recovering-hint,
.player-gesture-tip,
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
.gesture-bubble {
  border-radius: 14px;
  border: 1px solid rgba(255, 255, 255, 0.18);
  background: rgba(0, 0, 0, 0.75);
  backdrop-filter: blur(14px);
  padding: 10px 22px;
  font-size: 15px;
  font-weight: 600;
  color: #fff;
}

/* 错误层 */
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
  padding: 24px;
  text-align: center;
  z-index: 6;
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
  color: var(--muted);
  margin: 0;
}

/* 缓冲旋转 */
.player-spinner {
  width: 48px;
  height: 48px;
  border-radius: 50%;
  border: 3px solid rgba(255, 255, 255, 0.15);
  border-top-color: #987fff;
  animation: spin 0.8s linear infinite;
}

/* 居中大播放圆钮 */
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
.player-center-play .big-play {
  pointer-events: auto;
}

/* 底部悬浮控制台 */
.player-bottom-panel {
  position: absolute;
  inset-inline: 0;
  bottom: 0;
  background: linear-gradient(0deg, rgba(5, 8, 15, 0.98) 0%, rgba(5, 8, 15, 0.75) 60%, transparent 100%);
  padding: 38px 20px 18px;
  transition: opacity 0.25s ease;
  z-index: 5;
}
.player-bottom-panel.is-hidden {
  opacity: 0;
  pointer-events: none;
}

/* 双色发光进度滑轨 */
.timeline-wrapper {
  position: relative;
  height: 16px;
  display: flex;
  align-items: center;
}
.timeline-bar {
  position: absolute;
  inset-inline: 0;
  height: 4px;
  border-radius: 9999px;
  background: rgba(255, 255, 255, 0.2);
  overflow: hidden;
}
.timeline-buffered {
  position: absolute;
  left: 0;
  top: 0;
  bottom: 0;
  background: rgba(255, 255, 255, 0.32);
  border-radius: 9999px;
}
.timeline-played {
  position: absolute;
  left: 0;
  top: 0;
  bottom: 0;
  background: linear-gradient(90deg, #795bff, #6bd2ff);
  border-radius: 9999px;
  transition: width 0.1s linear;
}
.timeline-slider {
  position: relative;
  width: 100%;
  appearance: none;
  background: transparent;
  z-index: 2;
}

/* 控制按钮行 */
.player-ctl-row {
  margin-top: 10px;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 12px;
}
.player-time-display {
  font-family: monospace;
  font-size: 12px;
  color: #cfd8eb;
  display: flex;
  gap: 6px;
  align-items: center;
  margin-left: 2px;
}
.player-time-display .sep {
  color: var(--muted2);
}
.player-time-display .dur {
  color: var(--muted);
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
.speed-btn {
  font-size: 11px;
  font-weight: 800;
}

/* 进度条与范围输入 */
.player-range {
  height: 14px;
  cursor: pointer;
}
.player-range::-webkit-slider-runnable-track {
  height: 4px;
  border-radius: 9999px;
  background: transparent;
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
  background: transparent;
}
.player-range::-moz-range-thumb {
  height: 13px;
  width: 13px;
  border: none;
  border-radius: 9999px;
  background: #ffffff;
}

/* 控制条按钮 */
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
  cursor: pointer;
  transition: 0.18s ease;
  padding: 0;
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

/* 下拉菜单 */
.ctl-select {
  cursor: pointer;
  appearance: none;
  border-radius: 9999px;
  border: 1px solid rgba(255, 255, 255, 0.12);
  background: rgba(255, 255, 255, 0.08);
  padding: 7px 28px 7px 12px;
  font-size: 12px;
  font-weight: 500;
  color: #eaf0ff;
  backdrop-filter: blur(8px);
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
  color: var(--text);
}

/* 选集按钮 */
.episode-toggle-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 0 12px;
  border-radius: 9999px;
  font-size: 12px;
  font-weight: 600;
  color: #e2e8f0;
}
.ep-btn-label {
  font-size: 12px;
}

/* 自动连播倒计时浮层 */
.player-countdown-overlay {
  position: absolute;
  bottom: 96px;
  right: 28px;
  background: rgba(13, 20, 36, 0.92);
  border: 1px solid rgba(87, 200, 245, 0.35);
  border-radius: 12px;
  padding: 16px 20px;
  z-index: 55;
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.65);
  backdrop-filter: blur(14px);
  animation: fadeIn 0.25s ease;
  min-width: 220px;
}
.countdown-badge {
  font-size: 11px;
  color: #57c8f5;
  font-weight: 700;
  margin-bottom: 4px;
}
.countdown-title {
  font-size: 14px;
  font-weight: 600;
  color: #fff;
  margin-bottom: 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.countdown-btn-group {
  display: flex;
  gap: 8px;
}
.btn-sm {
  padding: 6px 12px;
  font-size: 12px;
  border-radius: 6px;
}

/* 侧滑选集抽屉 */
.player-drawer-backdrop {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  z-index: 60;
  display: flex;
  justify-content: flex-end;
  backdrop-filter: blur(4px);
  animation: fadeIn 0.2s ease;
}
.player-drawer {
  width: 320px;
  max-width: 82%;
  height: 100%;
  background: rgba(10, 16, 31, 0.96);
  border-left: 1px solid rgba(255, 255, 255, 0.1);
  padding: 20px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: -10px 0 30px rgba(0, 0, 0, 0.5);
  animation: slideInRight 0.25s ease;
}
.drawer-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding-bottom: 14px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
}
.drawer-title {
  margin: 0;
  font-size: 16px;
  font-weight: 700;
  color: #fff;
}
.drawer-count {
  font-size: 11px;
  color: var(--muted);
}
.drawer-close {
  background: rgba(255, 255, 255, 0.08);
  border: none;
  color: #fff;
  width: 28px;
  height: 28px;
  border-radius: 6px;
  cursor: pointer;
  display: grid;
  place-items: center;
  font-size: 14px;
}
.drawer-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(72px, 1fr));
  gap: 8px;
  overflow-y: auto;
  flex: 1;
  padding: 14px 2px 24px;
}
.drawer-ep-btn {
  min-height: 44px;
  padding: 8px 6px;
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.1);
  color: #e2e8f0;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  text-align: center;
  transition: all 0.15s ease;
}
.drawer-ep-btn:hover {
  background: rgba(87, 200, 245, 0.15);
  border-color: rgba(87, 200, 245, 0.4);
  color: #fff;
  transform: translateY(-1px);
}
.drawer-ep-btn.is-active {
  background: linear-gradient(135deg, rgba(121, 91, 255, 0.4), rgba(87, 200, 245, 0.3));
  border-color: #57c8f5;
  color: #57c8f5;
  box-shadow: 0 0 12px rgba(87, 200, 245, 0.35);
}
.playing-indicator {
  font-size: 9px;
  color: #57c8f5;
  margin-top: 2px;
}

@keyframes slideInRight {
  from { transform: translateX(100%); }
  to { transform: translateX(0); }
}

@keyframes fadeIn {
  from { opacity: 0; }
  to { opacity: 1; }
}

@media (max-width: 640px) {
  .player-countdown-overlay {
    bottom: 80px;
    right: 14px;
    left: 14px;
  }
  .player-drawer {
    width: 280px;
  }
}
</style>
