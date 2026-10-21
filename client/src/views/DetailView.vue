<script setup lang="ts">
/**
 * 条目详情页。
 *
 * **这里只做决策预览（`decidePlayback`），不建会话。**
 * 建会话有副作用 —— 会启动 ffmpeg 转码任务。
 * 如果详情页一打开就建会话，那用户只是点进来看看也会在服务端留下一路转码。
 * 真正的会话在播放器页创建。
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import EpisodeList from '../components/EpisodeList.vue'
import { toast } from '../stores/toast'
import * as api from '../api/client'
import type { CreditsView, DecideView, ItemDetail, SeriesRef } from '../api/client'
import { hdrLabel, isHdr } from '../lib/hdr'

const route = useRoute()
const id = computed(() => String(route.params.id))

const detail = ref<ItemDetail | null>(null)
const decision = ref<DecideView | null>(null)
const resumeMs = ref<number | null>(null)
const loading = ref(true)
const error = ref<string | null>(null)

/*
 * 演职员与季列表是**附属数据**：它们只在详情页用到，
 * 而且各自失败都不该让整个详情页打不开 —— 所以三次请求互相独立，
 * 各自 catch，缺失时对应的区块直接不渲染。
 */
const credits = ref<CreditsView | null>(null)
const series = ref<SeriesRef | null>(null)

/*
 * 收藏状态。同样属于附属数据：端点挂了不该让详情页打不开，
 * 所以失败时 favError 置起来、心形按钮不渲染 —— 而不是页面报错。
 */
const favorite = ref(false)
const favoriteCount = ref(0)
const favAvailable = ref(false)

async function loadFavorite(): Promise<void> {
  try {
    const r = await api.getFavorite(id.value)
    favorite.value = r.favorite
    favoriteCount.value = r.count
    favAvailable.value = true
  } catch {
    favAvailable.value = false
  }
}

async function toggleFavorite(): Promise<void> {
  // 先翻本地再发请求：点一下必须立刻有反馈，等服务端往返会觉得"没点上"。
  const next = !favorite.value
  favorite.value = next
  favoriteCount.value += next ? 1 : -1
  try {
    const r = await api.setFavorite(id.value, next)
    // 以服务端返回的为准：count 是全站计数，别人可能同时改过。
    favorite.value = r.favorite
    favoriteCount.value = r.count
    toast(r.favorite ? '已添加到收藏' : '已从收藏中移除', { type: 'info' })
  } catch {
    // 回滚。写失败就当没点过，比留一个"看起来收了其实没有"的状态好。
    favorite.value = !next
    favoriteCount.value += next ? -1 : 1
    toast('收藏状态更新失败', { type: 'error' })
  }
}

async function load() {
  loading.value = true
  error.value = null
  try {
    const [d, dec, res] = await Promise.all([
      api.getItem(id.value),
      api.decidePlayback({ item_id: id.value, client: 'web' }),
      api.resumeList(50).catch(() => [] as api.ResumePoint[]),
    ])
    detail.value = d
    decision.value = dec
    resumeMs.value = res.find((r) => r.item_id === id.value)?.position_ms ?? null
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }

// 附属数据并行加载，失败静默：没有演职员不是错误，是"还没刮削"。
  const [c, s] = await Promise.all([
    api.itemCredits(id.value).catch(() => null),
    api.itemSeries(id.value).catch(() => null),
    loadFavorite(),
  ])
  credits.value = c
  series.value = s
  posterError.value = false
}

const rescraping = ref(false)

async function rescrapeCurrent(): Promise<void> {
  if (!detail.value) return
  rescraping.value = true
  try {
    const raw = detail.value as Record<string, unknown>
    const provider = typeof raw.provider === 'string' ? raw.provider : null
    const providerId = typeof raw.provider_id === 'string' ? raw.provider_id : null
    const libId = typeof raw.library_id === 'string' ? raw.library_id : undefined
    if (provider && providerId) {
      await api.identify(id.value, {
        provider,
        provider_id: providerId,
      })
      toast('元数据与海报刷新成功', { type: 'info' })
    } else {
      await api.startScrape({
        library_id: libId,
        force: true,
      } as any)
      toast('已提交强制重新刮削任务，后台拉取中...', { type: 'info' })
    }
    posterError.value = false
    await load()
  } catch (err) {
    toast(`重新刮削失败：${err instanceof Error ? err.message : String(err)}`, { type: 'error' })
  } finally {
    rescraping.value = false
  }
}

onMounted(load)
watch(id, load)

const backdrop = computed(() => `/api/v1/items/${id.value}/images/backdrop`)
const poster = computed(() => `/api/v1/items/${id.value}/images/poster`)
const posterError = ref(false)


const kindLabels: Record<string, string> = {
  movie: '电影',
  series: '电视剧',
  episode: '电视剧',
  anime: '动画',
  documentary: '纪录片',
  variety: '综艺',
  concert: '演唱会',
  music: '音乐',
}

const displayTitle = computed(() => {
  if (series.value?.title) {
    return series.value.title
  }
  return detail.value?.title ?? ''
})

const displaySubtitle = computed(() => {
  if (series.value && detail.value?.title && detail.value.title !== series.value.title) {
    return detail.value.title
  }
  if (detail.value?.original_title && detail.value.original_title !== displayTitle.value) {
    return detail.value.original_title
  }
  if (series.value?.original_title && series.value.original_title !== displayTitle.value) {
    return series.value.original_title
  }
  return null
})

const displayOverview = computed(() => {
  const o = detail.value?.overview?.trim()
  if (o) return o
  const so = series.value?.overview?.trim()
  if (so) return so
  return ''
})

const metaItems = computed(() => {
  if (!detail.value) return []
  const items: Array<{ text: string; isRating?: boolean; isQuality?: boolean }> = []
  if (detail.value.rating) {
    items.push({ text: `★ ${detail.value.rating.toFixed(1)}`, isRating: true })
  }
  const yr = detail.value.year ?? series.value?.year
  if (yr) {
    items.push({ text: String(yr) })
  }
  if (detail.value.runtime_ms) {
    items.push({ text: fmtMs(detail.value.runtime_ms) })
  }
  if (detail.value.genres?.length) {
    items.push({ text: detail.value.genres.join(' / ') })
  }
  if (mainQuality.value) {
    items.push({ text: mainQuality.value, isQuality: true })
  }
  return items
})

/**
 * 演员头像位置是没有人物头像图的（person 表只存名字）。
 * 用姓名首字做一个圆形色块。
 */
function initialOf(name: string): string {
  const t = name.trim()
  return t ? [...t][0]!.toUpperCase() : '?'
}

/** 演员只显示前 12 个：一季剧的演员表动辄几十人，全列出来没人看。 */
const cast = computed(() => (credits.value?.cast ?? []).slice(0, 12))
const crew = computed(() => credits.value?.crew ?? [])

/** 幕后按职能归并：导演 / 编剧往往各有多人，竖着列一行一个很占地方。 */
const crewByRole = computed(() => {
  const map = new Map<string, string[]>()
  for (const c of crew.value) {
    const list = map.get(c.role) ?? []
    list.push(c.person)
    map.set(c.role, list)
  }
  return [...map.entries()]
})

const ROLE_LABELS: Record<string, string> = {
  director: '导演',
  writer: '编剧',
  producer: '制片',
  composer: '配乐',
}

function fmtMs(ms?: number | null): string {
  if (!ms) return '—'
  const total = Math.round(ms / 1000)
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  return h > 0 ? `${h} 小时 ${m} 分` : `${m} 分`
}

const mainQuality = computed(() => {
  const v = detail.value?.versions?.[0]
  if (!v) return 'HD'
  const res = v.resolution || '1080P'
  return isHdr(v.hdr_type) ? `${res} ${hdrLabel(v.hdr_type)}` : res
})
</script>

<template>
  <div class="detail-page">
    <p v-if="error" class="detail-error">
      {{ error }}
    </p>

    <div v-else-if="loading" class="detail-loading">
      <div class="skeleton" style="height: 360px; border-radius: 20px;"></div>
      <div class="skeleton" style="height: 28px; width: 35%; margin-top: 20px; border-radius: 8px;"></div>
      <div class="skeleton" style="height: 16px; width: 60%; margin-top: 12px; border-radius: 6px;"></div>
    </div>

    <div v-else-if="detail" class="detail-wrapper animate-fade-in">
      <!-- 顶部英雄背景区 -->
      <section class="detail-hero" :style="{ '--detailbg': `url('${backdrop}')` }">
        <div class="hero-inner">
          <!-- 经典竖版海报封套（含背投微光） -->
          <div class="hero-poster-box">
            <div
              v-if="!posterError"
              class="poster-glow"
              :style="{ backgroundImage: `url('${poster}')` }"
            ></div>
            <img
              v-if="!posterError"
              :src="poster"
              :alt="displayTitle"
              class="hero-poster-img"
              loading="lazy"
              @error="posterError = true"
            />
            <div v-else class="hero-poster-fallback">
              <span class="hero-fallback-char">
                {{ displayTitle.trim() ? [...displayTitle.trim()][0]?.toUpperCase() : '🎬' }}
              </span>
            </div>
          </div>

          <div class="detail-content">
            <div class="hero-tags">
              <span class="tag tag-kind">{{ series ? '电视剧' : (kindLabels[detail.kind] ?? detail.kind) }}</span>
              <span v-if="detail.year ?? series?.year" class="tag tag-year">{{ detail.year ?? series?.year }}</span>
              <span v-if="series" class="tag tag-series">全剧集</span>
            </div>

            <h1>{{ displayTitle }}</h1>
            <p v-if="displaySubtitle" class="detail-orig-title">
              {{ displaySubtitle }}
            </p>

            <div v-if="metaItems.length" class="hero-meta">
              <template v-for="(m, idx) in metaItems" :key="idx">
                <span v-if="idx > 0" class="meta-sep"></span>
                <span :class="{ 'meta-rating': m.isRating, 'meta-quality': m.isQuality }">{{ m.text }}</span>
              </template>
            </div>

            <p v-if="displayOverview" class="detail-desc">
              {{ displayOverview }}
            </p>

            <!-- 动作交互按钮 -->
            <div class="actions">
              <RouterLink
                :to="{ name: 'play', params: { id: detail.id }, query: { start: resumeMs ?? 0 } }"
                class="btn btn-primary"
              >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path d="m8 5 12 7-12 7z" />
                </svg>
                {{ resumeMs ? '继续播放' : '立即播放' }}
              </RouterLink>

              <RouterLink
                v-if="resumeMs"
                :to="{ name: 'play', params: { id: detail.id }, query: { start: 0 } }"
                class="btn btn-secondary"
              >
                从头播放
              </RouterLink>

              <!-- 收藏按钮 -->
              <button
                v-if="favAvailable"
                type="button"
                class="btn btn-secondary fav-toggle-btn"
                :class="{ 'is-active': favorite }"
                :aria-pressed="favorite"
                :title="favorite ? '取消收藏' : '加入收藏'"
                @click="toggleFavorite"
              >
                <svg
                  viewBox="0 0 24 24"
                  aria-hidden="true"
                  :style="{ fill: favorite ? '#ff799e' : 'none', stroke: favorite ? '#ff799e' : 'currentColor' }"
                >
                  <path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1.1-1.1a5.5 5.5 0 0 0-7.8 7.8l1.1 1.1L12 21l7.8-7.5 1.1-1.1a5.5 5.5 0 0 0-.1-7.8Z" />
                </svg>
                <span>{{ favorite ? '已收藏' : '加入收藏' }}</span>
                <span v-if="favoriteCount > 0" class="fav-count">({{ favoriteCount }})</span>
              </button>

              <!-- 重新刮削与海报刷新按钮 -->
              <button
                type="button"
                class="btn btn-secondary btn-rescrape"
                title="重新刮削并拉取最新海报与演职员"
                :disabled="rescraping"
                @click="rescrapeCurrent"
              >
                <svg
                  viewBox="0 0 24 24"
                  aria-hidden="true"
                  style="width: 15px; height: 15px; fill: none; stroke: currentColor; stroke-width: 2;"
                >
                  <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" stroke-linecap="round" stroke-linejoin="round" />
                  <path d="M3 3v5h5" stroke-linecap="round" stroke-linejoin="round" />
                  <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" stroke-linecap="round" stroke-linejoin="round" />
                  <path d="M16 21h5v-5" stroke-linecap="round" stroke-linejoin="round" />
                </svg>
                <span>{{ rescraping ? '刷新中...' : '重新刮削' }}</span>
              </button>
            </div>
          </div>
        </div>
      </section>

      <!-- 详情下半部分内容区 -->
      <div class="detail-body">
        <!-- 仅当不可播放时极简警示，日常直接播放绝不冗余打扰 -->
        <div v-if="decision?.strategy === 'unsupported'" class="unsupported-alert">
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2V7h2v6z" />
          </svg>
          <span>此片源格式在当前浏览器上暂不支持，推荐使用桌面客户端播放。</span>
        </div>

        <!-- 4 宫格科技事实参数矩阵 (Facts Grid) -->
        <div class="detail-facts">
          <div class="fact">
            <span class="fact-label">上映年份</span>
            <strong class="fact-value">{{ detail.year ?? (series?.year ?? '—') }}</strong>
          </div>
          <div class="fact">
            <span class="fact-label">内容类型</span>
            <strong class="fact-value">{{ series ? '电视剧' : (kindLabels[detail.kind] ?? detail.kind) }}</strong>
          </div>
          <div class="fact">
            <span class="fact-label">片长 / 规格</span>
            <strong class="fact-value">{{ fmtMs(detail.runtime_ms) }}</strong>
          </div>
          <div class="fact">
            <span class="fact-label">媒体规格</span>
            <strong class="fact-value quality-highlight">{{ mainQuality }}</strong>
          </div>
        </div>

        <!-- 类型 Chips 展示 -->
        <div v-if="detail.genres.length" class="genre-chips-row">
          <span v-for="g in detail.genres" :key="g" class="chip">{{ g }}</span>
        </div>

        <!-- 演职员表（刮削前为空时不展示） -->
        <section v-if="cast.length > 0" class="section">
          <div class="section-head">
            <div class="section-title">演职员</div>
          </div>

          <div class="cast-row">
            <div
              v-for="c in cast"
              :key="c.person + (c.character ?? '')"
              class="cast-card"
            >
              <div class="cast-avatar">
                {{ initialOf(c.person) }}
              </div>
              <div class="cast-name">{{ c.person }}</div>
              <div v-if="c.character" class="cast-role">{{ c.character }}</div>
            </div>
          </div>

          <!-- 幕后团队归并 -->
          <dl v-if="crewByRole.length > 0" class="crew-list">
            <div v-for="[role, names] in crewByRole" :key="role" class="crew-item">
              <dt>{{ ROLE_LABELS[role] ?? role }}</dt>
              <dd>{{ names.join('、') }}</dd>
            </div>
          </dl>
        </section>

        <!-- 剧集选集（所属某部剧时才渲染） -->
        <div v-if="series" class="section">
          <div class="section-head">
            <div class="section-title">剧集选集</div>
          </div>
          <EpisodeList :series-id="series.id" />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.detail-page {
  padding-top: 10px;
}
.detail-error {
  border-radius: 12px;
  border: 1px solid rgba(239, 68, 68, 0.4);
  background: rgba(127, 29, 29, 0.35);
  padding: 14px 18px;
  font-size: 13px;
  color: #fca5a5;
}
.detail-loading {
  display: flex;
  flex-direction: column;
}

/* 收藏按钮 */
.fav-toggle-btn.is-active {
  border-color: rgba(255, 121, 158, 0.4);
  color: #ff9db8;
  background: rgba(60, 20, 42, 0.6);
}
.fav-count {
  font-size: 11px;
  color: var(--muted);
}

/* 英雄榜区域微调 */
.detail-hero {
  border-radius: 24px;
  overflow: hidden;
  box-shadow: 0 24px 60px rgba(0, 0, 0, 0.6);
  background:
    linear-gradient(180deg, rgba(8, 14, 26, 0.4) 0%, rgba(8, 14, 26, 0.95) 100%),
    radial-gradient(ellipse at 80% 20%, rgba(56, 189, 248, 0.12) 0%, transparent 60%),
    var(--detailbg) center/cover no-repeat;
  position: relative;
  border: 1px solid rgba(148, 163, 184, 0.1);
}
.hero-inner {
  display: flex;
  align-items: flex-end;
  gap: 36px;
  width: 100%;
  padding: 40px 36px;
  position: relative;
  z-index: 1;
}

/* 海报封套与科技背投光晕 */
.hero-poster-box {
  width: 185px;
  flex-shrink: 0;
  border-radius: 16px;
  aspect-ratio: 2 / 3;
  position: relative;
  display: flex;
  perspective: 1000px;
}
.poster-glow {
  position: absolute;
  inset: -8px;
  border-radius: 20px;
  background-size: cover;
  background-position: center;
  filter: blur(28px) brightness(1.2) saturate(1.4);
  opacity: 0.45;
  z-index: 0;
  transition: opacity 0.3s ease;
  pointer-events: none;
}
.hero-poster-box:hover .poster-glow {
  opacity: 0.7;
}
.hero-poster-img {
  position: relative;
  z-index: 1;
  width: 100%;
  height: 100%;
  object-fit: cover;
  border-radius: 16px;
  border: 1px solid rgba(255, 255, 255, 0.15);
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.8);
  transition: transform 0.35s cubic-bezier(0.16, 1, 0.3, 1), box-shadow 0.35s ease;
}
.hero-poster-box:hover .hero-poster-img {
  transform: translateY(-4px) scale(1.02);
  box-shadow: 0 24px 48px rgba(0, 0, 0, 0.9), 0 0 24px rgba(56, 189, 248, 0.2);
}
.hero-poster-fallback {
  position: relative;
  z-index: 1;
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 16px;
  border: 1px solid rgba(56, 189, 248, 0.25);
  background: linear-gradient(135deg, #0f172a 0%, #1e1b4b 60%, #0369a1 100%);
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.8);
}
.hero-fallback-char {
  font-size: 56px;
  font-weight: 800;
  color: #38bdf8;
  text-shadow: 0 0 24px rgba(56, 189, 248, 0.6);
  user-select: none;
}

.detail-content {
  flex: 1;
  min-width: 0;
}
.detail-content h1 {
  font-size: 32px;
  font-weight: 800;
  color: #f8fafc;
  line-height: 1.25;
  margin: 0 0 6px 0;
  letter-spacing: -0.5px;
}
.detail-orig-title {
  font-size: 14px;
  color: #94a3b8;
  margin: 0 0 12px;
  font-style: italic;
  letter-spacing: 0.3px;
}
.detail-desc {
  font-size: 13.5px;
  line-height: 1.75;
  color: #cbd5e1;
  max-width: 720px;
  margin: 12px 0 24px;
  display: -webkit-box;
  -webkit-line-clamp: 4;
  -webkit-box-orient: vertical;
  overflow: hidden;
  text-overflow: ellipsis;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.5);
}

@media (max-width: 720px) {
  .hero-inner {
    flex-direction: column;
    align-items: flex-start;
    gap: 20px;
    padding: 24px 20px;
  }
  .hero-poster-box {
    width: 140px;
  }
  .detail-content h1 {
    font-size: 24px;
  }
}

/* 科技胶囊标签 */
.hero-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 12px;
}
.hero-tags .tag {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.5px;
  padding: 4px 12px;
  border-radius: 9999px;
  background: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.1);
  color: #e2e8f0;
  backdrop-filter: blur(8px);
}
.hero-tags .tag-kind {
  background: rgba(99, 102, 241, 0.15);
  border-color: rgba(99, 102, 241, 0.4);
  color: #a5b4fc;
}
.hero-tags .tag-series {
  background: rgba(56, 189, 248, 0.15);
  border-color: rgba(56, 189, 248, 0.4);
  color: #7dd3fc;
}

.hero-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 12px;
  font-size: 13px;
  color: #cbd5e1;
  margin: 10px 0 14px;
}
.hero-meta .meta-sep {
  width: 4px;
  height: 4px;
  border-radius: 50%;
  background: rgba(148, 163, 184, 0.4);
}
.hero-meta .meta-rating {
  color: #facc15;
  font-weight: 700;
  text-shadow: 0 0 10px rgba(250, 204, 21, 0.35);
}
.hero-meta .meta-quality {
  color: #38bdf8;
  font-weight: 600;
  text-shadow: 0 0 8px rgba(56, 189, 248, 0.3);
}

/* 操作按钮组 */
.actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 12px;
  margin-top: 14px;
}
.actions .btn-primary {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 10px 22px;
  border-radius: 10px;
  background: linear-gradient(135deg, #0284c7 0%, #4f46e5 100%);
  border: 1px solid rgba(56, 189, 248, 0.5);
  color: #fff;
  font-size: 14px;
  font-weight: 700;
  text-decoration: none;
  cursor: pointer;
  transition: all 0.25s ease;
  box-shadow: 0 4px 18px rgba(2, 132, 199, 0.4);
}
.actions .btn-primary:hover {
  transform: translateY(-2px);
  box-shadow: 0 8px 24px rgba(2, 132, 199, 0.6), 0 0 16px rgba(99, 102, 241, 0.4);
  border-color: #38bdf8;
}
.actions .btn-primary svg {
  width: 18px;
  height: 18px;
  fill: currentColor;
}
.actions .btn-secondary {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 9px 18px;
  border-radius: 10px;
  background: rgba(30, 41, 59, 0.65);
  border: 1px solid rgba(148, 163, 184, 0.2);
  color: #e2e8f0;
  font-size: 13.5px;
  font-weight: 600;
  text-decoration: none;
  cursor: pointer;
  backdrop-filter: blur(8px);
  transition: all 0.2s ease;
}
.actions .btn-secondary:hover {
  background: rgba(51, 65, 85, 0.8);
  border-color: rgba(56, 189, 248, 0.4);
  color: #fff;
  transform: translateY(-1px);
}
.fav-toggle-btn.is-active {
  border-color: rgba(244, 63, 94, 0.45);
  background: rgba(244, 63, 94, 0.12);
  color: #fda4af;
}

/* 详情下部内容区 */
.detail-body {
  margin-top: 28px;
}

/* 无法播放时的极简警示条 */
.unsupported-alert {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 18px;
  border-radius: 12px;
  background: rgba(239, 68, 68, 0.1);
  border: 1px solid rgba(239, 68, 68, 0.3);
  color: #fca5a5;
  font-size: 13px;
  margin-bottom: 24px;
}

/* 4 宫格科技事实参数矩阵 */
.detail-facts {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 14px;
  margin: 20px 0 24px;
}
@media (max-width: 640px) {
  .detail-facts {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
.fact {
  background: linear-gradient(135deg, rgba(15, 23, 42, 0.65) 0%, rgba(20, 31, 51, 0.45) 100%);
  border: 1px solid rgba(148, 163, 184, 0.1);
  border-radius: 14px;
  padding: 14px 18px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  backdrop-filter: blur(8px);
  transition: transform 0.25s ease, border-color 0.25s ease;
}
.fact:hover {
  transform: translateY(-2px);
  border-color: rgba(56, 189, 248, 0.3);
  box-shadow: 0 8px 20px rgba(0, 0, 0, 0.4);
}
.fact-label {
  font-size: 11px;
  color: #94a3b8;
  letter-spacing: 0.5px;
}
.fact-value {
  font-size: 15px;
  font-weight: 700;
  color: #f8fafc;
  font-family: 'JetBrains Mono', monospace, Inter, sans-serif;
}
.quality-highlight {
  color: #38bdf8 !important;
  text-shadow: 0 0 10px rgba(56, 189, 248, 0.25);
}

/* 标签行 */
.genre-chips-row {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 24px;
}
.genre-chips-row .chip {
  padding: 4px 12px;
  border-radius: 9999px;
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(148, 163, 184, 0.14);
  font-size: 12px;
  color: #cbd5e1;
  transition: all 0.2s ease;
}
.genre-chips-row .chip:hover {
  border-color: rgba(56, 189, 248, 0.4);
  color: #38bdf8;
  background: rgba(56, 189, 248, 0.08);
}

/* 演职员 */
.cast-row {
  display: flex;
  gap: 16px;
  overflow-x: auto;
  padding: 4px 2px 12px;
  scrollbar-width: thin;
}
.cast-card {
  width: 90px;
  flex-shrink: 0;
  text-align: center;
  transition: transform 0.2s ease;
}
.cast-card:hover {
  transform: translateY(-2px);
}
.cast-avatar {
  margin: 0 auto;
  width: 58px;
  height: 58px;
  border-radius: 50%;
  display: grid;
  place-items: center;
  font-size: 18px;
  font-weight: 700;
  color: #7dd3fc;
  background: linear-gradient(135deg, rgba(30, 41, 59, 0.9) 0%, rgba(15, 23, 42, 0.9) 100%);
  border: 1px solid rgba(56, 189, 248, 0.3);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  transition: all 0.2s ease;
}
.cast-card:hover .cast-avatar {
  border-color: #38bdf8;
  box-shadow: 0 0 16px rgba(56, 189, 248, 0.35);
  transform: scale(1.05);
}
.cast-name {
  margin-top: 8px;
  font-size: 12px;
  font-weight: 600;
  color: #f1f5f9;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cast-role {
  font-size: 11px;
  color: #94a3b8;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  margin-top: 2px;
}

/* 幕后归并 */
.crew-list {
  margin-top: 20px;
  padding-top: 16px;
  border-top: 1px solid rgba(148, 163, 184, 0.12);
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.crew-item {
  display: flex;
  gap: 16px;
  font-size: 13px;
}
.crew-item dt {
  width: 54px;
  flex-shrink: 0;
  color: #94a3b8;
  font-weight: 500;
}
.crew-item dd {
  color: #e2e8f0;
  margin: 0;
}
</style>

