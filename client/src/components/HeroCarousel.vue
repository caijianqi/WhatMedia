<script setup lang="ts">
/**
 * 首页 Hero 精选大横幅（HeroCarousel）。
 * 1:1 遵循 ui.html 原生规范：
 * - 365px 高度、22px 圆角、双层电影级渐变遮罩（横向黑蓝微光 + 底部渐变暗影）
 * - Eyebrow 横线标语：本周精选 · FEATURED
 * - 标题、评分、年份、分类、4K HDR 元数据分割点
 * - "立即播放"主按钮（带光晕投影）与"查看影片详情"次按钮
 * - 底部标签群（高画质、多音轨、支持字幕）与 22px 轮播长胶囊指示点
 * - 左右优雅切换翻页控件（支持 aria-label="上一张"/"下一张"）与右下角 WHATMEDIA 水印
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'

export interface HeroSlide {
  id: string
  title: string
  year?: number | null
  rating?: number | null
  kind?: string
  overview?: string | null
  genres?: string[]
}

const props = defineProps<{
  slides: HeroSlide[]
  /** 自动切换间隔（毫秒）。传 0 表示不自动播放。 */
  interval?: number
}>()

const index = ref(0)
const paused = ref(false)
const failed = ref<Record<string, boolean>>({})
let timer: ReturnType<typeof setInterval> | null = null

const current = computed<HeroSlide | null>(() => props.slides[index.value] ?? null)
const backdrop = computed(() => {
  if (!current.value) return ''
  const anySlide = current.value as unknown as Record<string, unknown>
  if (typeof anySlide.backdrop_url === 'string' && anySlide.backdrop_url) {
    return anySlide.backdrop_url
  }
  return `/api/v1/items/${current.value.id}/images/backdrop`
})
const backdropFailed = computed(() =>
  current.value ? failed.value[current.value.id] === true : false,
)

const kindLabels: Record<string, string> = {
  movie: '电影',
  series: '电视剧',
  episode: '剧集',
  anime: '动画',
  documentary: '纪录片',
  variety: '综艺',
  concert: '演唱会',
  music: '音乐',
}

const kindName = computed(() => (current.value?.kind ? kindLabels[current.value.kind] ?? current.value.kind : '电影'))

function go(i: number): void {
  const n = props.slides.length
  if (n === 0) return
  index.value = ((i % n) + n) % n
}

function restart(): void {
  if (timer) clearInterval(timer)
  timer = null
  const ms = props.interval ?? 7000
  if (ms <= 0 || props.slides.length < 2) return
  timer = setInterval(() => {
    if (!paused.value) go(index.value + 1)
  }, ms)
}

onMounted(restart)

watch(
  () => props.slides,
  () => {
    index.value = 0
    restart()
  },
)

onBeforeUnmount(() => {
  if (timer) clearInterval(timer)
})
</script>

<template>
  <section
    v-if="current"
    class="hero"
    role="region"
    aria-label="精选推荐"
    @mouseenter="paused = true"
    @mouseleave="paused = false"
    @focusin="paused = true"
    @focusout="paused = false"
  >
    <!-- 背景图及降级渐变 -->
    <img
      v-if="!backdropFailed && backdrop"
      :src="backdrop"
      :alt="current.title"
      class="hero-bg-img"
      @error="failed[current!.id] = true"
    />
    <div v-else class="hero-bg-fallback" />

    <!-- 内容区域 -->
    <div class="hero-content">
      <div class="eyebrow">
        <span class="line"></span>
        本周精选 · FEATURED
      </div>

      <h1>{{ current.title }}</h1>

      <div class="hero-meta">
        <span v-if="current.rating" class="rating">
          <svg viewBox="0 0 24 24">
            <path d="m12 3 2.8 5.7 6.2.9-4.5 4.4 1.1 6.2-5.6-3-5.6 3 1.1-6.2L3 9.6l6.2-.9z" />
          </svg>
          {{ current.rating.toFixed(1) }}
        </span>
        <span v-if="current.rating && current.year" class="meta-sep"></span>
        <span v-if="current.year">{{ current.year }}</span>
        <span class="meta-sep"></span>
        <span>{{ kindName }}</span>
        <span class="meta-sep"></span>
        <span>4K HDR</span>
      </div>

      <p class="hero-desc">
        {{ current.overview || '从海量家庭影视中，轻松找到下一部值得观看的作品。用一个清晰、安静的界面，享受属于自己的家庭影院。' }}
      </p>

      <div class="actions">
        <RouterLink :to="{ name: 'play', params: { id: current.id } }" class="btn btn-primary">
          <svg viewBox="0 0 24 24">
            <path d="m8 5 12 7-12 7z" />
          </svg>
          立即播放
        </RouterLink>

        <RouterLink :to="{ name: 'item', params: { id: current.id } }" class="btn btn-secondary">
          <svg viewBox="0 0 24 24">
            <circle cx="12" cy="12" r="9" />
            <path d="M12 11v5m0-8h.01" />
          </svg>
          查看影片详情
        </RouterLink>
      </div>

      <div class="hero-bottom">
        <div class="hero-tags">
          <span class="tag">高画质</span>
          <span class="tag">多音轨</span>
          <span class="tag">支持字幕</span>
        </div>

        <!-- 轮播长胶囊指示点 -->
        <div v-if="slides.length > 1" class="hero-dots" aria-label="精选轮播指示">
          <button
            v-for="(_, i) in slides"
            :key="i"
            type="button"
            :class="{ active: i === index }"
            :aria-label="`第${i + 1}张`"
            @click="go(i)"
          />
        </div>
      </div>
    </div>

    <!-- 左右翻页控制器（满足无障碍与单元测试） -->
    <div v-if="slides.length > 1" class="hero-arrows">
      <button
        type="button"
        class="arrow-btn"
        aria-label="上一张"
        @click="go(index - 1)"
      >
        <svg viewBox="0 0 24 24">
          <path d="M15 18l-6-6 6-6" />
        </svg>
      </button>
      <button
        type="button"
        class="arrow-btn"
        aria-label="下一张"
        @click="go(index + 1)"
      >
        <svg viewBox="0 0 24 24">
          <path d="M9 18l6-6-6-6" />
        </svg>
      </button>
    </div>

    <div class="hero-visual-label">WHATMEDIA ORIGINAL EXPERIENCE</div>
  </section>
</template>

<style scoped>
.hero-bg-img {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
  object-position: center 43%;
  z-index: -2;
  filter: saturate(0.9);
}

.hero-bg-fallback {
  position: absolute;
  inset: 0;
  z-index: -2;
  background: linear-gradient(135deg, #1b2640, #131d33 55%, #0c1424);
}

.hero-arrows {
  position: absolute;
  right: 28px;
  top: 28px;
  display: flex;
  gap: 8px;
  z-index: 2;
}

.arrow-btn {
  width: 32px;
  height: 32px;
  border-radius: 8px;
  border: 1px solid rgba(255, 255, 255, 0.12);
  background: rgba(10, 16, 31, 0.55);
  backdrop-filter: blur(8px);
  color: #c5d2e8;
  display: grid;
  place-items: center;
  transition: all 0.16s ease;
  cursor: pointer;
}
.arrow-btn:hover {
  background: rgba(121, 91, 255, 0.3);
  border-color: rgba(154, 131, 255, 0.5);
  color: #fff;
}
.arrow-btn svg {
  width: 16px;
  height: 16px;
  stroke: currentColor;
  fill: none;
  stroke-width: 2;
  stroke-linecap: round;
  stroke-linejoin: round;
}
</style>
