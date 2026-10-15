import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { createMemoryHistory, createRouter } from 'vue-router'
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks'
import ReaderView from '../../src/views/ReaderView.vue'
import { useReaderStore } from '../../src/stores/reader'
import { useBookshelfStore } from '../../src/stores/bookshelf'
import { repairSourceImageHtml } from '../../src/utils/sourceImageHtml'
import '../../src/styles/global.css'

const params = new URLSearchParams(location.search)
const long = params.has('long')
const sourceUrl = 'https://source.example.test'
const bookUrl = sourceUrl + '/book/1'
const chapterUrl = sourceUrl + '/chapter/1'
const urls = {
  paragraph: 'https://comments.example.test/paragraph?book_id=book-1&item_id=chapter-1&source=%E7%95%AA%E8%8C%84#part-4',
  god: 'https://comments.example.test/god?book_id=book-1&item_id=chapter-1',
  chapter: 'https://comments.example.test/chapter?book_id=book-1&item_id=chapter-1&source=%E7%95%AA%E8%8C%84',
}
const scripts = {
  paragraph: "showCmt('" + urls.paragraph + "', '源甲' ,'段评')",
  god: "showCmt('" + urls.god + "','段评' )",
  chapter: "showCmt('" + urls.chapter + "','源甲','本章说' )",
}
function icon(label: string) {
  return 'data:image/svg+xml;base64,' + btoa('<svg xmlns="http://www.w3.org/2000/svg" width="48" height="24" viewBox="0 0 48 24"><rect x="1" y="1" width="46" height="22" fill="#eee" stroke="#777"/><text x="18" y="18">' + label + '</text></svg>')
}
const imageUrls = { paragraph: icon('12'), god: icon('1'), chapter: icon('3') }
if (params.has('remote')) imageUrls.chapter = 'https://comments.example.test/chapter-review.svg'
const rawSources = {
  paragraph: imageUrls.paragraph + ',' + JSON.stringify({
    click: scripts.paragraph, style: 'text',
    extra: { examples: ['a > b', { quoted: 'the } ,{ "quoted" value' }] },
  }),
  god: imageUrls.god + ',' + JSON.stringify({ click: scripts.god, style: 'FULL' }),
  chapter: imageUrls.chapter + ",{'type':'qtbzs',\"click\":" + JSON.stringify(scripts.chapter) + ",'style':'FULL'}",
}
const passage = long
  ? '<em>' + '这是跨页正文，标点和顺序都应保留。'.repeat(95) + '</em><strong>' + '后半段继续阅读。'.repeat(60) + '</strong>'
  : '第一段正文。'
const rawHtml = '<p style="margin-top:0;margin-bottom:.2em">' + passage + '<img src="' + rawSources.paragraph + '"></p>'
  + '<p><img src="' + rawSources.god + '"></p>'
  + (params.has('remote') ? '<p>' + '继续查看这一章的讨论。'.repeat(10) + '</p>' : '')
  + '\n<img src="' + rawSources.chapter + '">'
const expected = document.createElement('div')
expected.innerHTML = repairSourceImageHtml(rawHtml)

const book = { name: '段评测试', author: '测试作者', origin: sourceUrl, originName: '聚合源', bookUrl, durChapterIndex: 0 }
const chapters = [{ index: 0, title: '第一章', url: chapterUrl }]
const calls: Array<{ command: string; req: unknown }> = []
mockIPC((command, payload) => {
  const args = payload as { req?: unknown }
  calls.push({ command, req: structuredClone(args?.req || {}) })
  const ok = (data: unknown) => ({ isSuccess: true, errorMsg: '', data })
  switch (command) {
    case 'get_book_content': return ok(rawHtml)
    case 'get_chapter_list': return ok(chapters)
    case 'list_custom_fonts':
    case 'get_bookmarks':
    case 'get_replace_rules':
    case 'get_cached_chapter_urls':
    case 'get_book_sources': return ok([])
    case 'get_bookshelf':
    case 'get_shelf_book_with_cache_info': return ok([book])
    case 'save_book_progress': return ok('')
    case 'eval_source_click': return ok({ result: '', logs: [] })
    default:
      if (command.startsWith('plugin:')) return
      throw new Error('Unexpected comment fixture command: ' + command)
  }
}, { shouldMockEvents: true })
mockWindows('main')

const pinia = createPinia()
const store = useReaderStore(pinia)
useBookshelfStore(pinia).books = [book]
store.book = book
store.chapters = chapters
store.currentIndex = 0
store.content = rawHtml
store.config.fontSize = 18
store.config.lineHeight = 1.8
store.config.pageWidth = 360
store.config.firstLineIndent = false
store.config.readMethod = params.has('horizontal') ? '左右翻页' : '上下滑动'

Object.assign(window, {
  __commentsFixture: { calls, scripts, urls, rawSources, imageUrls, expectedText: expected.textContent, sourceUrl, bookUrl, chapterUrl },
})
const router = createRouter({
  history: createMemoryHistory(),
  routes: [{ path: '/:pathMatch(.*)*', component: { render: () => null } }],
})
createApp(ReaderView).use(pinia).use(router).mount('#app')
