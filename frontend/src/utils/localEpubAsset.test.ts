import { describe, expect, it } from 'vitest'
import { readerOrigin } from '../api/scheme'
import { rewriteCssAssetUrls, toReaderAssetUrl } from './localEpubAsset'

describe('toReaderAssetUrl', () => {
  it('把后端资源地址改写成自定义协议地址', () => {
    const url = '/reader3/localEpubAsset?bookUrl=local-epub%3Aabc&path=OEBPS%2FImages%2Fcover.jpg'
    expect(toReaderAssetUrl(url)).toBe(
      `${readerOrigin}/epub?bookUrl=local-epub%3Aabc&path=OEBPS%2FImages%2Fcover.jpg`,
    )
  })

  it('已经是协议地址或外链时原样返回', () => {
    for (const value of [
      `${readerOrigin}/epub?bookUrl=x`,
      'https://example.com/a.jpg',
      'data:image/png;base64,AAA',
      '',
    ]) {
      expect(toReaderAssetUrl(value)).toBe(value)
    }
  })

  it('没有查询串时只换前缀', () => {
    expect(toReaderAssetUrl('/reader3/localEpubAsset')).toBe(`${readerOrigin}/epub`)
  })
})

describe('rewriteCssAssetUrls', () => {
  it('改写内联样式里的 url() 引用', () => {
    const css = '.epub-chapter-content .speci{background:url("/reader3/localEpubAsset?bookUrl=b&path=p") no-repeat;}'
    const result = rewriteCssAssetUrls(css)
    expect(result).toBe(
      `.epub-chapter-content .speci{background:url("${readerOrigin}/epub?bookUrl=b&path=p") no-repeat;}`,
    )
    expect(result).not.toContain('/reader3/localEpubAsset')
  })

  it('没有资源引用时原样返回', () => {
    const css = '.epub-chapter-content p{text-indent:2em;}'
    expect(rewriteCssAssetUrls(css)).toBe(css)
  })
})
