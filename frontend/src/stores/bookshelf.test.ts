import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useBookshelfStore } from './bookshelf'
import { getBookshelfWithCacheInfo, saveBooks } from '../api/bookshelf'

vi.mock('../api/bookshelf', () => ({
  getBookshelfWithCacheInfo: vi.fn(),
  getBookGroups: vi.fn(),
  deleteBook: vi.fn(),
  deleteBooks: vi.fn(),
  saveBookGroupId: vi.fn(),
  saveBookGroup: vi.fn(),
  deleteBookGroup: vi.fn(),
  saveBooks: vi.fn(),
}))

vi.mock('../utils/recentBooks', () => ({
  clearRecentReadBooks: vi.fn(),
  getRecentReadBookKey: vi.fn((book) => `${book.origin || ''}::${book.bookUrl}`),
  loadRecentReadBooks: vi.fn(() => []),
  removeRecentReadBook: vi.fn(),
}))

describe('bookshelf search state', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(getBookshelfWithCacheInfo).mockResolvedValue([])
    vi.mocked(saveBooks).mockResolvedValue([] as never)
  })

  it('starts searches in all-sources scope by default', () => {
    const store = useBookshelfStore()

    store.startSearch('星门')

    expect(store.searchKey).toBe('星门')
    expect(store.searchScope).toBe('all')
    expect(store.searchSourceUrl).toBe('')
    expect(store.searchGroup).toBe('')
  })


  it('keeps application cache counts from the backend', async () => {
    vi.mocked(getBookshelfWithCacheInfo).mockResolvedValue([
      {
        name: '本地书',
        author: '本地导入',
        origin: 'local-txt',
        bookUrl: 'local-txt:abc',
        cachedChapterCount: 12,
      },
      {
        name: '远程书',
        author: '作者',
        origin: 'https://source.example',
        bookUrl: 'https://book.example/1',
      },
    ] as never)
    const store = useBookshelfStore()

    await store.fetchBooks()

    expect(store.books.find((book) => book.bookUrl === 'local-txt:abc')?.cachedChapterCount).toBe(12)
    expect(store.books.find((book) => book.bookUrl === 'https://book.example/1')?.cachedChapterCount).toBeUndefined()
  })

  it('can start a search with the active explore source selected', () => {
    const store = useBookshelfStore()

    store.startSearch('星门', { scope: 'source', sourceUrl: 'https://m.cuoceng.com' })

    expect(store.searchKey).toBe('星门')
    expect(store.searchScope).toBe('source')
    expect(store.searchSourceUrl).toBe('https://m.cuoceng.com')
  })
})

describe('setBooksGroupMask (勾选式分组编辑器落盘)', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(getBookshelfWithCacheInfo).mockResolvedValue([
      {
        name: '书A',
        author: '作者',
        origin: 'local-txt',
        bookUrl: 'book:a',
        group: 1,
      },
      {
        name: '书B',
        author: '作者',
        origin: 'local-txt',
        bookUrl: 'book:b',
        group: 2,
      },
    ] as never)
    vi.mocked(saveBooks).mockResolvedValue([] as never)
  })

  it('把目标书的位域整体替换为掩码(勾选=加入, 取消勾选=移出)', async () => {
    const store = useBookshelfStore()
    await store.fetchBooks()

    // 书A 从分组 1 移到分组 1|4(同时属于两个分组), 书B 不动。
    await store.setBooksGroupMask(['book:a'], 1 | 4)

    expect(saveBooks).toHaveBeenCalled()
    const saved = lastSavedBooks()
    expect(saved.find((b) => b.bookUrl === 'book:a')?.group).toBe(5)
    expect(saved.find((b) => b.bookUrl === 'book:b')?.group).toBe(2)
    // 内存同步更新, 不等全量刷新
    expect(store.books.find((b) => b.bookUrl === 'book:a')?.group).toBe(5)
  })

  it('掩码为 0 时即「移出所有分组」', async () => {
    const store = useBookshelfStore()
    await store.fetchBooks()

    await store.setBooksGroupMask(['book:a'], 0)

    const saved = lastSavedBooks()
    expect(saved.find((b) => b.bookUrl === 'book:a')?.group).toBe(0)
  })

  it('落盘失败时回滚内存, 不留脏快照', async () => {
    const store = useBookshelfStore()
    await store.fetchBooks()
    vi.mocked(saveBooks).mockRejectedValueOnce(new Error('boom'))

    await expect(store.setBooksGroupMask(['book:a'], 8)).rejects.toThrow('boom')
    // 回滚: 内存里书 A 仍在分组 1
    expect(store.books.find((b) => b.bookUrl === 'book:a')?.group).toBe(1)
  })
})

/** 取 saveBooks 最近一次收到的载荷(vi 的 mock 状态跨测试保留, 必须取最后)。 */
function lastSavedBooks() {
  const calls = vi.mocked(saveBooks).mock.calls
  return calls[calls.length - 1][0]
}
