import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { normalizeKinds, useExploreStore } from './explore'
import { useSourceStore } from './source'
import { getBookSources } from '../api/source'
import { exploreBook, exploreKindLabel, getExploreKinds, runExploreAction } from '../api/explore'
import type { BookSource } from '../types'
import type { ExploreActionOutcome } from '../api/explore'

vi.mock('../api/source', () => ({
  getBookSources: vi.fn(),
}))

vi.mock('../api/explore', async () => {
  const actual = await vi.importActual<typeof import('../api/explore')>('../api/explore')
  return {
    // 保留纯函数（exploreKindType / exploreKindChars / exploreKindLabel），只 mock IPC 调用。
    exploreKindType: actual.exploreKindType,
    exploreKindChars: actual.exploreKindChars,
    exploreKindLabel: actual.exploreKindLabel,
    exploreBook: vi.fn(),
    getExploreKinds: vi.fn(),
    runExploreAction: vi.fn(),
  }
})

const getBookSourcesMock = vi.mocked(getBookSources)
const exploreBookMock = vi.mocked(exploreBook)
const getExploreKindsMock = vi.mocked(getExploreKinds)
const runExploreActionMock = vi.mocked(runExploreAction)

function idleOutcome(overrides: Partial<ExploreActionOutcome> = {}): ExploreActionOutcome {
  return {
    value: '',
    exploreRefresh: false,
    loginRefresh: false,
    login: false,
    intents: [],
    values: {},
    logs: [],
    ...overrides,
  }
}

describe('explore store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    getBookSourcesMock.mockReset()
    exploreBookMock.mockReset()
    getExploreKindsMock.mockReset()
    runExploreActionMock.mockReset()
    exploreBookMock.mockResolvedValue([])
    getExploreKindsMock.mockResolvedValue([])
    runExploreActionMock.mockResolvedValue(idleOutcome())
  })

  it('repairs a stale active source when explore sources are already loaded', async () => {
    const sourceStore = useSourceStore()
    sourceStore.sources = [sourceWithExplore()]
    const store = useExploreStore()
    store.activeSourceUrl = 'https://missing.example'

    await store.init()

    expect(store.activeSourceUrl).toBe('https://m.cuoceng.com')
    expect(store.categories.map((category) => category.title)).toEqual(['书 库', '排 行'])
    expect(store.activeCategoryUrl).toBe('/book/category/catalog.html')
  })

  // 纯文本/JSON 型规则前端自己解析即可，不该为它多跑一次 IPC。
  it('parses plain-text rules locally without calling the backend', async () => {
    const sourceStore = useSourceStore()
    sourceStore.sources = [sourceWithExplore()]
    const store = useExploreStore()

    await store.init()

    expect(getExploreKindsMock).not.toHaveBeenCalled()
    expect(store.activeCategoryUrl).toBe('/book/category/catalog.html')
  })

  it('asks the backend for js explore rules and uses its categories', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '线路', url: '' },
      { title: '小说', url: 'https://api.example.com/list?page={{page}}' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()

    await store.init()

    expect(getExploreKindsMock).toHaveBeenCalledWith({
      bookSourceUrl: 'https://api.example.com',
      infoMap: {},
    })
    expect(store.categories.map((category) => category.title)).toEqual(['小说'])
    expect(store.activeCategoryUrl).toBe('https://api.example.com/list?page={{page}}')
  })

  it('reports an error instead of a blank page when the js rule fails', async () => {
    getExploreKindsMock.mockRejectedValue(new Error('书源解析失败: boom'))
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()

    await store.init()

    expect(store.categories).toEqual([])
    expect(store.categoriesError).toContain('boom')
    expect(store.activeCategoryUrl).toBe('')
  })

  // 刻意不做长期缓存：发现页 JS 依赖源变量（线路/发现页来源），前端拿不到，
  // 缓存无法正确失效，宁可每次重新求值。这里锁住该语义，防止后人加回缓存。
  it('re-evaluates js categories on every load instead of caching them', async () => {
    getExploreKindsMock.mockResolvedValue([{ title: '旧分类', url: '/old' }])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()

    await store.init()
    expect(store.categories.map((c) => c.title)).toEqual(['旧分类'])

    getExploreKindsMock.mockResolvedValue([{ title: '新分类', url: '/new' }])
    await store.setSource('https://api.example.com')

    expect(store.categories.map((c) => c.title)).toEqual(['新分类'])
    expect(store.activeCategoryUrl).toBe('/new')
    expect(getExploreKindsMock).toHaveBeenCalledTimes(2)
  })
})

// ── 交互控件（本轮重构的核心） ──

describe('explore controls', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    getBookSourcesMock.mockReset()
    exploreBookMock.mockReset()
    getExploreKindsMock.mockReset()
    runExploreActionMock.mockReset()
    exploreBookMock.mockResolvedValue([])
    runExploreActionMock.mockResolvedValue(idleOutcome())
  })

  it('separates controls, categories and section headers', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '源站', type: 'select', chars: ['番茄', '七猫'], default: '番茄', url: '' },
      { title: '📖模式', type: 'toggle', chars: ['小说', '听书'], default: '小说', url: '' },
      { title: '排行榜', type: 'url', url: '' },
      { title: '总榜', type: 'url', url: '/rank' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()

    await store.init()

    expect(store.controls.map((c) => c.title)).toEqual(['源站', '📖模式'])
    expect(store.sections.map((c) => c.title)).toEqual(['排行榜'])
    expect(store.categories.map((c) => c.title)).toEqual(['总榜'])
  })

  // 书源的 exploreUrl 会把默认值写进 infoMap，后续 action 依赖它们。
  it('applies control defaults into infoMap', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '源站', type: 'select', chars: ['番茄', '七猫'], default: '番茄', url: '' },
      { title: '频道', type: 'select', chars: ['男频', '女频'], url: '' },
      { title: '总榜', type: 'url', url: '/rank' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()

    await store.init()

    expect(store.infoMap['源站']).toBe('番茄')
    // 无 default 时取首个选项
    expect(store.infoMap['频道']).toBe('男频')
    // 初次求值不该把默认值回传（避免覆盖书源自身逻辑）
    expect(getExploreKindsMock).toHaveBeenCalledWith({
      bookSourceUrl: 'https://api.example.com',
      infoMap: {},
    })
  })

  it('sends the selected value and existing infoMap when running an action', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '源站', type: 'select', chars: ['番茄', '七猫'], default: '番茄', action: 'onPick()', url: '' },
      { title: '总榜', type: 'url', url: '/rank' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()
    await store.init()

    const ctrl = store.controls[0]!
    await store.selectOption(ctrl, '七猫')

    expect(store.infoMap['源站']).toBe('七猫')
    expect(runExploreActionMock).toHaveBeenCalledWith({
      bookSourceUrl: 'https://api.example.com',
      action: 'onPick()',
      infoMap: { 源站: '七猫' },
    })
  })

  // 没配 action 的下拉框只写 infoMap，但必须重算分类 —— 否则用户改了下拉
  // 什么都不发生（exploreUrl 才是按取值生成分类的地方）。
  it('reloads categories when a control has no action', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '源站', type: 'select', chars: ['番茄', '七猫'], default: '番茄', url: '' },
      { title: '总榜', type: 'url', url: '/rank' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()
    await store.init()

    getExploreKindsMock.mockResolvedValue([{ title: '七猫榜', type: 'url', url: '/qm' }])
    await store.selectOption(store.controls[0]!, '七猫')

    expect(runExploreActionMock).not.toHaveBeenCalled()
    // 第二次求值要带上用户选中的值
    expect(getExploreKindsMock).toHaveBeenLastCalledWith({
      bookSourceUrl: 'https://api.example.com',
      infoMap: { 源站: '七猫' },
    })
    expect(store.categories.map((c) => c.title)).toEqual(['七猫榜'])
  })

  // java.refreshExplore() → 重新拉分类（带当前 infoMap）
  it('refreshes categories when the action reports exploreRefresh', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '总榜', type: 'url', url: '/rank' },
      { title: '🔄刷新', type: 'button', action: 'java.refreshExplore()' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()
    await store.init()

    runExploreActionMock.mockResolvedValue(idleOutcome({ exploreRefresh: true }))
    getExploreKindsMock.mockResolvedValue([{ title: '新榜', type: 'url', url: '/new' }])

    await store.pressButton(store.controls[0]!)

    expect(getExploreKindsMock).toHaveBeenCalledTimes(2)
    expect(store.categories.map((c) => c.title)).toEqual(['新榜'])
    expect(store.activeCategoryUrl).toBe('/new')
  })

  it('cycles toggle options through chars', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '男频', type: 'toggle', chars: ['男频', '女频'], default: '男频', action: 'onToggle()' },
      { title: '总榜', type: 'url', url: '/rank' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()
    await store.init()

    const ctrl = store.controls[0]!
    expect(store.valueOf(ctrl)).toBe('男频')
    await store.toggleOption(ctrl)
    expect(store.infoMap['男频']).toBe('女频')
    await store.toggleOption(ctrl)
    expect(store.infoMap['男频']).toBe('男频')
  })

  // 书源的搜索控件与桌面端「书架搜索」是两套不相通的入口，用户明确反馈
  // "用不上而且有点bug"，因此不再渲染。判定依据是 action 语义而非标题文字。
  it('hides source-provided search controls', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '☃关键词', type: 'text', url: '' },
      { title: '🔍搜索', type: 'button', action: "java.searchBook(infoMap['☃关键词'] || '')" },
      { title: '搜索', type: 'button', action: 'exploreSearch()' },
      // 光遇的输入框：action 只把值写进"搜索关键词"变量，供搜索按钮读取。
      // 按钮已被滤掉，输入框必须一起滤掉，否则"输入了没处用"。
      { title: '搜索关键词', type: 'text', action: "setVariable('搜索关键词',(infoMap['搜索关键词'] || ''))" },
      { title: '频道', type: 'select', chars: ['男频', '女频'], default: '男频', url: '' },
      { title: '总榜', type: 'url', url: '/rank' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()
    await store.init()

    // 只留下真正的筛选控件；搜索输入框与两个搜索按钮都不渲染
    expect(store.controls.map((c) => c.title)).toEqual(['频道'])
    expect(store.categories.map((c) => c.title)).toEqual(['总榜'])
  })

  // 书源自带的登录控件只发一个无效果的意图；发现页改为提供统一的登录按钮，
  // 因此把它滤掉，避免同一功能两个入口、其中一个没反应。
  it('hides source-provided login controls', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '⚙登录', type: 'button', action: "java.open('login','http',null)" },
      { title: '频道', type: 'select', chars: ['男频'], default: '男频', url: '' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()
    await store.init()

    expect(store.controls.map((c) => c.title)).toEqual(['频道'])
  })

  // 非搜索/登录的按钮（如「🔄刷新」）必须保留 —— 过滤不能误伤。
  it('keeps unrelated buttons', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '🔄刷新', type: 'button', action: 'java.refreshExplore()' },
      { title: '更新书源', type: 'button', action: 'renderVersionPage()' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()
    await store.init()

    expect(store.controls.map((c) => c.title)).toEqual(['🔄刷新', '更新书源'])
  })

  // 登录意图（java.open('login') / java.reLoginView）统一打开登录面板。
  it('opens the login panel when an action asks to log in', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '重登', type: 'button', action: 'java.reLoginView()' },
      { title: '总榜', type: 'url', url: '/rank' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()
    await store.init()

    expect(store.showLogin).toBe(false)
    runExploreActionMock.mockResolvedValue(idleOutcome({ loginRefresh: true }))
    await store.pressButton(store.controls[0]!)

    expect(store.showLogin).toBe(true)
  })

  // 「更新书源」「书源设置」的全部效果就是弹一个 HTML 面板。
  // 后端早先把 showBrowser 丢掉了，前端因此点了没反应。
  it('exposes the panel produced by the action', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '更新书源', type: 'button', action: 'renderVersionPage()' },
      { title: '总榜', type: 'url', url: '/rank' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()
    await store.init()

    runExploreActionMock.mockResolvedValue(
      idleOutcome({
        showBrowser: { url: '', panelId: 'panel-1' } as never,
      }),
    )
    await store.pressButton(store.controls[0]!)

    expect(store.pendingBrowser).toMatchObject({ panelId: 'panel-1' })
    store.closePendingBrowser()
    expect(store.pendingBrowser).toBeNull()
  })

  // java.toast 的文案要能传出来（"已是最新配置"这类反馈全靠它）。
  it('exposes toast messages produced by the action', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '更新配置', type: 'button', action: 'getCloudSettings(true)' },
      { title: '总榜', type: 'url', url: '/rank' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()
    await store.init()

    runExploreActionMock.mockResolvedValue(
      idleOutcome({ messages: ['正在更新最新配置', '已是最新配置：20261010'] }),
    )
    await store.pressButton(store.controls[0]!)

    expect(store.takePendingMessages()).toEqual(['正在更新最新配置', '已是最新配置：20261010'])
    expect(store.pendingMessages).toEqual([])
  })


  // 分组标题必须留在分类序列里（七猫 7 项里 6 个是分组、书山 229 项里 14 个），
  // 只渲染可点分类会把它们压平，用户看不出分类归属。
  it('keeps section headers in the category render list, in backend order', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '🏆 排行榜', type: 'url', url: '' },
      { title: '点击榜', type: 'url', url: '/rank/allvisit' },
      { title: '📚 分类', type: 'url', url: '' },
      { title: '科幻', type: 'url', url: '/c/41' },
      { title: '源站', type: 'select', chars: ['番茄'], url: '' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()
    await store.init()

    expect(store.categoryItems.map((c) => c.title)).toEqual([
      '🏆 排行榜',
      '点击榜',
      '📚 分类',
      '科幻',
    ])
    // 可点分类与分组各自成集
    expect(store.categories.map((c) => c.title)).toEqual(['点击榜', '科幻'])
    expect(store.sections.map((c) => c.title)).toEqual(['🏆 排行榜', '📚 分类'])
  })

  it('surfaces action failures without breaking the page', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '🔄刷新', type: 'button', action: 'boom()' },
      { title: '总榜', type: 'url', url: '/rank' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource()]
    const store = useExploreStore()
    await store.init()

    runExploreActionMock.mockRejectedValue(new Error('书源动作执行失败: boom'))
    await store.pressButton(store.controls[0]!)

    expect(store.categoriesError).toContain('boom')
    // 列表仍可用
    expect(store.categories.map((c) => c.title)).toEqual(['总榜'])
  })

  // 切源必须清掉 A 源的筛选条件，否则会把 A 的 infoMap 带进 B 源求值。
  it('clears infoMap when switching sources', async () => {
    getExploreKindsMock.mockResolvedValue([
      { title: '源站', type: 'select', chars: ['番茄', '七猫'], default: '番茄', url: '' },
      { title: '总榜', type: 'url', url: '/rank' },
    ])
    const sourceStore = useSourceStore()
    sourceStore.sources = [jsExploreSource(), otherJsSource()]
    const store = useExploreStore()
    await store.init()
    expect(store.infoMap['源站']).toBe('番茄')

    getExploreKindsMock.mockResolvedValue([{ title: '另一源', type: 'url', url: '/x' }])
    await store.setSource('https://other.example')

    expect(store.infoMap['源站']).toBeUndefined()
    expect(store.activeCategoryUrl).toBe('/x')
  })

})
describe('exploreKindLabel', () => {
  it('prefers the backend-resolved viewName over title', () => {
    // 光遇聚合把筛选控件显示名写成 `'请选择类型'`
    expect(exploreKindLabel({ title: '平台', viewName: '请选择类型' })).toBe('请选择类型')
  })

  it('falls back to title when viewName is missing or blank', () => {
    expect(exploreKindLabel({ title: '平台' })).toBe('平台')
    expect(exploreKindLabel({ title: '平台', viewName: null })).toBe('平台')
    expect(exploreKindLabel({ title: '平台', viewName: '   ' })).toBe('平台')
  })
})

describe('normalizeKinds', () => {
  it('keeps interactive fields and drops blank titles', () => {
    expect(normalizeKinds([
      { title: '源站', type: 'select', chars: ['番茄'], default: '番茄', action: 'a()', url: '' },
      { title: ' 小说 ', url: ' https://a.example/list ' },
      { title: '', url: '/ignored' },
    ])).toEqual([
      {
        title: '源站',
        url: '',
        type: 'select',
        chars: ['番茄'],
        default: '番茄',
        action: 'a()',
        viewName: null,
        style: undefined,
      },
      {
        title: '小说',
        url: 'https://a.example/list',
        type: null,
        chars: null,
        default: null,
        action: null,
        viewName: null,
        style: undefined,
      },
    ])
  })

  it('tolerates non-array payloads', () => {
    expect(normalizeKinds(null)).toEqual([])
    expect(normalizeKinds(undefined)).toEqual([])
    expect(normalizeKinds({ title: 'not-an-array' })).toEqual([])
  })
})

function sourceWithExplore(): BookSource {
  return {
    bookSourceName: 'm.cuoceng.com',
    bookSourceUrl: 'https://m.cuoceng.com',
    enabledExplore: true,
    exploreUrl: JSON.stringify([
      { style: { layout_flexBasisPercent: 1.0 }, title: '书 库', url: '/book/category/catalog.html' },
      { style: { layout_flexBasisPercent: 0.25 }, title: '排 行', url: '/book/ranking.html' },
    ]),
  }
}

function jsExploreSource(): BookSource {
  return {
    bookSourceName: '七猫聚合',
    bookSourceUrl: 'https://api.example.com',
    enabledExplore: true,
    exploreUrl: '@js:QMA_DISC.explore.call(this)',
  }
}

function otherJsSource(): BookSource {
  return {
    bookSourceName: '书山聚合',
    bookSourceUrl: 'https://other.example',
    enabledExplore: true,
    exploreUrl: '<js>\nsomeRule()\n</js>',
  }
}
