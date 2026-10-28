import { describe, expect, it } from 'vitest'
import {
  addBookSourceGroups,
  filterBookSources,
  getBookSourceGroups,
  getBookSourceOverview,
  getBookSourceStats,
  getVisibleSelection,
  isEditorTextUnchanged,
  moveBookSourcesToEdge,
  removeBookSourceGroups,
  toBookSourceDeletePayload,
  toRssSourceDeletePayload,
} from './sourceSelection'
import type { BookSource, RssSource } from '../types'

describe('sourceSelection', () => {
  const bookSources: BookSource[] = [
    { bookSourceName: 'Alpha', bookSourceUrl: 'https://alpha.example', enabled: true },
    { bookSourceName: 'Beta', bookSourceUrl: 'https://beta.example', enabled: true },
  ]

  const rssSources: RssSource[] = [
    { sourceName: 'News', sourceUrl: 'https://news.example', enabled: true },
    { sourceName: 'Tech', sourceUrl: 'https://tech.example', enabled: true },
  ]

  it('keeps bulk selection scoped to currently visible source urls', () => {
    const selected = new Set(['https://alpha.example', 'https://hidden.example'])

    expect(getVisibleSelection(bookSources, selected, (source) => source.bookSourceUrl)).toEqual([
      bookSources[0],
    ])
  })

  it('builds batch delete payloads using stable source identifiers', () => {
    expect(toBookSourceDeletePayload(bookSources)).toEqual([
      { bookSourceUrl: 'https://alpha.example' },
      { bookSourceUrl: 'https://beta.example' },
    ])
    expect(toRssSourceDeletePayload(rssSources)).toEqual([
      { sourceName: 'News', sourceUrl: 'https://news.example' },
      { sourceName: 'Tech', sourceUrl: 'https://tech.example' },
    ])
  })

  it('extracts sorted groups from comma-like source group separators', () => {
    const grouped: BookSource[] = [
      { bookSourceName: 'A', bookSourceUrl: 'a', bookSourceGroup: '小说, API' },
      { bookSourceName: 'B', bookSourceUrl: 'b', bookSourceGroup: 'API；精选、小说' },
      { bookSourceName: 'C', bookSourceUrl: 'c' },
    ]

    expect(getBookSourceGroups(grouped)).toEqual(['API', '小说', '精选'])
  })

  it('filters book sources by text and group', () => {
    const grouped: BookSource[] = [
      { bookSourceName: '示例阅读', bookSourceUrl: 'https://reader.example', bookSourceGroup: 'API' },
      { bookSourceName: '示例书库', bookSourceUrl: 'https://library.example', bookSourceGroup: '网页' },
    ]

    expect(filterBookSources(grouped, 'reader', '')).toEqual([grouped[0]])
    expect(filterBookSources(grouped, '', '网页')).toEqual([grouped[1]])
  })

  it('summarizes source counts and selected source metadata', () => {
    const list: BookSource[] = [
      { bookSourceName: 'Enabled', bookSourceUrl: 'enabled', enabled: true, ruleSearch: {}, ruleToc: {} },
      { bookSourceName: 'Disabled', bookSourceUrl: 'disabled', enabled: false },
    ]

    expect(getBookSourceStats(list, [list[0]])).toEqual({
      total: 2,
      enabled: 1,
      filtered: 1,
    })
    expect(getBookSourceOverview(list[0])).toMatchObject({
      group: '未分组',
      statusText: '启用',
      hasSearch: true,
      hasToc: true,
    })
  })

  it('merges source groups across separators without duplicates', () => {
    const source: BookSource = {
      bookSourceName: 'A',
      bookSourceUrl: 'a',
      bookSourceGroup: '小说,API',
    }

    expect(addBookSourceGroups(source, '玄幻；API、都市')?.bookSourceGroup).toBe('小说,API,玄幻,都市')
    expect(addBookSourceGroups(
      { bookSourceName: 'B', bookSourceUrl: 'b' },
      ' 新组 ',
    ).bookSourceGroup).toBe('新组')
    // 空输入(仅分隔符/空白)时保持原对象语义不变
    expect(addBookSourceGroups(source, ' , ; ')).toBe(source)
  })

  it('removes source groups and clears the field when emptied', () => {
    const source: BookSource = {
      bookSourceName: 'A',
      bookSourceUrl: 'a',
      bookSourceGroup: '小说,API,玄幻',
    }

    expect(removeBookSourceGroups(source, 'API；玄幻').bookSourceGroup).toBe('小说')
    expect(removeBookSourceGroups(
      { bookSourceName: 'B', bookSourceUrl: 'b', bookSourceGroup: '小说' },
      '小说',
    ).bookSourceGroup).toBe('')
    expect(removeBookSourceGroups(source, '不存在的').bookSourceGroup).toBe('小说,API,玄幻')
  })

  it('moves selected sources to the top while keeping their relative order', () => {
    const sources: BookSource[] = [
      { bookSourceName: 'P1', bookSourceUrl: 'p1', customOrder: -2 },
      { bookSourceName: 'A', bookSourceUrl: 'a' },
      { bookSourceName: 'B', bookSourceUrl: 'b', customOrder: 0 },
      { bookSourceName: 'C', bookSourceUrl: 'c' },
    ]

    const updated = moveBookSourcesToEdge(sources, new Set(['b', 'c']), 'top')
    const byUrl = Object.fromEntries(updated.map((source) => [source.bookSourceUrl, source]))

    // 列表顺序 B 在 C 前 → B 获得更小序号, 两者都排在已置顶的 P1(-2) 之前
    expect(byUrl.b.customOrder).toBe(-4)
    expect(byUrl.c.customOrder).toBe(-3)
    expect(byUrl.p1.customOrder).toBe(-2)
    expect(byUrl.a.customOrder).toBeUndefined()
  })

  it('moves selected sources to the bottom behind the largest existing order', () => {
    const sources: BookSource[] = [
      { bookSourceName: 'P1', bookSourceUrl: 'p1', customOrder: -1 },
      { bookSourceName: 'A', bookSourceUrl: 'a' },
      { bookSourceName: 'B', bookSourceUrl: 'b' },
      { bookSourceName: 'C', bookSourceUrl: 'c', customOrder: 5 },
    ]

    const updated = moveBookSourcesToEdge(sources, new Set(['a', 'b']), 'bottom')
    const byUrl = Object.fromEntries(updated.map((source) => [source.bookSourceUrl, source]))

    expect(byUrl.a.customOrder).toBe(6)
    expect(byUrl.b.customOrder).toBe(7)
    expect(byUrl.p1.customOrder).toBe(-1)
    expect(byUrl.c.customOrder).toBe(5)
  })

  it('returns the original list untouched when nothing is selected', () => {
    const sources: BookSource[] = [
      { bookSourceName: 'A', bookSourceUrl: 'a' },
      { bookSourceName: 'B', bookSourceUrl: 'b' },
    ]

    expect(moveBookSourcesToEdge(sources, new Set(), 'top')).toBe(sources)
    expect(moveBookSourcesToEdge(sources, new Set(), 'bottom')).toBe(sources)
  })

  it('treats reformatted editor text as unchanged but real edits as changed', () => {
    const baseline = JSON.stringify({ bookSourceName: 'A', enabled: true }, null, 2)

    // 原样、或仅缩进不同(点过「格式化」) → 无改动, 可以用最新数据覆盖
    expect(isEditorTextUnchanged(baseline, baseline)).toBe(true)
    expect(isEditorTextUnchanged(baseline, baseline.trim())).toBe(true)
    expect(isEditorTextUnchanged(
      JSON.stringify({ bookSourceName: 'A', enabled: true }),
      baseline,
    )).toBe(true)

    // 真改了字段 → 有改动, 绝不覆盖用户的编辑
    expect(isEditorTextUnchanged(
      JSON.stringify({ bookSourceName: 'B', enabled: true }, null, 2),
      baseline,
    )).toBe(false)
    expect(isEditorTextUnchanged(baseline, JSON.stringify({ bookSourceName: 'A', enabled: false }, null, 2))).toBe(false)

    // 写了一半、JSON 解析不了 → 按"有改动"处理, 不能当没改过而丢弃
    expect(isEditorTextUnchanged('{ "bookSourceName": ', baseline)).toBe(false)
  })
})
