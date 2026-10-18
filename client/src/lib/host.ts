/**
 * 宿主（Tauri 壳）→ 前端的单向信息通道。
 *
 * ## 为什么单开一个文件
 *
 * 设备形态这件事**不能由前端推断**，只能问宿主。
 * 把"问宿主"的调用集中在一处，好处是：没有宿主时（纯 Web 构建、
 * vitest 里）它有明确的兜底，而不是散落在各页面里的 `try/catch`。
 */

import { parseForm, type DeviceForm } from './platform'

/** 是否在 Tauri 壳里。判据与 player/mpv.ts 一致。 */
export function inShell(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/**
 * 问宿主"我跑在什么设备上"。
 *
 * 兜底逻辑有讲究：
 *   · 不在壳里 → `web`（浏览器，走桌面布局）
 *   · 在壳里但问不到 → `desktop`（Tauri 桌面端是壳的主要形态）
 *   · **绝不兜底成 `handheld`** —— 触摸 UI 在无触摸屏的 TV 上等于完全不可用，
 *     而桌面布局在手机上只是"挤一点"，还能操作。
 *     两个错误的代价差了一个数量级。
 */
export async function probeDeviceForm(): Promise<DeviceForm> {
  if (!inShell()) return 'web'
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const raw = await invoke<{ form?: string }>('plugin:media3|media3_device_form')
    const form = parseForm(raw?.form)
    return form === 'unknown' ? 'desktop' : form
  } catch {
    return 'desktop'
  }
}

/**
 * 安全区。从 CSS `env(safe-area-inset-*)` 读。
 *
 * **`index.html` 的 viewport meta 必须带 `viewport-fit=cover`**，
 * 否则这几个值恒为 0 —— 表现是 iPhone 上底栏被手势条盖住一半，
 * 而开发者拿不带刘海的测试机完全看不出来。
 */
export function readSafeAreaInsets(): Record<string, string | undefined> {
  if (typeof window === 'undefined' || !window.getComputedStyle) return {}
  const probe = document.createElement('div')
  // 用 CSS 变量把 env() 的值带出来，再读变量 ——
  // 直接读 env() 在部分内核上不生效。
  probe.style.cssText =
    'position:fixed;top:0;left:0;width:0;height:0;visibility:hidden;' +
    '--sa-t:env(safe-area-inset-top);' +
    '--sa-r:env(safe-area-inset-right);' +
    '--sa-b:env(safe-area-inset-bottom);' +
    '--sa-l:env(safe-area-inset-left);'
  document.body.appendChild(probe)
  const cs = window.getComputedStyle(probe)
  const out = {
    top: cs.getPropertyValue('--sa-t'),
    right: cs.getPropertyValue('--sa-r'),
    bottom: cs.getPropertyValue('--sa-b'),
    left: cs.getPropertyValue('--sa-l'),
  }
  probe.remove()
  return out
}
