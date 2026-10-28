import { describe, expect, it } from 'vitest'
import { parseImportLink, describeImportLinkFailure } from './sourceImportLink'

describe('parseImportLink', () => {
  it('解析 yuedu:// 一键导入链接', () => {
    const link = parseImportLink(
      'yuedu://booksource/importonline?src=https%3A%2F%2Frepo.example.com%2Fapi%2Fsource%2F1.json',
    )

    expect(link).not.toBeNull()
    expect(link!.protocol).toBe('yuedu:')
    expect(link!.sourceUrl).toBe('https://repo.example.com/api/source/1.json')
  })

  it('解析 legado:// 导入链接', () => {
    const link = parseImportLink(
      'legado://import/booksource?src=https://example.com/sources.json',
    )

    expect(link!.protocol).toBe('legado:')
    expect(link!.sourceUrl).toBe('https://example.com/sources.json')
  })

  it('兼容用 url 参数代替 src 的写法', () => {
    const link = parseImportLink('legado://import/auto?url=https://example.com/a.json')
    expect(link!.sourceUrl).toBe('https://example.com/a.json')
  })

  it('非导入协议返回 null', () => {
    expect(parseImportLink('https://example.com')).toBeNull()
    expect(parseImportLink('mailto:a@b.c')).toBeNull()
    expect(parseImportLink('')).toBeNull()
    expect(parseImportLink(null)).toBeNull()
    expect(parseImportLink(undefined)).toBeNull()
    expect(parseImportLink('这不是链接')).toBeNull()
  })

  it('缺少 src 时返回 null sourceUrl', () => {
    const link = parseImportLink('yuedu://booksource/importonline')
    expect(link).not.toBeNull()
    expect(link!.sourceUrl).toBeNull()
    expect(describeImportLinkFailure(link!)).not.toBe('')
  })

  it('拒绝非 http(s) 的目标, 避免读取本地文件', () => {
    expect(parseImportLink('legado://import/booksource?src=file:///C:/secret.json')!.sourceUrl).toBeNull()
    expect(parseImportLink('legado://import/booksource?src=javascript:alert(1)')!.sourceUrl).toBeNull()
  })
})
