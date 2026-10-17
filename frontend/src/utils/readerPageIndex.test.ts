import { describe, expect, it } from 'vitest'
import {
  clampReaderPageIndex,
  getReaderLastPage,
  readerLandingPageIndexAfterPrevChapter,
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

  /** 位置恢复的正确性依赖"存进去的比例能原样换回同一页"。
   *
   *  保存端写的是 `horizontalPageIndex / 末页索引`, 恢复端用本函数反算。
   *  若两端公式不一致, 重进阅读页就会落在别的页甚至第一页 —— 曾经横向恢复
   *  走的是 scrollLeft(恒为 0), 现象就是"退出再进回到章节开头"。 */
  it('round-trips every page index back to itself', () => {
    for (const total of [1, 2, 3, 7, 11, 40]) {
      const lastPage = total - 1
      for (let index = 0; index <= lastPage; index += 1) {
        // 保存端: syncHorizontalPageState 的写法。
        const progress = lastPage <= 0 ? 1 : index / lastPage
        expect(readerPageIndexFromProgress(progress, total)).toBe(index)
      }
    }
  })
})

describe('readerLandingPageIndexAfterPrevChapter', () => {
  it('lands on the last page of the chapter it just entered', () => {
    // 往前翻跨章: 目标必须是上一章末页。落到第 0 页就是标题页, 用户看到的现象
    // 就是"翻上一页还是标题页"。
    expect(readerLandingPageIndexAfterPrevChapter({
      intentIndex: 3,
      currentIndex: 3,
      loading: false,
      totalPages: 7,
    })).toBe(6)
  })

  it('returns the first page when the chapter fits in a single page', () => {
    // 只有一页时末页即首页 —— 此时不该再往前翻, 由调用方按"索引 0"处理。
    expect(readerLandingPageIndexAfterPrevChapter({
      intentIndex: 0,
      currentIndex: 0,
      loading: false,
      totalPages: 1,
    })).toBe(0)
  })

  it('refuses to land while the chapter content is still loading', () => {
    // 加载中时 horizontalPages 还是上一章的分页结果, 此时落位会把页数算错。
    expect(readerLandingPageIndexAfterPrevChapter({
      intentIndex: 3,
      currentIndex: 3,
      loading: true,
      totalPages: 7,
    })).toBeNull()
  })

  it('refuses to land once another chapter change has happened since', () => {
    // 残留的意图不能在之后任意一次重建时把用户拽到末页。
    expect(readerLandingPageIndexAfterPrevChapter({
      intentIndex: 3,
      currentIndex: 4,
      loading: false,
      totalPages: 7,
    })).toBeNull()
  })

  it('refuses to land without a pending intent', () => {
    expect(readerLandingPageIndexAfterPrevChapter({
      intentIndex: null,
      currentIndex: 3,
      loading: false,
      totalPages: 7,
    })).toBeNull()
  })
})
