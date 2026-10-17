import { createApp, h, ref } from 'vue'
import { createPinia } from 'pinia'
import { createMemoryHistory, createRouter } from 'vue-router'
import { mockIPC } from '@tauri-apps/api/mocks'
import type { Channel } from '@tauri-apps/api/core'
import BookDetailModal from '../../src/components/BookDetailModal.vue'
import ReaderSource from '../../src/components/reader/ReaderSource.vue'
import SearchResults from '../../src/components/SearchResults.vue'
import { useBookshelfStore } from '../../src/stores/bookshelf'
import { useReaderStore } from '../../src/stores/reader'
import { useSourceStore } from '../../src/stores/source'
import type { Book, BookSource, SearchBook } from '../../src/types'
import '../../src/styles/global.css'

const params = new URLSearchParams(location.search)
const mode = params.get('mode') || 'detail'
const sourceUrls = {
  deqi: 'https://deqi.example.invalid',
  xiaoyuan: 'https://xiaoyuan.example.invalid',
  aggregate: 'https://aggregate.example.invalid',
  sudugu: 'https://sudugu.example.invalid',
}
const sources: BookSource[] = [
  { bookSourceName: '书源一', bookSourceUrl: sourceUrls.deqi, enabled: true },
  { bookSourceName: '书源二', bookSourceUrl: sourceUrls.xiaoyuan, enabled: true },
  { bookSourceName: '聚合源', bookSourceUrl: sourceUrls.aggregate, enabled: true },
  { bookSourceName: '书源三', bookSourceUrl: sourceUrls.sudugu, enabled: true },
]
const common = { name: '斗破苍穹', author: '天蚕土豆' }
const deqi: SearchBook = {
  ...common, origin: sourceUrls.deqi, originName: sources[0].bookSourceName,
  bookUrl: 'https://deqi.example.invalid/book/1', lastChapter: '最新章节',
}
const xiaoyuan: SearchBook[] = Array.from({ length: 5 }, (_, index) => ({
  ...common, author: '', origin: sourceUrls.xiaoyuan, originName: sources[1].bookSourceName,
  bookUrl: 'https://xiaoyuan.example.invalid/book/' + (index + 1),
}))
const maoyan: SearchBook = {
  ...common, origin: sourceUrls.aggregate, originName: sources[2].bookSourceName,
  bookUrl: 'https://aggregate.example.invalid/book/1?site=maoyan', lastChapter: '猫眼',
}
const baidu: SearchBook = {
  ...maoyan, bookUrl: 'https://aggregate.example.invalid/book/1?site=baidu', lastChapter: '百度',
}
const variantAuthors = [
  '天蚕土豆_qd22', '作者： 天蚕土豆著', '', '天蚕土豆',
  '天蚕土豆_qd22', '作者： 天蚕土豆著', '', '天蚕土豆',
]
const authorVariants: SearchBook[] = variantAuthors.map((author, index) => ({
  ...maoyan, author, bookUrl: sourceUrls.aggregate + '/book/1?site=variant-' + index,
  lastChapter: '分站' + (index + 3),
}))
const sudugu: SearchBook = {
  ...deqi, origin: sourceUrls.sudugu, originName: '书源三',
  bookUrl: sourceUrls.sudugu + '/book/1',
}
const hits = [
  deqi, ...xiaoyuan, maoyan, baidu,
  ...(params.has('authors') ? [...authorVariants, sudugu] : []),
]
const related: SearchBook = {
  ...maoyan, name: '斗破苍穹之无上之境',
  bookUrl: sourceUrls.aggregate + '/book/related', lastChapter: '衍生作品',
}
const rawHits = params.has('authors') ? [...hits, related] : hits
const calls: Array<{ command: string; req: Record<string, unknown> }> = []
const toBook = (hit: SearchBook): Book => ({ ...hit, latestChapterTitle: hit.lastChapter })
const initial = toBook(mode === 'reader' || params.get('current') === 'aggregate' ? maoyan : deqi)
let shelfBooks = [initial]
let completePending: (() => void) | undefined
let chapterPending: (() => void) | undefined

function findHit(url: string, origin?: string): SearchBook {
  const hit = hits.find((item) => item.bookUrl === url && (!origin || item.origin === origin))
  if (!hit) throw new Error('Unexpected fixture book: ' + url)
  return hit
}

mockIPC((command, payload) => {
  const args = payload as { req?: Record<string, any>; onEvent?: Channel<Record<string, unknown>> }
  const req = args.req || {}
  calls.push({ command, req: structuredClone(req) })
  const ok = (data: unknown) => ({ isSuccess: true, errorMsg: '', data })

  switch (command) {
    case 'get_book_sources': return ok(sources)
    case 'search_book_multi_sse':
      queueMicrotask(() => {
        args.onEvent?.onmessage({ event: 'data', data: hits.slice(0, 7) })
        args.onEvent?.onmessage({ event: 'data', data: [baidu, maoyan] })
        args.onEvent?.onmessage({ event: 'end', hasMore: false })
      })
      return
    case 'get_available_book_source_sse':
      queueMicrotask(() => {
        const firstPage = req.lastIndex < 0
        const paged = params.get('pages') === '2'
        const sendPage = () => {
          const data = paged ? (firstPage ? rawHits.slice(0, 6) : rawHits.slice(6)) : rawHits
          const hasErrors = params.has('errors') && firstPage && req.refresh !== 1
          const available = hasErrors ? data.filter((hit) => hit.bookUrl !== xiaoyuan[4].bookUrl) : data
          args.onEvent?.onmessage({ event: 'data', data: available })
          args.onEvent?.onmessage({
            event: 'end',
            lastIndex: paged && firstPage ? 1 : sources.length - 1,
            hasMore: paged && firstPage,
            hasErrors,
          })
        }
        if (params.has('hold') && paged && !firstPage) {
          completePending = sendPage
        } else {
          sendPage()
        }
      })
      return
    case 'cancel_book_search': return
    case 'get_chapter_list': {
      const hit = findHit(req.bookUrl, req.bookSourceUrl)
      const label = hit.lastChapter || '第' + hit.bookUrl.split('/').at(-1) + '版'
      const chapters = [{ index: 0, title: label + ' · 第一章', url: hit.bookUrl + '#chapter=1' }]
      if (params.has('delay-initial-chapters') && req.bookSourceUrl === sourceUrls.deqi) {
        return new Promise((resolve) => {
          chapterPending = () => resolve(ok(chapters))
        })
      }
      if (params.has('delay-chapters') && req.bookSourceUrl === sourceUrls.xiaoyuan) {
        return new Promise((resolve) => {
          chapterPending = () => resolve(ok(chapters))
        })
      }
      return ok(chapters)
    }
    case 'get_book_info': return ok(toBook(findHit(req.url, req.bookSourceUrl)))
    case 'set_book_source': {
      const next = toBook(findHit(req.newUrl, req.bookSourceUrl))
      shelfBooks = [next]
      return ok(next)
    }
    case 'get_book_content': return ok('第一章正文。')
    case 'get_bookshelf':
    case 'get_shelf_book_with_cache_info': return ok(shelfBooks)
    case 'save_book':
      shelfBooks = [req as Book]
      return ok(req)
    case 'save_book_progress': return ok('')
    default: throw new Error('Unexpected fixture command: ' + command)
  }
})

const pinia = createPinia()
const sourceStore = useSourceStore(pinia)
sourceStore.sources = sources
const shelfStore = useBookshelfStore(pinia)
shelfStore.books = shelfBooks
const readerStore = useReaderStore(pinia)
readerStore.book = mode === 'reader' ? { ...initial, sourceCandidates: params.has('errors') ? [] : rawHits } : null
if (mode === 'reader') readerStore.openPanel('source')
if (mode === 'search') shelfStore.startSearch(common.name)

Object.assign(window, {
  __candidateHarness: {
    calls,
    searchResults: () => shelfStore.searchResults,
    currentBook: () => readerStore.book,
    completePending: () => completePending?.(),
    completeChapterPending: () => chapterPending?.(),
  },
})
const router = createRouter({
  history: createMemoryHistory(),
  routes: [{ path: '/:pathMatch(.*)*', component: { render: () => null } }],
})

createApp({
  setup() {
    const visible = ref(true)
    return () => {
      if (mode === 'search') return h(SearchResults)
      if (mode === 'reader') return readerStore.activePanel === 'source' ? h(ReaderSource) : h('p', '换源完成')
      return h('div', [
        h('button', { onClick: () => { visible.value = true } }, '打开详情'),
        h(BookDetailModal, {
          modelValue: visible.value,
          'onUpdate:modelValue': (value: boolean) => { visible.value = value },
          book: initial,
        }),
      ])
    }
  },
}).use(pinia).use(router).mount('#app')
