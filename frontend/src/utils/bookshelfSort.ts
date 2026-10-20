/** 书架排序。
 *
 *  对齐 Legado 的书架排序(`BaseBookshelfFragment.configBookshelf`,
 *  `BooksFragment.upRecyclerData` 里的 `when (bookSort)`), 取值与 Legado 完全
 *  一致, 因为分组级的排序偏好要随备份在两端之间往返:
 *
 *  | 值 | 文案     | 比较依据                                    |
 *  |----|----------|---------------------------------------------|
 *  | 0  | 最近阅读 | `durChapterTime` 降序                        |
 *  | 1  | 更新时间 | `latestChapterTime` 降序                     |
 *  | 2  | 书名     | `name` 中文排序                              |
 *  | 3  | 手动排序 | `order` 升序(缺 order 的排在末尾), 不在菜单里 |
 *  | 4  | 综合     | `max(latestChapterTime, durChapterTime)`     |
 *  | 5  | 作者     | `author` 中文排序                            |
 *
 *  比较器必须稳定: 同值的书保留调用方给的原有顺序(JS 的 `Array#sort` 自 2019
 *  起规范要求稳定), 书架才不会在每次刷新时抖动。
 */

/** 中文按拼音比较, 对齐 Legado 的 `String.cnCompare`(Java `Collator`)。 */
const chineseCollator = new Intl.Collator('zh-Hans-CN')

/** 排序值的合法范围; 越界(含备份里的脏值)一律回落到默认。 */
export function normalizeBookSort(value: unknown): number {
  const n = typeof value === 'number' ? value : Number(value)
  if (!Number.isInteger(n) || n < 0 || n > MAX_BOOK_SORT) {
    return DEFAULT_BOOK_SORT
  }
  return n
}

/** 排序要看到的字段子集。
 *
 *  `Book` 与 `SearchBook` 都满足; 抽出来是为了让纯函数能单测, 不必造完整 Book。 */
export interface SortableBook {
  name?: string
  author?: string
  durChapterTime?: number
  latestChapterTime?: number
  lastCheckTime?: number
  order?: number
}

/** 书架排序方式。取值与 Legado 的 `bookshelfSort` 一一对应, 必须保持不变:
 *  备份里的分组级 `bookSort` 与 localStorage 里已存的偏好都用这套数字,
 *  改动即错位。
 *
 *  菜单不再展示「手动排序」(值 3), 但取值保留 —— 老偏好、Legado 归档和
 *  分组设置里仍可能出现 3, 归一化与排序逻辑都要认得它。 */
export const BOOK_SORT_KEYS = {
  recent: 0,
  updated: 1,
  name: 2,
  manual: 3,
  comprehensive: 4,
  author: 5,
} as const

/** 菜单展示的排序项。不含「手动排序」: 它需要拖拽配合, 已从菜单移除。 */
export const BOOK_SORT_OPTIONS: ReadonlyArray<{ value: number; label: string }> = [
  { value: BOOK_SORT_KEYS.recent, label: '最近阅读' },
  { value: BOOK_SORT_KEYS.updated, label: '更新时间' },
  { value: BOOK_SORT_KEYS.name, label: '书名' },
  { value: BOOK_SORT_KEYS.comprehensive, label: '综合' },
  { value: BOOK_SORT_KEYS.author, label: '作者' },
]

/** 全部排序方式的文案, 按下标取值(与 Legado 的 bookshelfSort 一一对应)。
 *
 *  菜单只用其中的一部分(见 `BOOK_SORT_OPTIONS`), 但「手动排序」的文案仍要
 *  保留: 老偏好或分组设置里存着 3 时, 触发器得如实显示它, 而不是错显成
 *  「最近阅读」。 */
const BOOK_SORT_LABEL_BY_VALUE = [
  '最近阅读',
  '更新时间',
  '书名',
  '手动排序',
  '综合',
  '作者',
] as const

/** 排序值 → 文案。用于下拉触发器显示当前选择。 */
export function bookSortLabel(value: number): string {
  return BOOK_SORT_LABEL_BY_VALUE[value] ?? BOOK_SORT_LABEL_BY_VALUE[DEFAULT_BOOK_SORT]
}

/** 归一化排序值, 并把已下架的「手动排序」迁到默认档。
 *
 *  「手动排序」已从菜单移除, 但它仍需参与排序计算(见 `sortBooks`), 所以
 *  `normalizeBookSort` 继续认它。而**展示用的**设置必须迁移: 否则偏好停在 3
 *  的用户会卡在一个菜单里选不出来的状态 —— 书架按 order 排、拖拽可用, 但一旦
 *  切到别的排序就再也回不去。菜单只暴露五种, 设置也收敛到这五种。 */
export function toAvailableBookSort(value: unknown): number {
  const sort = normalizeBookSort(value)
  return sort === BOOK_SORT_KEYS.manual ? DEFAULT_BOOK_SORT : sort
}

/** 该排序值是否已从菜单下架(仅「手动排序」)。 */
export function isRetiredBookSort(value: number): boolean {
  return normalizeBookSort(value) === BOOK_SORT_KEYS.manual
}

/** 默认排序: 与 Legado 的 `AppConfig.bookshelfSort` 默认值 0 一致。 */
export const DEFAULT_BOOK_SORT = BOOK_SORT_KEYS.recent

/** 排序值的合法范围。手动排序(3)仍算合法, 只是不出现在菜单里。 */
const MAX_BOOK_SORT = BOOK_SORT_KEYS.author

/** 时间字段归一化到**毫秒**。
 *
 *  这是实际数据里踩到过的坑: 从书源抓来的 `durChapterTime` 是 Unix **秒**
 *  (如 1792383780), 而 `lastCheckTime` 是**毫秒** (如 1791026656621)。
 *  直接用 `Math.max` 比较会把所有书都判成「更新时间」相同, 综合排序形同失效。
 *  秒级阈值取 1e11 (约 5138 年): 毫秒时间戳自 1973 年起就超过它, 秒级则要到
 *  公元 5138 年, 现实数据不会误判。 */
function toMillis(value: number | undefined): number {
  if (!Number.isFinite(value) || !value || value <= 0) return 0
  const v = value as number
  return v < 1e11 ? v * 1000 : v
}

/** Legado 的「更新时间」用 `latestChapterTime`; 本应用没有这个字段, 由
 *  `lastCheckTime`(检查更新/抓目录的时间)代替。两者都是毫秒。 */
function latestUpdateOf(book: SortableBook): number {
  return toMillis(book.latestChapterTime) || toMillis(book.lastCheckTime)
}

function lastReadOf(book: SortableBook): number {
  return toMillis(book.durChapterTime)
}

/** 缺省 order 的书的统一排序键: 比任何现实 order 都大, 但彼此相等,
 *  借稳定排序保持它们在入参里的原有相对顺序。 */
const MISSING_ORDER = Number.MAX_SAFE_INTEGER

/** 手动排序的序号。
 *
 *  有 `order` 的书按它排; 没有的(老数据、Legado 归档里缺字段、以及**新加入
 *  的书**)一律排在所有有 order 的书**之后**, 彼此之间保持入参里的原有顺序
 *  (稳定排序, 不需要额外的次级键)。
 *
 *  不能简单地「缺 order 就用数组下标」: 下标与已有 order 是两套来源, 混排会
 *  撞出重复值。例如书架已有 order 0/1/2 三本, 新加两本没有 order —— 若用下标
 *  兜底, 新书拿到 3/4 恰好接上还相安无事, 但一旦中间有书被删过(order 变成
 *  0/1/5), 下标兜底就会产生重复, 顺序从此依赖排序稳定性, 每次刷新可能不同。
 *  统一「缺省排末尾」既不会撞值, 也符合直觉: 新书出现在书架末尾。 */
function manualOrderOf(book: SortableBook): number {
  const order = book.order
  return typeof order === 'number' && Number.isFinite(order) ? order : MISSING_ORDER
}

/** 降序比较, 但同值时返回 0 以保持稳定排序。 */
function compareDesc(a: number, b: number): number {
  return b - a
}

/** 按给定方式排序, 返回**新数组**(不改动入参)。
 *
 *  比较器只用书本身的字段, 不掺数组下标 —— 稳定排序(ES2019 起规范保证)
 *  已经能保住同值书的原有相对位置, 掺下标反而会让「先按 A 排再按 B 排」
 *  这类多级意图变得不可预测。 */
export function sortBooks<T extends SortableBook>(books: T[], mode: number): T[] {
  const sort = normalizeBookSort(mode)
  const ordered = books.slice().sort((a, b) => {
    switch (sort) {
      case 1:
        return compareDesc(latestUpdateOf(a), latestUpdateOf(b))
      case 2:
        return chineseCollator.compare(a.name || '', b.name || '')
      case 3:
        return manualOrderOf(a) - manualOrderOf(b)
      case 4:
        return compareDesc(
          Math.max(latestUpdateOf(a), lastReadOf(a)),
          Math.max(latestUpdateOf(b), lastReadOf(b)),
        )
      case 5:
        return chineseCollator.compare(a.author || '', b.author || '')
      case 0:
      default:
        return compareDesc(lastReadOf(a), lastReadOf(b))
    }
  })

  return ordered
}

/** 拖拽后重新计算 `order` 序号。
 *
 *  `books` 是当前展示顺序, `draggedUrl` 被移到 `targetUrl` 的位置。返回
 *  **新数组**, 其中每本书的 `order` 都被改写成 0..n-1 —— 只改被移动的那本
 *  会让序号在多次拖拽后出现重复值, 届时顺序取决于排序稳定性, 表现为
 *  「拖了但没完全生效」。
 *
 *  采用「全量重编号」而不是「只给被移动的书挑一个中间值」: 后者需要浮点数
 *  或大间隔, 而 Legado 的 `order` 是 Int, 全量重编号既简单又天然幂等。
 *
 *  只返回 `order` 有变化的书, 便于调用方复用整表写回接口。 */
export function applyManualOrder<T extends SortableBook>(
  books: T[],
  draggedUrl: string,
  targetUrl: string,
  urlOf: (book: T) => string,
): T[] {
  if (!draggedUrl || !targetUrl || draggedUrl === targetUrl) return []
  const fromIndex = books.findIndex((book) => urlOf(book) === draggedUrl)
  const toIndex = books.findIndex((book) => urlOf(book) === targetUrl)
  if (fromIndex === -1 || toIndex === -1 || fromIndex === toIndex) return []

  const next = books.slice()
  const [moved] = next.splice(fromIndex, 1)
  next.splice(toIndex, 0, moved!)

  const changed: T[] = []
  next.forEach((book, index) => {
    if (book.order !== index) changed.push({ ...book, order: index })
  })
  return changed
}
