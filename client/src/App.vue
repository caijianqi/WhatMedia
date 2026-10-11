<script setup lang="ts">
/**
 * 应用外壳：顶栏 + 路由出口。
 *
 * 顶栏在播放器页要**完全隐藏** —— 播放器是全屏 fixed 的，
 * 留一条顶栏会在顶部挤出一条黑边，而且返回按钮也有播放器页自己的。
 */
import { computed } from 'vue'
import { useRoute } from 'vue-router'

const route = useRoute()
const isPlayer = computed(() => route.name === 'play')

const nav = [
  { name: 'home', label: '首页' },
  { name: 'library', label: '媒体库' },
  { name: 'settings', label: '设置' },
] as const
</script>

<template>
  <div class="min-h-full">
    <header
      v-if="!isPlayer"
      class="sticky top-0 z-40 border-b border-ink-800 bg-ink-950/90 backdrop-blur"
    >
      <div class="mx-auto flex max-w-[1600px] items-center gap-6 px-4 py-3">
        <RouterLink to="/" class="focus-ring text-lg font-semibold tracking-tight text-white">
          WhatMedia
        </RouterLink>
        <nav class="flex gap-1">
          <RouterLink
            v-for="n in nav"
            :key="n.name"
            :to="{ name: n.name }"
            class="focus-ring rounded px-3 py-1.5 text-sm text-slate-300 transition hover:bg-ink-850 hover:text-white"
            active-class="bg-ink-850 text-white"
          >
            {{ n.label }}
          </RouterLink>
        </nav>
      </div>
    </header>

    <main class="mx-auto max-w-[1600px] px-4 py-6">
      <RouterView />
    </main>
  </div>
</template>
