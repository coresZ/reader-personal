import { describe, expect, it } from 'vitest'
import {
  DEFAULT_READER_BACKGROUND_OPACITY,
  READER_BACKGROUND_OPACITY_STEP,
  normalizeReaderBackgroundOpacity,
  percentToReaderOpacity,
  readerOpacityToPercent,
} from './readerBackground'

describe('reader background opacity', () => {
  it('uses the default for invalid saved values', () => {
    expect(normalizeReaderBackgroundOpacity(undefined)).toBe(DEFAULT_READER_BACKGROUND_OPACITY)
    expect(normalizeReaderBackgroundOpacity('invalid')).toBe(DEFAULT_READER_BACKGROUND_OPACITY)
  })

  it('clamps opacity to the visible range', () => {
    expect(normalizeReaderBackgroundOpacity(-0.5)).toBe(0)
    expect(normalizeReaderBackgroundOpacity(0.65)).toBe(0.65)
    expect(normalizeReaderBackgroundOpacity(2)).toBe(1)
  })

  it('steps the slider by 1% so fine adjustments are possible', () => {
    expect(READER_BACKGROUND_OPACITY_STEP).toBe(0.01)
  })
})

describe('opacity percentage input', () => {
  it('renders the stored opacity as a whole percentage', () => {
    expect(readerOpacityToPercent(0.35)).toBe(35)
    expect(readerOpacityToPercent(0.01)).toBe(1)
    expect(readerOpacityToPercent(1)).toBe(100)
    expect(readerOpacityToPercent(0)).toBe(0)
  })

  it('falls back to the default percentage for invalid stored values', () => {
    expect(readerOpacityToPercent(undefined)).toBe(
      Math.round(DEFAULT_READER_BACKGROUND_OPACITY * 100),
    )
  })

  it('parses typed percentages, including whitespace and a trailing unit', () => {
    expect(percentToReaderOpacity('42')).toBe(0.42)
    expect(percentToReaderOpacity(' 7 ')).toBe(0.07)
    expect(percentToReaderOpacity(0)).toBe(0)
    expect(percentToReaderOpacity(100)).toBe(1)
  })

  it('clamps out-of-range percentages instead of storing them', () => {
    expect(percentToReaderOpacity('-20')).toBe(0)
    expect(percentToReaderOpacity('250')).toBe(1)
  })

  it('returns null for non-numeric input so the caller keeps the old value', () => {
    // 关键: 用户误敲一个字符时背景不能突然跳回默认透明度。
    expect(percentToReaderOpacity('')).toBeNull()
    expect(percentToReaderOpacity('abc')).toBeNull()
    expect(percentToReaderOpacity(undefined)).toBeNull()
    expect(percentToReaderOpacity(Number.NaN)).toBeNull()
  })

  it('round-trips percentage → stored → percentage', () => {
    for (const percent of [0, 1, 17, 35, 50, 99, 100]) {
      expect(readerOpacityToPercent(percentToReaderOpacity(percent))).toBe(percent)
    }
  })
})
