import { describe, expect, it, vi } from 'vitest'
import { PDF_PAGE_ATTR, localPdfFileUrl, releasePdfBook } from './pdfPages'

describe('localPdfFileUrl', () => {
  it('points the reader protocol at the pdf route with an encoded bookUrl', () => {
    const url = localPdfFileUrl({ bookUrl: 'local-pdf:abc123' })
    expect(url).toBe('http://reader.localhost/pdf?bookUrl=local-pdf%3Aabc123')
  })

  it('keeps hashes and other query-unsafe characters encoded', () => {
    const url = localPdfFileUrl({ bookUrl: 'local-pdf:abc#1' })
    expect(url).not.toContain('#')
    expect(url).toContain('local-pdf%3Aabc%231')
  })
})

describe('PDF_PAGE_ATTR', () => {
  it('is the attribute the backend writes on each page placeholder', () => {
    expect(PDF_PAGE_ATTR).toBe('data-pdf-page')
  })
})

describe('releasePdfBook', () => {
  it('is a no-op for books that were never rendered', async () => {
    await expect(releasePdfBook('local-pdf:never-opened')).resolves.toBeUndefined()
  })

  it('does not throw when the same book is released twice', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined)
    await releasePdfBook('local-pdf:abc')
    await expect(releasePdfBook('local-pdf:abc')).resolves.toBeUndefined()
  })
})
