import { test, expect, type Page } from '@playwright/test'

const fixture = '/tests/fixtures/source-comments.html'
const images = '.chapter-text img[data-source-click]'

async function waitForReader(page: Page) {
  await expect(page.locator(images)).toHaveCount(3)
  await expect.poll(() => page.evaluate(() => (window as any).__commentsFixture.calls
    .some((call: any) => call.command === 'get_replace_rules'))).toBe(true)
  await expect.poll(() => page.locator(images).evaluateAll((items) => items.every((item) => {
    const image = item as HTMLImageElement
    return image.complete && image.naturalWidth > 0
  }))).toBe(true)
}

async function pageBodyText(page: Page) {
  return page.locator('.horizontal-page-content').evaluateAll((pages) => pages.map((page) => {
    const copy = page.cloneNode(true) as HTMLElement
    copy.querySelectorAll('.horizontal-flow-title').forEach((title) => title.remove())
    return copy.textContent || ''
  }).join(''))
}

test('raw paragraph, god-comment and chapter-review images keep their URLs and execute their clicks', async ({ page }) => {
  await page.goto(fixture)
  await waitForReader(page)
  const expected = await page.evaluate(() => (window as any).__commentsFixture)
  const rendered = await page.locator(images).evaluateAll((items) => items.map((item) => {
    const image = item as HTMLImageElement
    return { url: image.getAttribute('src'), raw: image.dataset.sourceRawSrc, click: image.dataset.sourceClick }
  }))
  expect(rendered).toEqual(['paragraph', 'god', 'chapter'].map((kind) => ({
    url: expected.imageUrls[kind], raw: expected.rawSources[kind], click: expected.scripts[kind],
  })))
  await expect(page.locator(images).nth(0)).toHaveClass(/source-inline-bubble/)
  for (let index = 0; index < 3; index++) await page.locator(images).nth(index).click()
  const calls = await page.evaluate(() => (window as any).__commentsFixture.calls.filter((call: any) => call.command === 'eval_source_click'))
  expect(calls.map((call: any) => call.req)).toEqual(['paragraph', 'god', 'chapter'].map((kind) => ({
    bookSourceUrl: expected.sourceUrl,
    bookUrl: expected.bookUrl,
    chapterUrl: expected.chapterUrl,
    result: expected.rawSources[kind],
    script: expected.scripts[kind],
  })))
})

test('short horizontal content includes the top-level chapter review once', async ({ page }) => {
  await page.setViewportSize({ width: 1100, height: 650 })
  await page.goto(fixture + '?horizontal=1')
  await waitForReader(page)
  const expected = await page.evaluate(() => (window as any).__commentsFixture)
  expect(await pageBodyText(page)).toBe(expected.expectedText)
  expect(await page.locator(images).evaluateAll((items) => items.map((item) => (item as HTMLElement).dataset.sourceClick)))
    .toEqual([expected.scripts.paragraph, expected.scripts.god, expected.scripts.chapter])
  await page.locator(images).last().click()
  await expect.poll(() => page.evaluate(() => (window as any).__commentsFixture.calls
    .filter((call: any) => call.command === 'eval_source_click').length)).toBe(1)
})

test('long paragraphs retain their final bubble, text and inline markup across measured pages', async ({ page }) => {
  await page.setViewportSize({ width: 1100, height: 650 })
  await page.goto(fixture + '?horizontal=1&long=1')
  await waitForReader(page)
  await expect.poll(() => page.locator('.horizontal-page-content').count()).toBeGreaterThan(2)
  const expected = await page.evaluate(() => (window as any).__commentsFixture)
  expect(await pageBodyText(page)).toBe(expected.expectedText)
  const scripts = await page.locator(images).evaluateAll((items) => items.map((item) => (item as HTMLElement).dataset.sourceClick))
  expect(scripts).toEqual([expected.scripts.paragraph, expected.scripts.god, expected.scripts.chapter])
  await expect(page.locator('.horizontal-page-content em').first()).toBeAttached()
  await expect(page.locator('.horizontal-page-content strong').first()).toBeAttached()
  const overflowing = await page.locator('.horizontal-page-content').evaluateAll((pages) => pages
    .map((page, index) => ({ index, excess: page.scrollHeight - page.clientHeight }))
    .filter((page) => page.excess > 2))
  expect(overflowing).toEqual([])
  await page.locator(images).first().dispatchEvent('click')
  const calls = await page.evaluate(() => (window as any).__commentsFixture.calls.filter((call: any) => call.command === 'eval_source_click'))
  expect(calls).toHaveLength(1)
  expect(calls[0].req).toMatchObject({ script: expected.scripts.paragraph, result: expected.rawSources.paragraph })
})

test('every DOM split boundary preserves media exactly once and keeps Unicode text intact', async ({ page }) => {
  await page.goto(fixture)
  await waitForReader(page)
  const outcomes = await page.evaluate(async () => {
    const { parseReaderParagraph } = await import('/src/utils/readerParagraphRange.ts')
    const html = '<p id="paragraph"><em>前🙂<img data-image="middle" data-source-click="middle()">后</em><br><strong>尾声。</strong><img data-image="last" data-source-click="last()"></p>'
    const original = document.createElement('div')
    original.innerHTML = html
    const parsed = parseReaderParagraph(html)!
    return Array.from({ length: parsed.tokens.length - 1 }, (_, index) => {
      const cut = index + 1
      const split = document.createElement('div')
      split.innerHTML = parsed.slice(0, cut, '', '') + parsed.slice(cut, parsed.tokens.length, '', '')
      return {
        cut,
        text: split.textContent,
        expected: original.textContent,
        images: Array.from(split.querySelectorAll('img')).map((image) => image.dataset.image),
        clicks: Array.from(split.querySelectorAll('img')).map((image) => image.dataset.sourceClick),
        breaks: split.querySelectorAll('br').length,
      }
    })
  })
  for (const outcome of outcomes) {
    expect(outcome.text, 'split at ' + outcome.cut).toBe(outcome.expected)
    expect(outcome.images).toEqual(['middle', 'last'])
    expect(outcome.clicks).toEqual(['middle()', 'last()'])
    expect(outcome.breaks).toBe(1)
  }
})

test('late chapter-review images repaginate without clipping, losing position or repeating load rebuilds', async ({ page }) => {
  await page.setViewportSize({ width: 1100, height: 650 })
  let releaseImage!: () => void
  const imageReady = new Promise<void>((resolve) => { releaseImage = resolve })
  await page.route('https://comments.example.test/chapter-review.svg', async (route) => {
    await imageReady
    await route.fulfill({
      contentType: 'image/svg+xml',
      body: '<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="1500"><rect width="1000" height="1500" fill="#ddd"/></svg>',
    })
  })
  await page.goto(fixture + '?horizontal=1&long=1&remote=1', { waitUntil: 'domcontentloaded' })
  await expect(page.locator(images)).toHaveCount(3)
  await expect.poll(() => page.locator('.horizontal-page-content').count()).toBeGreaterThan(2)
  await expect.poll(() => page.evaluate(() => (window as any).__commentsFixture.calls
    .some((call: any) => call.command === 'get_replace_rules'))).toBe(true)
  await page.keyboard.press('PageDown')
  const pageOffset = () => page.locator('.horizontal-pages').evaluate((element) =>
    parseFloat((element as HTMLElement).style.transform.match(/translate3d\(([-\d.]+)/)?.[1] || '0'))
  await expect.poll(pageOffset).toBeLessThan(0)
  const before = await pageOffset()
  const pageCountBefore = await page.locator('.horizontal-page-content').count()
  const overflowingPages = () => page.locator('.horizontal-page-content').evaluateAll((pages) => pages
    .map((element, index) => ({ index, excess: element.scrollHeight - element.clientHeight }))
    .filter((element) => element.excess > 2))
  expect(await overflowingPages()).toEqual([])
  releaseImage()
  await waitForReader(page)
  await expect.poll(() => page.locator('.horizontal-page-content').count()).toBeGreaterThan(pageCountBefore)
  await expect.poll(overflowingPages).toEqual([])
  expect(await pageOffset()).toBe(before)
  const expected = await page.evaluate(() => (window as any).__commentsFixture)
  expect(await pageBodyText(page)).toBe(expected.expectedText)
  await expect(page.locator(images)).toHaveCount(3)
  await page.evaluate(() => {
    ;(window as any).__imageRebuildMutations = 0
    new MutationObserver(() => { (window as any).__imageRebuildMutations++ })
      .observe(document.querySelector('.horizontal-pages')!, { childList: true, subtree: true })
    document.querySelectorAll('.chapter-text img').forEach((image) => image.dispatchEvent(new Event('load')))
  })
  await page.waitForTimeout(400)
  expect(await page.evaluate(() => (window as any).__imageRebuildMutations)).toBe(0)
})
