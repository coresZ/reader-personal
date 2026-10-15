import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useRssStore } from './rss'
import { getRssArticles, getRssSources } from '../api/rss'
import type { RssArticle, RssSource } from '../types'

vi.mock('../api/rss', () => ({
  getRssSources: vi.fn(),
  getRssArticles: vi.fn(),
  getRssContent: vi.fn(),
  saveRssSources: vi.fn(),
  deleteRssSource: vi.fn(),
  deleteRssSources: vi.fn(),
}))

const getRssSourcesMock = vi.mocked(getRssSources)
const getRssArticlesMock = vi.mocked(getRssArticles)

function source(url: string, name: string): RssSource {
  return { sourceUrl: url, sourceName: name, enabled: true }
}

function article(link: string, title: string): RssArticle {
  return { origin: 'o', sort: '默认', title, order: 1, link }
}

describe('rss store singleUrl 分流', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    getRssSourcesMock.mockReset()
    getRssArticlesMock.mockReset()
  })

  it('当前源是单 URL 源时进入网页入口模式', async () => {
    getRssSourcesMock.mockResolvedValueOnce([source('http@js:x', '脚本地址源')])
    getRssArticlesMock.mockResolvedValueOnce({
      first: [article('https://mirror.example.com', '脚本地址源')],
      second: null,
      singleUrl: true,
      urlEvaluated: true,
    })

    const store = useRssStore()
    await store.fetchSources()
    await store.setSource('http@js:x')

    expect(store.singleUrlMode).toBe(true)
    expect(store.singleUrlResolved).toBe(true)
    // 合成条目必须带标记, 供列表按条目判断(而非依赖全局模式)。
    expect(store.articles[0]!.singleUrlEntry).toBe(true)
  })

  it('混合范围(真 feed + 单 URL 源)不把真 feed 连坐成网页入口', async () => {
    // 真实数据里常是这种组合: 单 URL 源 + 真 atom 源。
    getRssSourcesMock.mockResolvedValueOnce([
      source('https://feeds.example.com/releases.atom', '版本'),
      source('http@js:x', '脚本地址源'),
    ])
    getRssArticlesMock.mockImplementation(async (params) => {
      if (params.sourceUrl.includes('releases.atom')) {
        return {
          first: [article('https://feeds.example.com/releases/tag/v1.5.4', 'Reader v1.5.4')],
          second: null,
        }
      }
      return {
        first: [article('https://mirror.example.com', '脚本地址源')],
        second: null,
        singleUrl: true,
      }
    })

    const store = useRssStore()
    await store.fetchSources()
    await store.setAllSources()

    // 范围里不全是单 URL 源 → 不能整体进入"网页入口"模式,
    // 否则真 feed 的标题/数量/加载更多都会被隐藏。
    expect(store.singleUrlMode).toBe(false)

    // 但每条仍按自己的来源正确标记。
    const atom = store.articles.find((item) => item.title === 'Reader v1.5.4')
    const entry = store.articles.find((item) => item.title === '脚本地址源')
    expect(atom?.singleUrlEntry).toBeUndefined()
    expect(entry?.singleUrlEntry).toBe(true)
  })

  it('只有单 URL 源的范围才进入网页入口模式', async () => {
    // 两个源都在 legado 分组, 且都是单 URL 源 → 该分组范围内全是网页入口。
    getRssSourcesMock.mockResolvedValueOnce([
      { ...source('http@js:a', '脚本地址源'), sourceGroup: 'legado' },
      { ...source('https://pan.example', '网盘入口'), sourceGroup: 'legado' },
    ])
    getRssArticlesMock.mockResolvedValue({
      first: [article('https://example.test/x', '网页')],
      second: null,
      singleUrl: true,
    })

    const store = useRssStore()
    await store.fetchSources()
    await store.setGroup('legado')

    expect(store.singleUrlMode).toBe(true)
  })

  it('单 URL 源的合成条目不算"还有下一页"', async () => {
    getRssSourcesMock.mockResolvedValueOnce([source('http@js:x', '脚本地址源')])
    getRssArticlesMock.mockResolvedValue({
      first: [article('https://mirror.example.com', '脚本地址源')],
      second: null,
      singleUrl: true,
    })

    const store = useRssStore()
    await store.fetchSources()
    await store.setSource('http@js:x')

    // 单 URL 源恒为 1 条, 不能因此显示"加载更多"。
    expect(store.hasMore).toBe(false)
  })
})
