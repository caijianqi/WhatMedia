import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { mount } from '@vue/test-utils'
import { ref } from 'vue'

const currentRoute = ref({ name: 'home', fullPath: '/', query: {} })

vi.mock('vue-router', async (importOriginal) => {
  const actual = await importOriginal<typeof import('vue-router')>()
  return {
    ...actual,
    useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
    useRoute: () => currentRoute.value,
  }
})

vi.mock('./api/client', () => ({
  ApiError: class ApiError extends Error {
    constructor(
      public detail: { code: string; message: string },
      public status: number,
    ) {
      super(detail.message)
    }
    get isNetwork(): boolean {
      return this.status === 0
    }
  },
  search: vi.fn(),
  listNotifications: vi.fn().mockResolvedValue({ data: [] }),
  readNotification: vi.fn(),
  readAllNotifications: vi.fn(),
  health: vi.fn().mockResolvedValue({ status: 'ok', version: '0.1.0' }),
  whoami: vi.fn().mockResolvedValue({
    user: { id: 'u1', username: 'admin', role: 'admin', is_child: false },
  }),
}))

import App from './App.vue'
import AppSidebar from './components/AppSidebar.vue'
import TopBar from './components/TopBar.vue'

describe('App.vue 独立页面隔离', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('处于 login 路由时为独立全屏页面：绝不渲染侧栏 AppSidebar 和顶栏 TopBar', () => {
    currentRoute.value = { name: 'login', fullPath: '/login', query: {} }
    const wrapper = mount(App, {
      global: {
        stubs: {
          RouterView: { template: '<div id="test-login-view">登录视图</div>' },
          RouterLink: { template: '<a><slot /></a>' },
          AppSidebar: true,
          TopBar: true,
          GlobalToast: true,
        },
      },
    })

    // 侧栏与顶栏必须被隔离隐藏
    expect(wrapper.findComponent(AppSidebar).exists()).toBe(false)
    expect(wrapper.findComponent(TopBar).exists()).toBe(false)

    // 不包含侧栏里的导航关键词
    const html = wrapper.html()
    expect(html).not.toContain('发现内容')
    expect(html).not.toContain('媒体库')
    expect(html).not.toContain('直播')

    // 路由内容正常挂载
    expect(wrapper.find('#test-login-view').exists()).toBe(true)
  })

  it('处于 home 路由时在桌面端正常渲染侧栏与顶栏', () => {
    currentRoute.value = { name: 'home', fullPath: '/', query: {} }
    const wrapper = mount(App, {
      global: {
        stubs: {
          RouterView: { template: '<div id="test-home-view">首页视图</div>' },
          RouterLink: true,
          AppSidebar: true,
          TopBar: true,
          GlobalToast: true,
        },
      },
    })

    expect(wrapper.findComponent(AppSidebar).exists()).toBe(true)
    expect(wrapper.findComponent(TopBar).exists()).toBe(true)
    expect(wrapper.find('#test-home-view').exists()).toBe(true)
  })

  it('处于 play 路由时为沉浸播放模式：隐藏侧栏与顶栏', () => {
    currentRoute.value = { name: 'play', fullPath: '/play/123', query: {} }
    const wrapper = mount(App, {
      global: {
        stubs: {
          RouterView: { template: '<div id="test-player-view">播放视图</div>' },
          RouterLink: true,
          AppSidebar: true,
          TopBar: true,
          GlobalToast: true,
        },
      },
    })

    expect(wrapper.findComponent(AppSidebar).exists()).toBe(false)
    expect(wrapper.findComponent(TopBar).exists()).toBe(false)
  })
})
