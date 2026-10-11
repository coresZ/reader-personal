import { describe, expect, it } from 'vitest'
import { getReaderTextWidth } from './readerTextWidth'

describe('getReaderTextWidth', () => {
  it('uses the configured width when the window is wider — text is centred', () => {
    // 1920 窗口、左右各 24px padding → 可用 1872; 设置 800 → 行宽 800。
    // 这正是「窗口最大化时也保持舒服行宽」的场景。
    expect(getReaderTextWidth(1872, 800)).toBe(800)
    expect(getReaderTextWidth(1200, 800)).toBe(800)
  })

  it('shrinks to the available width when the window is narrower', () => {
    // 窄窗口不能用设置值, 否则正文会横向溢出被裁。
    expect(getReaderTextWidth(600, 800)).toBe(600)
    expect(getReaderTextWidth(799, 800)).toBe(799)
  })

  it('equals the available width when the two are the same', () => {
    expect(getReaderTextWidth(800, 800)).toBe(800)
  })

  it('falls back to the available width for a missing or bogus setting', () => {
    expect(getReaderTextWidth(900, 0)).toBe(900)
    expect(getReaderTextWidth(900, -100)).toBe(900)
    expect(getReaderTextWidth(900, Number.NaN)).toBe(900)
  })

  it('stays positive for degenerate available width instead of returning zero', () => {
    expect(getReaderTextWidth(0, 800)).toBe(1)
    expect(getReaderTextWidth(Number.NaN, 800)).toBe(1)
  })
})
