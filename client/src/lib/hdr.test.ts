import { describe, expect, it } from 'vitest'
import { hdrLabel, isHdr } from './hdr'

describe('hdrLabel', () => {
  it('uses the labels people in this domain actually write', () => {
    expect(hdrLabel('dolby_vision')).toBe('DV')
    expect(hdrLabel('hdr10_plus')).toBe('HDR10+')
    expect(hdrLabel('hdr10')).toBe('HDR10')
    expect(hdrLabel('hlg')).toBe('HLG')
  })

  it('passes unknown types through instead of blanking them', () => {
    // 后端加了新类型而前端没跟上时，"露一个陌生词"比"什么都不显示"诚实 ——
    // 空白会被读成 SDR，那是假信息。
    expect(hdrLabel('hdr10_plus_adv')).toBe('hdr10_plus_adv')
  })

  it('tolerates missing values', () => {
    expect(hdrLabel(null)).toBe('')
    expect(hdrLabel(undefined)).toBe('')
    expect(hdrLabel('')).toBe('')
  })
})

describe('isHdr', () => {
  it('treats only sdr as non-hdr', () => {
    expect(isHdr('hdr10')).toBe(true)
    expect(isHdr('hlg')).toBe(true)
    expect(isHdr('sdr')).toBe(false)
    expect(isHdr(null)).toBe(false)
  })
})
