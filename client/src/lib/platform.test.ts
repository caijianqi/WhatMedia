import { describe, expect, it } from 'vitest'
import {
  MIN_TOUCH_TARGET,
  TV_OVERSCAN_RATIO,
  columnsFor,
  layoutFor,
  orientationOf,
  parseForm,
  parseSafeArea,
  seekSecondsForDrag,
  tvOverscan,
  volumeDeltaForDrag,
  isNarrow,
  type DeviceForm,
  type Viewport,
} from './platform'

const phone: Viewport = { width: 390, height: 844 }
const tablet: Viewport = { width: 820, height: 1180 }
const desktop: Viewport = { width: 1920, height: 1080 }
const tv: Viewport = { width: 1920, height: 1080 }

describe('orientationOf', () => {
  it('正方形算竖屏 —— 分屏时会出现，竖屏布局更不容易挤爆', () => {
    expect(orientationOf({ width: 600, height: 600 })).toBe('portrait')
  })

  it('横竖屏按长短边判定，不按绝对宽度', () => {
    // 横屏手机：宽 844 高 390。按宽度判会错。
    expect(orientationOf({ width: 844, height: 390 })).toBe('landscape')
    expect(orientationOf(phone)).toBe('portrait')
  })
})

describe('isNarrow', () => {
  it('横屏手机也是窄屏', () => {
    // 手机上横屏时短边只有 390，按宽度（844）判会走桌面布局 → 两栏导航挤成一团。
    expect(isNarrow({ width: 844, height: 390 })).toBe(true)
    expect(isNarrow(desktop)).toBe(false)
  })
})

describe('columnsFor', () => {
  it('手机上列数明显少于同宽桌面', () => {
    expect(columnsFor('handheld', phone)).toBe(2)
    expect(columnsFor('desktop', desktop)).toBe(8)
  })

  it('TV 上列数刻意压低 —— 遥控器横向移动成本高', () => {
    // 同样的 1920 宽，桌面 8 列，TV 只有 6 列。
    expect(columnsFor('tv', tv)).toBeLessThan(columnsFor('desktop', desktop))
    expect(columnsFor('tv', tv)).toBe(6)
  })

  it('TV 在最窄的 720p 下也不少于 4 列', () => {
    expect(columnsFor('tv', { width: 1280, height: 720 })).toBe(5)
    expect(columnsFor('tv', { width: 1024, height: 768 })).toBe(4)
  })

  it('手机横屏变宽时列数增加', () => {
    expect(columnsFor('handheld', { width: 844, height: 390 })).toBe(4)
  })
})

describe('layoutFor', () => {
  it('手机用底部标签栏，桌面与 TV 用顶栏', () => {
    expect(layoutFor('handheld', phone).nav).toBe('bottom')
    expect(layoutFor('desktop', desktop).nav).toBe('top')
    expect(layoutFor('tv', tv).nav).toBe('top')
  })

  it('只有桌面与 web 有悬停态 —— TV 没有指针，悬停永远不会触发', () => {
    expect(layoutFor('desktop', desktop).hover).toBe(true)
    expect(layoutFor('tv', tv).hover).toBe(false)
    expect(layoutFor('handheld', phone).hover).toBe(false)
  })

  it('TV 上字号放大', () => {
    expect(layoutFor('tv', tv).fontScale).toBeGreaterThan(1)
    expect(layoutFor('handheld', phone).fontScale).toBe(1)
  })

  it('TV 上安全区走 overscan，不用系统的刘海安全区', () => {
    const withSafe = { top: 47, right: 0, bottom: 34, left: 0 }
    const m = layoutFor('tv', tv, withSafe)
    expect(m.safe.top).toBe(Math.round(1080 * TV_OVERSCAN_RATIO))
    // 底部留得更多：很多电视底部裁得比其它边多
    expect(m.safe.bottom).toBeGreaterThan(m.safe.top)
  })

  it('手机上安全区原样透传', () => {
    const withSafe = { top: 47, right: 0, bottom: 34, left: 0 }
    expect(layoutFor('handheld', phone, withSafe).safe).toEqual(withSafe)
  })

  it('触摸目标不小于 48', () => {
    expect(layoutFor('handheld', phone).minTarget).toBe(MIN_TOUCH_TARGET)
    expect(MIN_TOUCH_TARGET).toBeGreaterThanOrEqual(44)
  })

  it('平板竖屏列数多于手机，但不追平桌面', () => {
    const c = columnsFor('handheld', tablet)
    expect(c).toBeGreaterThan(columnsFor('handheld', phone))
    expect(c).toBeLessThan(columnsFor('desktop', desktop))
  })
})

describe('tvOverscan', () => {
  it('4K 与 720p 的边距按短边比例算，不是固定像素', () => {
    const a = tvOverscan({ width: 3840, height: 2160 })
    const b = tvOverscan({ width: 1280, height: 720 })
    // 4K 的边距应当明显更大（短边更长）
    expect(a.top).toBeGreaterThan(b.top)
  })
})

describe('parseSafeArea', () => {
  it('解析带 px 后缀的值', () => {
    expect(parseSafeArea({ top: '47px', bottom: '34px' })).toEqual({
      top: 47,
      right: 0,
      bottom: 34,
      left: 0,
    })
  })

  it('解析不出的值一律归 0，不猜', () => {
    // `viewport-fit=cover` 没配时 env() 恒为 0px；
    // 某些内核还会返回 `calc(...)` —— 解析不了就当 0，
    // 猜错方向的后果是"内容被刘海盖住"或"整个页面上移一截"。
    expect(parseSafeArea({ top: 'calc(1px + 2px)', bottom: undefined })).toEqual({
      top: 0,
      right: 0,
      bottom: 0,
      left: 0,
    })
    expect(parseSafeArea({ top: '-10px' }).top).toBe(0)
    expect(parseSafeArea({}).top).toBe(0)
    expect(parseSafeArea({ top: 'auto' }).top).toBe(0)
  })
})

describe('parseForm', () => {
  it('认得宿主报告的形态', () => {
    expect(parseForm('handheld')).toBe('handheld')
    expect(parseForm('tv')).toBe('tv')
    expect(parseForm('desktop')).toBe('desktop')
  })

  it('认不出来归 unknown，绝不猜成 handheld', () => {
    // 猜成 handheld 的后果：TV 用户拿到触摸 UI，而遥控器不产生触摸事件
    // → 完全无法操作，且没有任何报错。
    expect(parseForm('unknown')).toBe('unknown')
    expect(parseForm(null)).toBe('unknown')
    expect(parseForm('toaster')).toBe('unknown')
  })
})

describe('seekSecondsForDrag', () => {
  it('非线性：小幅拖动精细，大幅拖动快速', () => {
    const duration = 7200 // 2 小时
    const small = seekSecondsForDrag(0.1, duration)
    const large = seekSecondsForDrag(0.5, duration)
    // 线性映射下 large 应该是 small 的 5 倍；非线性下应当更大。
    expect(large).toBeGreaterThan(small * 5)
  })

  it('负向拖动往回跳', () => {
    expect(seekSecondsForDrag(-0.5, 7200)).toBeLessThan(0)
  })

  it('超出屏幕的拖动被夹住，不会跳到片外', () => {
    expect(seekSecondsForDrag(5, 7200)).toBe(7200)
    expect(seekSecondsForDrag(-5, 7200)).toBe(-7200)
  })

  it('时长未知时不跳', () => {
    expect(seekSecondsForDrag(0.5, 0)).toBe(0)
    expect(seekSecondsForDrag(Number.NaN, 7200)).toBe(0)
  })
})

describe('volumeDeltaForDrag', () => {
  it('整屏高度对应满量程，且夹在 ±1', () => {
    expect(volumeDeltaForDrag(1)).toBe(1)
    expect(volumeDeltaForDrag(-1)).toBe(-1)
    expect(volumeDeltaForDrag(3)).toBe(1)
    expect(volumeDeltaForDrag(-3)).toBe(-1)
    expect(volumeDeltaForDrag(Number.NaN)).toBe(0)
  })
})

describe('形态组合的冒烟检查', () => {
  const forms: DeviceForm[] = ['desktop', 'handheld', 'tv']

  it('任何形态下列数都至少为 2 且不超过 8', () => {
    for (const f of forms) {
      for (const v of [phone, tablet, desktop]) {
        const c = columnsFor(f, v)
        expect(c).toBeGreaterThanOrEqual(2)
        expect(c).toBeLessThanOrEqual(8)
      }
    }
  })
})
