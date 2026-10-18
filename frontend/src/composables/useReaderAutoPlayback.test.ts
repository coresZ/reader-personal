import { describe, expect, it } from 'vitest'
import { findParagraphPageIndex, getSpeechProgressItemIndex } from './useReaderAutoPlayback'

describe('merged speech paragraph progress', () => {
  it('moves the active paragraph according to each paragraph text length', () => {
    const lengths = [20, 60, 20]

    expect(getSpeechProgressItemIndex(lengths, 0)).toBe(0)
    expect(getSpeechProgressItemIndex(lengths, 0.19)).toBe(0)
    expect(getSpeechProgressItemIndex(lengths, 0.2)).toBe(1)
    expect(getSpeechProgressItemIndex(lengths, 0.79)).toBe(1)
    expect(getSpeechProgressItemIndex(lengths, 0.8)).toBe(2)
    expect(getSpeechProgressItemIndex(lengths, 1)).toBe(2)
  })
})

describe('horizontal page lookup for speech following', () => {
  /** 假页容器: 只需要 `contains`, 与运行时用到的能力一致。 */
  function fakePage(...members: object[]): { contains(node: unknown): boolean } {
    return { contains: (node: unknown) => members.includes(node as object) }
  }

  it('finds the page that contains the spoken paragraph', () => {
    const secondParagraph = { text: '第三段' }
    const pages = [fakePage({ text: '第一段' }, { text: '第二段' }), fakePage(secondParagraph)]

    expect(findParagraphPageIndex(pages, secondParagraph)).toBe(1)
  })

  it('returns -1 when the paragraph is not rendered yet', () => {
    // 换章/重建分页期间段落可能已被卸载: 此时不能翻页, 也不能越界。
    const detached = { text: '不在任何页里' }
    expect(findParagraphPageIndex([fakePage({ text: '正文' })], detached)).toBe(-1)
    expect(findParagraphPageIndex([], detached)).toBe(-1)
    expect(findParagraphPageIndex([fakePage({ text: '正文' })], null)).toBe(-1)
  })
})
