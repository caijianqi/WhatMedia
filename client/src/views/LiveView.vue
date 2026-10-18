<script setup lang="ts">
/**
 * 电视直播。
 *
 * 与点播页最大的三处不同，都体现在这个组件里：
 *
 * 1. **没有进度条**。直播的"进度"是时间轴上的绝对位置，不是片内的相对位置，
 *    拖进度条没有意义（L1 的时移做出来之前只能看实时边缘）。
 * 2. **播放器要显式"离开频道"**。点播关掉页面进程就没了，直播的 ffmpeg 是
 *    服务端长驻的，浏览器关了没人通知服务端 —— 所以卸载前必须调一次 stop。
 * 3. **同一个频道被多人点播时复用一路流**，所以这里显示的 `viewers` 可能大于 1，
 *    那是正常状态而不是异常。
 */
import { computed, onBeforeUnmount, onMounted, ref, shallowRef } from 'vue'
import Hls from 'hls.js'
import * as api from '../api/client'
import type { LiveChannel, LiveSource } from '../api/client'

const channels = ref<LiveChannel[]>([])
const sources = ref<LiveSource[]>([])
const loading = ref(true)
const error = ref<string | null>(null)

// 源管理
const showSources = ref(false)
const newName = ref('')
const newUrl = ref('')
const newEpg = ref('')
const busy = ref(false)
const notice = ref<string | null>(null)

// 播放
const current = ref<LiveChannel | null>(null)
const video = ref<HTMLVideoElement | null>(null)
const hls = shallowRef<Hls | null>(null)
const starting = ref(false)

const groups = computed(() => {
  const set = new Set<string>()
  for (const c of channels.value) if (c.group) set.add(c.group)
  return ['全部', ...Array.from(set).sort()]
})
const activeGroup = ref('全部')

const visible = computed(() =>
  activeGroup.value === '全部'
    ? channels.value
    : channels.value.filter((c) => c.group === activeGroup.value),
)

async function reload() {
  loading.value = true
  error.value = null
  try {
    const [ch, src] = await Promise.all([api.listLiveChannels(), api.listLiveSources()])
    channels.value = ch
    sources.value = src
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

async function addSource() {
  const name = newName.value.trim()
  const url = newUrl.value.trim()
  if (!name || !url) {
    notice.value = '名称和订阅地址都要填'
    return
  }
  busy.value = true
  notice.value = null
  try {
    const s = await api.createLiveSource({
      name,
      kind: 'm3u',
      url,
      ...(newEpg.value.trim() ? { epg_url: newEpg.value.trim() } : {}),
    })
    newName.value = ''
    newUrl.value = ''
    newEpg.value = ''
    await sync(s.id)
    await reload()
  } catch (e) {
    notice.value = e instanceof Error ? e.message : String(e)
  } finally {
    busy.value = false
  }
}

async function sync(id: string) {
  busy.value = true
  notice.value = null
  try {
    const r = await api.syncLiveSource(id)
    notice.value = `同步完成，导入 ${r.channels} 个频道`
  } catch (e) {
    // 同步失败是最常见的用户错误（地址贴错、源站要鉴权），
    // 把服务端那句人话直接显示出来，别自己再编一句。
    notice.value = e instanceof Error ? e.message : String(e)
  } finally {
    busy.value = false
  }
}

async function removeSource(id: string) {
  busy.value = true
  try {
    await api.deleteLiveSource(id)
    await reload()
  } finally {
    busy.value = false
  }
}

function teardown() {
  if (hls.value) {
    hls.value.destroy()
    hls.value = null
  }
  if (video.value) video.value.removeAttribute('src')
}

async function play(ch: LiveChannel) {
  // 换台：先停掉上一个频道的引用，否则服务端会一直挂着拉流。
  if (current.value) {
    teardown()
    await api.stopLiveChannel(current.value.id).catch(() => undefined)
  }
  starting.value = true
  error.value = null
  current.value = ch
  try {
    const p = await api.playLiveChannel(ch.id)
    // 等 DOM 上的 video 就绪（current 变了才渲染出来）
    await new Promise((r) => requestAnimationFrame(r))
    const el = video.value
    if (!el) return
    if (Hls.isSupported()) {
      const h = new Hls({
        // 直播要靠**实时边缘**起播，不是从头：
        // 默认 liveSyncDurationCount 是 3，已经够靠近边缘了。
        liveSyncDurationCount: 3,
        // 起播失败多半是源在冷启动，让它自己重试几轮。
        manifestLoadingRetryDelay: 500,
        manifestLoadingMaxRetry: 4,
      })
      h.loadSource(p.playlist_url)
      h.attachMedia(el)
      h.on(Hls.Events.ERROR, (_e, data) => {
        if (data.fatal) error.value = '播放中断，正在重试…'
      })
      hls.value = h
    } else if (el.canPlayType('application/vnd.apple.mpegurl')) {
      // Safari / iOS 原生就吃 m3u8
      el.src = p.playlist_url
    } else {
      error.value = '这个浏览器不支持 HLS 播放'
      return
    }
    await el.play().catch(() => undefined)
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    starting.value = false
  }
}

onMounted(reload)

// 关页面 / 换路由时必须释放：服务端的 ffmpeg 是长驻的，
// 浏览器关了不会有人替我们通知它。
onBeforeUnmount(async () => {
  teardown()
  if (current.value) await api.stopLiveChannel(current.value.id).catch(() => undefined)
})
</script>

<template>
  <div class="space-y-6">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <div>
        <h1 class="section-title">电视直播</h1>
        <p class="mt-1 text-sm text-wm-muted">
          频道不是媒体资产：没有时长、没有"看完"，所以它们不进海报墙，也不进继续观看。
        </p>
      </div>
      <button class="btn-secondary" data-test="toggle-sources" @click="showSources = !showSources">
        {{ showSources ? '收起' : '管理订阅源' }}
      </button>
    </div>

    <!-- 源管理 -->
    <section v-if="showSources" class="card p-4" data-test="sources">
      <h2 class="mb-3 text-sm font-medium text-wm-sub">订阅源</h2>

      <ul v-if="sources.length" class="mb-4 space-y-2">
        <li
          v-for="s in sources"
          :key="s.id"
          class="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-lg border border-ink-800 bg-ink-850/50 px-3 py-2 text-sm"
        >
          <span class="font-medium text-wm-sub">{{ s.name }}</span>
          <span class="chip">{{ s.kind }}</span>
          <span class="text-xs text-wm-muted">{{ s.channel_count }} 个频道</span>
          <span v-if="s.sync_error" class="text-xs text-amber-400">{{ s.sync_error }}</span>
          <span class="ml-auto flex gap-2">
            <button class="btn-ghost !px-2 !py-1 !text-xs" @click="sync(s.id)">同步</button>
            <button
              class="btn-ghost !px-2 !py-1 !text-xs hover:!text-red-300"
              @click="removeSource(s.id)"
            >
              删除
            </button>
          </span>
        </li>
      </ul>
      <p v-else class="mb-4 text-sm text-wm-muted">
        还没有订阅源。贴一个 M3U 地址（本地路径也行），同步后频道会出现在下面。
      </p>

      <div class="grid gap-2 sm:grid-cols-[1fr_2fr_2fr_auto]">
        <input
          v-model="newName"
          class="focus-ring rounded-lg border border-wm-line-strong bg-ink-850 px-3 py-2 text-sm text-wm-text"
          placeholder="名称，如「央视 + 卫视」"
          data-test="source-name"
        />
        <input
          v-model="newUrl"
          class="focus-ring rounded-lg border border-wm-line-strong bg-ink-850 px-3 py-2 text-sm text-wm-text"
          placeholder="M3U 地址或本地路径"
          data-test="source-url"
        />
        <input
          v-model="newEpg"
          class="focus-ring rounded-lg border border-wm-line-strong bg-ink-850 px-3 py-2 text-sm text-wm-text"
          placeholder="节目单地址（可选）"
        />
        <button class="btn-primary" :disabled="busy" data-test="add-source" @click="addSource">
          添加并同步
        </button>
      </div>
      <p v-if="notice" class="mt-2 text-xs text-wm-muted" data-test="notice">{{ notice }}</p>
    </section>

    <!-- 播放器 -->
    <section v-if="current" class="card overflow-hidden" data-test="player">
      <div class="relative aspect-video bg-black">
        <video
          ref="video"
          class="h-full w-full"
          playsinline
          controls
          autoplay
        />
        <span
          class="absolute left-3 top-3 flex items-center gap-1.5 rounded bg-red-600/90 px-2 py-0.5 text-xs font-semibold text-white"
        >
          <span class="h-1.5 w-1.5 animate-pulse rounded-full bg-white" />
          LIVE
        </span>
        <div
          v-if="starting"
          class="absolute inset-0 flex items-center justify-center bg-black/60 text-sm text-wm-sub"
        >
          正在拉流…
        </div>
      </div>
      <div class="flex items-center justify-between gap-3 px-4 py-3">
        <div class="min-w-0">
          <p class="truncate font-medium text-wm-text">{{ current.name }}</p>
          <p v-if="current.now_playing" class="truncate text-sm text-wm-muted">
            {{ current.now_playing }}
          </p>
        </div>
        <button
          class="btn-secondary shrink-0"
          data-test="stop"
          @click="
            async () => {
              if (current) await api.stopLiveChannel(current.id).catch(() => undefined)
              teardown()
              current = null
            }
          "
        >
          停止
        </button>
      </div>
    </section>

    <p v-if="error" class="rounded-lg bg-red-950/60 px-3 py-2 text-sm text-red-300" data-test="error">
      {{ error }}
    </p>

    <!-- 分组 -->
    <div v-if="groups.length > 1" class="flex flex-wrap gap-2">
      <button
        v-for="g in groups"
        :key="g"
        class="rounded-full px-3 py-1 text-sm transition"
        :class="
          activeGroup === g
            ? 'bg-gradient-to-r from-brand to-accent text-white'
            : 'bg-ink-850 text-wm-muted hover:text-wm-sub'
        "
        @click="activeGroup = g"
      >
        {{ g }}
      </button>
    </div>

    <!-- 频道网格 -->
    <div v-if="loading" class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
      <div v-for="i in 6" :key="i" class="skeleton h-20" />
    </div>

    <div
      v-else-if="!channels.length"
      class="empty-state"
      data-test="empty"
    >
      <p class="text-wm-sub">还没有频道</p>
      <p class="text-sm text-wm-muted">先添加一个 M3U 订阅源，同步后这里会出现频道列表。</p>
    </div>

    <div v-else class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
      <button
        v-for="c in visible"
        :key="c.id"
        class="card-hover flex items-center gap-3 p-3 text-left"
        :class="current?.id === c.id ? 'border-brand/60' : ''"
        data-test="channel"
        @click="play(c)"
      >
        <img
          v-if="c.logo_url"
          :src="c.logo_url"
          :alt="c.name"
          class="h-10 w-10 shrink-0 rounded object-contain bg-ink-850"
          @error="($event.target as HTMLImageElement).style.display = 'none'"
        />
        <span
          v-else
          class="grid h-10 w-10 shrink-0 place-items-center rounded bg-ink-850 text-sm font-semibold text-wm-muted"
        >
          {{ c.number ?? '·' }}
        </span>
        <span class="min-w-0 flex-1">
          <span class="block truncate text-sm font-medium text-wm-text">{{ c.name }}</span>
          <span class="block truncate text-xs text-wm-muted">
            {{ c.now_playing ?? c.group ?? '—' }}
          </span>
        </span>
        <span v-if="c.viewers > 0" class="chip !px-1.5 !py-0 !text-[10px]">
          {{ c.viewers }} 人在看
        </span>
      </button>
    </div>
  </div>
</template>
