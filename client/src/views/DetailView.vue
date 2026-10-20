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

onMounted(load)
watch(id, load)

const backdrop = computed(() => `/api/v1/items/${id.value}/images/backdrop`)
const poster = computed(() => `/api/v1/items/${id.value}/images/poster`)
const posterError = ref(false)

const strategyLabel: Record<string, string> = {
  direct_play: '直接播放',
  remux: '重新封装',
  transcode: '需要转码',
  unsupported: '无法播放',
}

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
          <!-- 经典竖版海报封套 -->
          <div v-if="!posterError" class="hero-poster-box">
            <img
              :src="poster"
              :alt="displayTitle"
              class="hero-poster-img"
              loading="lazy"
              @error="posterError = true"
            />
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
            </div>
          </div>
        </div>
      </section>

      <!-- 详情下半部分内容区 -->
      <div class="detail-body">
        <!-- 播放策略预览（Decide 决策报告） -->
        <div v-if="decision" class="card decision-card" :class="decision.strategy">
          <div class="decision-head">
            <div class="decision-badge" :class="decision.strategy">
              <span class="decision-dot"></span>
              <span class="decision-tag" :class="decision.strategy">
                {{ strategyLabel[decision.strategy] ?? decision.strategy }}
              </span>
            </div>
            <span class="decision-reason">{{ decision.reason }}</span>
          </div>

          <ul v-if="decision.reasons.length" class="decision-list">
            <li v-for="r in decision.reasons" :key="r.code">
              <code>{{ r.code }}</code>
              {{ r.message }}
            </li>
          </ul>

          <p v-if="decision.strategy === 'unsupported'" class="unsupported-hint">
            这个片源在当前客户端上放不了。换桌面端（mpv）通常可以直出。
          </p>
        </div>

        <!-- 4 宫格事实参数矩阵 (Facts Grid) -->
        <div class="detail-facts">
          <div class="fact">
            <span>年份</span>
            <strong>{{ detail.year ?? (series?.year ?? '—') }}</strong>
          </div>
          <div class="fact">
            <span>内容类型</span>
            <strong>{{ series ? '电视剧' : (kindLabels[detail.kind] ?? detail.kind) }}</strong>
          </div>
          <div class="fact">
            <span>片长 / 规格</span>
            <strong>{{ fmtMs(detail.runtime_ms) }}</strong>
          </div>
          <div class="fact">
            <span>媒体质量</span>
            <strong class="quality-highlight">{{ mainQuality }}</strong>
          </div>
        </div>

        <!-- 类型 Chips 展示（确保单元测试及多维标签正常展示） -->
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
            <div class="section-title">剧集</div>
          </div>
          <EpisodeList :series-id="series.id" />
        </div>

        <!-- 媒体版本与文件详情 -->
        <section v-if="detail.versions.length" class="section">
          <div class="section-head">
            <div class="section-title">文件</div>
          </div>

          <div class="versions-list">
            <div
              v-for="v in detail.versions"
              :key="v.id"
              class="card version-card"
            >
              <div class="version-head">
                <strong>{{ v.name }}</strong>
                <span v-if="v.resolution" class="chip">{{ v.resolution }}</span>
                <span v-if="isHdr(v.hdr_type)" class="chip hdr-chip">
                  {{ hdrLabel(v.hdr_type) }}
                </span>
                <span v-if="v.is_default" class="chip default-chip">默认</span>
              </div>

              <div v-for="f in v.files" :key="f.id" class="file-item">
                <div class="file-path">{{ f.path }}</div>
                <div class="file-meta">
                  <span>{{ (f.size_bytes / 1024 ** 3).toFixed(2) }} GB</span>
                  <span v-if="f.duration_ms">{{ fmtMs(f.duration_ms) }}</span>
                  <span :class="f.is_available ? 'status-online' : 'status-offline'">
                    {{ f.is_available ? '在线' : '离线' }}
                  </span>
                </div>
              </div>
            </div>
          </div>
        </section>
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
  border-radius: 20px;
  overflow: hidden;
  box-shadow: 0 20px 50px rgba(0, 0, 0, 0.4);
}
.hero-inner {
  display: flex;
  align-items: flex-end;
  gap: 30px;
  width: 100%;
  position: relative;
  z-index: 1;
}
.hero-poster-box {
  width: 175px;
  flex-shrink: 0;
  border-radius: 14px;
  overflow: hidden;
  box-shadow: 0 16px 36px rgba(0, 0, 0, 0.65), 0 0 0 1px rgba(255, 255, 255, 0.12);
  aspect-ratio: 2 / 3;
  background: #0d1627;
  display: flex;
}
.hero-poster-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  transition: transform 0.3s cubic-bezier(0.16, 1, 0.3, 1);
}
.hero-poster-box:hover .hero-poster-img {
  transform: scale(1.04);
}
.detail-orig-title {
  font-size: 14px;
  color: var(--muted);
  margin: -4px 0 10px;
  font-style: italic;
}
.detail-desc {
  font-size: 13px;
  line-height: 1.75;
  color: #cbd5e1;
  max-width: 680px;
  margin: 12px 0 20px;
  display: -webkit-box;
  -webkit-line-clamp: 4;
  -webkit-box-orient: vertical;
  overflow: hidden;
  text-overflow: ellipsis;
}
@media (max-width: 720px) {
  .hero-inner {
    flex-direction: column;
    align-items: flex-start;
    gap: 16px;
  }
  .hero-poster-box {
    width: 130px;
  }
}
.hero-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 14px;
}
.hero-tags .tag {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.5px;
  padding: 4px 10px;
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.08);
  border: 1px solid rgba(255, 255, 255, 0.12);
  color: #cbd5e1;
}
.hero-tags .tag-kind {
  background: rgba(121, 91, 255, 0.2);
  border-color: rgba(121, 91, 255, 0.4);
  color: #c4b5fd;
}
.hero-tags .tag-series {
  background: rgba(87, 200, 245, 0.15);
  border-color: rgba(87, 200, 245, 0.35);
  color: #7dd3fc;
}

.hero-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 10px;
  font-size: 13px;
  color: #cbd5e1;
  margin: 12px 0 16px;
}
.hero-meta .meta-sep {
  width: 3px;
  height: 3px;
  border-radius: 50%;
  background: rgba(255, 255, 255, 0.3);
}
.hero-meta .meta-rating {
  color: #fbbf24;
  font-weight: 700;
}
.hero-meta .meta-quality {
  color: var(--cyan);
  font-weight: 600;
}

/* 决策卡片重塑 */
.decision-card {
  margin: 16px 0;
  padding: 12px 18px;
  border-radius: 12px;
  background: rgba(16, 24, 42, 0.5);
  border: 1px solid var(--line);
  transition: all 0.2s ease;
}
.decision-card.direct_play {
  background: rgba(16, 185, 129, 0.06);
  border-color: rgba(16, 185, 129, 0.22);
}
.decision-card.remux {
  background: rgba(14, 165, 233, 0.06);
  border-color: rgba(14, 165, 233, 0.22);
}
.decision-card.transcode {
  background: rgba(245, 158, 11, 0.06);
  border-color: rgba(245, 158, 11, 0.22);
}
.decision-card.unsupported {
  background: rgba(239, 68, 68, 0.06);
  border-color: rgba(239, 68, 68, 0.22);
}
.decision-head {
  display: flex;
  align-items: center;
  gap: 12px;
}
.decision-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 600;
  padding: 3px 10px;
  border-radius: 6px;
}
.decision-badge.direct_play {
  background: rgba(16, 185, 129, 0.15);
  color: #6ee7b7;
}
.decision-badge.remux {
  background: rgba(14, 165, 233, 0.15);
  color: #7dd3fc;
}
.decision-badge.transcode {
  background: rgba(245, 158, 11, 0.15);
  color: #fcd34d;
}
.decision-badge.unsupported {
  background: rgba(239, 68, 68, 0.15);
  color: #fca5a5;
}
.decision-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
}
.decision-tag {
  font-size: 11px;
  font-weight: 700;
  padding: 0;
  border: 0;
  background: transparent;
  color: inherit;
}
.decision-reason {
  font-size: 13px;
  color: var(--muted);
}
.decision-list {
  margin: 12px 0 0;
  padding-top: 10px;
  border-top: 1px solid var(--line);
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.decision-list li {
  font-size: 11px;
  color: var(--muted);
}
.decision-list code {
  background: rgba(255, 255, 255, 0.08);
  padding: 2px 6px;
  border-radius: 4px;
  margin-right: 6px;
  color: var(--muted);
}
.unsupported-hint {
  margin-top: 10px;
  font-size: 12px;
  color: #fca5a5;
}

/* 4 宫格事实参数矩阵 */
.detail-facts {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 12px;
  margin: 18px 0;
}
.fact {
  background: rgba(20, 31, 52, 0.6);
  border: 1px solid rgba(174, 193, 226, 0.09);
  border-radius: 12px;
  padding: 14px 18px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  backdrop-filter: blur(8px);
  transition: transform 0.2s ease, border-color 0.2s ease;
}
.fact:hover {
  transform: translateY(-2px);
  border-color: rgba(174, 193, 226, 0.2);
}
.fact span {
  font-size: 11px;
  color: var(--muted2);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}
.fact strong {
  font-size: 15px;
  font-weight: 700;
  color: var(--text);
}
.quality-highlight {
  color: var(--cyan) !important;
}

/* 标签行 */
.genre-chips-row {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin: 18px 0;
}

/* 演职员 */
.cast-row {
  display: flex;
  gap: 16px;
  overflow-x: auto;
  padding-bottom: 8px;
}
.cast-card {
  width: 90px;
  flex-shrink: 0;
  text-align: center;
}
.cast-avatar {
  margin: 0 auto;
  width: 60px;
  height: 60px;
  border-radius: 50%;
  display: grid;
  place-items: center;
  font-size: 19px;
  font-weight: 700;
  color: #ced9ee;
  background-image: linear-gradient(150deg, #1d2d4c, #131d31);
  border: 1px solid rgba(255, 255, 255, 0.08);
}
.cast-name {
  margin-top: 8px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cast-role {
  font-size: 10px;
  color: var(--muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  margin-top: 2px;
}

/* 幕后归并 */
.crew-list {
  margin-top: 18px;
  padding-top: 14px;
  border-top: 1px solid var(--line);
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.crew-item {
  display: flex;
  gap: 14px;
  font-size: 12px;
}
.crew-item dt {
  width: 50px;
  flex-shrink: 0;
  color: var(--muted);
}
.crew-item dd {
  color: var(--text);
  margin: 0;
}

/* 版本与文件 */
.versions-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.version-card {
  padding: 16px;
  transition: border-color 0.2s ease;
}
.version-card:hover {
  border-color: rgba(155, 134, 255, 0.35);
}
.version-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 9px;
}
.hdr-chip {
  border-color: rgba(245, 158, 11, 0.3);
  color: #fcd34d;
  background: rgba(245, 158, 11, 0.1);
}
.default-chip {
  border-color: rgba(121, 91, 255, 0.35);
  color: #b3a4ff;
  background: rgba(121, 91, 255, 0.12);
}
.file-item {
  margin-top: 12px;
  font-size: 11px;
}
.file-path {
  font-family: monospace;
  color: var(--muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.file-meta {
  margin-top: 6px;
  display: flex;
  gap: 14px;
  color: var(--muted2);
}
.status-online {
  color: #34d399;
}
.status-offline {
  color: #f87171;
}
</style>

