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
  const xiaoyuan = page.locator('.detail-modal .source-item').filter({ hasText: '小原文学网' })
  await expect(xiaoyuan).toHaveCount(5)
  await xiaoyuan.nth(3).click()
  await expect(xiaoyuan.nth(3)).toHaveClass(/selected/)
  await expect(page.locator('.detail-modal .chapter-title')).toHaveText('小原第4版 · 第一章')
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
  await expect(rows.filter({ hasText: '光遇聚合26.10.9' })).toHaveCount(10)
  await expect(rows.filter({ hasText: '小原文学网' })).toHaveCount(5)
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
  await expect(rows.filter({ hasText: '光遇聚合26.10.9' })).toHaveCount(9)
  await expect(rows.filter({ hasText: '小原文学网' })).toHaveCount(5)
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
