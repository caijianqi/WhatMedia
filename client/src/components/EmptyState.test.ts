/**
 * 空状态。
 *
 * 钉死的是"四种语义必须给出不同的下一步动作"：
 * 合成一个"暂无数据"会把用户晾在那里 —— 他不知道该去扫描、改筛选、
 * 重试，还是去配 TMDB Key。文案不同不是排版问题，是可操作性问题。
 */

import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import EmptyState, { type Kind } from './EmptyState.vue'

function mountWith(kind: Kind) {
  return mount(EmptyState, { props: { kind } })
}

describe('EmptyState', () => {
  it('四种语义给出四套不同的提示', () => {
    const texts = (['empty', 'none', 'error', 'unset'] as Kind[]).map(
      (k) => mountWith(k).text(),
    )
    // 任意两种的提示都不能相同，否则等于没分。
    for (let i = 0; i < texts.length; i++) {
      for (let j = i + 1; j < texts.length; j++) {
        expect(texts[i]).not.toBe(texts[j])
      }
    }
  })

  it('空库提示指向"扫描一个目录"', () => {
    expect(mountWith('empty').text()).toContain('扫描')
  })

  it('筛选无结果提示指向"换个条件"', () => {
    const t = mountWith('none').text()
    expect(t).toContain('筛选')
    // 不该再叫用户去扫描 —— 他库里有东西，只是被筛掉了。
    expect(t).not.toContain('扫描')
  })

  it('自定义文案覆盖预设', () => {
    const w = mount(EmptyState, { props: { kind: 'empty', title: '自定义标题' } })
    expect(w.text()).toContain('自定义标题')
  })

  it('没传 action 就不渲染按钮 —— 不该出现点了没反应的按钮', () => {
    expect(mountWith('empty').find('button').exists()).toBe(false)
    const w = mount(EmptyState, { props: { kind: 'error', action: '重试' } })
    expect(w.find('button').text()).toBe('重试')
  })

  it('点击动作按钮会抛出事件', async () => {
    const w = mount(EmptyState, { props: { kind: 'error', action: '重试', alt: '去设置' } })
    await w.findAll('button')[0]!.trigger('click')
    await w.findAll('button')[1]!.trigger('click')
    expect(w.emitted('action')).toHaveLength(1)
    expect(w.emitted('alt')).toHaveLength(1)
  })
})
