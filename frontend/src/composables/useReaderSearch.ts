import { computed, nextTick, ref } from 'vue'
import type { useReaderStore } from '../stores/reader'
import { cancelPageScrollAnimation } from '../utils/readerPaging'

type ReaderStore = ReturnType<typeof useReaderStore>

export interface SearchResultItem {
  chapterIndex: number
  chapterTitle: string
  snippet: string
}

/** 定位请求的上下文。
 *
 *  `explicit` 区分**用户跳转**与**后台重算**:
 *  - true  = 点结果项 / 上一条 / 下一条 / 点搜索按钮或回车 —— 明确的"带我去那里"。
 *  - false = 换章、改排版、逐字输入引起的自动重算 —— 只更新高亮与计数。
 *
 *  左右分页下这个区分是必需的: 内容位移只能由页索引驱动, 若自动重算也去改索引,
 *  用户每敲一个字都会被送去"第 1 个命中所在页", 关键词又在章首出现时看起来就是
 *  跳回本章第一页。 */
export interface SearchLocateOptions {
  explicit: boolean
}

/** 页码定位回调: 把第 `matchIndex` 个高亮定位到视野内。
 *
 *  纵向/连续阅读靠滚动容器, 直接 scrollIntoView 即可; 左右分页的画面位移由
 *  `.horizontal-pages` 的 transform 驱动(容器本身 overflow: hidden, 没有可滚动
 *  空间), scrollIntoView 对它是空操作, 必须换页索引。两种定位方式的差异由
 *  ReaderView 注入 —— 分页重建会把 DOM 节点整个换掉, 所以除了元素本身, 还要把
 *  匹配序号一并交出去, 让注入方能在新分页里重新解析目标页。
 *
 *  `target` 为 null 表示**当前 DOM 里还找不到高亮**(左右分页渲染的是上一份分页
 *  快照, 高亮要等重分页之后才出现)。注入方按序号在新分页里定位, 非分页模式
 *  没有元素可定位, 直接返回即可。 */
export type SearchLocateHandler = (
  target: HTMLElement | null,
  matchIndex: number,
  options: SearchLocateOptions,
) => void | Promise<void>

export function useReaderSearch(store: ReaderStore, locateMatch?: SearchLocateHandler) {
  const showSearch = ref(false)
  const searchQuery = ref('')
  const searchResults = ref<SearchResultItem[]>([])
  const searchIndex = ref(0)
  const searchInputRef = ref<HTMLInputElement>()
  const bookSearchStatus = ref('')
  const pendingSearchResult = ref<SearchResultItem | null>(null)
  const searchCount = computed(() => searchResults.value.length)

  function toggleSearch() {
    showSearch.value = !showSearch.value
    if (showSearch.value) {
      nextTick(() => searchInputRef.value?.focus())
    } else {
      closeSearch()
    }
  }

  function openSearch() {
    if (!showSearch.value) {
      showSearch.value = true
    }
    nextTick(() => searchInputRef.value?.focus())
  }

  function closeSearch() {
    showSearch.value = false
    searchQuery.value = ''
    searchIndex.value = 0
    searchResults.value = []
    bookSearchStatus.value = ''
    pendingSearchResult.value = null
  }

  function buildSearchSnippet(text: string, keyword: string, startIndex: number) {
    const radius = 18
    const start = Math.max(0, startIndex - radius)
    const end = Math.min(text.length, startIndex + keyword.length + radius)
    return `${start > 0 ? '...' : ''}${text.slice(start, end).trim()}${end < text.length ? '...' : ''}`
  }

  function extractSearchResults(text: string, chapterIndex: number, chapterTitle: string, keyword: string, limit = Infinity) {
    if (!keyword) return [] as SearchResultItem[]
    const normalizedText = text.replace(/\s+/g, ' ').trim()
    if (!normalizedText) return [] as SearchResultItem[]
    const escaped = keyword.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
    const regex = new RegExp(escaped, 'gi')
    const list: SearchResultItem[] = []
    let match: RegExpExecArray | null
    while ((match = regex.exec(normalizedText)) && list.length < limit) {
      list.push({
        chapterIndex,
        chapterTitle,
        snippet: buildSearchSnippet(normalizedText, match[0], match.index),
      })
    }
    return list
  }

  /** 重算当前章的匹配列表。
   *
   *  `explicit` 只在**用户明确要求跳转**时传 true(点结果项/上一条/下一条/回车/点
   *  搜索按钮)。换章、改排版、逐字输入都会走到这里, 那些场景只该刷新高亮与计数,
   *  不能动正文位置 —— 否则左右分页下输入关键词会把读者送到第 1 个命中所在页。
   *  见 SearchLocateOptions。 */
  function runChapterSearch(options: { explicit?: boolean } = {}) {
    const explicit = options.explicit === true
    searchIndex.value = 0
    if (!searchQuery.value) {
      searchResults.value = []
      bookSearchStatus.value = ''
      return
    }
    const plainText = (store.displayContent || '').replace(/<[^>]+>/g, '')
    searchResults.value = extractSearchResults(
      plainText,
      store.currentIndex,
      store.currentChapter?.title || '当前章节',
      searchQuery.value,
    )
    nextTick(() => {
      if (searchResults.value.length) scrollToMatch({ explicit })
    })
    bookSearchStatus.value = searchResults.value.length
      ? `共 ${searchResults.value.length} 条结果`
      : '当前章节没有匹配内容'
  }

  function runSearch() {
    if (!searchQuery.value.trim()) {
      searchResults.value = []
      bookSearchStatus.value = ''
      return
    }
    // 用户主动发起的搜索: "去第 1 个命中"是预期行为。
    runChapterSearch({ explicit: true })
  }

  /** 当前章的搜索范围。
   *
   *  连续阅读模式下 DOM 里同时挂着多章正文, 每一章都做过高亮, 而搜索结果只针对
   *  当前章 —— 用 document 查询会把别章的命中一起编号, 序号整体错位, 定位就会落
   *  到别的章去。带 data-chapter-index 的章节 section 存在时以它为界; 纵向/左右
   *  分页只有一个正文容器, 回退到 document 与改动前一致。 */
  function currentChapterScope(): ParentNode {
    const index = store.currentIndex
    if (Number.isInteger(index)) {
      const section = document.querySelector(
        `.continuous-chapter[data-chapter-index="${index}"]`,
      )
      if (section) return section
    }
    return document
  }

  /** 当前范围内的高亮元素, 按文档顺序 —— 与搜索结果的顺序对应。 */
  function queryMatches(): HTMLElement[] {
    return Array.from(currentChapterScope().querySelectorAll('.search-highlight')) as HTMLElement[]
  }

  /** 标出当前匹配。
   *
   *  必须**在定位之后**按序号重新查询: 左右分页的定位会重建分页, 传进来的那个
   *  元素已经被 v-html 换成了新节点, 直接对它加类不会显示在任何地方。按序号取
   *  则不受影响 —— 分页只是切片, 高亮的先后顺序与搜索结果一致。 */
  function markCurrentMatch(matchIndex: number) {
    const matches = queryMatches()
    const target = matches[matchIndex]
    // 取不到目标就别动现有标记: 先清空再取不到会把当前位置指示整个抹掉, 变成
    // "跳转失败且看不出哪条是当前项"的无声失败。
    if (!target) return
    matches.forEach((item) => item.classList.remove('current-match'))
    target.classList.add('current-match')
  }

  /** 定位到某一条匹配并标出它。
   *
   *  `matchIndex` 由调用方在"已经知道要去第几条"时显式传入(点结果项)。不传则用
   *  searchIndex —— 那正是上一条/下一条刚刚写好的值。
   *
   *  `pendingSearchResult` 的摘录只作**兜底**: 序号取不到对应元素时才用它去找。
   *  反过来(用摘录覆盖序号)会把用户点的第 N 条改成"第一个摘录相同"的那条 ——
   *  正文里套话重复时就是跳到第一条, 而这与被点的那条往往相隔很远。 */
  function scrollToMatch(options: { explicit?: boolean; matchIndex?: number } = {}) {
    const explicit = options.explicit === true
    nextTick(async () => {
      const matches = queryMatches()
      // 没有高亮时只有分页模式还能定位: 它渲染的是分页快照, 高亮要等重分页后才
      // 出现, 而 locateSearchMatch 会自己先把分页重建出来。
      if (!matches.length && !locateMatch) return
      let targetIndex = options.matchIndex ?? searchIndex.value
      let target = matches[targetIndex]
      if (pendingSearchResult.value) {
        const snippet = pendingSearchResult.value.snippet.replace(/\.\.\./g, '').trim()
        pendingSearchResult.value = null
        if (!target) {
          const foundIndex = matches.findIndex((match) => {
            const line = (match.parentElement?.innerText || '').replace(/\s+/g, ' ').trim()
            return !!snippet && line.includes(snippet.slice(0, Math.min(12, snippet.length)))
          })
          if (foundIndex >= 0) {
            targetIndex = foundIndex
            target = matches[foundIndex]
          }
        }
      }
      if (!target && !locateMatch) return
      // 翻页补间每帧写 scrollTop, 与 scrollIntoView 的平滑滚动会互抢位置。
      // 搜索跳转是用户意图, 让补间先让位。
      cancelPageScrollAnimation(target?.closest('.reader-scroll-container') as HTMLElement | null)
      if (locateMatch) {
        // 左右分页: 正文位移由 transform + 页索引驱动, 没有可滚动空间,
        // scrollIntoView 是空操作。交给 ReaderView 按页索引换页。
        await locateMatch(target ?? null, targetIndex, { explicit })
      } else if (target && explicit) {
        // 纵向/连续阅读的自动重算也不该挪视口: 逐字输入时视口每敲一下跳一次。
        target.scrollIntoView({ block: 'center', behavior: 'smooth' })
      }
      markCurrentMatch(targetIndex)
    })
  }

  function nextSearchResult() {
    if (!searchCount.value) return
    searchIndex.value = (searchIndex.value + 1) % searchCount.value
    scrollToMatch({ explicit: true })
  }

  function prevSearchResult() {
    if (!searchCount.value) return
    searchIndex.value = (searchIndex.value - 1 + searchCount.value) % searchCount.value
    scrollToMatch({ explicit: true })
  }

  async function jumpToSearchResult(result: SearchResultItem, idx: number) {
    searchIndex.value = idx
    pendingSearchResult.value = result
    if (result.chapterIndex !== store.currentIndex) {
      // 换章会让 store.content 变化, 触发 handleContentUpdated 里的自动重算 ——
      // 那次重算只刷新列表、并且会把 searchIndex 清 0, 所以目标序号必须在这里
      // 显式带上, 不能指望它活过 await。
      await store.loadChapter(result.chapterIndex)
    }
    nextTick(() => {
      scrollToMatch({ explicit: true, matchIndex: idx })
    })
  }

  function handleContentUpdated() {
    if (showSearch.value && searchQuery.value) {
      runChapterSearch()
    }
  }

  function handlePresentationUpdated() {
    if (showSearch.value && searchQuery.value) {
      runChapterSearch()
    }
  }

  return {
    showSearch,
    searchQuery,
    searchResults,
    searchIndex,
    searchInputRef,
    searchCount,
    bookSearchStatus,
    toggleSearch,
    openSearch,
    closeSearch,
    runSearch,
    nextSearchResult,
    prevSearchResult,
    jumpToSearchResult,
    handleContentUpdated,
    handlePresentationUpdated,
  }
}
