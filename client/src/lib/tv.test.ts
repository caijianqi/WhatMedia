import { describe, expect, it } from 'vitest'
import {
  FocusMemory,
  initialFocus,
  moveFocus,
  recoverFocus,
  remoteKeyToAction,
  SEEK_JUMP_SECONDS,
  type Focusable,
} from './tv'

/** 3 行 × 4 列的海报墙。每项 200×300，间距 20。 */
function grid(rows = 3, cols = 4): Focusable[] {
  const items: Focusable[] = []
  for (let r = 0; r < rows; r++) {
    for (let c = 0; c < cols; c++) {
      items.push({
        id: `r${r}c${c}`,
        rect: { x: c * 220, y: r * 320, w: 200, h: 300 },
      })
    }
  }
  return items
}

const idx = (r: number, c: number) => r * 4 + c

describe('moveFocus 水平', () => {
  it('同一行内左右移动', () => {
    const g = grid()
    expect(moveFocus(g, idx(1, 1), 'right')).toBe(idx(1, 2))
    expect(moveFocus(g, idx(1, 1), 'left')).toBe(idx(1, 0))
  })

  it('行首按左、行尾按右都不动 —— 不越到相邻行', () => {
    // 越到相邻行是 TV 上最常见的"按了跳到奇怪地方"。
    const g = grid()
    expect(moveFocus(g, idx(1, 0), 'left')).toBe(idx(1, 0))
    expect(moveFocus(g, idx(1, 3), 'right')).toBe(idx(1, 3))
  })

  it('不会跳到别的行', () => {
    const g = grid()
    const from = idx(0, 3)
    expect(moveFocus(g, from, 'right')).toBe(from)
    expect(moveFocus(g, from, 'left')).toBe(idx(0, 2))
  })
})

describe('moveFocus 纵向', () => {
  it('正上正下落在同一列', () => {
    const g = grid()
    expect(moveFocus(g, idx(0, 2), 'down')).toBe(idx(1, 2))
    expect(moveFocus(g, idx(2, 2), 'up')).toBe(idx(1, 2))
  })

  it('首行按上、末行按下都不动', () => {
    const g = grid()
    expect(moveFocus(g, idx(0, 1), 'up')).toBe(idx(0, 1))
    expect(moveFocus(g, idx(2, 1), 'down')).toBe(idx(2, 1))
  })

  it('跨行时优先落到最近的下一行，而不是跳过一行', () => {
    const g = grid()
    expect(moveFocus(g, idx(0, 0), 'down')).toBe(idx(1, 0))
  })
})

describe('FocusMemory', () => {
  it('记住列：从窄行的第 4 列上移到宽行时落在第 4 列而不是第 1 列', () => {
    // 第 0 行 4 列、第 1 行 2 列的"最近观看 → 推荐"布局。
    const items: Focusable[] = [
      { id: 'a', rect: { x: 0, y: 0, w: 200, h: 300 } },
      { id: 'b', rect: { x: 220, y: 0, w: 200, h: 300 } },
      { id: 'c', rect: { x: 440, y: 0, w: 200, h: 300 } },
      { id: 'd', rect: { x: 660, y: 0, w: 200, h: 300 } },
      { id: 'wide1', rect: { x: 0, y: 320, w: 430, h: 300 } },
      { id: 'wide2', rect: { x: 450, y: 320, w: 410, h: 300 } },
    ]
    const memory = new FocusMemory()
    memory.remember(3, items) // 焦点在第 0 行第 4 列（x 中心 760）
    const next = moveFocus(items, 3, 'down', { memory })
    // 应当落在 wide2（中心 655），因为它的中心 x 离 760 更近
    expect(next).toBe(5)
  })

  it('没有记忆时退化为取综合距离最小者', () => {
    const g = grid()
    expect(moveFocus(g, idx(0, 0), 'down')).toBe(idx(1, 0))
  })

  it('reset 后不再有偏好', () => {
    const m = new FocusMemory()
    m.remember(idx(0, 3), grid())
    expect(m.targetX()).not.toBeNull()
    m.reset()
    expect(m.targetX()).toBeNull()
  })
})

describe('initialFocus', () => {
  it('取视觉上的左上角，不是数组第一个', () => {
    // DOM 顺序被打乱时（grid order / 绝对定位）数组第一个未必在左上。
    const items: Focusable[] = [
      { id: 'second', rect: { x: 220, y: 0, w: 200, h: 300 } },
      { id: 'first', rect: { x: 0, y: 0, w: 200, h: 300 } },
    ]
    expect(initialFocus(items)).toBe(1)
  })

  it('跳过禁用项', () => {
    const items: Focusable[] = [
      { id: 'disabled', rect: { x: 0, y: 0, w: 200, h: 300 }, disabled: true },
      { id: 'ok', rect: { x: 220, y: 0, w: 200, h: 300 } },
    ]
    expect(initialFocus(items)).toBe(1)
  })

  it('空列表不炸', () => {
    expect(initialFocus([])).toBe(0)
  })
})

describe('recoverFocus', () => {
  it('上次位置仍可用就回去', () => {
    const g = grid()
    expect(recoverFocus(g, idx(1, 2))).toBe(idx(1, 2))
  })

  it('上次位置消失了就回初始位置，不返回 -1', () => {
    // 弹层关闭后焦点常落在 body 上，必须有兜底。
    const g = grid()
    expect(recoverFocus(g, 99)).toBe(0)
  })

  it('上次位置被禁用就换一个', () => {
    const g = grid()
    g[2]!.disabled = true
    expect(recoverFocus(g, 2)).toBe(0)
  })
})

describe('remoteKeyToAction', () => {
  it('浏览态：方向键移动焦点，确认激活', () => {
    expect(remoteKeyToAction('down')).toEqual({ kind: 'move', direction: 'down' })
    expect(remoteKeyToAction('select')).toEqual({ kind: 'activate' })
    expect(remoteKeyToAction('back')).toEqual({ kind: 'back' })
  })

  it('快进快退的步长与 Kotlin 侧一致', () => {
    expect(remoteKeyToAction('seek-forward')).toEqual({
      kind: 'seek',
      seconds: SEEK_JUMP_SECONDS,
    })
    expect(remoteKeyToAction('seek-backward')).toEqual({
      kind: 'seek',
      seconds: -SEEK_JUMP_SECONDS,
    })
    expect(SEEK_JUMP_SECONDS).toBe(10)
  })

  it('播放键一律映射成切换播放，不区分 play/pause', () => {
    // 遥控器上往往只有一个播放/暂停键，映射成 toggle 才不会状态打架。
    for (const k of ['toggle', 'play', 'pause'] as const) {
      expect(remoteKeyToAction(k).kind).toBe('toggle-play')
    }
  })

  it('弹层里方向键仍归弹层焦点系统，返回键是关闭', () => {
    const ctx = { playing: true, overlay: true }
    expect(remoteKeyToAction('down', ctx)).toEqual({ kind: 'move', direction: 'down' })
    expect(remoteKeyToAction('back', ctx)).toEqual({ kind: 'back' })
    // 弹层里按播放键不该切播放
    expect(remoteKeyToAction('toggle', ctx).kind).toBe('ignore')
  })

  it('没有对应行为的按键明确忽略，不当成未知去猜', () => {
    expect(remoteKeyToAction('next').kind).toBe('ignore')
    expect(remoteKeyToAction('stop').kind).toBe('ignore')
    expect(remoteKeyToAction('previous').kind).toBe('ignore')
  })
})
