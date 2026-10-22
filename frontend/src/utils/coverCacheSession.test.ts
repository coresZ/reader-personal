import { describe, expect, it, vi, beforeEach } from 'vitest'
import { createCoverCacheSession } from './coverCacheSession'

const clearSearchCoverCache = vi.fn().mockResolvedValue({ deleted: 0 })
vi.mock('../api/bookshelf', () => ({
  clearSearchCoverCache: (...args: unknown[]) => clearSearchCoverCache(...args),
}))

const book = (coverUrl?: string) => ({
  name: 'b',
  author: 'a',
  bookUrl: 'https://x/1',
  origin: 'https://s',
  coverUrl,
})

describe('coverCacheSession', () => {
  beforeEach(() => clearSearchCoverCache.mockClear())

  it('releases collected urls on session end', () => {
    const s = createCoverCacheSession()
    s.collect([book('https://img/a.jpg'), book('https://img/b.jpg')])
    s.release([])
    expect(clearSearchCoverCache).toHaveBeenCalledTimes(1)
    expect(clearSearchCoverCache.mock.calls[0][0].sort()).toEqual([
      'https://img/a.jpg',
      'https://img/b.jpg',
    ])
  })

  it('keeps urls that are still on the shelf', () => {
    const s = createCoverCacheSession()
    s.collect([book('https://img/a.jpg')])
    s.release([book('https://img/a.jpg')] as never)
    expect(clearSearchCoverCache).not.toHaveBeenCalled()
  })

  /** 关键：命中书架的那条会被从 pending 移除，但它**不能**因此从
   *  "已收集"里漏掉后续的释放。这里验证第二次 release 不会重复提交。 */
  it('does not resubmit urls already released', () => {
    const s = createCoverCacheSession()
    s.collect([book('https://img/a.jpg')])
    s.release([])
    s.release([])
    expect(clearSearchCoverCache).toHaveBeenCalledTimes(1)
  })
})
