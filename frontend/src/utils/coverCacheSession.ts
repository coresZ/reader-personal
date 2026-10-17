import type { Book, SearchBook } from '../types'
import { clearSearchCoverCache } from '../api/bookshelf'

/**
 * Tracks cover URLs displayed by a transient list (search results or explore
 * results) and releases them when that list is replaced or left.
 *
 * The backend owns the `public/cover` cache because both shelf and transient
 * views share the same URL-addressed cache. Releasing a URL is safe: if it is
 * still used by another cached view, the backend keeps the shared content file.
 */
export function createCoverCacheSession() {
  let active = new Set<string>()
  let pending = new Set<string>()

  function collect(books: Array<Book | SearchBook>) {
    for (const book of books) {
      const url = book.coverUrl?.trim()
      if (!url || url.startsWith('/local-pdf/')) continue
      active.add(url)
    }
  }

  function release(shelfBooks: Book[]) {
    for (const url of active) pending.add(url)
    active = new Set()

    const shelfCoverUrls = new Set(
      shelfBooks
        .map((book) => book.coverUrl?.trim())
        .filter((url): url is string => !!url),
    )
    const urls: string[] = []
    for (const url of pending) {
      if (shelfCoverUrls.has(url)) {
        pending.delete(url)
      } else {
        urls.push(url)
      }
    }
    if (!urls.length) return
    for (const url of urls) pending.delete(url)
    void clearSearchCoverCache(urls).catch(() => {
      for (const url of urls) pending.add(url)
    })
  }

  return { collect, release }
}
