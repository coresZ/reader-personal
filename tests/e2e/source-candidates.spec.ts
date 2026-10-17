import { test, expect } from '@playwright/test'

const fixture = '/tests/fixtures/source-candidates.html'
const aggregateOrigin = 'https://aggregate.example.invalid'
const maoyanUrl = aggregateOrigin + '/book/1?site=maoyan'
const baiduUrl = aggregateOrigin + '/book/1?site=baidu'

test('search and detail retain all links returned by each source', async ({ page }) => {
  await page.goto(fixture + '?mode=search')
  await expect(page.locator('.book-card')).toHaveCount(2)
  const merged = await page.evaluate(() => (window as any).__candidateHarness.searchResults())
  const authored = merged.find((book: any) => book.author === '天蚕土豆')
  expect(authored.sourceCandidates.map((book: any) => book.bookUrl)).toEqual([
    'https://deqi.example.invalid/book/1', maoyanUrl, baiduUrl,
  ])
  expect(merged.find((book: any) => !book.author).sourceCandidates).toHaveLength(5)

  await page.locator('.book-card').filter({ hasText: '天蚕土豆' }).click()
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(8)
  const xiaoyuan = page.locator('.detail-modal .source-item').filter({ hasText: '书源二' })
  await expect(xiaoyuan).toHaveCount(5)
  await xiaoyuan.nth(3).click()
  await expect(xiaoyuan.nth(3)).toHaveClass(/selected/)
  await expect(page.locator('.detail-modal .chapter-title')).toHaveText('第4版 · 第一章')
})

test('detail can read another link from the currently selected aggregate source', async ({ page }) => {
  await page.goto(fixture + '?current=aggregate')
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(8)
  const baidu = page.locator('.detail-modal .source-item').filter({ hasText: '百度' })
  await baidu.click()
  await expect(baidu).toHaveClass(/selected/)
  await expect(page.locator('.detail-modal .chapter-title')).toHaveText('百度 · 第一章')
  await page.getByRole('button', { name: '开始阅读', exact: true }).click()
  await expect.poll(() => page.evaluate(() => (window as any).__candidateHarness.currentBook()?.bookUrl)).toBe(baiduUrl)
  const calls = await page.evaluate(() => (window as any).__candidateHarness.calls)
  expect(calls.find((call: any) => call.command === 'set_book_source').req).toMatchObject({
    bookUrl: maoyanUrl, newUrl: baiduUrl, bookSourceUrl: aggregateOrigin,
  })
  await expect.poll(() => page.evaluate(() => (window as any).__candidateHarness.calls
    .filter((call: any) => call.command === 'get_book_content')
    .map((call: any) => call.req.bookUrl))).toContain(baiduUrl)
})

test('detail clears stale chapters and identifies the source while the new catalog loads', async ({ page }) => {
  await page.goto(fixture + '?delay-chapters=1')
  await expect(page.locator('.detail-modal .chapter-title')).toHaveText('最新章节 · 第一章')
  await expect(page.locator('.detail-modal .chapter-source-status')).toHaveText('书源一')

  await page.locator('.detail-modal .source-item').filter({ hasText: '书源二' }).first().click()
  await expect(page.locator('.detail-modal .chapter-heading')).toContainText('正在加载目录')
  await expect(page.locator('.detail-modal .chapter-source-status')).toHaveText('书源二')
  await expect(page.locator('.detail-modal .chapter-section')).toHaveCount(0)
  await expect(page.locator('.detail-modal .chapter-loading')).toContainText('正在获取章节列表')

  await page.evaluate(() => (window as any).__candidateHarness.completeChapterPending())
  await expect(page.locator('.detail-modal .chapter-title')).toHaveText('第1版 · 第一章')
  await expect(page.locator('.detail-modal .chapter-source-status')).toHaveText('书源二')
  await expect(page.locator('.chapter-source-status')).not.toContainText('正在加载')
})

test('a source picked by the user survives later candidate batches', async ({ page }) => {
  // 分两页到达候选: 用户在第一页选源, 第二页到达时会重新合并候选列表。
  await page.goto(fixture + '?pages=2&hold')
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(6)
  const xiaoyuan = page.locator('.detail-modal .source-item').filter({ hasText: '书源二' })
  await xiaoyuan.nth(2).click()
  await expect(xiaoyuan.nth(2)).toHaveClass(/selected/)
  await expect(page.locator('.detail-modal .chapter-title')).toHaveText('第3版 · 第一章')

  // 合并新候选不能把选中项拨回本书原源: 否则目录仍是手选源的, 选中项却是原源,
  // 读正文时会用错误书源的章节号。选中态与已加载目录必须始终同源。
  await page.evaluate(() => (window as any).__candidateHarness.completePending())
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(8)
  await expect(xiaoyuan.nth(2)).toHaveClass(/selected/)
  await expect(page.locator('.detail-modal .chapter-title')).toHaveText('第3版 · 第一章')

  await page.getByRole('button', { name: '开始阅读', exact: true }).click()
  await expect.poll(() => page.evaluate(() => (window as any).__candidateHarness.currentBook()?.bookUrl))
    .toBe('https://xiaoyuan.example.invalid/book/3')
})

test('candidate sources load while the initial slow catalog is pending', async ({ page }) => {
  await page.goto(fixture + '?delay-initial-chapters=1')
  await expect(page.locator('.detail-modal .chapter-heading')).toContainText('正在加载目录')
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(8)
  await expect(page.locator('.detail-modal .source-item').filter({ hasText: '书源一' })).toHaveCount(1)

  await page.evaluate(() => (window as any).__candidateHarness.completeChapterPending())
  await expect(page.locator('.detail-modal .chapter-title')).toHaveText('最新章节 · 第一章')
})

test('reader keeps and switches to another link from its current source', async ({ page }) => {
  await page.goto(fixture + '?mode=reader')
  await expect(page.locator('.reader-source .source-item:not(.current)')).toHaveCount(7)
  const baidu = page.locator('.reader-source .source-item:not(.current)').filter({ hasText: '最新: 百度' })
  await baidu.click()
  await expect(baidu).toHaveClass(/selected/)
  await expect(page.locator('.compare-card.highlight')).toContainText('最新章节：百度')
  await baidu.getByRole('button', { name: '切换', exact: true }).click()
  await expect(page.getByText('换源完成', { exact: true })).toBeVisible()
  const calls = await page.evaluate(() => (window as any).__candidateHarness.calls)
  expect(calls.find((call: any) => call.command === 'set_book_source').req).toMatchObject({
    bookUrl: maoyanUrl, newUrl: baiduUrl, bookSourceUrl: aggregateOrigin,
  })
  expect(calls.some((call: any) => call.command === 'get_book_content' && call.req.bookUrl === baiduUrl)).toBe(true)
})

test('detail continues from the backend cursor and caches only the complete result', async ({ page }) => {
  await page.goto(fixture + '?pages=2')
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(8)
  await expect(page.getByText('继续查找其他书源...', { exact: true })).toHaveCount(0)
  const calls = await page.evaluate(() => (window as any).__candidateHarness.calls
    .filter((call: any) => call.command === 'get_available_book_source_sse'))
  expect(calls.map((call: any) => call.req.lastIndex)).toEqual([-1, 1])
  expect(calls.every((call: any) => call.req.resultLimit === undefined)).toBe(true)

  await page.locator('.detail-modal .modal-close').click()
  await page.getByRole('button', { name: '打开详情', exact: true }).click()
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(8)
  expect(await page.evaluate(() => (window as any).__candidateHarness.calls
    .filter((call: any) => call.command === 'get_available_book_source_sse').length)).toBe(2)
})

test('closing detail cancels an unfinished continuation and ignores its late response', async ({ page }) => {
  await page.goto(fixture + '?pages=2&hold=1')
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(6)
  await expect(page.getByText('继续查找其他书源...', { exact: true })).toBeVisible()
  await expect.poll(() => page.evaluate(() => (window as any).__candidateHarness.calls
    .filter((call: any) => call.command === 'get_available_book_source_sse').length)).toBe(2)
  await page.locator('.detail-modal .modal-close').click()
  await expect(page.locator('.detail-modal')).toHaveCount(0)
  await page.evaluate(() => (window as any).__candidateHarness.completePending())
  const calls = await page.evaluate(() => (window as any).__candidateHarness.calls)
  const searches = calls.filter((call: any) => call.command === 'get_available_book_source_sse')
  expect(searches).toHaveLength(2)
  expect(calls.some((call: any) => call.command === 'cancel_book_search'
    && call.req.taskId === searches[1].req.taskId)).toBe(true)

  await page.getByRole('button', { name: '打开详情', exact: true }).click()
  await expect.poll(() => page.evaluate(() => (window as any).__candidateHarness.calls
    .filter((call: any) => call.command === 'get_available_book_source_sse').length)).toBe(4)
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(6)
})

test('detail keeps all aggregate author spellings and empty-author editions', async ({ page }) => {
  await page.goto(fixture + '?authors=1&current=aggregate')
  const rows = page.locator('.detail-modal .source-item')
  await expect(rows).toHaveCount(17)
  await expect(rows.filter({ hasText: '聚合源' })).toHaveCount(10)
  await expect(rows.filter({ hasText: '书源二' })).toHaveCount(5)
  await expect(rows.filter({ hasText: '衍生作品' })).toHaveCount(0)
  const variant = rows.filter({ hasText: '分站3' })
  await expect(variant.locator('.source-author')).toHaveText('天蚕土豆_qd22')
  await variant.click()
  await expect(page.locator('.detail-modal .chapter-title')).toHaveText('分站3 · 第一章')
  await page.getByRole('button', { name: '开始阅读', exact: true }).click()
  const variantUrl = aggregateOrigin + '/book/1?site=variant-0'
  await expect.poll(() => page.evaluate(() => (window as any).__candidateHarness.currentBook()?.bookUrl)).toBe(variantUrl)
  const calls = await page.evaluate(() => (window as any).__candidateHarness.calls)
  expect(calls.find((call: any) => call.command === 'set_book_source').req).toMatchObject({
    bookUrl: maoyanUrl, newUrl: variantUrl, bookSourceUrl: aggregateOrigin,
  })
})

test('reader keeps author variants and excludes other titles in saved candidates', async ({ page }) => {
  await page.goto(fixture + '?authors=1&mode=reader')
  const rows = page.locator('.reader-source .source-item:not(.current)')
  await expect(rows).toHaveCount(16)
  await expect(rows.filter({ hasText: '聚合源' })).toHaveCount(9)
  await expect(rows.filter({ hasText: '书源二' })).toHaveCount(5)
  await expect(rows.filter({ hasText: '斗破苍穹之无上之境' })).toHaveCount(0)
  const variant = rows.filter({ hasText: '最新: 分站4' })
  await expect(variant.locator('.source-author')).toHaveText('作者： 天蚕土豆著')
  await variant.getByRole('button', { name: '切换', exact: true }).click()
  await expect(page.getByText('换源完成', { exact: true })).toBeVisible()
  const calls = await page.evaluate(() => (window as any).__candidateHarness.calls)
  expect(calls.find((call: any) => call.command === 'set_book_source').req).toMatchObject({
    bookUrl: maoyanUrl,
    newUrl: aggregateOrigin + '/book/1?site=variant-1',
    bookSourceUrl: aggregateOrigin,
  })
})


test('a failed source on an earlier page is not cached and retry fetches a complete list', async ({ page }) => {
  await page.goto(fixture + '?pages=2&errors=1')
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(7)
  await expect(page.getByText('部分书源访问失败，可重试。', { exact: true })).toBeVisible()
  await page.locator('.detail-modal .modal-close').click()
  await page.getByRole('button', { name: '打开详情', exact: true }).click()
  await expect.poll(() => page.evaluate(() => (window as any).__candidateHarness.calls
    .filter((call: any) => call.command === 'get_available_book_source_sse').length)).toBe(4)
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(7)
  await page.getByRole('button', { name: '重试', exact: true }).click()
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(8)
  await expect(page.getByText('部分书源访问失败，可重试。', { exact: true })).toHaveCount(0)
  const requests = await page.evaluate(() => (window as any).__candidateHarness.calls
    .filter((call: any) => call.command === 'get_available_book_source_sse'))
  expect(requests.slice(-2).map((call: any) => call.req.refresh)).toEqual([1, 1])
  await page.locator('.detail-modal .modal-close').click()
  await page.getByRole('button', { name: '打开详情', exact: true }).click()
  await expect(page.locator('.detail-modal .source-item')).toHaveCount(8)
  expect(await page.evaluate(() => (window as any).__candidateHarness.calls
    .filter((call: any) => call.command === 'get_available_book_source_sse').length)).toBe(6)
})

test('reader preserves successful candidates when a source fails and can retry', async ({ page }) => {
  await page.goto(fixture + '?mode=reader&errors=1')
  const rows = page.locator('.reader-source .source-item:not(.current)')
  await expect(rows).toHaveCount(6)
  await expect(page.getByText('部分书源访问失败，可重试。', { exact: true })).toBeVisible()
  await rows.filter({ hasText: '最新: 百度' }).click()
  await expect(page.locator('.compare-card.highlight')).toContainText('最新章节：百度')
  await page.getByRole('button', { name: '重试', exact: true }).click()
  await expect(rows).toHaveCount(7)
  await expect(page.getByText('部分书源访问失败，可重试。', { exact: true })).toHaveCount(0)
  const requests = await page.evaluate(() => (window as any).__candidateHarness.calls
    .filter((call: any) => call.command === 'get_available_book_source_sse'))
  expect(requests.at(-1).req.refresh).toBe(1)
})
