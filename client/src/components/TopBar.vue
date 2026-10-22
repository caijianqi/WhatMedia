<script setup lang="ts">
/**
 * 顶栏：全局搜索 + 历史 + 通知 + 个人中心胶囊。
 * 1:1 遵循 ui.html 原生规范：
 * - 82px 规范高度
 * - 带 / 快捷键的原生 .searchbox
 * - 历史记录按钮与微光通知按钮 .notify-dot
 * - 个人中心胶囊 .profile（双色渐变头像 + 角色信息 + 展开菜单）
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import * as api from '../api/client'
import { useSessionStore } from '../stores/session'
import VodDetailModal from './VodDetailModal.vue'

const props = withDefaults(defineProps<{ showSearch?: boolean }>(), { showSearch: true })

const router = useRouter()
const session = useSessionStore()

// ── 搜索 ──────────────────────────────────────────────────
const query = ref('')
const results = ref<api.ItemSummary[]>([])
const remoteResults = ref<api.AggregatedVodItem[]>([])
const searching = ref(false)
const open = ref(false)
const selectedVod = ref<api.AggregatedVodItem | null>(null)
const vodModalOpen = ref(false)
let debounce: number | undefined
let inflight: AbortController | null = null
const searchInput = ref<HTMLInputElement | null>(null)

const MIN_QUERY = 1

function schedule(): void {
  if (debounce) window.clearTimeout(debounce)
  debounce = window.setTimeout(run, 250)
}

async function run(): Promise<void> {
  const q = query.value.trim()
  if (q.length < MIN_QUERY) {
    results.value = []
    remoteResults.value = []
    searching.value = false
    return
  }
  if (inflight) inflight.abort()
  inflight = new AbortController()
  searching.value = true
  try {
    const res = await api.searchAggregate(q, { page_size: 6 })
    if (!inflight.signal.aborted) {
      results.value = res.local.data
      remoteResults.value = res.remote
      open.value = true
    }
  } catch {
    // 降级回退到本地普通搜索
    try {
      const page = await api.search(q, { page_size: 6 })
      if (!inflight.signal.aborted) {
        results.value = page.data
        remoteResults.value = []
        open.value = true
      }
    } catch {
      results.value = []
      remoteResults.value = []
    }
  } finally {
    searching.value = false
  }
}

watch(query, schedule)

function pick(item: api.ItemSummary): void {
  open.value = false
  query.value = ''
  results.value = []
  remoteResults.value = []
  void router.push({ name: 'item', params: { id: item.id } })
}

function pickVod(vod: api.AggregatedVodItem): void {
  open.value = false
  selectedVod.value = vod
  vodModalOpen.value = true
}

function playVodEpisode(payload: {
  item: api.AggregatedVodItem
  route: api.VodRoute
  episode: api.VodEpisode
}): void {
  vodModalOpen.value = false
  const proxyUrl = api.getVodStreamProxyUrl(payload.episode.play_url, payload.route.source_id)
  void router.push({
    name: 'play',
    params: { id: 'vod' },
    query: {
      vod_url: proxyUrl,
      vod_title: `${payload.item.title} - ${payload.episode.name}`,
      vod_source: payload.route.source_name,
    },
  })
}

function submit(): void {
  open.value = false
  void router.push({ name: 'library' })
}

// ── 通知 ──────────────────────────────────────────────────
const unread = ref(0)
const notes = ref<api.NotificationRow[]>([])
const pendingApprovals = ref(0)
const notesOpen = ref(false)
const profileOpen = ref(false)
let timer: number | undefined

async function loadNotes(): Promise<void> {
  try {
    const r = await api.listNotifications({ limit: 20 })
    unread.value = r.unread
    notes.value = r.data
    pendingApprovals.value = r.pending_approvals
  } catch {
    unread.value = 0
  }
}

async function markRead(id: string): Promise<void> {
  try {
    await api.readNotification(id)
    await loadNotes()
  } catch {
    /* 静默失败 */
  }
}

async function markAll(): Promise<void> {
  try {
    await api.readAllNotifications()
    await loadNotes()
  } catch {
    /* 静默失败 */
  }
}

function onGlobalKeyDown(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    open.value = false
    notesOpen.value = false
    profileOpen.value = false
  }
  if (e.key === '/') {
    const tag = (e.target as HTMLElement | null)?.tagName
    const isEditing = tag === 'INPUT' || tag === 'TEXTAREA' || (e.target as HTMLElement | null)?.isContentEditable
    if (!isEditing) {
      e.preventDefault()
      searchInput.value?.focus()
    }
  }
}

onMounted(() => {
  void loadNotes()
  timer = window.setInterval(() => void loadNotes(), 60_000)
  window.addEventListener('keydown', onGlobalKeyDown)
})

onBeforeUnmount(() => {
  if (timer) window.clearInterval(timer)
  if (debounce) window.clearTimeout(debounce)
  if (inflight) inflight.abort()
  window.removeEventListener('keydown', onGlobalKeyDown)
})

const ROLE_LABEL: Record<string, string> = { admin: '系统管理员', user: '普通成员', child: '儿童空间' }

const displayName = computed(() => session.user?.display_name || session.user?.username || '影音用户')
const roleLabel = computed(() => ROLE_LABEL[session.user?.role ?? 'user'] || '个人空间')
const userInitials = computed(() => {
  const name = session.user?.username || 'WM'
  return name.slice(0, 2).toUpperCase()
})

async function signOut(): Promise<void> {
  profileOpen.value = false
  await session.signOut(false)
  void router.push({ name: 'login' })
}

function fmtTime(iso: string): string {
  const d = new Date(iso)
  const diff = Date.now() - d.getTime()
  if (diff < 60_000) return '刚刚'
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`
  return d.toLocaleDateString('zh-CN')
}
</script>

<template>
  <header class="topbar">
    <!-- 移动端汉堡菜单 -->
    <button class="icon-btn mobile-menu" id="mobileMenu" aria-label="打开导航">
      <svg viewBox="0 0 24 24">
        <path d="M4 6h16M4 12h16M4 18h16" />
      </svg>
    </button>

    <!-- 原生全局搜索 -->
    <div v-if="props.showSearch" class="search-container">
      <label class="searchbox">
        <svg viewBox="0 0 24 24">
          <circle cx="10.8" cy="10.8" r="6.8" />
          <path d="m16 16 4.5 4.5" />
        </svg>
        <input
          ref="searchInput"
          v-model="query"
          type="search"
          placeholder="搜索电影、电视剧、演员…"
          autocomplete="off"
          aria-label="搜索"
          @focus="open = results.length > 0"
          @keydown.enter="submit"
        />
        <span class="search-shortcut">/</span>
      </label>

      <!-- 搜索结果下拉浮层 -->
      <div v-if="open && query.trim().length >= MIN_QUERY" class="search-dropdown card">
        <!-- 本地媒体库结果 -->
        <div v-if="results.length > 0" class="search-section">
          <div class="search-section-title">本地媒体库 (原画)</div>
          <button
            v-for="r in results"
            :key="r.id"
            type="button"
            class="search-item"
            @click="pick(r)"
          >
            <span class="chip-local">本地</span>
            <span class="chip">{{ r.year ?? '—' }}</span>
            <span class="search-title">{{ r.title }}</span>
          </button>
        </div>

        <!-- 云端多源聚合结果 (MoonTV 即播) -->
        <div v-if="remoteResults.length > 0" class="search-section">
          <div class="search-section-title">云端聚合在线源 (MoonTV 秒播)</div>
          <button
            v-for="r in remoteResults"
            :key="r.id"
            type="button"
            class="search-item search-item-remote"
            @click="pickVod(r)"
          >
            <span class="chip-remote">全网</span>
            <span class="chip-routes">{{ r.routes.length }} 线路</span>
            <span class="search-title">{{ r.title }}</span>
            <span v-if="r.remark" class="search-remark">{{ r.remark }}</span>
          </button>
        </div>

        <p v-if="searching" class="search-tip">跨源检索中…</p>
        <p v-else-if="results.length === 0 && remoteResults.length === 0" class="search-tip">
          没有匹配「{{ query.trim() }}」的条目
        </p>
      </div>
    </div>

    <!-- 聚合影视选集与详情弹窗 -->
    <VodDetailModal
      :open="vodModalOpen"
      :item="selectedVod"
      @close="vodModalOpen = false"
      @play="playVodEpisode"
    />

    <div class="top-spacer"></div>

    <!-- 观看历史 -->
    <button
      class="icon-btn"
      id="historyBtn"
      aria-label="观看历史"
      title="观看历史"
      @click="router.push({ name: 'home' })"
    >
      <svg viewBox="0 0 24 24">
        <path d="M3 12a9 9 0 1 0 2.5-6.2L3 8" />
        <path d="M3 3v5h5M12 7v5l3.5 2" />
      </svg>
    </button>

    <!-- 消息通知 -->
    <div class="popover-anchor">
      <button
        class="icon-btn"
        id="noticeBtn"
        aria-label="通知"
        title="通知"
        @click="notesOpen = !notesOpen"
      >
        <svg viewBox="0 0 24 24">
          <path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9M10 21h4" />
        </svg>
        <!-- 原型微光小紫点与未读徽标（1:1 对齐 ui.html） -->
        <span class="notify-dot"></span>
        <span v-if="unread > 0" class="notify-count-badge">
          {{ unread > 99 ? '99+' : unread }}
        </span>
      </button>

      <!-- 通知浮层 -->
      <div v-if="notesOpen" class="notes-dropdown card">
        <div class="notes-header">
          <span style="font-weight:650;font-size:13px">通知</span>
          <button v-if="unread > 0" type="button" class="text-btn" @click="markAll">
            全部已读
          </button>
        </div>

        <RouterLink
          v-if="pendingApprovals > 0"
          to="/settings"
          class="approval-banner"
          @click="notesOpen = false"
        >
          <span class="status-dot" style="background:#f59e0b"></span>
          有 {{ pendingApprovals }} 台设备等待家长批准
        </RouterLink>

        <div class="notes-list">
          <div
            v-for="n in notes"
            :key="n.id"
            class="note-item"
            :class="{ 'is-read': n.read_at }"
          >
            <div style="flex:1;min-width:0">
              <strong class="note-title">{{ n.title }}</strong>
              <p v-if="n.body" class="note-body">{{ n.body }}</p>
              <span class="note-time">{{ fmtTime(n.created_at) }}</span>
            </div>
            <button
              v-if="!n.read_at"
              type="button"
              class="text-btn mark-read-btn"
              @click="markRead(n.id)"
            >
              标为已读
            </button>
          </div>

          <p v-if="notes.length === 0" class="notes-empty">
            还没有通知。媒体扫描与元数据刮削完成后会在此汇总。
          </p>
        </div>
      </div>
    </div>

    <!-- 个人中心胶囊 -->
    <div class="popover-anchor">
      <button
        class="profile"
        id="profileBtn"
        aria-label="个人中心"
        @click="profileOpen = !profileOpen"
      >
        <span class="avatar">{{ userInitials }}</span>
        <span class="profile-info">
          <strong>{{ displayName }}</strong>
          <span>{{ roleLabel }}</span>
        </span>
        <span style="color:#8290ab;font-size:10px">⌄</span>
      </button>

      <!-- 个人菜单下拉 -->
      <div v-if="profileOpen" class="profile-dropdown card">
        <RouterLink to="/settings" class="dropdown-item" @click="profileOpen = false">
          设置中心
        </RouterLink>
        <button type="button" class="dropdown-item" @click="signOut">
          退出登录
        </button>
      </div>
    </div>
  </header>
</template>

<style scoped>
.mobile-menu {
  display: none;
}
@media (max-width: 620px) {
  .mobile-menu {
    display: grid;
  }
}

.search-container {
  position: relative;
  max-width: 485px;
  flex: 1;
}

.search-dropdown {
  position: absolute;
  top: calc(100% + 8px);
  left: 0;
  right: 0;
  z-index: 40;
  max-height: 380px;
  overflow-y: auto;
  padding: 6px;
}

.search-item {
  width: 100%;
  border: 0;
  background: transparent;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border-radius: 8px;
  text-align: left;
  transition: background-color 0.15s;
}
.search-item:hover {
  background: rgba(255, 255, 255, 0.06);
}
.search-title {
  font-size: 13px;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.search-tip {
  padding: 14px 10px;
  font-size: 11px;
  color: var(--muted);
  text-align: center;
  margin: 0;
}

.popover-anchor {
  position: relative;
}

.notify-count-badge {
  position: absolute;
  top: 4px;
  right: 4px;
  min-width: 15px;
  height: 15px;
  padding: 0 4px;
  border-radius: 9999px;
  background: var(--purple);
  color: white;
  font-size: 9px;
  font-weight: 700;
  line-height: 15px;
  text-align: center;
}

.notes-dropdown {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  width: 320px;
  z-index: 40;
  overflow: hidden;
}

.notes-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 10px 14px;
  border-bottom: 1px solid var(--line);
}

.approval-banner {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 9px 14px;
  background: rgba(245, 158, 11, 0.1);
  border-bottom: 1px solid var(--line);
  font-size: 11px;
  color: #fcd34d;
  text-decoration: none;
}

.notes-list {
  max-height: 320px;
  overflow-y: auto;
}

.note-item {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 10px 14px;
  border-bottom: 1px solid rgba(174, 193, 226, 0.06);
}
.note-item:last-child {
  border-bottom: 0;
}
.note-item.is-read {
  opacity: 0.6;
}

.note-title {
  display: block;
  font-size: 12px;
  color: var(--text);
}
.note-body {
  font-size: 11px;
  color: var(--muted);
  margin: 3px 0 0;
  line-height: 1.5;
}
.note-time {
  display: block;
  font-size: 10px;
  color: var(--muted2);
  margin-top: 4px;
}
.mark-read-btn {
  padding: 2px 4px;
  font-size: 10px;
  flex-shrink: 0;
}

.notes-empty {
  padding: 30px 14px;
  text-align: center;
  font-size: 11px;
  color: var(--muted);
  margin: 0;
}

.profile-dropdown {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  width: 140px;
  z-index: 40;
  padding: 6px;
}
.dropdown-item {
  display: block;
  width: 100%;
  border: 0;
  background: transparent;
  padding: 8px 12px;
  border-radius: 8px;
  font-size: 12px;
  color: var(--text);
  text-align: left;
  cursor: pointer;
  text-decoration: none;
  transition: background-color 0.15s;
}
.dropdown-item:hover {
  background: rgba(255, 255, 255, 0.08);
}

.search-section {
  padding: 4px 0;
  border-bottom: 1px solid rgba(255, 255, 255, 0.06);
}
.search-section:last-child {
  border-bottom: 0;
}
.search-section-title {
  padding: 6px 12px 2px;
  font-size: 10px;
  font-weight: 700;
  text-transform: uppercase;
  color: var(--muted);
  letter-spacing: 0.5px;
}
.chip-local {
  background: rgba(168, 85, 247, 0.2);
  color: #c084fc;
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 4px;
  font-weight: 600;
  flex-shrink: 0;
}
.chip-remote {
  background: rgba(245, 158, 11, 0.2);
  color: #fbbf24;
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 4px;
  font-weight: 600;
  flex-shrink: 0;
}
.chip-routes {
  background: rgba(59, 130, 246, 0.18);
  color: #60a5fa;
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 4px;
  font-weight: 500;
  flex-shrink: 0;
}
.search-item-remote {
  display: flex;
  align-items: center;
  gap: 8px;
}
.search-remark {
  margin-left: auto;
  font-size: 11px;
  color: var(--muted2);
  white-space: nowrap;
}
</style>
