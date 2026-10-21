<script setup lang="ts">
/**
 * 左侧主导航。
 * 1:1 遵循 ui.html 原生规范：
 * - 品牌标志带渐变投影与 <small>YOUR HOME CINEMA</small>
 * - 分组导航：发现内容与个人空间
 */
</script>

<template>
  <aside class="sidebar" aria-label="主导航">
    <RouterLink to="/" class="brand">
      <span class="brand-mark">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M7 4.5c0-1 1.1-1.6 2-.9l10 7.1a1.6 1.6 0 0 1 0 2.6L9 20.4c-.9.7-2 .1-2-.9z" />
        </svg>
      </span>
      <span class="brand-name">
        WhatMedia
        <small>YOUR HOME CINEMA</small>
      </span>
    </RouterLink>

    <div class="nav-label">发现内容</div>
    <nav class="nav" id="mainNav">
      <RouterLink
        :to="{ name: 'home' }"
        active-class=""
        exact-active-class=""
        :class="{ active: $route.name === 'home' }"
      >
        <span class="ico">
          <svg viewBox="0 0 24 24">
            <path d="m3 10 9-7 9 7v10a1 1 0 0 1-1 1h-5v-7H9v7H4a1 1 0 0 1-1-1z" />
          </svg>
        </span>
        <span class="nav-text">首页</span>
      </RouterLink>

      <RouterLink
        :to="{ name: 'library' }"
        active-class=""
        exact-active-class=""
        :class="{ active: $route.name === 'library' && !$route.query.filter && !$route.query.sort && !$route.query.focus }"
      >
        <span class="ico">
          <svg viewBox="0 0 24 24">
            <rect x="3" y="4" width="7" height="7" rx="1.5" />
            <rect x="14" y="4" width="7" height="7" rx="1.5" />
            <rect x="3" y="15" width="7" height="6" rx="1.5" />
            <rect x="14" y="15" width="7" height="6" rx="1.5" />
          </svg>
        </span>
        <span class="nav-text">媒体库</span>
      </RouterLink>

      <RouterLink
        :to="{ name: 'live' }"
        active-class=""
        exact-active-class=""
        :class="{ active: $route.name === 'live' }"
      >
        <span class="ico">
          <svg viewBox="0 0 24 24">
            <rect x="2" y="7" width="20" height="13" rx="2" fill="none" stroke="currentColor" stroke-width="1.8" />
            <path d="m17 2-5 5-5-5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        </span>
        <span class="nav-text">电视直播</span>
      </RouterLink>

      <RouterLink
        :to="{ name: 'library', query: { filter: 'continue' } }"
        active-class=""
        exact-active-class=""
        :class="{ active: $route.name === 'library' && $route.query.filter === 'continue' }"
      >
        <span class="ico">
          <svg viewBox="0 0 24 24">
            <circle cx="12" cy="12" r="9" />
            <path d="m10 8 6 4-6 4z" />
          </svg>
        </span>
        <span class="nav-text">继续观看</span>
        <span class="count">3</span>
      </RouterLink>

      <RouterLink
        :to="{ name: 'favorites' }"
        active-class=""
        exact-active-class=""
        :class="{ active: $route.name === 'favorites' }"
      >
        <span class="ico">
          <svg viewBox="0 0 24 24">
            <path d="m12 3 2.8 5.7 6.2.9-4.5 4.4 1.1 6.2-5.6-3-5.6 3 1.1-6.2L3 9.6l6.2-.9z" />
          </svg>
        </span>
        <span class="nav-text">我的收藏</span>
      </RouterLink>

      <RouterLink
        :to="{ name: 'library', query: { sort: 'added_at' } }"
        active-class=""
        exact-active-class=""
        :class="{ active: $route.name === 'library' && $route.query.sort === 'added_at' }"
      >
        <span class="ico">
          <svg viewBox="0 0 24 24">
            <circle cx="12" cy="12" r="9" />
            <path d="M12 7v5l3 2" />
          </svg>
        </span>
        <span class="nav-text">最近添加</span>
      </RouterLink>

      <RouterLink
        :to="{ name: 'library', query: { focus: 'search' } }"
        active-class=""
        exact-active-class=""
        :class="{ active: $route.name === 'library' && $route.query.focus === 'search' }"
      >
        <span class="ico">
          <svg viewBox="0 0 24 24">
            <circle cx="10.5" cy="10.5" r="6.5" />
            <path d="m16 16 5 5" />
          </svg>
        </span>
        <span class="nav-text">搜索</span>
      </RouterLink>
    </nav>

    <div class="nav-label">个人空间</div>
    <nav class="nav">
      <RouterLink
        :to="{ name: 'settings' }"
        active-class=""
        exact-active-class=""
        :class="{ active: $route.name === 'settings' }"
      >
        <span class="ico">
          <svg viewBox="0 0 24 24">
            <path d="M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Z" />
            <path d="m19.4 15 .1.1 1.2.9-1.2 2.1-1.5-.6a8 8 0 0 1-1.7 1l-.2 1.6h-2.4l-.3-1.6a8 8 0 0 1-1.7-1l-1.5.6L7 16l1.2-.9A8 8 0 0 1 8 13.5l-1.5-.7v-2.4l1.5-.3A8 8 0 0 1 8.7 8L7.5 6.8l1.2-2.1 1.5.6a8 8 0 0 1 1.7-1l.3-1.6h2.4l.2 1.6a8 8 0 0 1 1.7 1l1.5-.6 1.2 2.1L18 8a8 8 0 0 1 .5 2.1l1.5.3v2.4l-1.5.7a8 8 0 0 1-.1 1.5Z" />
          </svg>
        </span>
        <span class="nav-text">设置</span>
      </RouterLink>
    </nav>

    <div class="sidebar-spacer"></div>
  </aside>
</template>
