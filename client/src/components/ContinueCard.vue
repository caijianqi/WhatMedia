<script setup lang="ts">
/**
 * 继续观看横向卡片（ContinueCard）。
 * 1:1 遵循 ui.html 原生规范：
 * - 112px 缩略图 + 居中发光播放圆钮（.play-over）
 * - 右侧文案：标题、剩余时间/集数
 * - 双色渐变进度条（.progress span）
 * - 底部 "已观看 XX%" + "继续播放 →" 微指引
 */
import { computed, ref, watch } from 'vue'
import type { ResumePoint } from '../api/client'

const props = defineProps<{
  resume: ResumePoint
  title?: string
  subtitle?: string
}>()

const failed = ref(false)

watch(
  () => props.resume.item_id,
  () => {
    failed.value = false
  },
)

const displayTitle = computed(() => props.title || props.resume.title || '未知影片')

const initial = computed(() => {
  const t = displayTitle.value.trim()
  return t ? [...t][0]!.toUpperCase() : '?'
})

const thumbUrl = computed(() => {
  const anyResume = props.resume as Record<string, unknown>
  if (typeof anyResume.thumb_url === 'string' && anyResume.thumb_url) {
    return anyResume.thumb_url
  }
  return `/api/v1/items/${props.resume.item_id}/images/backdrop`
})

const pct = computed(() => {
  const d = props.resume.duration_ms ?? 0
  if (d <= 0) return 0
  return Math.min(100, Math.max(0, Math.round((props.resume.position_ms / d) * 100)))
})

const captionText = computed(() => {
  if (props.subtitle) return props.subtitle
  const pos = props.resume.position_ms
  const dur = props.resume.duration_ms
  if (dur && dur > pos) {
    const remMin = Math.max(1, Math.round((dur - pos) / 60000))
    return `剩余 ${remMin} 分钟`
  }
  const posMin = Math.round(pos / 60000)
  return `已观看 ${posMin} 分钟`
})

const startSec = computed(() => Math.floor(props.resume.position_ms / 1000))
</script>

<template>
  <RouterLink
    :to="{ name: 'play', params: { id: resume.item_id }, query: { start: startSec } }"
    class="continue-card"
  >
    <!-- 左侧封面缩略图 -->
    <div class="continue-thumb">
      <img
        v-if="!failed"
        :src="thumbUrl"
        :alt="displayTitle"
        loading="lazy"
        @error="failed = true"
      />
      <div v-else class="thumb-fallback">
        {{ initial }}
      </div>

      <!-- 居中圆形微光播放钮 -->
      <div class="play-over">
        <span>
          <svg viewBox="0 0 24 24">
            <path d="m8 5 12 7-12 7z" />
          </svg>
        </span>
      </div>
    </div>

    <!-- 右侧文案与进度 -->
    <div class="continue-copy">
      <strong>{{ displayTitle }}</strong>
      <p>{{ captionText }}</p>

      <!-- 双色渐变进度条 -->
      <div class="progress">
        <span :style="{ width: pct + '%' }"></span>
      </div>

      <!-- 底部已看比例与继续播放 -->
      <div class="progress-caption">
        <span>已观看 {{ pct }}%</span>
        <em>继续播放 →</em>
      </div>
    </div>
  </RouterLink>
</template>

<style scoped>
.thumb-fallback {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-weight: 700;
  font-size: 1.25rem;
  color: var(--muted);
  background: linear-gradient(135deg, #1f2c47, #131b2e);
}
</style>
