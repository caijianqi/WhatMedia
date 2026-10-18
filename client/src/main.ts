import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import router from './router'
import { setUnauthorizedHandler } from './api/client'
import { useSessionStore } from './stores/session'
import './style.css'

const app = createApp(App).use(createPinia()).use(router)

/**
 * 会话失效的集中出口。
 *
 * `api` 模块在收到 401 时回调这里。之所以要注册而不是让每个页面自己听：
 * "票据过期"这件事在一屏里可能同时发生十次（列表、海报、评分各打一次请求），
 * 若各自处理就会弹十个提示、跳十次登录页。
 */
setUnauthorizedHandler(() => {
  const session = useSessionStore()
  session.forget()
  void router.push({ name: 'login' })
})

app.mount('#app')
