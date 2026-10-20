<script setup lang="ts">
/**
 * 剧集的季 / 集列表（详情页用）。
 *
 * 设计要点：
 * 1. 集一次性取完，不做多余分页（单季通常 12~24 集，上限 500）。
 * 2. 默认选中第一季。
 * 3. 采用现代化流媒体卡片网格布局（16:9 缩略封面、剧集浮水印、悬停光效播按钮与元信息），
 *    同时配备快速数字选集条，大幅提升点选效率。
 */
import { computed, onMounted, ref, watch } from 'vue'
import * as api from '../api/client'
import type { EpisodeSummary, SeasonSummary } from '../api/client'

const props = defineProps<{
  seriesId: string
}>()

const seasons = ref<SeasonSummary[]>([])
const activeSeason = ref<string>('')
const episodes = ref<EpisodeSummary[]>([])
const loading = ref(true)
const listLoading = ref(false)
const error = ref<string | null>(null)

const activeNumber = computed(
  () => seasons.value.find((s) => s.id === activeSeason.value)?.number ?? 1,
)

function displayEpTitle(ep: EpisodeSummary): string {
  if (ep.title) {
    if (/^\d+$/.test(ep.title.trim())) {
      return `第 ${Number(ep.title)} 集`
    }
    return ep.title
  }
  return `第 ${ep.episode_number} 集`
}

async function loadSeasons(): Promise<void> {
  loading.value = true
  error.value = null
  try {
    seasons.value = await api.seriesSeasons(props.seriesId)
    activeSeason.value = seasons.value[0]?.id ?? ''
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

async function loadEpisodes(): Promise<void> {
  if (!activeSeason.value) {
    episodes.value = []
    return
  }
  listLoading.value = true
  try {
    const p = await api.listEpisodes({ season_id: activeSeason.value, page_size: 500 })
    episodes.value = p.data
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    listLoading.value = false
  }
}

onMounted(async () => {
  await loadSeasons()
  await loadEpisodes()
})

watch(activeSeason, () => void loadEpisodes())
watch(() => props.seriesId, () => void loadSeasons())

/** 没拆出季的剧（目录不规整时会有）：给一个明确的说明，而不是空白。 */
const noSeasons = computed(() => !loading.value && seasons.value.length === 0)
</script>

<template>
  <div class="episode-section">
    <p v-if="error" class="episode-error">{{ error }}</p>

    <div v-else-if="loading" class="episodes-skeleton">
      <div v-for="n in 4" :key="n" class="skeleton skeleton-card" />
    </div>

    <div v-else-if="noSeasons" class="empty-episodes-hint">
      <p>这部剧还没有拆出季。重新扫描（目录结构规整后会按季集归类）就会出现在这里。</p>
    </div>

    <template v-else>
      <!-- 季选择栏。集数写在徽章上 -->
      <div class="seasons-bar">
        <button
          v-for="s in seasons"
          :key="s.id"
          type="button"
          class="season-pill"
          :class="{ 'is-active': s.id === activeSeason }"
          :aria-pressed="s.id === activeSeason"
          @click="activeSeason = s.id"
        >
          <span class="season-name">第 {{ s.number }} 季</span>
          <span class="season-badge">{{ s.episode_count }} 集</span>
        </button>
      </div>

      <!-- 快速数字选集条（方便多集快速点选） -->
      <div v-if="episodes.length > 1" class="quick-episodes-bar">
        <span class="quick-label">快速选集</span>
        <div class="quick-list">
          <RouterLink
            v-for="ep in episodes"
            :key="ep.id"
            :to="{ name: 'play', params: { id: ep.item_id } }"
            class="quick-ep-btn"
            :title="displayEpTitle(ep)"
          >
            {{ ep.episode_number }}
          </RouterLink>
        </div>
      </div>

      <div v-if="listLoading" class="episodes-skeleton">
        <div v-for="n in 4" :key="n" class="skeleton skeleton-card" />
      </div>

      <!-- 现代流媒体剧集卡片网格 -->
      <ol v-else class="episodes-grid">
        <li v-for="ep in episodes" :key="ep.id" class="episode-item">
          <RouterLink
            :to="{ name: 'play', params: { id: ep.item_id } }"
            class="episode-card"
          >
            <div class="episode-thumb">
              <img
                :src="`/api/v1/items/${ep.item_id}/images/still`"
                :alt="displayEpTitle(ep)"
                class="thumb-img"
                loading="lazy"
                @error="($event.target as HTMLImageElement).style.opacity = '0'"
              />
              <div class="thumb-watermark">{{ String(ep.episode_number).padStart(2, '0') }}</div>
              <div class="thumb-play-btn">
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M8 5.5 19 12 8 18.5z" />
                </svg>
              </div>
              <span class="ep-number-tag">EP {{ ep.episode_number }}</span>
            </div>
            <div class="episode-meta">
              <div class="episode-title-row">
                <span class="episode-title" :title="displayEpTitle(ep)">
                  {{ displayEpTitle(ep) }}
                </span>
              </div>
              <div class="episode-sub">
                <span>第 {{ activeNumber }} 季 · 第 {{ ep.episode_number }} 集</span>
                <span class="play-action-hint">立即播放 ›</span>
              </div>
            </div>
          </RouterLink>
        </li>
      </ol>
    </template>
  </div>
</template>

<style scoped>
.episode-section {
  margin-top: 10px;
}
.episode-error {
  border-radius: 12px;
  border: 1px solid rgba(239, 68, 68, 0.4);
  background: rgba(127, 29, 29, 0.35);
  padding: 14px 18px;
  font-size: 13px;
  color: #fca5a5;
}

/* 季切换栏 */
.seasons-bar {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  margin-bottom: 16px;
}
.season-pill {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  border-radius: 9999px;
  background: rgba(20, 31, 51, 0.7);
  border: 1px solid rgba(174, 193, 226, 0.14);
  color: var(--muted);
  font-size: 13px;
  font-weight: 500;
  transition: all 0.25s cubic-bezier(0.16, 1, 0.3, 1);
  cursor: pointer;
  user-select: none;
}
.season-pill:hover {
  background: rgba(26, 39, 64, 0.95);
  border-color: rgba(174, 193, 226, 0.35);
  color: var(--text);
  transform: translateY(-1px);
}
.season-pill.is-active {
  background: linear-gradient(135deg, rgba(121, 91, 255, 0.25), rgba(87, 200, 245, 0.15));
  border-color: rgba(121, 91, 255, 0.6);
  color: #ffffff;
  box-shadow: 0 4px 16px rgba(121, 91, 255, 0.2);
}
.season-badge {
  font-size: 11px;
  padding: 2px 7px;
  border-radius: 10px;
  background: rgba(255, 255, 255, 0.08);
  color: inherit;
}
.season-pill.is-active .season-badge {
  background: rgba(121, 91, 255, 0.35);
  color: #fff;
}

/* 快速选集条 */
.quick-episodes-bar {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-bottom: 20px;
  padding: 10px 16px;
  background: rgba(16, 24, 42, 0.6);
  border: 1px solid rgba(174, 193, 226, 0.1);
  border-radius: 12px;
}
.quick-label {
  font-size: 12px;
  font-weight: 600;
  color: var(--muted);
  white-space: nowrap;
}
.quick-list {
  display: flex;
  gap: 8px;
  overflow-x: auto;
  padding: 2px 0;
  scrollbar-width: thin;
}
.quick-ep-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 36px;
  height: 36px;
  padding: 0 10px;
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(174, 193, 226, 0.12);
  color: var(--text);
  font-size: 13px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  transition: all 0.2s ease;
  text-decoration: none;
}
.quick-ep-btn:hover {
  background: rgba(121, 91, 255, 0.25);
  border-color: var(--purple);
  color: #fff;
  transform: translateY(-2px);
  box-shadow: 0 4px 12px rgba(121, 91, 255, 0.25);
}

/* 集卡片网格 */
.episodes-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: 18px;
  list-style: none;
  padding: 0;
  margin: 0;
}
.episode-item {
  display: flex;
}
.episode-card {
  display: flex;
  flex-direction: column;
  width: 100%;
  background: rgba(20, 31, 51, 0.65);
  border: 1px solid rgba(174, 193, 226, 0.1);
  border-radius: 14px;
  overflow: hidden;
  transition: all 0.3s cubic-bezier(0.16, 1, 0.3, 1);
  text-decoration: none;
  color: inherit;
  backdrop-filter: blur(8px);
}
.episode-card:hover {
  border-color: rgba(121, 91, 255, 0.45);
  background: rgba(24, 37, 61, 0.9);
  transform: translateY(-3px);
  box-shadow: 0 12px 28px rgba(0, 0, 0, 0.35), 0 0 16px rgba(121, 91, 255, 0.15);
}
.episode-thumb {
  position: relative;
  aspect-ratio: 16 / 9;
  width: 100%;
  background: linear-gradient(135deg, #131c30 0%, #1a2742 50%, #0d1527 100%);
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
}
.thumb-img {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
  transition: transform 0.25s ease, opacity 0.2s ease;
  z-index: 1;
}
.episode-card:hover .thumb-img {
  transform: scale(1.05);
}
.thumb-watermark {
  position: absolute;
  right: 14px;
  bottom: 6px;
  font-size: 54px;
  font-weight: 900;
  color: rgba(255, 255, 255, 0.08);
  letter-spacing: -2px;
  user-select: none;
  font-family: Inter, sans-serif;
  line-height: 1;
  z-index: 2;
}
.thumb-play-btn {
  width: 44px;
  height: 44px;
  border-radius: 50%;
  background: rgba(121, 91, 255, 0.85);
  display: flex;
  align-items: center;
  justify-content: center;
  color: #fff;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
  transition: all 0.3s ease;
  transform: scale(0.9);
  opacity: 0.85;
  z-index: 2;
}
.thumb-play-btn svg {
  width: 18px;
  height: 18px;
  fill: currentColor;
  margin-left: 2px;
}
.episode-card:hover .thumb-play-btn {
  transform: scale(1.08);
  opacity: 1;
  background: var(--purple);
  box-shadow: 0 6px 20px rgba(121, 91, 255, 0.5);
}
.ep-number-tag {
  position: absolute;
  top: 10px;
  left: 10px;
  font-size: 11px;
  font-weight: 700;
  padding: 3px 8px;
  border-radius: 6px;
  background: rgba(0, 0, 0, 0.65);
  backdrop-filter: blur(4px);
  color: #e2e8f0;
  border: 1px solid rgba(255, 255, 255, 0.1);
}
.episode-meta {
  padding: 14px 16px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  flex: 1;
}
.episode-title-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.episode-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.episode-sub {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 12px;
  color: var(--muted);
}
.play-action-hint {
  font-size: 11px;
  color: var(--cyan);
  opacity: 0;
  transition: opacity 0.2s ease;
}
.episode-card:hover .play-action-hint {
  opacity: 1;
}

/* 空状态与骨架 */
.empty-episodes-hint {
  padding: 28px;
  border-radius: 14px;
  background: rgba(20, 31, 51, 0.4);
  border: 1px dashed rgba(174, 193, 226, 0.18);
  text-align: center;
  color: var(--muted);
  font-size: 13px;
  line-height: 1.6;
}
.episodes-skeleton {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: 18px;
}
.skeleton-card {
  height: 200px;
  border-radius: 14px;
}
</style>
