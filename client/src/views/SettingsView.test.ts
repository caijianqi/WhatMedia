/**
 * 设置页测试。
 *
 * 验证：
 *   1. 服务端自检与硬件、存储、设置正常加载并渲染
 *   2. 存储与媒体库面板中完整呈现"开始使用 · 媒体库连接指南"
 *   3. 包含 CLI 扫描命令（whatmedia-cli scan）以及 TMDB API 密钥配置指引
 *   4. 支持按操作系统（Windows / Linux / macOS）动态切换命令路径
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { createPinia } from 'pinia'
import SettingsView from './SettingsView.vue'

let mockQuery: Record<string, string> = {}

vi.mock('vue-router', async () => {
  const actual = await vi.importActual<typeof import('vue-router')>('vue-router')
  return {
    ...actual,
    useRoute: () => ({
      query: mockQuery,
    }),
  }
})

const mockHealth = {
  status: 'ok',
  version: '0.1.0',
  arch: 'x86_64',
  os: 'windows',
  uptime_secs: 7200,
}

const mockHardware = {
  profiles: [],
}

const mockSettings = {
  entries: [],
  rejected: [],
}

const mockStorage = {
  libraries: [],
  cache: { bytes: 1024 * 1024 * 50, unavailable: false },
  total_bytes: 1024 * 1024 * 50,
}

function stubFetch(handlers: Array<[string, () => unknown]>) {
  globalThis.fetch = vi.fn((url: string | URL | Request) => {
    const u = String(url)
    for (const [needle, h] of handlers) {
      if (u.includes(needle)) {
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () => Promise.resolve(JSON.stringify(h())),
        } as Response)
      }
    }
    return Promise.resolve({
      ok: true,
      status: 200,
      text: () => Promise.resolve('{}'),
    } as Response)
  }) as unknown as typeof fetch
}

const standardHandlers: Array<[string, () => unknown]> = [
  ['/health', () => mockHealth],
  ['/hardware/profile', () => mockHardware],
  ['/settings', () => mockSettings],
  ['/system/storage', () => mockStorage],
  ['/ai/tools', () => ({ data: [] })],
]

function mountSettings(routeQuery: Record<string, string> = {}) {
  mockQuery = routeQuery
  return mount(SettingsView, {
    global: {
      plugins: [createPinia()],
      stubs: {
        RouterLink: { template: '<a><slot /></a>' },
      },
    },
  })
}

beforeEach(() => {
  vi.restoreAllMocks()
  mockQuery = {}
})

describe('SettingsView', () => {
  it('正常加载并渲染服务端健康状态与设置侧栏', async () => {
    stubFetch(standardHandlers)

    const w = mountSettings()
    await flushPromises()

    expect(w.text()).toContain('服务端')
    expect(w.text()).toContain('windows')
    expect(w.text()).toContain('存储与媒体库')
  })

  it('存储与媒体库面板展示"开始使用"与 whatmedia-cli scan 引导', async () => {
    stubFetch(standardHandlers)

    const w = mountSettings({ tab: 'storage' })
    await flushPromises()

    expect(w.text()).toContain('开始使用')
    expect(w.text()).toContain('媒体库连接指南')
    expect(w.text()).toContain('把你的媒体目录交给 WhatMedia')
    expect(w.text()).toContain('whatmedia-cli scan')
    expect(w.text()).toContain('WHATMEDIA_METADATA_TMDB_API_KEY')
  })

  it('点击操作系统切换按钮能切换对应的 CLI 命令路径', async () => {
    stubFetch(standardHandlers)

    const w = mountSettings({ tab: 'storage' })
    await flushPromises()

    // 默认 Windows
    expect(w.text()).toContain('D:/Media')

    // 切换到 Linux
    const linuxBtn = w.findAll('.os-btn').find((b) => b.text().includes('Linux'))
    expect(linuxBtn).toBeTruthy()
    await linuxBtn!.trigger('click')
    await flushPromises()

    expect(w.text()).toContain('/media')

    // 切换到 macOS
    const macBtn = w.findAll('.os-btn').find((b) => b.text().includes('macOS'))
    expect(macBtn).toBeTruthy()
    await macBtn!.trigger('click')
    await flushPromises()

    expect(w.text()).toContain('/Users/username/Movies')
  })
})
