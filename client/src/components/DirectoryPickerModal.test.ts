import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import DirectoryPickerModal from './DirectoryPickerModal.vue'
import * as api from '../api/client'

vi.mock('../api/client', async () => {
  const actual = await vi.importActual<typeof import('../api/client')>('../api/client')
  return {
    ...actual,
    browseFsDir: vi.fn(),
    browseStorage: vi.fn(),
    listStorageAccounts: vi.fn().mockResolvedValue([]),
  }
})

describe('DirectoryPickerModal', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('打开时请求服务端物理路径并渲染子目录', async () => {
    vi.mocked(api.browseFsDir).mockResolvedValueOnce({
      current: 'D:\\Media',
      parent: 'D:\\',
      directories: [
        { name: 'Movies', path: 'D:\\Media\\Movies' },
        { name: 'TV', path: 'D:\\Media\\TV' },
      ],
    })

    const w = mount(DirectoryPickerModal, {
      props: {
        open: true,
        initialPath: 'D:\\Media',
      },
    })

    await flushPromises()

    expect(api.browseFsDir).toHaveBeenCalledWith('D:\\Media')
    expect(w.text()).toContain('D:\\Media')
    expect(w.text()).toContain('Movies')
    expect(w.text()).toContain('TV')
  })

  it('点击条目高亮选中并在确认时触发 select 事件', async () => {
    vi.mocked(api.browseFsDir).mockResolvedValueOnce({
      current: 'D:\\Media',
      parent: 'D:\\',
      directories: [
        { name: 'Movies', path: 'D:\\Media\\Movies' },
      ],
    })

    const w = mount(DirectoryPickerModal, {
      props: {
        open: true,
      },
    })

    await flushPromises()

    const item = w.find('.dir-item')
    expect(item.exists()).toBe(true)
    await item.trigger('click')

    const confirmBtn = w.find('.btn-confirm')
    await confirmBtn.trigger('click')

    expect(w.emitted('select')).toBeTruthy()
    expect(w.emitted('select')![0]).toEqual(['D:\\Media\\Movies', 'local'])
    expect(w.emitted('close')).toBeTruthy()
  })

  it('点击上一级按钮能够向上层导航', async () => {
    vi.mocked(api.browseFsDir).mockResolvedValueOnce({
      current: 'D:\\Media',
      parent: 'D:\\',
      directories: [],
    })

    const w = mount(DirectoryPickerModal, {
      props: {
        open: true,
      },
    })

    await flushPromises()

    vi.mocked(api.browseFsDir).mockResolvedValueOnce({
      current: 'D:\\',
      parent: null,
      directories: [{ name: 'Media', path: 'D:\\Media' }],
    })

    const upBtn = w.find('.nav-btn')
    await upBtn.trigger('click')
    await flushPromises()

    expect(api.browseFsDir).toHaveBeenLastCalledWith('D:\\')
  })

  it('指定 storageId 时浏览远程云盘目录树', async () => {
    vi.mocked(api.browseStorage).mockResolvedValueOnce([
      { name: '电影', path: '/电影', is_dir: true },
      { name: '电视剧', path: '/电视剧', is_dir: true },
      { name: 'readme.txt', path: '/readme.txt', is_dir: false },
    ])

    const w = mount(DirectoryPickerModal, {
      props: {
        open: true,
        storageId: 'storage-123',
        initialPath: '/',
      },
    })

    await flushPromises()

    expect(api.browseStorage).toHaveBeenCalledWith('storage-123', '/')
    expect(w.text()).toContain('电影')
    expect(w.text()).toContain('电视剧')
    // 非目录文件不应被列入文件夹列表
    expect(w.text()).not.toContain('readme.txt')
  })
})
