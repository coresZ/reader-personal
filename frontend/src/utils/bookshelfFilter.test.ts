import { describe, expect, it } from 'vitest'
import {
  filterShelfBooks,
  matchesShelfKeyword,
  normalizeShelfKeyword,
} from './bookshelfFilter'

/** 造一本够过滤用的最小书。 */
function book(name: string, extra: Record<string, string> = {}) {
  return { name, author: '', bookUrl: `url:${name}`, ...extra }
}

describe('normalizeShelfKeyword', () => {
  it('去掉首尾空白并转小写', () => {
    expect(normalizeShelfKeyword('  三体 ')).toBe('三体')
    expect(normalizeShelfKeyword('Cosmetic')).toBe('cosmetic')
  })

  it('纯空白归一化为空串', () => {
    expect(normalizeShelfKeyword('   ')).toBe('')
  })
})

describe('matchesShelfKeyword', () => {
  it('命中书名/作者/简介/来源/章节名中的任意一个', () => {
    const target = book('三体', {
      author: '刘慈欣',
      intro: '地球往事三部曲',
      originName: '示例书城',
      origin: 'https://www.example-shucheng.com',
      latestChapterTitle: '第三十九章 黑暗森林',
      durChapterTitle: '第一章 科学边界',
    })
    expect(matchesShelfKeyword(target, '三体')).toBe(true)
    expect(matchesShelfKeyword(target, '刘慈欣')).toBe(true)
    expect(matchesShelfKeyword(target, '地球往事')).toBe(true)
    expect(matchesShelfKeyword(target, '示例书城')).toBe(true)
    expect(matchesShelfKeyword(target, 'shucheng')).toBe(true)
    expect(matchesShelfKeyword(target, '黑暗森林')).toBe(true)
    expect(matchesShelfKeyword(target, '科学边界')).toBe(true)
  })

  it('英文关键词大小写不敏感', () => {
    expect(matchesShelfKeyword(book('Cosmetic'), 'cosm')).toBe(true)
    expect(matchesShelfKeyword(book('cosmetic'), 'COSM')).toBe(true)
  })

  it('不命中时返回 false', () => {
    expect(matchesShelfKeyword(book('三体'), '沙丘')).toBe(false)
  })

  it('空关键词视为命中', () => {
    expect(matchesShelfKeyword(book('三体'), '')).toBe(true)
    expect(matchesShelfKeyword(book('三体'), '   ')).toBe(true)
  })

  it('字段缺失或非字符串时不报错', () => {
    const broken = { name: undefined, author: undefined } as never
    expect(matchesShelfKeyword(broken, '三体')).toBe(false)
    expect(matchesShelfKeyword(broken, '')).toBe(true)
  })
})

describe('filterShelfBooks', () => {
  const shelf = [
    book('三体', { author: '刘慈欣' }),
    book('沙丘', { author: '赫伯特' }),
    book('三体前传', { author: '刘慈欣' }),
  ]

  it('只保留命中的书, 并保持入参顺序', () => {
    const result = filterShelfBooks(shelf, '三体')
    expect(result.map((b) => b.name)).toEqual(['三体', '三体前传'])
  })

  it('按作者过滤可跨多本命中', () => {
    const result = filterShelfBooks(shelf, '刘慈欣')
    expect(result.map((b) => b.name)).toEqual(['三体', '三体前传'])
  })

  it('可按书源 URL 过滤(与最近阅读页口径一致)', () => {
    const list = [
      book('三体', { origin: 'https://www.example-a.com' }),
      book('沙丘', { origin: 'https://www.example-b.com' }),
    ]
    expect(filterShelfBooks(list, 'example-b').map((b) => b.name)).toEqual(['沙丘'])
  })

  it('大小写不敏感在过滤层同样成立', () => {
    const list = [book('Cosmetic Playlover'), book('沙丘')]
    expect(filterShelfBooks(list, 'cosm').map((b) => b.name)).toEqual(['Cosmetic Playlover'])
  })

  it('空数组返回空数组', () => {
    expect(filterShelfBooks([], '三体')).toEqual([])
  })

  it('空关键词返回全部(且不是同一引用, 避免调用方误改)', () => {
    const result = filterShelfBooks(shelf, '  ')
    expect(result.map((b) => b.name)).toEqual(['三体', '沙丘', '三体前传'])
    expect(result).not.toBe(shelf)
  })

  it('无命中时返回空数组', () => {
    expect(filterShelfBooks(shelf, '不存在的书')).toEqual([])
  })
})