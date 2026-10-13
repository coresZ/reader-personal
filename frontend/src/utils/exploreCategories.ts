export interface ExploreCategory {
  title: string
  url: string
}

/**
 * exploreUrl 是否是「需要执行 JS 才能得到分类」的规则。
 *
 * 这类规则（七猫/书山/墨阅/光遇/故事族都用）在前端只做字符串启发式解析是
 * 永远得不到分类的 —— `parseExploreCategories` 会返回空数组，于是发现页
 * 分类栏与书列表一起空掉。它们必须交给后端 `get_explore_kinds` 在沙盒里
 * 真正求值（对齐 legado `BookSourceExtensions.exploreKinds()`）。
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
