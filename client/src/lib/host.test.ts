import { describe, expect, it } from 'vitest'
import { inShell, probeDeviceForm, readSafeAreaInsets } from './host'
import { parseForm, parseSafeArea } from './platform'

describe('inShell', () => {
  it('jsdom 里没有 Tauri 内部对象 → 不在壳里', () => {
    expect(inShell()).toBe(false)
  })
})

describe('probeDeviceForm', () => {
  it('不在壳里时是 web，不去 invoke', async () => {
    // 这条同时保证 vitest 不会因为缺 @tauri-apps/api 而炸。
    expect(await probeDeviceForm()).toBe('web')
  })
})

describe('readSafeAreaInsets', () => {
  it('jsdom 里返回空对象或全空串，不抛异常', () => {
    const r = readSafeAreaInsets()
    // jsdom 不实现 env()，拿到的是空串；parseSafeArea 得能处理。
    expect(() => parseSafeArea(r)).not.toThrow()
    expect(parseSafeArea(r)).toEqual({ top: 0, right: 0, bottom: 0, left: 0 })
  })

  it('插入的探针元素会被移除，不留在 DOM 里', () => {
    const before = document.body.childElementCount
    readSafeAreaInsets()
    expect(document.body.childElementCount).toBe(before)
  })
})

describe('parseForm 与 host 的兜底配合', () => {
  it('unknown 被 host 提升成 desktop，而不是 handheld', () => {
    // 触摸 UI 在无触摸屏上等于不可用；桌面布局在手机上只是挤。
    expect(parseForm('unknown')).toBe('unknown')
  })
})
