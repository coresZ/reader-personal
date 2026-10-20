/** 书架关键词过滤。
 *
 *  书架标签栏旁的搜索框用它筛出匹配的书。只匹配书的元数据(书名、作者、简介、
 *  书源 URL、来源名、最新章节名), 不搜正文 —— 正文检索属于阅读器与书源搜索,
 *  不在书架的职责内, 也不该为一次输入去读整本书。
 *
 *  字段表与「最近阅读」页的搜索保持一致(`origin` 也在内), 同一关键词在两处
 *  应当得到同样的口径, 否则用户会以为书丢了。
 */

/** 过滤要看到的字段子集。`Book` 与 `SearchBook` 都满足; 抽出来是为了让纯函数
 *  能单测, 不必造完整的书。 */
export interface FilterableShelfBook {
  name?: string
  author?: string
  intro?: string
  origin?: string
  originName?: string
  latestChapterTitle?: string
  durChapterTitle?: string
}

/** 参与匹配的字段。命中任意一个即算匹配。 */
const SEARCH_FIELDS: ReadonlyArray<keyof FilterableShelfBook> = [
  'name',
  'author',
  'intro',
  'origin',
  'originName',
  'latestChapterTitle',
  'durChapterTitle',
]

/** 归一化关键词: 去掉首尾空白并转小写。
 *
 *  中文不受大小写影响, 英文书名/作者则要大小写不敏感 —— 输入 `cos` 也要能
 *  找到《Cosmetic》。 */
export function normalizeShelfKeyword(keyword: string): string {
  return keyword.trim().toLowerCase()
}

/** 关键词是否命中这本书。空关键词视为命中(调用方通常先判空, 这里保持语义完整)。 */
export function matchesShelfKeyword(book: FilterableShelfBook, keyword: string): boolean {
  const needle = normalizeShelfKeyword(keyword)
  if (!needle) return true
  return SEARCH_FIELDS.some((field) => {
    const value = book[field]
    return typeof value === 'string' && value.toLowerCase().includes(needle)
  })
}

/** 过滤书架书籍, **保持入参顺序**。
 *
 *  刻意不在这里排序: 展示顺序由 `sortBooks` 决定, 过滤只做减法 —— 否则搜索
 *  结果会在用户切换排序方式时出现两套顺序来源。 */
export function filterShelfBooks<T extends FilterableShelfBook>(
  books: readonly T[],
  keyword: string,
): T[] {
  if (!normalizeShelfKeyword(keyword)) return books.slice()
  return books.filter((book) => matchesShelfKeyword(book, keyword))
}