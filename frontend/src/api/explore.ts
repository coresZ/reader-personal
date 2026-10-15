import { post } from './invoke'
import type { SearchBook } from '../types'
import type { ShowBrowserRequest } from './sourceClick'

export interface ExploreBookParams {
  bookSourceUrl: string
  ruleFindUrl: string
  page?: number
}

/** 后端 `ExploreKind`（legado 同名实体）。 */
export interface ExploreKind {
  title: string
  url?: string | null
  /** `url` | `text` | `button` | `toggle` | `select`（legado `ExploreKind.Type`）。 */
  type?: string | null
  /** 控件点击/变更时执行的 JS。 */
  action?: string | null
  /** select / toggle 的选项。 */
  chars?: (string | null)[] | null
  default?: string | null
  /** 控件显示名：`'字面量'` 直接取用，否则是段 JS。 */
  viewName?: string | null
  style?: unknown
}

/** 发现页控件的规范类型。 */
export type ExploreKindType = 'url' | 'text' | 'button' | 'toggle' | 'select'

/** 归一化控件类型；legado 缺省为 `url`。 */
export function exploreKindType(kind: ExploreKind): ExploreKindType {
  const raw = (kind.type ?? '').trim()
  if (raw === 'text' || raw === 'button' || raw === 'toggle' || raw === 'select') return raw
  return 'url'
}

/** 非空选项列表（后端可能给出含 null 的数组）。 */
export function exploreKindChars(kind: ExploreKind): string[] {
  return (kind.chars ?? [])
    .map((item) => String(item ?? '').trim())
    .filter((item) => item.length > 0)
}

/**
 * 控件的显示名：优先后端求值好的 `viewName`，否则回退 `title`。
 *
 * legado 让书源自定义控件文字（聚合源的筛选用 `'请选择类型'`，源乙用一段读源
 * 变量的 JS），显示名已在后端沙箱解析完成。
 */
export function exploreKindLabel(kind: ExploreKind): string {
  const view = (kind.viewName ?? '').trim()
  return view.length > 0 ? view : kind.title
}

/** 后端 `ExploreActionOutcome`：action 脚本产生的副作用意图。 */
export interface ExploreActionOutcome {
  value: string
  /** `java.refreshExplore()`：应重新拉分类与列表。 */
  exploreRefresh: boolean
  /** `java.reLoginView()` / `source.login()`：应重开登录面板。 */
  loginRefresh: boolean
  login: boolean
  openUrls?: string | null
  /** `java.open` / `java.searchBook` / `java.openBook` 登记的待执行动作。 */
  intents: { kind: string; payload: unknown }[]
  /**
   * `java.startBrowser` / `java.showBrowser` 弹出的面板。
   *
   * 「更新书源」「书源设置」这类按钮的全部效果就是这个面板 —— 缺了它按钮
   * 点了毫无反应。
   */
  showBrowser?: ShowBrowserRequest | null
  /** 脚本产生的提示（`java.toast`）。 */
  messages?: string[]
  values: Record<string, unknown>
  logs: string[]
}

/**
 * 获取探索发现的书本列表
 * 对应后端 /reader3/exploreBook 接口
 */
export function exploreBook(params: ExploreBookParams) {
  return post<SearchBook[]>('/exploreBook', params).then((r) => r.data)
}

/**
 * 求值书源的 exploreUrl，拿到发现页分类（含交互控件）。
 *
 * 对应后端 `/reader3/getExploreKinds` → `BookService::explore_kinds`，会真正在
 * rquickjs 沙盒里执行 `@js:` / `<js>` 规则（对齐 legado `exploreKinds()`）。
 *
 * `infoMap` 是发现页控件的当前取值：JS 型规则会读它来决定生成哪些分类
 * （如"源站=源甲"下的排行榜），缺省时后端按空 map 求值并回填默认值。
 */
export function getExploreKinds(params: {
  bookSourceUrl: string
  infoMap?: Record<string, string>
}) {
  return post<ExploreKind[]>('/getExploreKinds', params).then((r) => r.data)
}

/**
 * 执行发现页控件的 `action`（按钮点击 / 下拉选择 / 输入框防抖回调）。
 *
 * action 里的 `java.refreshExplore()`、`java.open('login',...)`、
 * `java.searchBook(...)` 都会在**后端**求值并回传意图标志 —— WebView 里没有
 * `java` 对象，这类副作用只能在宿主侧产生。
 */
export function runExploreAction(params: {
  bookSourceUrl: string
  action: string
  infoMap: Record<string, string>
}) {
  return post<ExploreActionOutcome>('/runExploreAction', params).then((r) => r.data)
}
