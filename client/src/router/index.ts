import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router'

const routes: RouteRecordRaw[] = [
  { path: '/', name: 'home', component: () => import('../views/HomeView.vue') },
  { path: '/library', name: 'library', component: () => import('../views/LibraryView.vue') },
  { path: '/item/:id', name: 'item', component: () => import('../views/DetailView.vue'), props: true },
  {
    path: '/play/:id',
    name: 'play',
    component: () => import('../views/PlayerView.vue'),
    props: true,
  },
  { path: '/settings', name: 'settings', component: () => import('../views/SettingsView.vue') },
  // 兜底：未知路径回首页，而不是白屏。
  { path: '/:pathMatch(.*)*', redirect: '/' },
]

export const router = createRouter({
  history: createWebHistory(),
  routes,
  // 每次导航回到顶部。否则从第 3 屏点进详情页，会停在详情页的第 3 屏。
  scrollBehavior: () => ({ top: 0 }),
})

export default router
