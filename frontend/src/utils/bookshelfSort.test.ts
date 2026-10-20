import { describe, expect, it } from 'vitest'
import {
  BOOK_SORT_KEYS,
  BOOK_SORT_OPTIONS,
  DEFAULT_BOOK_SORT,
  applyManualOrder,
  bookSortLabel,
  normalizeBookSort,
  sortBooks,
} from './bookshelfSort'

/** 造一本够排序用的最小书。 */
function book(
  name: string,
  extra: Partial<{
    author: string
    durChapterTime: number
    latestChapterTime: number
    lastCheckTime: number
    order: number
  }> = {},
) {
  return { name, author: '作者', ...extra }
}

const names = (books: { name?: string }[]) => books.map((b) => b.name)

describe('normalizeBookSort', () => {
  it('接受 0 到 5(含已从菜单移除的手动排序 3)', () => {
    for (let i = 0; i <= BOOK_SORT_KEYS.author; i += 1) {
      expect(normalizeBookSort(i)).toBe(i)
    }
  })

  it('兜住非法值', () => {
    expect(normalizeBookSort(-1)).toBe(DEFAULT_BOOK_SORT)
    expect(normalizeBookSort(6)).toBe(DEFAULT_BOOK_SORT)
    expect(normalizeBookSort(1.5)).toBe(DEFAULT_BOOK_SORT)
    expect(normalizeBookSort(null)).toBe(DEFAULT_BOOK_SORT)
    expect(normalizeBookSort('abc')).toBe(DEFAULT_BOOK_SORT)
  })

  it('接受数字字符串(来自已存偏好)', () => {
    expect(normalizeBookSort('3')).toBe(3)
  })
})

describe('排序菜单项', () => {
  it('不含「手动排序」, 其余五种按 Legado 的值排列', () => {
    expect(BOOK_SORT_OPTIONS.map((option) => option.label)).toEqual([
      '最近阅读', '更新时间', '书名', '综合', '作者',
    ])
    expect(BOOK_SORT_OPTIONS.map((option) => option.value)).toEqual([0, 1, 2, 4, 5])
  })

  it('bookSortLabel 对已存的「手动排序」偏好仍显示原名', () => {
    expect(bookSortLabel(BOOK_SORT_KEYS.manual)).toBe('手动排序')
    expect(bookSortLabel(BOOK_SORT_KEYS.author)).toBe('作者')
    expect(bookSortLabel(99)).toBe('最近阅读')
  })
})

describe('sortBooks', () => {
  it('0 最近阅读: 按 durChapterTime 降序', () => {
    const list = [
      book('A', { durChapterTime: 100 }),
      book('B', { durChapterTime: 300 }),
      book('C', { durChapterTime: 200 }),
    ]
    expect(names(sortBooks(list, 0))).toEqual(['B', 'C', 'A'])
  })

  it('1 更新时间: 优先 latestChapterTime, 缺失时用 lastCheckTime', () => {
    const list = [
      book('A', { lastCheckTime: 100 }),
      book('B', { lastCheckTime: 300 }),
      book('C', { latestChapterTime: 200 }),
    ]
    expect(names(sortBooks(list, 1))).toEqual(['B', 'C', 'A'])
  })

  it('2 书名: 中文按拼音排序', () => {
    const list = [book('遮天'), book('斗破苍穹'), book('夜无疆'), book('红楼梦')]
    // 拼音首字: 斗(d) 红(h) 夜(y) 遮(z)
    expect(names(sortBooks(list, 2))).toEqual(['斗破苍穹', '红楼梦', '夜无疆', '遮天'])
  })

  it('3 手动排序: 按 order 升序', () => {
    const list = [
      book('A', { order: 2 }),
      book('B', { order: 0 }),
      book('C', { order: 1 }),
    ]
    expect(names(sortBooks(list, 3))).toEqual(['B', 'C', 'A'])
  })

  it('3 手动排序: 没有 order 时保持原顺序(稳定排序)', () => {
    const list = [book('A'), book('B'), book('C')]
    expect(names(sortBooks(list, 3))).toEqual(['A', 'B', 'C'])
  })

  it('3 手动排序: 缺 order 的书排在已有 order 之后, 不会撞出重复序号', () => {
    // 书架被删过中间的书, order 出现空洞(0,1,5); 新加的两本没有 order。
    // 若「缺 order 用数组下标」兜底, 新书会拿到 2/3 插到中间, 且与下标撞值;
    // 正确行为是新书排在所有已有 order 之后, 并保持它们彼此的加入顺序。
    const list = [
      book('有0', { order: 0 }),
      book('新A'),
      book('有1', { order: 1 }),
      book('新B'),
      book('有5', { order: 5 }),
    ]
    expect(names(sortBooks(list, 3))).toEqual(['有0', '有1', '有5', '新A', '新B'])
  })

  it('4 综合: 取更新时间与最近阅读的较大者', () => {
    const list = [
      book('A', { durChapterTime: 100, lastCheckTime: 999 }),
      book('B', { durChapterTime: 500, lastCheckTime: 100 }),
      book('C', { durChapterTime: 300, lastCheckTime: 300 }),
    ]
    expect(names(sortBooks(list, 4))).toEqual(['A', 'B', 'C'])
  })

  it('5 作者: 中文按拼音排序', () => {
    const list = [
      { name: 'X', author: '辰东' },
      { name: 'Y', author: '爱潜水的乌贼' },
      { name: 'Z', author: '唐家三少' },
    ]
    // 拼音首字: 爱(a) 辰(c) 唐(t)
    expect(names(sortBooks(list, 5))).toEqual(['Y', 'X', 'Z'])
  })

  it('不修改入参', () => {
    const list = [book('A', { durChapterTime: 100 }), book('B', { durChapterTime: 300 })]
    const before = names(list)
    sortBooks(list, 0)
    expect(names(list)).toEqual(before)
  })

  it('同值时保持原有相对顺序(稳定)', () => {
    const list = [
      book('A', { durChapterTime: 100 }),
      book('B', { durChapterTime: 100 }),
      book('C', { durChapterTime: 100 }),
    ]
    expect(names(sortBooks(list, 0))).toEqual(['A', 'B', 'C'])
  })
})

describe('sortBooks 的时间单位归一', () => {
  it('秒级 durChapterTime 不会被当成比毫秒级更大', () => {
    // A 是秒级 (1.6e9 秒 → 1.6e12 毫秒), B 是毫秒级 1.7e12。统一单位后 B 更晚;
    // 若不归一, 1.6e9 会被当成远小于 1.7e12 的数值, 两者顺序恰好反过来。
    const list = [
      book('A', { durChapterTime: 1_600_000_000 }),
      book('B', { durChapterTime: 1_700_000_000_000 }),
    ]
    expect(names(sortBooks(list, 0))).toEqual(['B', 'A'])
  })

  it('综合排序能比较秒级最近阅读与毫秒级更新时间', () => {
    // 秒级 durChapterTime 换算后是 1.7e12, 大于毫秒级 lastCheckTime 1.6e12。
    const list = [
      book('A', { durChapterTime: 1_700_000_000 }),
      book('B', { lastCheckTime: 1_600_000_000_000 }),
    ]
    expect(names(sortBooks(list, 4))).toEqual(['A', 'B'])
  })

  it('缺失或 0 的时间排在最后而不是被当成极大值', () => {
    const list = [
      book('A', { durChapterTime: 0 }),
      book('B', { durChapterTime: 100 }),
      book('C'),
    ]
    expect(names(sortBooks(list, 0))[0]).toBe('B')
  })
})

describe('applyManualOrder', () => {
  const urlOf = (b: { name?: string }) => String(b.name)

  it('把被拖拽的书移到目标位置并重编号 0..n-1', () => {
    const list = [book('A'), book('B'), book('C')]
    const changed = applyManualOrder(list, 'A', 'C', urlOf)

    expect(changed).toHaveLength(3)
    const byName = new Map(changed.map((b) => [b.name, b.order]))
    expect(byName.get('B')).toBe(0)
    expect(byName.get('C')).toBe(1)
    expect(byName.get('A')).toBe(2)
  })

  it('顺序没变化时不产生改动', () => {
    const list = [book('A', { order: 0 }), book('B', { order: 1 })]
    expect(applyManualOrder(list, 'A', 'A', urlOf)).toEqual([])
  })

  it('找不到书时返回空数组', () => {
    const list = [book('A'), book('B')]
    expect(applyManualOrder(list, 'X', 'B', urlOf)).toEqual([])
    expect(applyManualOrder(list, 'A', 'X', urlOf)).toEqual([])
  })

  it('只返回 order 真的变了的书', () => {
    const list = [book('A', { order: 0 }), book('B', { order: 1 }), book('C', { order: 2 })]
    // A 从 0 移到 B 的位置: 结果 B(1) A(2) C(3)? 重编号后 A 和 B 都变。
    const changed = applyManualOrder(list, 'A', 'B', urlOf)
    expect(changed.map((b) => b.name).sort()).toEqual(['A', 'B'])
  })
})
