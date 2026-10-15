export interface ExploreCategory {
  title: string
  url: string
}

/**
 * exploreUrl 是否是「必须执行 JS 才能得到分类」的规则。
 *
 * `@js:` / `<js>` 前缀（聚合类书源常用）在前端只做字符串启发式
 * 解析永远得不到分类 —— `parseExploreCategories` 返回空数组，于是发现页分类栏
 * 与书列表一起空掉。它们必须交给后端 `get_explore_kinds` 在沙盒里真正求值
 * （对齐 legado `BookSourceExtensions.exploreKinds()`）。
 *
 * 注意这个判据**不是**决定要不要走后端的唯一条件：`loadKinds` 在本地解析不出
 * 任何分类时也会回退到后端，覆盖 `[{'title':...}]` 这类裸 JS 字面量
 * —— 它没有 `@js:` 前缀，但同样不是合法 JSON。两处配合，不必在这里堆规则。
 */
export function isJsExploreUrl(rule?: string | null): boolean {
  const trimmed = rule?.trim().toLowerCase()
  if (!trimmed) return false
  return trimmed.startsWith('@js:') || trimmed.startsWith('<js>')
}

export function parseExploreCategories(rule?: string | null): ExploreCategory[] {
  const trimmedRule = rule?.trim()
  if (!trimmedRule) return []

  try {
    if (trimmedRule.startsWith('[')) {
      const parsed = JSON.parse(normalizeRelaxedExploreJson(trimmedRule))
      if (Array.isArray(parsed)) {
        return parsed
          .map((item) => ({
            title: String(item?.title || '').trim(),
            url: String(item?.url || '').trim(),
          }))
          .filter((item) => item.title)
      }
    }
  } catch {
    // Fall back to the plain text parser below.
  }

  return trimmedRule
    .split(/\n|<br>/i)
    .map((line) => line.trim())
    .filter(Boolean)
    .flatMap((line) => {
      if (!line.includes('::')) return []
      const [title, url] = line.split('::').map((part) => part.trim())
      return title ? [{ title, url: url || '' }] : []
    })
}

function normalizeRelaxedExploreJson(rule: string) {
  let normalized = ''
  let inString = false
  let quote = ''
  let escaped = false

  for (const char of rule) {
    if (inString) {
      normalized += char
      if (escaped) {
        escaped = false
      } else if (char === '\\') {
        escaped = true
      } else if (char === quote) {
        inString = false
      }
      continue
    }

    if (char === '"' || char === "'") {
      inString = true
      quote = char
      normalized += char
    } else if (char === '<') {
      normalized += '{'
    } else if (char === '>') {
      normalized += '}'
    } else {
      normalized += char
    }
  }

  return normalized
}

export function isExploreCategorySection(category: ExploreCategory) {
  return !category.url.trim()
}

export function getInitialExploreCategoryUrl(categories: ExploreCategory[]) {
  return categories.find((category) => !isExploreCategorySection(category))?.url || ''
}

export function getExploreCategoryKey(category: ExploreCategory, index: number) {
  return `${category.url || 'section'}:${index}:${category.title}`
}
