<script setup lang="ts">
/**
 * 海报卡片（PosterCard）。
 * 1:1 遵循 ui.html 原生规范：
 * - 2 / 2.72 比例与 13px 圆角
 * - 悬停时向上平移 4px、高亮紫光边框、图片平滑缩放 1.045
 * - 顶部遮罩：左侧品质徽标 .quality / .badge-quality + 右侧心形收藏微按钮 .fav-btn
 * - 底部渐变暗影：黄色星级评分 .mini-rating、加粗标题、分类标签
 * - 下方信息栏：单行省略标题与年份信息
 * - 首字降级兜底与进度条展示
 */
import { computed, ref, watch } from 'vue'
import type { ItemSummary } from '../api/client'
import * as api from '../api/client'
import { toast } from '../stores/toast'

const props = defineProps<{
  item: ItemSummary
  /** 0..1。有值时画一条进度条。 */
  progress?: number
  /** 品质标签，例如 "4K HDR" / "1080P" / "4K" */
  quality?: string
  /** 外部传入的收藏状态 */
  isFavorite?: boolean
}>()

const emits = defineEmits<{
  (e: 'favorite-change', isFav: boolean): void
}>()

const failed = ref(false)
const localFav = ref<boolean>(props.isFavorite ?? false)
const favBusy = ref(false)

watch(
  () => props.isFavorite,
  (val) => {
    if (val !== undefined) localFav.value = val
  },
)

watch(
  () => props.item.id,
  () => {
    failed.value = false
    localFav.value = props.isFavorite ?? false
  },
)

const posterUrl = computed(() => {
  const anyItem = props.item as Record<string, unknown>
  if (typeof anyItem.poster_url === 'string' && anyItem.poster_url) {
    return anyItem.poster_url
  }
  return `/api/v1/items/${props.item.id}/images/poster`
})

const effectiveQuality = computed(() => {
  if (props.quality) return props.quality
  const anyItem = props.item as Record<string, unknown>
  if (typeof anyItem.quality === 'string' && anyItem.quality) {
    return anyItem.quality
  }
  return ''
})

const initial = computed(() => {
  const t = props.item.title.trim()
  return t ? [...t][0]!.toUpperCase() : '?'
})

const pct = computed(() => Math.round(Math.min(1, Math.max(0, props.progress ?? 0)) * 100))

const kindLabels: Record<string, string> = {
  movie: '电影',
  series: '电视剧',
  season: '季',
  episode: '集',
  anime: '动画',
  variety: '综艺',
  documentary: '纪录片',
  concert: '演唱会',
  music: '音乐',
  music_video: 'MV',
  short_video: '短片',
  home_video: '家庭影像',
}

const kindLabel = computed(() => kindLabels[props.item.kind] ?? '')

const rating = computed(() => {
  const r = props.item.rating
  return r && r > 0 ? r.toFixed(1) : null
})

async function toggleFavorite(e: Event) {
  e.preventDefault()
  e.stopPropagation()
  if (favBusy.value) return

  const next = !localFav.value
  localFav.value = next
  favBusy.value = true

  try {
    const res = await api.setFavorite(props.item.id, next)
    localFav.value = res.favorite
    emits('favorite-change', res.favorite)
    toast(res.favorite ? '已添加到收藏' : '已从收藏中移除', { type: 'info' })
  } catch {
    localFav.value = !next
    toast('收藏状态更新失败', { type: 'error' })
  } finally {
    favBusy.value = false
  }
}
</script>

<template>
  <RouterLink
    :to="{ name: 'item', params: { id: item.id } }"
    class="poster-card"
    :aria-label="`查看${item.title}详情`"
  >
    <div class="poster-image">
      <img
        v-if="!failed"
        :src="posterUrl"
        :alt="`${item.title}海报`"
        loading="lazy"
        @error="failed = true"
      />
      <div v-else class="poster-fallback">
        <span class="fallback-letter">{{ initial }}</span>
        <span v-if="kindLabel" class="fallback-kind">{{ kindLabel }}</span>
      </div>

      <!-- 遮罩层内容：左上品标 + 右上心标 + 底部评分与大字 -->
      <div class="poster-shade">
        <div class="poster-top">
          <span v-if="effectiveQuality" class="quality badge-quality">{{ effectiveQuality }}</span>
          <span v-else />

          <button
            type="button"
            class="fav-btn"
            :class="{ 'is-fav': localFav }"
            :aria-label="localFav ? '取消收藏' : '添加收藏'"
            :title="localFav ? '取消收藏' : '添加收藏'"
            @click="toggleFavorite"
          >
            <svg
              viewBox="0 0 24 24"
              :class="localFav ? 'fill-current stroke-current' : 'fill-none stroke-current'"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1.1-1.1a5.5 5.5 0 0 0-7.8 7.8l1.1 1.1L12 21l7.8-7.5 1.1-1.1a5.5 5.5 0 0 0-.1-7.8Z" />
            </svg>
          </button>
        </div>

        <div class="poster-bottom">
          <div v-if="rating" class="mini-rating">
            ★ {{ rating }}
            <span v-if="item.year" style="color:#a9b7d0;font-weight:500">· {{ item.year }}</span>
          </div>
          <strong>{{ item.title }}</strong>
          <span>{{ (item as any).genres?.length ? (item as any).genres.join(' / ') : kindLabel }}</span>
        </div>
      </div>

      <!-- 进度条 -->
      <div
        v-if="pct > 0"
        class="progress poster-progress"
        role="progressbar"
        :aria-valuenow="pct"
        aria-valuemin="0"
        aria-valuemax="100"
      >
        <span :style="{ width: pct + '%' }"></span>
      </div>
    </div>

    <!-- 底部文字信息区 -->
    <div class="poster-info">
      <div style="min-width: 0; flex: 1">
        <div class="poster-name">{{ item.title }}</div>
        <div class="poster-year">
          {{ item.year ? `${item.year} · ` : '' }}{{ (item as any).episode_count ? `全 ${(item as any).episode_count} 集` : (kindLabel || '影视') }}
        </div>
      </div>
    </div>
  </RouterLink>
</template>

<style scoped>
.poster-fallback {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: linear-gradient(135deg, #243150, #141e33 60%, #10182a);
}
.fallback-letter {
  font-size: 3rem;
  font-weight: 700;
  color: var(--muted);
  opacity: 0.8;
}
.fallback-kind {
  position: absolute;
  bottom: 8px;
  left: 8px;
  border-radius: 4px;
  background: rgba(0, 0, 0, 0.45);
  padding: 2px 6px;
  font-size: 10px;
  color: var(--text);
  backdrop-filter: blur(4px);
}
.poster-progress {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 3px;
  border-radius: 0;
  background: rgba(0, 0, 0, 0.6);
  z-index: 2;
}
</style>
