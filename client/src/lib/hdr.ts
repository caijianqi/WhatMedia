/**
 * HDR 类型 → 界面文案。
 *
 * 为什么不直接打后端枚举值：后端给的是 `hdr10_plus` 这类 snake_case 内部标识，
 * 而影音领域有公认写法（DV / HDR10+ / HDR10 / HLG），用户认的是后者。
 * 直接把 `hdr10_plus` 打到海报上是能看的，但一眼"没做完"。
 *
 * **未知的 HDR 类型原样输出，不返回空。** 后端以后加类型（比如 HDR10+ ADV）
 * 而前端没跟上时，露一个陌生词比什么都不显示强 —— 什么都不显示等于
 * 告诉用户"这片是 SDR"，那是假信息。
 */

export type HdrType = 'sdr' | 'hdr10' | 'hdr10_plus' | 'hlg' | 'dolby_vision'

const LABELS: Record<HdrType, string> = {
  sdr: 'SDR',
  hdr10: 'HDR10',
  hdr10_plus: 'HDR10+',
  hlg: 'HLG',
  dolby_vision: 'DV',
}

export function hdrLabel(t: string | null | undefined): string {
  if (!t) return ''
  return LABELS[t as HdrType] ?? t
}

/** SDR 不值得占一个徽标位 —— 绝大多数片都是 SDR，打出来就是噪点。 */
export function isHdr(t: string | null | undefined): boolean {
  return !!t && t !== 'sdr'
}
