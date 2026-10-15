import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { deleteRssSource, deleteRssSources, getRssArticles, getRssContent, getRssSources, saveRssSources } from '../api/rss'
import type { RssArticle, RssSource } from '../types'
import { saveRecentReadBook } from '../utils/recentBooks'
import { toRssSourceDeletePayload } from '../utils/sourceSelection'

type ArticleScope = 'source' | 'group' | 'all'
type ScopedArticle = RssArticle & {
  variable?: string
  /** 该条目是"单 URL 源"的网页入口(源地址本身就是一篇文章), 而非真 feed 文章。 */
  singleUrlEntry?: boolean
}

export const useRssStore = defineStore('rss', () => {
  const sources = ref<RssSource[]>([])
  const activeSourceUrl = ref('')
  const articleScope = ref<ArticleScope>('source')
  const activeGroupName = ref('')
  const articles = ref<ScopedArticle[]>([])
  const page = ref(1)
  const hasMore = ref(true)
  const loading = ref(false)
  const contentLoading = ref(false)
  const activeArticle = ref<ScopedArticle | null>(null)
  const activeContent = ref('')
  /** 当前范围是否包含"单 URL 源"(源地址即一篇文章, 点开看网页而非文章列表)。 */
  const singleUrlMode = ref(false)
  /** 单 URL 源的地址是否由源里的 JS 求值得到(自动选到可用节点)。 */
  const singleUrlResolved = ref(false)

  const activeSource = computed(() => sources.value.find((item) => item.sourceUrl === activeSourceUrl.value) || null)
  const enabledSources = computed(() => sources.value.filter((source) => source.enabled !== false))
  const groupNames = computed(() => {
    const groups = new Set<string>()
    enabledSources.value.forEach((source) => {
      ;(source.sourceGroup || '')
        .split(/[,;，、]/)
        .map((item) => item.trim())
        .filter(Boolean)
        .forEach((item) => groups.add(item))
    })
    return Array.from(groups).sort()
  })

  function normalizeRssContent(content?: string | null, description?: string | null) {
    const raw = (content || description || '').trim()
    if (!raw) return ''

    const lowered = raw.toLowerCase()
    if (lowered.includes('<html') || lowered.includes('<body') || lowered.includes('<head')) {
      return ''
    }

    return raw
      .replace(/<script[\s\S]*?<\/script>/gi, '')
      .replace(/<style[\s\S]*?<\/style>/gi, '')
      .replace(/<noscript[\s\S]*?<\/noscript>/gi, '')
      .trim()
  }

  function normalizeFetchedContent(content?: string | null) {
    const raw = (content || '').trim()
    if (!raw) return ''

    const lowered = raw.toLowerCase()
    const looksLikeWholePage =
      lowered.includes('<html') ||
      lowered.includes('<body') ||
      lowered.includes('<head') ||
      lowered.includes('<form') ||
      lowered.includes('搜索') ||
      lowered.includes('首页') ||
      lowered.includes('广告拦截器')

    if (looksLikeWholePage) {
      return ''
    }

    return raw
      .replace(/<script[\s\S]*?<\/script>/gi, '')
      .replace(/<style[\s\S]*?<\/style>/gi, '')
      .replace(/<noscript[\s\S]*?<\/noscript>/gi, '')
      .trim()
  }

  function sourceGroups(source: RssSource) {
    return (source.sourceGroup || '')
      .split(/[,;，、]/)
      .map((item) => item.trim())
      .filter(Boolean)
  }

  function resolveScopeSources() {
    if (articleScope.value === 'all') return enabledSources.value
    if (articleScope.value === 'group' && activeGroupName.value) {
      return enabledSources.value.filter((source) => sourceGroups(source).includes(activeGroupName.value))
    }
    return enabledSources.value.filter((source) => source.sourceUrl === activeSourceUrl.value)
  }

  async function fetchSources() {
    sources.value = await getRssSources().catch(() => [])
    if (!activeSourceUrl.value && enabledSources.value.length) {
      activeSourceUrl.value = enabledSources.value[0]!.sourceUrl
    }
  }

  async function addSources(list: RssSource[]) {
    await saveRssSources(list)
    await fetchSources()
  }

  async function removeSource(source: RssSource) {
    await deleteRssSource({ sourceUrl: source.sourceUrl, sourceName: source.sourceName })
    if (activeSourceUrl.value === source.sourceUrl) {
      activeSourceUrl.value = ''
      articles.value = []
      activeArticle.value = null
      activeContent.value = ''
    }
    await fetchSources()
  }

  async function removeSources(targets: RssSource[]) {
    if (!targets.length) return 0
    const targetUrls = new Set(targets.map((source) => source.sourceUrl))
    const result = await deleteRssSources(toRssSourceDeletePayload(targets))
    if (targetUrls.has(activeSourceUrl.value)) {
      activeSourceUrl.value = ''
      articles.value = []
      activeArticle.value = null
      activeContent.value = ''
    }
    await fetchSources()
    return result.deleted
  }

  async function fetchArticlesForSource(source: RssSource, pageNo: number) {
    const res = await getRssArticles({
      sourceUrl: source.sourceUrl,
      sortName: '默认',
      sortUrl: source.sortUrl || source.sourceUrl,
      page: pageNo,
    })
    // `singleUrl` 源没有文章列表, 后端只回一条合成条目; 标记它以便逐条区分,
    // 避免"范围内有单 URL 源"就把真 feed 的文章也当成网页入口(会丢列表/数量/加载更多)。
    const singleUrl = res.singleUrl === true
    return {
      articles: (res.first || []).map((article) => ({
        ...article,
        variable: source.sourceUrl,
        origin: source.sourceName || article.origin,
        singleUrlEntry: singleUrl || undefined,
      })),
      singleUrl,
      urlEvaluated: res.urlEvaluated === true,
    }
  }

  async function fetchArticles(reset = false) {
    if (loading.value) return
    if (!enabledSources.value.length) return
    if (articleScope.value === 'source' && !activeSourceUrl.value) return

    if (reset) {
      page.value = 1
      hasMore.value = true
      articles.value = []
    }

    loading.value = true
    try {
      const targets = resolveScopeSources()
      const results = await Promise.all(
        targets.map((source) => fetchArticlesForSource(source, page.value)),
      )
      const chunks = results.map((result) => result.articles)
      // `singleUrlMode` 只表示"**当前范围里每一条都是网页入口**", 用于决定列表标题等
      // 整体呈现。混合范围(真 feed + 单 URL 源)必须按条目判断, 否则真 feed 的文章
      // 会被连坐成网页入口。
      const allSingleUrl = results.length > 0 && results.every((result) => result.singleUrl)
      singleUrlMode.value = allSingleUrl
      singleUrlResolved.value = results.some((result) => result.singleUrl && result.urlEvaluated)
      const list = chunks
        .flat()
        .sort((a, b) => {
          const timeA = Date.parse(a.pubDate || '') || a.order || 0
          const timeB = Date.parse(b.pubDate || '') || b.order || 0
          return timeB - timeA
        })

      articles.value = reset ? list : articles.value.concat(list)
      // 单 URL 源没有"下一页", 它的合成条目恒为 1 条, 不能据此认为还有更多。
      hasMore.value = chunks.some(
        (chunk) => chunk.length >= 50 && !chunk.every((item) => item.singleUrlEntry),
      )

      if (list.length) {
        page.value += 1
      }
    } finally {
      loading.value = false
    }
  }

  async function openArticle(article: ScopedArticle) {
    const sourceUrl = article.variable || activeSourceUrl.value
    if (!sourceUrl) return
    activeArticle.value = article
    const inlineContent = normalizeRssContent(article.content, article.description)
    if (inlineContent) {
      activeContent.value = inlineContent
      contentLoading.value = false
      saveRecentReadBook({
        name: article.title || 'RSS 文章',
        author: article.origin || 'RSS',
        bookUrl: article.link,
        origin: `rss:${sourceUrl}`,
        intro: article.description,
        durChapterTime: Date.now(),
        recentKind: 'rss',
        rssSourceUrl: sourceUrl,
        rssLink: article.link,
        rssPubDate: article.pubDate,
      })
      return
    }
    contentLoading.value = true
    try {
      const fetchedContent = await getRssContent({
        sourceUrl,
        link: article.link,
        origin: article.origin || sourceUrl,
      })
      activeContent.value = normalizeFetchedContent(fetchedContent) || inlineContent || ''
      saveRecentReadBook({
        name: article.title || 'RSS 文章',
        author: article.origin || 'RSS',
        bookUrl: article.link,
        origin: `rss:${sourceUrl}`,
        intro: article.description,
        durChapterTime: Date.now(),
        recentKind: 'rss',
        rssSourceUrl: sourceUrl,
        rssLink: article.link,
        rssPubDate: article.pubDate,
      })
    } finally {
      contentLoading.value = false
    }
  }

  async function setSource(url: string) {
    activeSourceUrl.value = url
    articleScope.value = 'source'
    activeGroupName.value = ''
    activeArticle.value = null
    activeContent.value = ''
    await fetchArticles(true)
  }

  async function setGroup(name: string) {
    articleScope.value = 'group'
    activeGroupName.value = name
    activeArticle.value = null
    activeContent.value = ''
    await fetchArticles(true)
  }

  async function setAllSources() {
    articleScope.value = 'all'
    activeGroupName.value = ''
    activeArticle.value = null
    activeContent.value = ''
    await fetchArticles(true)
  }

  return {
    sources,
    activeSourceUrl,
    activeSource,
    enabledSources,
    groupNames,
    articleScope,
    activeGroupName,
    articles,
    page,
    hasMore,
    loading,
    contentLoading,
    activeArticle,
    activeContent,
    singleUrlMode,
    singleUrlResolved,
    fetchSources,
    addSources,
    removeSource,
    removeSources,
    fetchArticles,
    openArticle,
    setSource,
    setGroup,
    setAllSources,
  }
})
