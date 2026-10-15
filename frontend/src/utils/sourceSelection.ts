import type { BookSource, RssSource } from '../types'

export function getVisibleSelection<T>(
  visibleItems: T[],
  selectedKeys: ReadonlySet<string>,
  keyOf: (item: T) => string,
) {
  return visibleItems.filter((item) => selectedKeys.has(keyOf(item)))
}

export function toBookSourceDeletePayload(sources: Pick<BookSource, 'bookSourceUrl'>[]) {
  return sources.map((source) => ({ bookSourceUrl: source.bookSourceUrl }))
}

export function toRssSourceDeletePayload(sources: Pick<RssSource, 'sourceName' | 'sourceUrl'>[]) {
  return sources.map((source) => ({
    sourceName: source.sourceName,
    sourceUrl: source.sourceUrl,
  }))
}

export function getBookSourceGroups(sources: Pick<BookSource, 'bookSourceGroup'>[]) {
  const groups = new Set<string>()
  sources.forEach((source) => {
    source.bookSourceGroup?.split(/[,;；、]/).forEach((group) => {
      const trimmed = group.trim()
      if (trimmed) groups.add(trimmed)
    })
  })
  return Array.from(groups).sort()
}

/** legado `BookSource.addGroup` 的前端对应: 合并分组集合后以逗号拼接。 */
export function addBookSourceGroups(source: BookSource, groupsInput: string): BookSource {
  const additions = splitBookSourceGroupNames(groupsInput)
  if (!additions.length) return source
  const current = splitBookSourceGroupNames(source.bookSourceGroup || '')
  const merged = Array.from(new Set([...current, ...additions]))
  return { ...source, bookSourceGroup: merged.join(',') }
}

/** legado `BookSource.removeGroup` 的前端对应: 差集为空时清掉分组字段。 */
export function removeBookSourceGroups(source: BookSource, groupsInput: string): BookSource {
  const removals = new Set(splitBookSourceGroupNames(groupsInput))
  if (!removals.size) return source
  const remaining = splitBookSourceGroupNames(source.bookSourceGroup || '')
    .filter((group) => !removals.has(group))
  return { ...source, bookSourceGroup: remaining.length ? remaining.join(',') : '' }
}

function splitBookSourceGroupNames(value: string) {
  return value
    .split(/[,;；、]/)
    .map((group) => group.trim())
    .filter(Boolean)
}

/**
 * 批量置顶/沉底(参考 legado topSource/bottomSource):
 * 以现有最小/最大 customOrder 为基准连续取值, 保持选中项之间的相对顺序。
 * 返回更新后的完整列表(仅选中项的 customOrder 被改写, 其余原样返回)。
 *
 * 排序语义依赖后端 `BookSourceService::list` 的 None → 0 处理:
 * 置顶分配负数排在一切普通源之前; 沉底分配比现有最大值更大的正数排在最后。
 */
export function moveBookSourcesToEdge(
  sources: BookSource[],
  selectedUrls: ReadonlySet<string>,
  edge: 'top' | 'bottom',
) {
  const selected = sources.filter((source) => selectedUrls.has(source.bookSourceUrl))
  if (!selected.length) return sources
  if (edge === 'top') {
    // reduce 而非 Math.min(0, ...arr): 数万书源时 spread 会撞 V8 参数上限抛 RangeError。
    const minOrder = sources.reduce((acc, source) => Math.min(acc, source.customOrder ?? 0), 0)
    // 列表中靠前的选中项获得更小的序号(排更前), 保持选中项原有相对顺序。
    const targets = new Map(
      selected.map((source, index) => [source.bookSourceUrl, minOrder - selected.length + index]),
    )
    return sources.map((source) => {
      const order = targets.get(source.bookSourceUrl)
      return order === undefined ? source : { ...source, customOrder: order }
    })
  }
  const maxOrder = sources.reduce((acc, source) => Math.max(acc, source.customOrder ?? 0), 0)
  const targets = new Map(
    selected.map((source, index) => [source.bookSourceUrl, maxOrder + 1 + index]),
  )
  return sources.map((source) => {
    const order = targets.get(source.bookSourceUrl)
    return order === undefined ? source : { ...source, customOrder: order }
  })
}

/**
 * 编辑器内容是否仍等于最近一次载入的文本(忽略缩进等格式差异)。
 *
 * 用于判断"用户有没有动过编辑器": 只有没动过时, 调用方才能安全地用最新数据
 * 覆盖编辑器内容。**不能**拿编辑对象重新序列化来比 —— 就地修改对象
 * (`Object.assign`) 的路径会让重新序列化的结果提前变成"已更新", 从而把用户
 * 正在写的内容误判成没改过而丢弃。
 */
export function isEditorTextUnchanged(currentText: string, baselineText: string) {
  const current = currentText.trim()
  const baseline = baselineText.trim()
  if (current === baseline) return true
  try {
    return JSON.stringify(JSON.parse(current)) === JSON.stringify(JSON.parse(baseline))
  } catch {
    // 解析不了(例如用户只写了一半) → 当作有改动, 调用方绝不能覆盖它。
    return false
  }
}

export function filterBookSources(
  sources: BookSource[],
  filterText: string,
  filterGroup: string,
) {
  const keyword = filterText.trim().toLowerCase()
  return sources.filter((source) => {
    const matchesText = !keyword
      || source.bookSourceName.toLowerCase().includes(keyword)
      || source.bookSourceUrl.toLowerCase().includes(keyword)
    const matchesGroup = !filterGroup || source.bookSourceGroup?.includes(filterGroup)
    return matchesText && matchesGroup
  })
}

export function getBookSourceStats(allSources: BookSource[], filteredSources: BookSource[]) {
  return {
    total: allSources.length,
    enabled: allSources.filter((source) => source.enabled !== false).length,
    filtered: filteredSources.length,
  }
}

export function getBookSourceOverview(source: BookSource | null) {
  if (!source) {
    return null
  }
  return {
    group: source.bookSourceGroup?.trim() || '未分组',
    statusText: source.enabled === false ? '停用' : '启用',
    exploreText: source.enabledExplore === false ? '发现停用' : '发现可用',
    cookieText: source.enabledCookieJar ? 'Cookie 独立' : '默认 Cookie',
    hasSearch: Boolean(source.searchUrl || source.ruleSearch),
    hasExplore: Boolean(source.exploreUrl || source.ruleExplore),
    hasBookInfo: Boolean(source.ruleBookInfo),
    hasToc: Boolean(source.ruleToc),
    hasContent: Boolean(source.ruleContent),
    hasLogin: Boolean(source.loginUrl?.trim() || source.loginUi?.trim()),
  }
}
