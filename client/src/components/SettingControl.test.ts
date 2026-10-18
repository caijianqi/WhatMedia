/**
 * 设置控件。
 *
 * 钉死两件事：
 *   · 控件形态**由服务端给的 kind 决定**。前端自己猜"这个值像不像布尔"
 *     会让两边规则漂移 —— 服务端加了枚举值而前端还在渲染文本框。
 *   · "改完要重启"必须显示出来。静默地"保存成功但不生效"是最难查的一类问题。
 */

import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import SettingControl from './SettingControl.vue'
import type { SettingEntry } from '../api/client'

function entry(over: Partial<SettingEntry> = {}): SettingEntry {
  return {
    key: 'ai.enabled',
    label: '启用 AI 助手',
    hint: '没配模型时打开也没用。',
    scope: 'server',
    kind: 'bool',
    choices: [],
    min: null,
    max: null,
    value: 'false',
    default: 'false',
    overridden: false,
    restart_required: true,
    ...over,
  }
}

function mountWith(e: SettingEntry) {
  return mount(SettingControl, { props: { entry: e } })
}

describe('SettingControl', () => {
  it('bool 渲染成开关，点一下提交反向值', async () => {
    const w = mountWith(entry({ kind: 'bool', value: 'false' }))
    const sw = w.get('[role="switch"]')
    expect(sw.attributes('aria-checked')).toBe('false')
    await sw.trigger('click')
    expect(w.emitted('save')?.[0]).toEqual(['true'])
  })

  it('enum 渲染成下拉，选项是服务端给的 choices', async () => {
    const w = mountWith(
      entry({ kind: 'enum', value: 'direct', choices: ['direct', '1080p', '720p'] }),
    )
    const opts = w.findAll('option').map((o) => o.attributes('value'))
    expect(opts).toEqual(['direct', '1080p', '720p'])
    await w.get('select').setValue('720p')
    expect(w.emitted('save')?.[0]).toEqual(['720p'])
  })

  it('int 渲染成数字框并带上服务端的取值范围', () => {
    const w = mountWith(entry({ kind: 'int', value: '4', min: 0, max: 64 }))
    const input = w.get('input')
    expect(input.attributes('type')).toBe('number')
    expect(input.attributes('min')).toBe('0')
    expect(input.attributes('max')).toBe('64')
  })

  it('text 渲染成文本框', () => {
    const w = mountWith(entry({ kind: 'text', value: 'zh-CN', default: 'zh-CN' }))
    expect(w.get('input').attributes('type')).toBe('text')
  })

  it('需要重启时必须明说', () => {
    expect(mountWith(entry({ restart_required: true })).text()).toContain('重启')
    // 用户级偏好是立刻生效的，写"要重启"就是误导。
    expect(mountWith(entry({ restart_required: false })).text()).not.toContain('重启')
  })

  it('被改动过才显示"恢复默认"，且带上默认值', async () => {
    expect(mountWith(entry({ overridden: false })).text()).not.toContain('恢复默认')

    const w = mountWith(entry({ overridden: true, default: 'zh-CN' }))
    expect(w.text()).toContain('恢复默认')
    expect(w.text()).toContain('zh-CN')
    await w.get('button:not([role="switch"])').trigger('click')
    expect(w.emitted('reset')).toHaveLength(1)
  })

  it('服务端回写新值时本地草稿同步', async () => {
    const w = mountWith(entry({ value: 'false' }))
    await w.setProps({ entry: entry({ value: 'true' }) })
    expect(w.get('[role="switch"]').attributes('aria-checked')).toBe('true')
  })
})
