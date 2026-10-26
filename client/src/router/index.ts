import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router'
import { useSessionStore } from '../stores/session'

/**
 * 路由守卫：没登录一律去登录页。
 *
 * **守卫生在这里、而不是每个页面各自判断**，理由和后端把鉴权放进中间件一样：
 * 逐页判断的方案里，新增页面的人会忘掉那一行，而忘掉的后果是静默地
 * 把一整页开放给没登录的人 —— 这类 bug 平时看不出来。
 *
 * 放行名单只有登录页本身。其余一律要会话。
 */
const PUBLIC_ROUTES = new Set(['login'])

const routes: RouteRecordRaw[] = [
  { path: '/login', name: 'login', component: () => import('../views/LoginView.vue') },
  { path: '/', name: 'home', component: () => import('../views/HomeView.vue') },
  { path: '/library', name: 'library', component: () => import('../views/LibraryView.vue') },
  { path: '/favorites', name: 'favorites', component: () => import('../views/FavoritesView.vue') },
  { path: '/item/:id', name: 'item', component: () => import('../views/DetailView.vue'), props: true },
  {
    path: '/play/:id',
    name: 'play',
    component: () => import('../views/PlayerView.vue'),
    props: true,
  },
  { path: '/settings', name: 'settings', component: () => import('../views/SettingsView.vue') },
  { path: '/assist', name: 'assist', component: () => import('../views/AssistView.vue') },
  { path: '/live', name: 'live', component: () => import('../views/LiveView.vue') },
  { path: '/aggregator', name: 'aggregator', component: () => import('../views/AggregatorView.vue') },
  // 兜底：未知路径回首页，而不是白屏。
  { path: '/:pathMatch(.*)*', redirect: '/' },
]

export const router = createRouter({
  history: createWebHistory(),
  routes,
  // 每次导航回到顶部。否则从第 3 屏点进详情页，会停在详情页的第 3 屏。
  scrollBehavior: () => ({ top: 0 }),
})

router.beforeEach(async (to) => {
  const session = useSessionStore()

  // 首屏自检：顺带判断这个服务端是不是"还没装过"。
  // 只在完全没有线索时做一次，之后靠 isAuthed 决定。
  if (!session.isAuthed && !session.needsSetup && !session.checking) {
    await session.bootstrap()
  }

  if (PUBLIC_ROUTES.has(String(to.name))) {
    // 已经登录了还去登录页 —— 直接回首页，别让人对着一个用不上的表单。
    return session.isAuthed ? { name: 'home' } : true
  }
  if (session.isAuthed) return true
  return { name: 'login', query: { redirect: to.fullPath } }
})

export default router
