/**
 * 布局 store：设备形态 + 视口 + 安全区 → 一套页面直接消费的参数。
 *
 * ## 为什么是 store 而不是 composable
 *
 * 形态、视口、安全区三者在**多个页面**都要用（顶栏/底栏、海报墙列数、
 * 播放器控制条尺寸、弹层位置）。各自去问一遍宿主会拿到不一致的结果，
 * 而且旋转屏幕时要通知所有使用方。
 *
 * ## 为什么要监听三件事
 *
 *   1. `resize` —— 旋转屏幕 / 分屏
 *   2. `orientationchange` —— 部分 Android WebView 在旋转时不发 resize，
 *      只发这个。少监听它的表现是"转到横屏后布局没变，要等两秒"。
 *   3. 宿主主动推送的形态变化 —— 外接显示器 / 投屏
 */

import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import {
  layoutFor,
  parseSafeArea,
  ZERO_SAFE_AREA,
  type DeviceForm,
  type LayoutMetrics,
  type SafeArea,
} from '../lib/platform'
import { inShell, probeDeviceForm, readSafeAreaInsets } from '../lib/host'

export const useLayoutStore = defineStore('layout', () => {
  const form = ref<DeviceForm>('web')
  const width = ref(1280)
  const height = ref(800)
  const dpr = ref(1)
  const safe = ref<SafeArea>(ZERO_SAFE_AREA)

  /** 是否已完成一次初始化。避免首帧用默认值闪一下桌面布局。 */
  const ready = ref(false)

  const metrics = computed<LayoutMetrics>(
    () => layoutFor(form.value, { width: width.value, height: height.value, dpr: dpr.value }, safe.value),
  )

  const isTv = computed(() => form.value === 'tv')
  const isHandheld = computed(() => form.value === 'handheld')
  /** TV 与手机都没有指针 —— 悬停样式在这两种形态上是死代码。 */
  const hasPointer = computed(() => !isTv.value && !isHandheld.value)

  function measure(): void {
    if (typeof window === 'undefined') return
    width.value = window.innerWidth
    height.value = window.innerHeight
    dpr.value = window.devicePixelRatio || 1
  }

  function measureSafeArea(): void {
    if (typeof window === 'undefined') return
    // 只有手持设备需要安全区；TV 走 overscan（在 layoutFor 里覆盖），
    // 桌面没有刘海。都读也没坏处，但 TV 上 env() 有值会盖掉 overscan 计算，
    // 所以在 layoutFor 里对 TV 做了强制覆盖。
    safe.value = parseSafeArea(readSafeAreaInsets())
  }

  /** 初始化。在 App 挂载时调一次。 */
  async function init(): Promise<void> {
    measure()
    measureSafeArea()
    form.value = await probeDeviceForm()
    // 形态可能改变布局（列数、安全区），所以形态确定后再量一次安全区。
    measureSafeArea()
    ready.value = true

    if (typeof window === 'undefined') return
    window.addEventListener('resize', onResize)
    window.addEventListener('orientationchange', onOrientationChange)
  }

  function onResize(): void {
    measure()
    measureSafeArea()
  }

  /**
   * 旋转后延时再量。
   *
   * Android WebView 上 `orientationchange` 触发时 `innerWidth`
   * **还是旧值** —— 立刻量会得到错误的方向，布局就反了。
   * 100ms 是实测够用的值；轮询到变化为止更稳，但没必要为此起定时器。
   */
  function onOrientationChange(): void {
    window.setTimeout(() => {
      measure()
      measureSafeArea()
    }, 100)
  }

  function dispose(): void {
    if (typeof window === 'undefined') return
    window.removeEventListener('resize', onResize)
    window.removeEventListener('orientationchange', onOrientationChange)
  }

  /** 给测试与调试用：直接指定形态。 */
  function setForm(f: DeviceForm): void {
    form.value = f
    ready.value = true
  }

  return {
    form,
    width,
    height,
    dpr,
    safe,
    ready,
    metrics,
    isTv,
    isHandheld,
    hasPointer,
    init,
    dispose,
    setForm,
    measure,
    measureSafeArea,
    inShell,
  }
})
