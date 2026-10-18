<script setup lang="ts">
/**
 * 类型探索氛围卡（GenreCard）。
 * 1:1 遵循 ui.html 原生规范：
 * - .genre-card 容器（圆角 14px、暗蓝底、悬停上浮 3px）
 * - 内部背景图支持悬停平滑缩放 scale(1.06)
 * - 底部暗色渐变蒙版
 * - 粗体类型标题 + 辅助描述
 */
import type { RouteLocationRaw } from 'vue-router'

defineProps<{
  title: string
  subtitle: string
  bgImage?: string
  to: RouteLocationRaw
}>()
</script>

<template>
  <RouterLink :to="to" class="genre-card">
    <!-- 背景图层（带悬停缩放微动效） -->
    <div
      v-if="bgImage"
      class="genre-bg"
      :style="{ backgroundImage: `url('${bgImage}')` }"
    />
    <!-- 降级渐变底（无图时） -->
    <div v-else class="bg-gradient-to-br fallback-bg" />

    <strong>{{ title }}</strong>
    <span>{{ subtitle }}</span>
  </RouterLink>
</template>

<style scoped>
.genre-bg {
  position: absolute;
  inset: 0;
  z-index: -2;
  background-size: cover;
  background-position: center;
  transition: transform 0.4s ease;
}
.genre-card:hover .genre-bg {
  transform: scale(1.06);
}

.fallback-bg {
  position: absolute;
  inset: 0;
  z-index: -2;
  background: linear-gradient(135deg, #243354, #1b2844 50%, #121c32);
}
</style>
