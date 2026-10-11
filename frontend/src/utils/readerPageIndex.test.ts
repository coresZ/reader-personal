import { describe, expect, it } from 'vitest'
import {
  clampReaderPageIndex,
  getReaderLastPage,
  readerPageIndexFromProgress,
} from './readerPageIndex'

describe('getReaderLastPage', () => {
  it('is the page count minus one', () => {
    expect(getReaderLastPage(11)).toBe(10)
    expect(getReaderLastPage(1)).toBe(0)
  })

  it('returns zero when there are no pages yet', () => {
    expect(getReaderLastPage(0)).toBe(0)
  })
})

describe('clampReaderPageIndex', () => {
  it('keeps an in-range index untouched', () => {
    expect(clampReaderPageIndex(0, 11)).toBe(0)
    expect(clampReaderPageIndex(5, 11)).toBe(5)
    expect(clampReaderPageIndex(10, 11)).toBe(10)
  })

  it('clamps past the end — page count shrinking after a re-pagination', () => {
    // 重新分页后页数可能变少, 旧索引会指向不存在的页, 画面会停在空白处。
    expect(clampReaderPageIndex(10, 5)).toBe(4)
    expect(clampReaderPageIndex(99, 3)).toBe(2)
  })

  it('clamps negatives to the first page', () => {
    expect(clampReaderPageIndex(-5, 11)).toBe(0)
  })

  it('returns zero for a bogus index or empty page list', () => {
    expect(clampReaderPageIndex(Number.NaN, 11)).toBe(0)
    expect(clampReaderPageIndex(3, 0)).toBe(0)
  })
})

describe('readerPageIndexFromProgress', () => {
  it('maps the ends of the range to the first and last page', () => {
    expect(readerPageIndexFromProgress(0, 11)).toBe(0)
    expect(readerPageIndexFromProgress(1, 11)).toBe(10)
  })

  it('maps the middle proportionally', () => {
    expect(readerPageIndexFromProgress(0.5, 11)).toBe(5)
  })

  it('stays monotonic across the whole range', () => {
    let previous = -1
    for (let p = 0; p <= 1.0001; p += 0.05) {
      const value = readerPageIndexFromProgress(p, 11)
      expect(value).toBeGreaterThanOrEqual(previous)
      previous = value
    }
  })

  it('clamps out-of-range progress and handles empty page lists', () => {
    expect(readerPageIndexFromProgress(-1, 11)).toBe(0)
    expect(readerPageIndexFromProgress(2, 11)).toBe(10)
    expect(readerPageIndexFromProgress(0.5, 0)).toBe(0)
    expect(readerPageIndexFromProgress(Number.NaN, 11)).toBe(0)
  })
})
