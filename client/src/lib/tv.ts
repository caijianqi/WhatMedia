/**
 * Android TV 的 D-Pad 焦点管理。
 *
 * ## 为什么不能交给浏览器自己管
 *
 * DOM 的 focus 只在 `tabindex` 元素间按**文档顺序**移动。
 * 遥控器按"下"时，浏览器会从当前元素跳到文档里的下一个可聚焦元素 ——
 * 在海报墙（CSS grid）里这碰巧是对的，但一旦有侧栏、搜索框、分页器，
 * 顺序就完全不对了：按"下"会跳进侧栏，而不是下一行海报。
 *
 * 更糟的是**焦点丢失**：弹层关闭后浏览器把焦点还给 `<body>`，
 * 于是遥控器按什么都没反应，用户只能重启应用。
 *
 * 所以焦点位置自己算。算法抽成纯函数，能在 vitest 里定死 ——
 * TV 上"按上键跳到了奇怪的位置"是最常见也最难手测的 bug。
 */

/** 方向。 */
export type Direction = 'up' | 'down' | 'left' | 'right'

/** 可聚焦项的几何信息。单位任意（CSS 像素即可），只要同一份数据内部一致。 */
export interface FocusRect {
  x: number
  y: number
  w: number
  h: number
}

export interface Focusable {
  id: string
  rect: FocusRect
  /** 不可见 / 禁用的项要跳过，否则焦点会跑到屏幕外。 */
  disabled?: boolean
}

/** 判定"同一行"的纵向重叠比例阈值。 */
const SAME_ROW_OVERLAP = 0.5

function center(r: FocusRect): { cx: number; cy: number } {
  return { cx: r.x + r.w / 2, cy: r.y + r.h / 2 }
}

function overlap2(a: FocusRect, b: FocusRect): number {
  return Math.max(0, Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y))
}

/**
 * 焦点记忆。
 *
 * **为什么需要它**：TV 上的通行行为是"记住这一行停在第几列"。
 * 用户从第 3 行第 5 个按上到第 2 行，期望落在第 2 行第 5 个（或最接近的），
 * 而不是第 2 行第 1 个。没有记忆的话，在多行海报墙里纵向移动会一直往左漂，
 * 用户得反复按右键 —— 这是 TV 应用最常见的体验缺陷。
 */
export class FocusMemory {
  /** 上一次聚焦时的中心 x。纵向移动时用它当目标列。 */
  private preferredX: number | null = null

  /** 每次焦点变化后调用。 */
  remember(index: number, items: Focusable[]): void {
    const it = items[index]
    if (!it) return
    this.preferredX = center(it.rect).cx
  }

  /** 纵向移动时的目标列。没有历史就返回 null，由调用方退化为"最小距离"。 */
  targetX(): number | null {
    return this.preferredX
  }

  reset(): void {
    this.preferredX = null
  }
}

export interface MoveOptions {
  /** 焦点记忆。不传则纵向移动时不做列记忆。 */
  memory?: FocusMemory
}

/**
 * 从 `from` 往 `dir` 移动一格。返回目标下标；无处可去时返回 `from`（**不动**）。
 *
 * **不动比丢焦点好**：TV 上焦点一旦丢失，遥控器就完全失效了。
 * 所以宁可在边界上"按了没反应"，也不能让焦点跑到 null。
 */
export function moveFocus(
  items: Focusable[],
  from: number,
  dir: Direction,
  options: MoveOptions = {},
): number {
  const cur = items[from]
  if (!cur) return from
  const c = center(cur.rect)

  const candidates: number[] = []
  for (let i = 0; i < items.length; i++) {
    if (i === from) continue
    const it = items[i]
    if (!it || it.disabled) continue
    const o = center(it.rect)
    if (dir === 'left' || dir === 'right') {
      // 同一行：纵向重叠超过阈值
      const ov = overlap2(cur.rect, it.rect)
      const need = Math.min(cur.rect.h, it.rect.h) * SAME_ROW_OVERLAP
      if (ov < need) continue
      if (dir === 'left' ? o.cx >= c.cx : o.cx <= c.cx) continue
    } else {
      // 纵向：必须真的在上方/下方（纵向不重叠）
      const ov = overlap2(cur.rect, it.rect)
      if (ov > 0) continue
      if (dir === 'up' ? o.cy >= c.cy : o.cy <= c.cy) continue
    }
    candidates.push(i)
  }

  if (candidates.length === 0) return from

  if (dir === 'left' || dir === 'right') {
    // 水平：取横向距离最小者
    let best = candidates[0]!
    let bestD = Infinity
    for (const i of candidates) {
      const o = center(items[i]!.rect)
      const d = Math.abs(o.cx - c.cx)
      if (d < bestD) {
        bestD = d
        best = i
      }
    }
    return best
  }

  // 纵向：优先按"记忆的列"对齐，其次取综合距离最小者。
  const targetX = options.memory?.targetX() ?? null
  let best = candidates[0]!
  let bestScore = Infinity
  for (const i of candidates) {
    const o = center(items[i]!.rect)
    const dy = Math.abs(o.cy - c.cy)
    const dx = targetX === null ? Math.abs(o.cx - c.cx) : Math.abs(o.cx - targetX)
    // 纵向距离权重更高：跨两行的远项不该因为列对齐就被选中。
    const score = dy * 2 + dx
    if (score < bestScore) {
      bestScore = score
      best = i
    }
  }
  return best
}

/**
 * 没有焦点时给一个初始项。
 *
 * **取第一行最靠左的**，不是 `items[0]`：DOM 顺序不一定等于视觉顺序
 * （grid 的 `order`、RTL、绝对定位都会打乱）。按几何取才稳。
 */
export function initialFocus(items: Focusable[]): number {
  let best = -1
  let bestScore = Infinity
  for (let i = 0; i < items.length; i++) {
    const it = items[i]
    if (!it || it.disabled) continue
    const score = it.rect.y * 1000 + it.rect.x
    if (score < bestScore) {
      bestScore = score
      best = i
    }
  }
  return best < 0 ? 0 : best
}

/**
 * 焦点丢了之后恢复。
 *
 * 优先回到"上次记住的位置"，其次是初始位置。
 * **一定要有这个兜底**：弹层关闭后浏览器把焦点还给 body，
 * 不主动恢复的话遥控器就彻底失灵。
 */
export function recoverFocus(items: Focusable[], lastIndex: number): number {
  if (items[lastIndex] && !items[lastIndex]!.disabled) return lastIndex
  return initialFocus(items)
}

// ── 遥控器按键 ────────────────────────────────────────────

/** 遥控器按键的语义名。与 Rust `RemoteKey` / Kotlin `Contract.RemoteKey` 对齐。 */
export type RemoteKeyName =
  | 'up'
  | 'down'
  | 'left'
  | 'right'
  | 'select'
  | 'back'
  | 'menu'
  | 'toggle'
  | 'play'
  | 'pause'
  | 'seek-forward'
  | 'seek-backward'
  | 'next'
  | 'previous'
  | 'stop'

/**
 * 将原生键盘/遥控器事件标准化为统一的语义按键名。
 *
 * 优先采用 Android TV 物理 KeyCode（19~23, 4, 82, 85 等）兜底，
 * 彻底解决老旧机顶盒或第三方电视盒子在 WebView 下 `e.key` 为 "Unidentified" 的兼容隐患。
 */
export function parseRemoteKeyEvent(e: KeyboardEvent): RemoteKeyName | null {
  const code = e.keyCode || e.which

  // 1. Android TV / 机顶盒硬件 KeyCode 优先匹配
  switch (code) {
    case 19: // KEYCODE_DPAD_UP
    case 38: // VK_UP
    case 166: // KEYCODE_CHANNEL_UP
      return 'up'
    case 20: // KEYCODE_DPAD_DOWN
    case 40: // VK_DOWN
    case 167: // KEYCODE_CHANNEL_DOWN
      return 'down'
    case 21: // KEYCODE_DPAD_LEFT
    case 37: // VK_LEFT
      return 'left'
    case 22: // KEYCODE_DPAD_RIGHT
    case 39: // VK_RIGHT
      return 'right'
    case 23: // KEYCODE_DPAD_CENTER
    case 66: // KEYCODE_ENTER
    case 13: // VK_RETURN
      return 'select'
    case 4: // KEYCODE_BACK
    case 27: // VK_ESCAPE
    case 8: // VK_BACK (Backspace)
      return 'back'
    case 82: // KEYCODE_MENU
      return 'menu'
    case 85: // KEYCODE_MEDIA_PLAY_PAUSE
    case 179: // VK_MEDIA_PLAY_PAUSE
      return 'toggle'
    case 126: // KEYCODE_MEDIA_PLAY
      return 'play'
    case 127: // KEYCODE_MEDIA_PAUSE
      return 'pause'
    case 86: // KEYCODE_MEDIA_STOP
    case 178: // VK_MEDIA_STOP
      return 'stop'
    case 87: // KEYCODE_MEDIA_NEXT
    case 34: // VK_NEXT (PageDown)
      return 'next'
    case 88: // KEYCODE_MEDIA_PREVIOUS
    case 33: // VK_PRIOR (PageUp)
      return 'previous'
    case 89: // KEYCODE_MEDIA_REWIND
      return 'seek-backward'
    case 90: // KEYCODE_MEDIA_FAST_FORWARD
      return 'seek-forward'
  }

  // 2. W3C 标准 e.key 兜底
  switch (e.key) {
    case 'ArrowUp':
      return 'up'
    case 'ArrowDown':
      return 'down'
    case 'ArrowLeft':
      return 'left'
    case 'ArrowRight':
      return 'right'
    case 'Enter':
      return 'select'
    case 'Escape':
    case 'Backspace':
      return 'back'
    case 'ContextMenu':
    case 'Menu':
      return 'menu'
    case 'MediaPlayPause':
      return 'toggle'
    case 'MediaPlay':
      return 'play'
    case 'MediaPause':
      return 'pause'
    case 'MediaStop':
      return 'stop'
    case 'MediaTrackNext':
    case 'PageDown':
      return 'next'
    case 'MediaTrackPrevious':
    case 'PageUp':
      return 'previous'
    case 'MediaFastForward':
      return 'seek-forward'
    case 'MediaRewind':
      return 'seek-backward'
    default:
      return null
  }
}

/** 按键映射后的动作。UI 只认这个，不认原始按键。 */
export type RemoteAction =
  | { kind: 'move'; direction: Direction }
  | { kind: 'activate' }
  | { kind: 'back' }
  | { kind: 'menu' }
  | { kind: 'toggle-play' }
  | { kind: 'seek'; seconds: number }
  | { kind: 'ignore' }

/** 快进/快退步长。与 Kotlin `PlayerHolder.SEEK_JUMP_MS` 一致。 */
export const SEEK_JUMP_SECONDS = 10

/**
 * 按键 → 动作。
 *
 * `context` 决定同一按键在不同界面下的含义：
 * 播放器页上"下"是显示菜单，浏览页上"下"是移动焦点。
 * 这个差别不写清楚，TV 上就会出现"看片时按方向键没反应"。
 */
export interface KeyContext {
  /** 当前是否在播放器页。 */
  playing: boolean
  /** 是否有打开的弹层。此时方向键归弹层，返回键是关闭弹层。 */
  overlay: boolean
}

export function remoteKeyToAction(
  key: RemoteKeyName,
  context: KeyContext = { playing: false, overlay: false },
): RemoteAction {
  // 弹层里：方向键与确认都归弹层的焦点系统，只有返回是"关闭"。
  if (context.overlay) {
    switch (key) {
      case 'up':
        return { kind: 'move', direction: 'up' }
      case 'down':
        return { kind: 'move', direction: 'down' }
      case 'left':
        return { kind: 'move', direction: 'left' }
      case 'right':
        return { kind: 'move', direction: 'right' }
      case 'select':
        return { kind: 'activate' }
      case 'back':
        return { kind: 'back' }
      default:
        return { kind: 'ignore' }
    }
  }

  switch (key) {
    case 'up':
    case 'down':
    case 'left':
    case 'right':
      return { kind: 'move', direction: key }
    case 'select':
      return { kind: 'activate' }
    case 'back':
      return { kind: 'back' }
    case 'menu':
      return { kind: 'menu' }
    case 'toggle':
    case 'play':
    case 'pause':
      return { kind: 'toggle-play' }
    case 'seek-forward':
      return { kind: 'seek', seconds: SEEK_JUMP_SECONDS }
    case 'seek-backward':
      return { kind: 'seek', seconds: -SEEK_JUMP_SECONDS }
    default:
      // next / previous / stop 在客户端里没有对应行为（没有播放列表概念），
      // 明确忽略而不是当成未知按键去猜。
      return { kind: 'ignore' }
  }
}

/**
 * 播放器页上是否应当显示控制条。
 *
 * TV 上没有鼠标"动一下就显示控制条"这回事，只能靠**任意按键**唤醒。
 * 所以休眠逻辑是"一段时间无按键则隐藏"，而不是"鼠标不动则隐藏"。
 */
export const TV_CONTROLS_TIMEOUT_MS = 5000
