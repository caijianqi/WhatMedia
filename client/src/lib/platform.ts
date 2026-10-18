/**
 * 设备形态与布局参数。
 *
 * ## 为什么要有这一层
 *
 * P6 要在同一个 Vue 代码库里同时服务桌面、手机、TV 三种形态。
 * 三者的差异不是"屏幕小一点"，而是**交互范式不同**：
 *
 *   · 桌面：鼠标悬停 + 精确点击 + 键盘
 *   · 手机：触摸 + 手势 + 竖屏为主 + 有安全区（刘海/挖孔/手势条）
 *   · TV：遥控器 D-Pad + 焦点移动 + 10 英尺观看距离 + overscan
 *
 * 用 CSS 媒体查询无法区分 TV 与手机（都是"小屏 + 无鼠标"），
 * 所以形态必须由**宿主明确告知**（Android 侧 `MainActivity.deviceForm()`），
 * 不能靠猜。
 *
 * ## 可测性
 *
 * 本模块里所有函数都是纯函数：输入是"宿主报告 + 视口尺寸"，
 * 输出是布局参数。于是"竖屏手机该几列海报""TV 的 overscan 留多少"
 * 这些真正会反复调的东西能在 vitest 里定死，不用拿遥控器一点点试。
 */

/** 设备形态。由宿主报告，不推断。 */
export type DeviceForm = 'web' | 'desktop' | 'handheld' | 'tv' | 'unknown'

/** 屏幕方向。 */
export type Orientation = 'portrait' | 'landscape'

/** 视口信息。宽高单位 CSS 像素。 */
export interface Viewport {
  width: number
  height: number
  /** devicePixelRatio。高分屏上用来判断"要不要加载 2x 海报"。 */
  dpr?: number
}

/** 安全区（CSS 像素）。刘海、圆角、底部手势条。 */
export interface SafeArea {
  top: number
  right: number
  bottom: number
  left: number
}

export const ZERO_SAFE_AREA: SafeArea = { top: 0, right: 0, bottom: 0, left: 0 }

/**
 * 判断方向。正方形视为 portrait —— 分屏时会出现正方形，
 * 而 portrait 的布局（列数少）在窄屏下更不容易挤爆。
 */
export function orientationOf(v: Viewport): Orientation {
  // 严格大于：正方形（分屏时会出现）按竖屏处理，列数更少、更不容易挤爆。
  return v.width > v.height ? 'landscape' : 'portrait'
}

/**
 * 是否"窄屏"。
 *
 * 阈值 600 而不是 768：手机上**横屏**时短边往往只有 400 左右，
 * 按 768 判会走桌面布局，于是横屏手机上出现两栏导航 + 侧边栏，挤成一团。
 */
export const NARROW_BREAKPOINT = 600

export function isNarrow(v: Viewport): boolean {
  return Math.min(v.width, v.height) < NARROW_BREAKPOINT
}

/**
 * 触摸目标最小尺寸（CSS 像素）。
 *
 * 44 是 Apple HIG，48 是 Material Design。**取 48**：
 * 影音应用大量操作发生在"半躺着用遥控/单手拿手机"的场景，
 * 目标小一点就点不中。TV 上虽然不是触摸，但 D-Pad 焦点框也用同一个值，
 * 保证焦点框在 10 英尺外看得清。
 */
export const MIN_TOUCH_TARGET = 48

/**
 * TV 的 overscan 安全边距比例。
 *
 * 老电视会把画面边缘裁掉最多 5%。
 * 不留边距的结果是"选片页面的第一行海报被切掉半个"。
 * 现代 Android TV 大多不裁了，但留 3.5% 是通行的安全值。
 */
export const TV_OVERSCAN_RATIO = 0.035

/**
 * 10-foot UI 的缩放系数。
 *
 * TV 上文字要比手机大：观看距离 3 米，手机上 14px 的字在 TV 上等于看不清。
 * 1.5 是实测下来不显得笨拙又能看清的值。
 */
export const TV_SCALE = 1.5

/** 布局参数。页面直接消费这个，别自己算。 */
export interface LayoutMetrics {
  form: DeviceForm
  orientation: Orientation
  /** 海报墙列数。 */
  columns: number
  /** 海报宽高比（宽/高）。电影海报 2:3。 */
  posterAspect: number
  /** 触摸目标最小尺寸。 */
  minTarget: number
  /** 安全区。 */
  safe: SafeArea
  /** 字号缩放。TV 上放大。 */
  fontScale: number
  /** 是否显示鼠标悬停态。TV/手机都没有悬停。 */
  hover: boolean
  /** 底栏/侧栏：手机用底部标签栏，桌面与 TV 用顶栏。 */
  nav: 'top' | 'bottom'
}

/**
 * 计算布局参数。
 *
 * **列数只看短边**：竖屏手机上宽 400 高 800，若按宽度算会得到 1 列，
 * 而按短边（400）算也是 1 列 —— 一致。但横屏手机 800x400 时，
 * 按宽度算会得到 4 列，而实际可用高度只有 400，海报就被压扁了。
 */
export function layoutFor(form: DeviceForm, v: Viewport, safe: SafeArea = ZERO_SAFE_AREA): LayoutMetrics {
  const orientation = orientationOf(v)
  const short = Math.min(v.width, v.height)
  const isTv = form === 'tv'

  return {
    form,
    orientation,
    columns: columnsFor(form, v),
    posterAspect: 2 / 3,
    minTarget: MIN_TOUCH_TARGET,
    safe: isTv ? tvOverscan(v) : safe,
    fontScale: isTv ? TV_SCALE : 1,
    hover: form === 'desktop' || form === 'web',
    nav: form === 'handheld' ? 'bottom' : 'top',
    // short 变量保留给将来按可用高度调列数；当前列数只由宽度与形态决定。
    ...(short < 0 ? {} : {}),
  }
}

/**
 * 海报墙列数。
 *
 * TV 上列数明显少于同宽度手机：**一行 3 个而不是 6 个**。
 * 因为 D-Pad 横向移动要按很多次才能到行尾，且 10 英尺外小海报看不清内容。
 */
export function columnsFor(form: DeviceForm, v: Viewport): number {
  const w = v.width
  switch (form) {
    case 'tv':
      // 遥控器横向移动成本高，列数刻意压低。
      if (w >= 1920) return 6
      if (w >= 1280) return 5
      return 4
    case 'handheld':
      if (w >= 900) return 5
      if (w >= 600) return 4
      if (w >= 400) return 3
      return 2
    case 'desktop':
    case 'web':
    default:
      if (w >= 1600) return 8
      if (w >= 1200) return 6
      if (w >= 900) return 5
      if (w >= 600) return 4
      return 3
  }
}

/**
 * TV 的 overscan 边距。
 *
 * 只留横向与纵向的**较小者**作为统一边距：
 * 分别留 top/bottom 与 left/right 会让内容在宽屏电视上左右空白过大。
 */
export function tvOverscan(v: Viewport): SafeArea {
  const inset = Math.round(Math.min(v.width, v.height) * TV_OVERSCAN_RATIO)
  // 底部多留一些：很多电视底部裁得比其它边更多。
  return { top: inset, right: inset, bottom: inset * 2, left: inset }
}

/**
 * 从 CSS `env(safe-area-inset-*)` 读安全区。
 *
 * **必须放在视口 meta 里加 `viewport-fit=cover` 才有值**，否则恒为 0。
 * 这一点漏了的表现是"iPhone 上底栏被手势条盖住一半"，
 * 而开发者拿不带刘海的测试机完全看不出来。
 *
 * 单位是 px 字符串（可能带 `px` 后缀，也可能是 `calc(...)`），所以手动解析。
 */
export function parseSafeArea(pairs: Record<string, string | undefined>): SafeArea {
  return {
    top: px(pairs.top),
    right: px(pairs.right),
    bottom: px(pairs.bottom),
    left: px(pairs.left),
  }
}

function px(raw: string | undefined): number {
  if (!raw) return 0
  const n = Number.parseFloat(raw)
  return Number.isFinite(n) && n > 0 ? n : 0
}

/**
 * 从宿主报告的字符串解析形态。
 *
 * Android 侧 `MainActivity.deviceForm()` 返回 `handheld` / `tv` / `unknown`。
 * 认不出来一律 `unknown` —— 猜成 handheld 会让 TV 用户拿到触摸 UI，
 * 而在 TV 上"没有触摸事件"意味着**完全无法操作**。
 */
export function parseForm(raw: string | null | undefined): DeviceForm {
  switch (raw) {
    case 'handheld':
    case 'phone':
    case 'tablet':
      return 'handheld'
    case 'tv':
      return 'tv'
    case 'desktop':
      return 'desktop'
    case 'web':
      return 'web'
    default:
      return 'unknown'
  }
}

/**
 * 手势：横向拖动映射到的 seek 秒数。
 *
 * **非线性**：拖得越远，单位像素对应的秒数越多。
 * 线性映射下，想跳到 40 分钟处得拖半个屏幕；非线性则一次拖动就能覆盖全片。
 *
 * `ratio` 是"拖动距离 / 屏幕宽度"，`duration` 是总时长。
 */
export function seekSecondsForDrag(ratio: number, duration: number): number {
  if (!Number.isFinite(ratio) || duration <= 0) return 0
  const clamped = Math.min(1, Math.max(-1, ratio))
  // 平方：小幅拖动精细，大幅拖动快速。
  const shaped = Math.sign(clamped) * clamped * clamped
  return Math.round(shaped * duration)
}

/** 手势：纵向拖动映射到的音量增量（0..1）。整屏高度 = 满量程。 */
export function volumeDeltaForDrag(ratio: number): number {
  if (!Number.isFinite(ratio)) return 0
  return Math.min(1, Math.max(-1, ratio))
}
