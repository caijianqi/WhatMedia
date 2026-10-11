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
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useRoute } from 'vue-router'
import { usePlayerStore } from '../stores/player'

const route = useRoute()
const store = usePlayerStore()

const itemId = computed(() => String(route.params.id))
const startMs = computed(() => Number(route.query.start ?? 0))

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
}

const position = computed(() => (seeking.value ? dragValue.value : store.snapshot.position))
const duration = computed(() => store.snapshot.duration || 0)

const isPlaying = computed(() => store.snapshot.state === 'playing')

// ── 生命周期 ────────────────────────────────────────────────

onMounted(async () => {
  if (!stage.value) return
  await store.open({ container: stage.value, fullscreenTarget: shell.value ?? undefined }, itemId.value, {
    startMs: startMs.value,
    autoplay: true,
  })
  window.addEventListener('keydown', onKeyDown)
  window.addEventListener('pagehide', onPageHide)
  document.addEventListener('fullscreenchange', onFullscreenChange)
  armHideTimer()
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeyDown)
  window.removeEventListener('pagehide', onPageHide)
  document.removeEventListener('fullscreenchange', onFullscreenChange)
  if (hideTimer) window.clearTimeout(hideTimer)
  void store.close()
})

function onPageHide() {
  store.flushBeforeUnload()
}

// ── 控制条显隐 ──────────────────────────────────────────────

function armHideTimer() {
  if (hideTimer) window.clearTimeout(hideTimer)
  controlsVisible.value = true
  hideTimer = window.setTimeout(() => {
    // 播放中才隐藏；暂停时用户多半在操作，藏起来反而烦。
    if (isPlaying.value) controlsVisible.value = false
  }, 2500)
}

function onPointerMove() {
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
    class="fixed inset-0 z-50 flex flex-col bg-black"
    @pointermove="onPointerMove"
  >
    <!-- 画面 -->
    <div ref="stage" class="relative flex-1 overflow-hidden" @click="togglePlay" />

    <!-- 顶部条 -->
    <div
      class="absolute inset-x-0 top-0 flex items-center justify-between p-4 transition-opacity duration-200"
      :class="controlsVisible ? 'opacity-100' : 'pointer-events-none opacity-0'"
    >
      <RouterLink
        :to="{ name: 'item', params: { id: itemId } }"
        class="focus-ring rounded bg-black/50 px-3 py-1.5 text-sm text-slate-200 backdrop-blur hover:bg-black/70"
      >
        ← 返回
      </RouterLink>

      <div class="flex items-center gap-2 text-xs">
        <span
          v-if="store.strategy"
          class="rounded bg-black/50 px-2 py-1 text-slate-300 backdrop-blur"
        >
          {{ store.strategy }}
        </span>
      </div>
    </div>

    <!-- 错误 -->
    <div
      v-if="store.error"
      class="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-black/85 p-8 text-center"
    >
      <p class="text-lg text-red-300">{{ store.error }}</p>
      <p v-if="store.reasons.length" class="max-w-md text-sm text-slate-400">
        {{ store.reasons.map((r) => r.message).join('；') }}
      </p>
      <RouterLink
        :to="{ name: 'item', params: { id: itemId } }"
        class="focus-ring rounded bg-ink-800 px-4 py-2 text-sm text-slate-100"
      >
        返回详情
      </RouterLink>
    </div>

    <!-- 缓冲指示 -->
    <div
      v-else-if="store.snapshot.state === 'buffering'"
      class="pointer-events-none absolute inset-0 flex items-center justify-center"
    >
      <div class="h-10 w-10 animate-spin rounded-full border-2 border-white/20 border-t-white/80" />
    </div>

    <!-- 底部控制条 -->
    <div
      class="absolute inset-x-0 bottom-0 bg-gradient-to-t from-black/90 to-transparent px-4 pb-4 pt-10 transition-opacity duration-200"
      :class="controlsVisible ? 'opacity-100' : 'pointer-events-none opacity-0'"
    >
      <!-- 进度条 -->
      <div class="relative">
        <div class="absolute inset-x-0 top-1/2 h-1 -translate-y-1/2 rounded bg-white/20">
          <div class="h-full rounded bg-white/30" :style="{ width: bufferedPct + '%' }" />
        </div>
        <input
          type="range"
          min="0"
          :max="duration || 0"
          step="0.1"
          :value="position"
          class="focus-ring relative w-full appearance-none bg-transparent accent-brand"
          aria-label="播放进度"
          @pointerdown="seeking = true"
          @input="onSeekInput"
          @change="commitSeek"
        />
      </div>

      <div class="mt-2 flex flex-wrap items-center gap-3">
        <button
          class="focus-ring rounded px-3 py-1.5 text-sm text-slate-100 hover:bg-white/10"
          @click="togglePlay"
        >
          {{ isPlaying ? '暂停' : '播放' }}
        </button>

        <span class="font-mono text-xs text-slate-300">
          {{ fmt(position) }} / {{ fmt(duration) }}
        </span>

        <!-- 音量 -->
        <div class="flex items-center gap-2">
          <button
            class="focus-ring rounded px-2 py-1 text-sm hover:bg-white/10"
            @click="store.setMuted(!store.snapshot.muted)"
          >
            {{ store.snapshot.muted ? '🔇' : '🔊' }}
          </button>
          <input
            type="range"
            min="0"
            max="1"
            step="0.01"
            :value="store.snapshot.volume"
            class="focus-ring w-20 accent-brand"
            aria-label="音量"
            @input="store.setVolume(Number(($event.target as HTMLInputElement).value))"
          />
        </div>

        <!-- 字幕菜单：只有真能切的时候才出现 -->
        <select
          v-if="store.canSwitchSubtitle && store.snapshot.subtitleTracks.length > 0"
          class="focus-ring rounded bg-white/10 px-2 py-1 text-sm text-slate-100"
          :value="store.snapshot.currentSubtitleTrackId ?? ''"
          @change="
            store.selectSubtitle(($event.target as HTMLSelectElement).value || null)
          "
        >
          <option value="">关闭字幕</option>
          <option v-for="t in store.snapshot.subtitleTracks" :key="t.id" :value="t.id">
            {{ t.label }}
          </option>
        </select>

        <!-- 音轨菜单：Web 端 canSwitchAudioTrack 为 false，根本不会渲染 -->
        <select
          v-if="store.canSwitchAudio && store.snapshot.audioTracks.length > 0"
          class="focus-ring rounded bg-white/10 px-2 py-1 text-sm text-slate-100"
          :value="store.snapshot.currentAudioTrackId ?? ''"
          @change="store.snapshot.currentAudioTrackId = ($event.target as HTMLSelectElement).value"
        >
          <option v-for="t in store.snapshot.audioTracks" :key="t.id" :value="t.id">
            {{ t.label }}
          </option>
        </select>

        <!-- 画质菜单：单码率时没有意义 -->
        <select
          v-if="store.hasQualities"
          class="focus-ring rounded bg-white/10 px-2 py-1 text-sm text-slate-100"
          :value="store.snapshot.currentQualityId ?? ''"
          @change="store.selectQuality(($event.target as HTMLSelectElement).value)"
        >
          <option v-for="q in store.snapshot.qualities" :key="q.id" :value="q.id">
            {{ q.label }}
          </option>
        </select>

        <button
          class="focus-ring ml-auto rounded px-3 py-1.5 text-sm text-slate-100 hover:bg-white/10"
          @click="store.setFullscreen(!isFullscreen)"
        >
          全屏
        </button>
      </div>
    </div>
  </div>
</template>
