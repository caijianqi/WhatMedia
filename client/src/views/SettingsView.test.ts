/**
 * 设置页测试。
 *
 * 验证：
 *   1. 服务端自检与硬件、存储、设置正常加载并渲染
 *   2. 存储与媒体库面板中完整呈现"开始使用 · 媒体库连接指南"
 *   3. 包含 CLI 扫描命令（whatmedia scan）以及 TMDB API 密钥配置指引
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

const mockJobStats = {
  done: 120,
  failed: 2,
  pending: 10,
  review: 5,
  running: 1,
  skipped: 3,
}

const mockLiveSources = [
  {
    id: 'src-1',
    name: 'GuoVin 直播源',
    kind: 'm3u',
    url: 'https://guovin.github.io/iptv-api/result.txt',
    epg_url: null,
    channel_count: 150,
    last_sync_at: '2026-10-20T00:00:00Z',
    sync_error: null,
  },
]

const mockStorageAccounts = [
  {
    id: 'storage-baidu',
    name: '百度网盘',
    driver: 'webdav',
    endpoint: 'http://127.0.0.1:5244/dav/baidu',
    auth_type: 'basic',
    username: 'admin',
    direct_stream: true,
    created_at: '2026-10-21T00:00:00Z',
    updated_at: '2026-10-21T00:00:00Z',
  },
]

const standardHandlers: Array<[string, () => unknown]> = [
  ['/health', () => mockHealth],
  ['/hardware/profile', () => mockHardware],
  ['/settings', () => mockSettings],
  ['/system/storage', () => mockStorage],
  ['/ai/tools', () => ({ data: [] })],
  ['/scrape/stats', () => mockJobStats],
  ['/scrape/runs', () => []],
  ['/live/sources', () => mockLiveSources],
  ['/storages', () => mockStorageAccounts],
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

  it('存储与媒体库面板展示"开始使用"与 whatmedia scan 引导', async () => {
    stubFetch(standardHandlers)

    const w = mountSettings({ tab: 'storage' })
    await flushPromises()

    expect(w.text()).toContain('开始使用')
    expect(w.text()).toContain('媒体库连接指南')
    expect(w.text()).toContain('把你的媒体目录交给 WhatMedia')
    expect(w.text()).toContain('whatmedia scan')
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

  it('展示 Web 扫盘表单且输入框右侧包含目录选择图标按钮，无快捷药丸标签', async () => {
    stubFetch([
      ...standardHandlers,
      ['/tasks', () => []],
      ['/scan', () => ({ task_id: 'task-1', library_id: 'lib-1' })],
    ])

    const w = mountSettings({ tab: 'storage' })
    await flushPromises()

    expect(w.text()).toContain('添加媒体库与扫盘入库')
    const pathWrap = w.find('.path-input-wrap')
    expect(pathWrap.exists()).toBe(true)

    // 右侧小图标存在
    const iconBtn = pathWrap.find('.dir-picker-icon-btn')
    expect(iconBtn.exists()).toBe(true)
    expect(iconBtn.text()).toBe('📁')

    // 确认无快捷药丸标签
    expect(w.find('.pill-tag').exists()).toBe(false)
    expect(w.find('.quick-path-pill').exists()).toBe(false)

    // 点击小图标触发打开目录选择器
    await iconBtn.trigger('click')
    await flushPromises()

    const picker = w.findComponent({ name: 'DirectoryPickerModal' })
    expect(picker.exists()).toBe(true)
    expect(picker.props('modelValue')).toBe(true)
  })

  it('输入路径后点击开始扫描可提交后台扫描入库任务', async () => {
    let scanPayload: unknown = null
    globalThis.fetch = vi.fn((url: string | URL | Request, init?: RequestInit) => {
      const u = String(url)
      if (u.includes('/scan')) {
        scanPayload = JSON.parse(String(init?.body))
        return Promise.resolve({
          ok: true,
          status: 202,
          text: () => Promise.resolve(JSON.stringify({ task_id: 'task-1', library_id: 'lib-1' })),
        } as Response)
      }
      for (const [needle, h] of standardHandlers) {
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
        text: () => Promise.resolve('[]'),
      } as Response)
    }) as unknown as typeof fetch

    const w = mountSettings({ tab: 'storage' })
    await flushPromises()

    const pathInput = w.find('.path-input')
    await pathInput.setValue('D:/Media/Movies')

    const scanBtn = w.find('.btn-start-scan')
    await scanBtn.trigger('click')
    await flushPromises()

    expect(scanPayload).toMatchObject({
      path: 'D:/Media/Movies',
      probe: true,
    })
    expect(w.text()).toContain('已向服务端提交入库任务')
  })

  it('点击保存媒体库按钮能正确调用 /libraries 接口并持久化保存路径', async () => {
    let createLibraryPayload: unknown = null
    globalThis.fetch = vi.fn((url: string | URL | Request, init?: RequestInit) => {
      const u = String(url)
      if (u.includes('/libraries') && init?.method === 'POST') {
        createLibraryPayload = JSON.parse(String(init?.body))
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () =>
            Promise.resolve(
              JSON.stringify({
                id: 'lib-101',
                name: 'Movies',
                root_path: 'D:/Media/Movies',
              }),
            ),
        } as Response)
      }
      for (const [needle, h] of standardHandlers) {
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
        text: () => Promise.resolve('[]'),
      } as Response)
    }) as unknown as typeof fetch

    const w = mountSettings({ tab: 'storage' })
    await flushPromises()

    const pathInput = w.find('.path-input')
    await pathInput.setValue('D:/Media/Movies')

    const saveBtn = w.find('.btn-save-lib')
    expect(saveBtn.exists()).toBe(true)
    await saveBtn.trigger('click')
    await flushPromises()

    expect(createLibraryPayload).toMatchObject({
      name: 'Movies',
      path: 'D:/Media/Movies',
    })
    expect(w.text()).toContain('已成功持久化保存')
  })

  it('后台扫描任务仅展示未完成的任务，已完成的任务自动过滤隐藏', async () => {
    const mockTasks = [
      {
        id: 'task-active',
        library: '影视库-未完成',
        progress: {
          done: 5,
          total: 20,
          added: 2,
          updated: 1,
          skipped: 2,
          failed: 0,
          finished: false,
          current: 'D:/Media/active.mp4',
        },
      },
      {
        id: 'task-done',
        library: '影视库-已完成',
        progress: {
          done: 100,
          total: 100,
          added: 50,
          updated: 10,
          skipped: 40,
          failed: 0,
          finished: true,
          current: 'D:/Media/done.mp4',
        },
      },
    ]

    globalThis.fetch = vi.fn((url: string | URL | Request) => {
      const u = String(url)
      if (u.includes('/tasks')) {
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () => Promise.resolve(JSON.stringify(mockTasks)),
        } as Response)
      }
      for (const [needle, h] of standardHandlers) {
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
        text: () => Promise.resolve('[]'),
      } as Response)
    }) as unknown as typeof fetch

    const w = mountSettings({ tab: 'storage' })
    await flushPromises()

    // 应该只展示未完成的任务 task-active，不展示已完成的 task-done
    const taskCards = w.findAll('.task-card')
    expect(taskCards.length).toBe(1)
    expect(taskCards[0].text()).toContain('影视库-未完成')
    expect(taskCards[0].text()).not.toContain('影视库-已完成')
  })

  it('可写设置按业务分类渲染为卡片，且包含 TMDB 接入指引卡片并支持折叠切换', async () => {
    const mockSettingsData = {
      entries: [
        {
          key: 'player.autoplay_next',
          label: '自动播放下一集',
          hint: '一集播完自动跳下一集',
          scope: 'user',
          kind: 'bool',
          choices: [],
          min: null,
          max: null,
          value: 'true',
          default: 'false',
          overridden: true,
          restart_required: false,
        },
        {
          key: 'metadata.tmdb_api_key',
          label: 'TMDB API 密钥 (v3 Key)',
          hint: 'The Movie Database (TMDB) v3 API Key',
          scope: 'server',
          kind: 'text',
          choices: [],
          min: null,
          max: 128,
          value: '',
          default: '',
          overridden: false,
          restart_required: true,
        },
        {
          key: 'transcode.max_concurrent',
          label: '最大并发转码',
          hint: '0 表示自动',
          scope: 'server',
          kind: 'int',
          choices: [],
          min: 0,
          max: 64,
          value: '2',
          default: '0',
          overridden: true,
          restart_required: true,
        },
      ],
      rejected: [],
    }

    stubFetch([
      ...standardHandlers.filter(([p]) => p !== '/settings'),
      ['/settings', () => mockSettingsData],
    ])

    const w = mountSettings({ tab: 'entries' })
    await flushPromises()

    expect(w.text()).toContain('播放与界面偏好')
    expect(w.text()).toContain('元数据与 TMDB 刮削凭据')
    expect(w.text()).toContain('The Movie Database (TMDB) 接入指引')
    expect(w.text()).toContain('申请 TMDB Key ↗')
    expect(w.text()).toContain('转码参数与存储缓存运维')

    // 转码参数默认折叠，body 处于隐藏状态
    const transcodeCard = w.findAll('.entries-card')[2]
    expect(transcodeCard.find('.fold-arrow').text()).toContain('展开')
    expect(transcodeCard.find('.entries-card-body').isVisible()).toBe(false)

    // 点击转码卡片头部触发展开
    await transcodeCard.find('.entries-card-head').trigger('click')
    await flushPromises()

    const updatedCard = w.findAll('.entries-card')[2]
    expect(updatedCard.find('.fold-arrow').text()).toContain('收起')
    expect(updatedCard.find('.entries-card-body').attributes('style') || '').not.toContain('display: none')
  })

  it('AI 助手面板展示服务商预设卡片，支持选用预设并执行连通性测试', async () => {
    const mockSettingsWithAi = {
      entries: [
        {
          key: 'ai.enabled',
          label: '启用 AI 助手',
          hint: '开启后可用',
          scope: 'server',
          kind: 'bool',
          choices: [],
          min: null,
          max: null,
          value: 'true',
          default: 'false',
          overridden: true,
          restart_required: true,
        },
        {
          key: 'ai.provider',
          label: 'AI 服务商 (Provider)',
          hint: '选择 Provider',
          scope: 'server',
          kind: 'enum',
          choices: ['deepseek', 'openai', 'qwen', 'ollama'],
          min: null,
          max: null,
          value: 'openai',
          default: 'openai',
          overridden: false,
          restart_required: true,
        },
        {
          key: 'ai.base_url',
          label: 'AI 接口接入点 (Base URL)',
          hint: '接入点',
          scope: 'server',
          kind: 'text',
          choices: [],
          min: null,
          max: 256,
          value: '',
          default: '',
          overridden: false,
          restart_required: true,
        },
        {
          key: 'ai.model',
          label: 'AI 模型标识 (Model)',
          hint: '模型名称',
          scope: 'server',
          kind: 'text',
          choices: [],
          min: null,
          max: 128,
          value: 'gpt-4o-mini',
          default: 'gpt-4o-mini',
          overridden: false,
          restart_required: true,
        },
      ],
      rejected: [],
    }

    const savedKeys: string[] = []
    globalThis.fetch = vi.fn((url: string | URL | Request, init?: RequestInit) => {
      const u = String(url)
      if (u.includes('/settings') && init?.method === 'PUT') {
        const body = JSON.parse(String(init?.body))
        savedKeys.push(body.key)
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () =>
            Promise.resolve(
              JSON.stringify({
                ...body,
                label: body.key,
                hint: '',
                scope: 'server',
                kind: 'text',
                choices: [],
                min: null,
                max: null,
                default: '',
                overridden: true,
                restart_required: true,
              }),
            ),
        } as Response)
      }
      if (u.includes('/settings')) {
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () => Promise.resolve(JSON.stringify(mockSettingsWithAi)),
        } as Response)
      }
      if (u.includes('/ai/chat')) {
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () =>
            Promise.resolve(
              JSON.stringify({
                agent: 'media_expert',
                answer: '您好！我是 WhatMedia 智能助理，已成功与 DeepSeek 模型建立连接。',
                degraded: false,
                traces: [],
              }),
            ),
        } as Response)
      }
      for (const [needle, h] of standardHandlers) {
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
        text: () => Promise.resolve('[]'),
      } as Response)
    }) as unknown as typeof fetch

    const w = mountSettings({ tab: 'ai' })
    await flushPromises()

    expect(w.text()).toContain('选择大模型服务商')
    expect(w.text()).toContain('DeepSeek')
    expect(w.text()).toContain('Ollama')

    // 点击选用 DeepSeek 预设
    const deepseekCard = w.findAll('.ai-preset-card').find((c) => c.text().includes('DeepSeek'))
    expect(deepseekCard).toBeTruthy()
    await deepseekCard!.trigger('click')
    await flushPromises()

    expect(savedKeys).toContain('ai.provider')

    // 点击测试连通性按钮
    const testBtn = w.find('.btn-test-ai')
    expect(testBtn.exists()).toBe(true)
    await testBtn.trigger('click')
    await flushPromises()

    expect(w.text()).toContain('模型连通成功')
    expect(w.text()).toContain('DeepSeek')
  })

  it('展示独立的"元数据与TMDB" Tab，支持手动刮削控制台与状态指标展示', async () => {
    let scrapeCalledWith: unknown = null
    const mockStorageWithLibs = {
      libraries: [
        {
          id: '11111111-1111-1111-1111-111111111111',
          name: '电影库',
          root_path: 'D:/Movies',
          item_count: 50,
          file_count: 50,
          bytes: 1024 * 1024 * 1024,
          reachable: true,
        },
      ],
      cache: { bytes: 0, unavailable: false },
      total_bytes: 1024 * 1024 * 1024,
    }

    globalThis.fetch = vi.fn((url: string | URL | Request, init?: RequestInit) => {
      const u = String(url)
      if (u.includes('/scrape') && init?.method === 'POST') {
        scrapeCalledWith = JSON.parse(String(init.body))
        return Promise.resolve({
          ok: true,
          status: 202,
          text: () =>
            Promise.resolve(
              JSON.stringify({
                run_id: '88888888-8888-8888-8888-888888888888',
                queued: 42,
              }),
            ),
        } as Response)
      }
      if (u.includes('/system/storage')) {
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () => Promise.resolve(JSON.stringify(mockStorageWithLibs)),
        } as Response)
      }
      for (const [needle, h] of standardHandlers) {
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

    const w = mountSettings({ tab: 'metadata' })
    await flushPromises()

    // 验证侧栏 Tab 和标题
    expect(w.text()).toContain('元数据与TMDB')
    expect(w.text()).toContain('手动刮削控制台')
    expect(w.text()).toContain('TMDB 凭据与网络加速配置')

    // 验证指标看板
    expect(w.text()).toContain('120') // done
    expect(w.text()).toContain('已完成 (Done)')
    expect(w.text()).toContain('失败项 (Failed)')

    // 触发手动刮削
    const scrapeBtn = w.findAll('.btn-start-scan').find((b) => b.text().includes('开始手动刮削'))
    expect(scrapeBtn).toBeTruthy()
    await scrapeBtn!.trigger('click')
    await flushPromises()

    expect(scrapeCalledWith).toEqual({
      library_id: null,
      retry_failed: false,
      force: false,
    })
    expect(w.text()).toContain('刮削任务已受理')
    expect(w.text()).toContain('42 项条目')
  })

  it('AI 模型服务商预设列表中包含 MiniMax，并可正常点击选用', async () => {
    let savedProviderValue: string | null = null
    const mockSettingsWithMiniMax = {
      entries: [
        {
          key: 'ai.provider',
          value: 'openai',
          scope: 'server',
          kind: 'choice',
          choices: ['deepseek', 'openai', 'qwen', 'glm', 'minimax', 'ollama', 'vllm', 'custom'],
          min: null,
          max: null,
          default: 'openai',
          overridden: false,
          restart_required: true,
        },
        {
          key: 'ai.base_url',
          value: 'https://api.openai.com/v1',
          scope: 'server',
          kind: 'text',
          choices: [],
          min: null,
          max: null,
          default: 'https://api.openai.com/v1',
          overridden: false,
          restart_required: true,
        },
        {
          key: 'ai.model',
          value: 'gpt-4o-mini',
          scope: 'server',
          kind: 'text',
          choices: [],
          min: null,
          max: null,
          default: 'gpt-4o-mini',
          overridden: false,
          restart_required: true,
        },
      ],
      rejected: [],
    }

    globalThis.fetch = vi.fn((url: string | URL | Request, init?: RequestInit) => {
      const u = String(url)
      if (u.includes('/settings') && init?.method === 'PUT') {
        const body = JSON.parse(String(init.body))
        if (body.key === 'ai.provider') {
          savedProviderValue = body.value
        }
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () =>
            Promise.resolve(
              JSON.stringify({
                key: body.key,
                value: body.value,
                scope: 'server',
                kind: 'text',
                choices: [],
                min: null,
                max: null,
                default: '',
                overridden: true,
                restart_required: true,
              }),
            ),
        } as Response)
      }
      if (u.includes('/settings')) {
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () => Promise.resolve(JSON.stringify(mockSettingsWithMiniMax)),
        } as Response)
      }
      for (const [needle, h] of standardHandlers) {
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
        text: () => Promise.resolve('[]'),
      } as Response)
    }) as unknown as typeof fetch

    const w = mountSettings({ tab: 'ai' })
    await flushPromises()

    // 验证 MiniMax 预设卡片存在
    expect(w.text()).toContain('MiniMax')
    expect(w.text()).toContain('万亿 MoE · 超长上下文')

    // 选中 MiniMax 预设
    const minimaxCard = w.findAll('.ai-preset-card').find((c) => c.text().includes('MiniMax'))
    expect(minimaxCard).toBeTruthy()
    await minimaxCard!.trigger('click')
    await flushPromises()

    expect(savedProviderValue).toBe('minimax')
  })

  it('支持切换到电视直播选项卡并展示已配置的直播源与添加表单', async () => {
    stubFetch(standardHandlers)
    const w = mountSettings({ tab: 'live' })
    await flushPromises()

    const liveSection = w.find('[data-test="live-settings"]')
    expect(liveSection.exists()).toBe(true)
    expect(liveSection.text()).toContain('电视直播 (Live TV)')
    expect(liveSection.text()).toContain('添加直播订阅源')
    expect(liveSection.text()).toContain('GuoVin 直播源')
    expect(liveSection.text()).toContain('150 个频道')
  })

  it('展示云盘与多存储源管理模块，支持展示已配置存储源并切换预设', async () => {
    stubFetch(standardHandlers)
    const w = mountSettings({ tab: 'storage' })
    await flushPromises()

    expect(w.text()).toContain('云盘与多存储源管理')
    expect(w.text()).toContain('百度网盘')
    expect(w.text()).toContain('http://127.0.0.1:5244/dav/baidu')

    // 点击添加存储源按钮展开表单
    const addBtn = w.find('.btn-add-storage')
    expect(addBtn.exists()).toBe(true)
    await addBtn.trigger('click')
    await flushPromises()

    // 检查预设按钮存在
    const pills = w.findAll('.preset-pill')
    expect(pills.length).toBeGreaterThanOrEqual(4)
    expect(w.text()).toContain('夸克网盘')
    expect(w.text()).toContain('迅雷云盘')
  })

  it('保存媒体库时若选择云盘存储源，正确携带 storage_provider 参数', async () => {
    let createLibraryPayload: unknown = null
    globalThis.fetch = vi.fn((url: string | URL | Request, init?: RequestInit) => {
      const u = String(url)
      if (u.includes('/libraries') && init?.method === 'POST') {
        createLibraryPayload = JSON.parse(String(init?.body))
        return Promise.resolve({
          ok: true,
          status: 200,
          text: () =>
            Promise.resolve(
              JSON.stringify({
                id: 'lib-cloud-1',
                name: '电影',
                root_path: '/电影',
                storage_provider: 'storage-baidu',
              }),
            ),
        } as Response)
      }
      for (const [needle, h] of standardHandlers) {
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
        text: () => Promise.resolve('[]'),
      } as Response)
    }) as unknown as typeof fetch

    const w = mountSettings({ tab: 'storage' })
    await flushPromises()

    // 选择云端存储源
    const select = w.find('.scan-select')
    expect(select.exists()).toBe(true)
    await select.setValue('storage-baidu')

    // 输入网盘路径
    const pathInput = w.find('.path-input')
    await pathInput.setValue('/电影')

    // 点击保存媒体库
    const saveBtn = w.find('.btn-save-lib')
    await saveBtn.trigger('click')
    await flushPromises()

    expect(createLibraryPayload).toEqual({
      name: '电影',
      path: '/电影',
      storage_provider: 'storage-baidu',
    })
    expect(w.text()).toContain('已成功持久化保存')
    expect(w.text()).toContain('百度网盘')
  })
})




