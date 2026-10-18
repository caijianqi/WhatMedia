<script setup lang="ts">
/**
 * 应用外壳：侧栏 + 顶栏 + 路由出口。
 *
 * 三条与设备形态相关的约束：
 *
 *   1. **播放器页不带任何栏**。播放器是全屏 fixed 的，
 *      留一条顶栏会在顶部挤出一条黑边，而且返回按钮播放器页自己有。
 *   2. **手持设备用底部标签栏**。单手握持时屏幕上半部分够不着，
 *      底部才是拇指能到的区域 —— 这是手机与桌面的本质差异。
 *      手机上同时也没有侧栏：238px 在 400px 宽的屏上会吃掉一半。
 *   3. **TV 上不放搜索框**。遥控器没法舒服地输入中文，
 *      给一个输入框只会让人卡在那里 —— 侧栏的分组导航才是 TV 的主路径。
 *
 * 安全区必须吃到布局里。不处理的后果是 iPhone 上底栏被手势条盖住一半，
 * 而开发者拿不带刘海的测试机完全看不出来。
 */
import { computed, onBeforeUnmount, onMounted } from 'vue'
import { useRoute } from 'vue-router'
import AppSidebar from './components/AppSidebar.vue'
import TopBar from './components/TopBar.vue'
import GlobalToast from './components/GlobalToast.vue'
import { useLayoutStore } from './stores/layout'
import { useSessionStore } from './stores/session'

const route = useRoute()
const layout = useLayoutStore()
// session 只在模板里被读（顶栏的身份 chip 在 TopBar 内），
// 但这里必须建一次：它是路由守卫与 api 层未授权回调的依赖。
useSessionStore()

const isStandalone = computed(() => route.name === 'play' || route.name === 'login')

/**
 * 手机上没有侧栏，导航退回底栏。
 * 独立页面（播放器沉浸视听、登录页独立鉴权）不展示任何侧栏、顶栏或底栏。
 */
const showSidebar = computed(() => !isStandalone.value && !layout.isHandheld)
const showBottomNav = computed(() => !isStandalone.value && layout.isHandheld)
const showTopBar = computed(() => !isStandalone.value)

const bottomNav = [
  { name: 'home', label: '首页', icon: 'M3 10.6 12 3.5l9 7.1M5.5 9.4V20h13V9.4' },
  { name: 'library', label: '媒体库', icon: 'M4 5.5h5.5v13H4zM14.5 5.5H20v13h-5.5M11.5 5.5h1M11.5 18.5h1' },
  { name: 'live', label: '直播', icon: 'M3 7h18v10H3zM8 3.5l4 3.5 4-3.5M9.5 20h5' },
  {
    name: 'favorites',
    label: '收藏',
    icon: 'M12 20s-7-4.4-7-9.3A3.9 3.9 0 0 1 12 8a3.9 3.9 0 0 1 7 2.7C19 15.6 12 20 12 20z',
  },
  { name: 'settings', label: '设置', icon: 'M4 7h10M18 7h2M4 12h4M12 12h8M4 17h12M20 17h0M15 7v0' },
]

onMounted(() => void layout.init())
onBeforeUnmount(() => layout.dispose())

/** 主内容区的内边距：仅在真实异形屏安全区生效，不覆盖 .main 原生 padding (0 34px 40px) */
const contentStyle = computed(() => {
  if (isStandalone.value) return undefined
  const s = layout.metrics.safe
  // 普通桌面端全为 0 时返回 undefined，把排版留白完整交给 CSS .main 媒体查询
  if (!s.top && !s.right && !s.bottom && !s.left) {
    return undefined
  }
  return {
    paddingTop: s.top ? `${s.top}px` : undefined,
    paddingRight: s.right ? `max(14px, ${s.right}px)` : undefined,
    paddingBottom: s.bottom ? `max(40px, ${s.bottom}px)` : undefined,
    paddingLeft: s.left ? `max(14px, ${s.left}px)` : undefined,
  }
})

/** 底栏还要再留出底部安全区，否则会被手势条压住。 */
const bottomBarStyle = computed(() => ({
  paddingBottom: `max(6px, ${layout.metrics.safe.bottom}px)`,
}))

/** TV 上整体放大字号。用 CSS 变量下发，避免每个组件各写一遍。 */
const rootStyle = computed(() => ({
  '--wm-font-scale': String(layout.metrics.fontScale),
}))
</script>

<template>
  <div :class="isStandalone ? 'app-standalone' : 'app'" :style="rootStyle">
    <AppSidebar v-if="showSidebar" />

    <main class="main" :style="contentStyle">
      <TopBar v-if="showTopBar" :show-search="!layout.isTv" />
      <RouterView />
    </main>

    <!-- 移动端原生悬浮底栏 -->
    <nav v-if="showBottomNav" class="mobile-bottom" :style="bottomBarStyle">
      <RouterLink
        v-for="n in bottomNav"
        :key="n.name"
        :to="{ name: n.name }"
        active-class="active"
      >
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path :d="n.icon" />
        </svg>
        <span>{{ n.label }}</span>
      </RouterLink>
    </nav>

    <!-- 全局浮动提示 -->
    <GlobalToast />
  </div>
</template>

<style scoped>
.app-standalone {
  min-height: 100vh;
  width: 100%;
}
.app-standalone .main {
  padding: 0;
  width: 100%;
  min-height: 100vh;
}
</style>
