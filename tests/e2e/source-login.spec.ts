import { test, expect } from '@playwright/test'
import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'

test.beforeEach(async ({ page }) => {
  await page.route('http://reader.localhost/**', (route) => route.fulfill({ contentType: 'text/html; charset=utf-8', body: '<p>登录页</p>' }))
})

test('source form sends entered fields and applies script updates', async ({ page }) => {
  await page.goto('/tests/fixtures/source-login.html')
  await page.getByLabel('设备ID', { exact: true }).fill('fixture-device')
  await page.getByLabel('密码', { exact: true }).fill('fixture-value')
  await page.getByRole('button', { name: '设置设备' }).click()
  await expect(page.getByLabel('设备ID', { exact: true })).toHaveValue('已配置')
  await expect(page.getByText('设置完成', { exact: true })).toBeVisible()
  const actions = await page.evaluate(() => (window as any).__loginCalls.filter((call: any) => call.req.action))
  expect(actions[0].req).toMatchObject({ action: 'button:2', values: { 设备ID: 'fixture-device', 密码: 'fixture-value' } })
  await page.getByRole('button', { name: '确认登录' }).click()
  const submit = await page.evaluate(() => (window as any).__loginCalls.find((call: any) => call.req.action === 'login'))
  expect(submit.req.values.设备ID).toBe('已配置')
})

test('long login URLs leave cookie input and footer in the laptop viewport', async ({ page }) => {
  await page.setViewportSize({ width: 1366, height: 768 })
  await page.goto('/tests/fixtures/source-login.html?mode=long')
  const cookie = page.getByRole('textbox', { name: '手动导入 Cookie', exact: true })
  await expect(cookie).toBeInViewport()
  await cookie.fill('session=fixture')
  await expect(page.getByRole('button', { name: '导入 Cookie', exact: true })).toBeInViewport()
  await page.getByRole('button', { name: '导入 Cookie', exact: true }).click()
  await expect(page.getByText('Cookie 已保存', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: '关闭', exact: true })).toBeInViewport()
})

test('cookie input remains usable when loading the login configuration fails', async ({ page }) => {
  await page.goto('/tests/fixtures/source-login.html?mode=failure')
  await expect(page.getByRole('alert')).toContainText('登录配置加载失败')
  const cookie = page.getByRole('textbox', { name: '手动导入 Cookie', exact: true })
  await expect(cookie).toBeEditable()
  await cookie.fill('session=fixture')
  await page.getByRole('button', { name: '导入 Cookie', exact: true }).click()
  await expect(page.getByText('Cookie 已保存', { exact: true })).toBeVisible()
})


test('source flex widths preserve full, half and quarter rows in a long form', async ({ page }) => {
  await page.setViewportSize({ width: 1366, height: 768 })
  await page.goto('/tests/fixtures/source-login.html?mode=layout')
  const full = await page.getByRole('button', { name: '切换线路', exact: true }).boundingBox()
  const halves = await Promise.all(['书源设置', '段评设置'].map((name) => page.getByRole('button', { name, exact: true }).boundingBox()))
  const quarters = await Promise.all(['段评开关', '图片开关', '简介开关', '同步书架'].map((name) => page.getByRole('button', { name, exact: true }).boundingBox()))
  expect(halves[0]!.y).toBe(halves[1]!.y)
  expect(halves[0]!.width).toBeCloseTo((full!.width - 12) / 2, 0)
  expect(new Set(quarters.map((box) => box!.y)).size).toBe(1)
  await expect(page.getByRole('button', { name: '设置说明' })).toHaveCount(0)
  await expect(page.getByText('设置说明', { exact: true })).toBeVisible()
  await page.getByLabel('设置选项 26', { exact: true }).fill('可编辑')
  await expect(page.getByRole('button', { name: '确认登录', exact: true })).toBeInViewport()
})

test.describe('source browser panels', () => {
  const fixturePath = resolve('target/source-panel-fixtures/settings.html')
  test.skip(!existsSync(fixturePath), 'Run cargo test --test source_login_panels first to generate the backend bridge fixture.')

  test.beforeEach(async ({ page }) => {
    const html = readFileSync(fixturePath, 'utf8')
    await page.route('http://reader.localhost/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === '/sourcePanelBridge') {
        const body = route.request().postDataJSON()
        if (body.action === 'complete') {
          await page.evaluate((result) => {
            ;(window as any).__panelSnapshot = result.body
            ;(window as any).__panelResult = result
            ;(window as any).__fixtureCompletePanel()
          }, body.payload)
          await route.fulfill({ json: { ok: true } })
        } else if (body.action === 'showBrowser') {
          await route.fulfill({ json: { ok: true, value: { panelId: 'fixture-nested', url: body.payload.url, options: body.payload.options } } })
        } else {
          await route.fulfill({ json: { ok: true, value: null } })
        }
        return
      }
      const nested = url.searchParams.get('__readerPanelId') === 'fixture-nested'
      const target = url.searchParams.get('__readerPanelUrl') || (nested ? 'https://fixture.test/nested' : 'https://fixture.test/settings')
      const document = html
        .replace(/window\.__readerPanelUrl=[^;]*;/, () => 'window.__readerPanelUrl=' + JSON.stringify(target) + ';')
        .replace(/<base href="[^"]*">/, () => '<base href="' + target.replaceAll('&', '&amp;').replaceAll('"', '&quot;') + '">')
      await route.fulfill({
        contentType: 'text/html; charset=utf-8',
        body: nested ? document.replaceAll('fixture-settings', 'fixture-nested').replace('<title>线路设置</title>', '<title>子面板</title>').replace(/<body>[\s\S]*<\/body>/, '<body><button onclick="window.close()">完成</button></body>') : document,
      })
    })
    await page.goto('/tests/fixtures/source-login.html?mode=panels')
  })

  test('confirm returns live input, textarea and selection values through the backend bridge', async ({ page }) => {
    await page.getByLabel('设备ID', { exact: true }).fill('未提交的输入')
    await page.getByRole('button', { name: '设置设备', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: '线路设置', exact: true })
    const frame = page.frameLocator('.source-browser-frame')
    await expect(dialog.getByRole('button', { name: '确认', exact: true })).toBeEnabled()
    const preload = await page.locator('.source-browser-frame').elementHandle()
    expect(await (await preload!.contentFrame())!.evaluate(() => (window as any).pageSawPreload)).toBe('ready')
    await frame.getByLabel('线路', { exact: true }).fill('新线路')
    await frame.getByLabel('备注', { exact: true }).fill('第一行\n第二行 <自定义>')
    await frame.getByLabel('样式', { exact: true }).selectOption('dark')
    await dialog.getByRole('button', { name: '确认', exact: true }).click()
    await expect(dialog).toHaveCount(0)
    await expect(page.getByText('设置完成', { exact: true })).toBeVisible()
    const snapshot = await page.evaluate(() => (window as any).__panelSnapshot)
    expect(snapshot).toContain('value="新线路"')
    expect(snapshot).toContain('第一行\n第二行 &lt;自定义&gt;')
    expect(snapshot).toContain('value="dark" selected')
    const call = await page.evaluate(() => (window as any).__loginCalls.find((call: any) => call.req.action === 'button:2'))
    expect(call.req.values.设备ID).toBe('未提交的输入')
  })

  test('cancel releases the waiting action and lets the same source reopen', async ({ page }) => {
    await page.getByLabel('设备ID', { exact: true }).fill('保留的输入')
    for (let attempt = 0; attempt < 2; attempt++) {
      await page.getByRole('button', { name: '设置设备', exact: true }).click()
      const dialog = page.getByRole('dialog', { name: '线路设置', exact: true })
      await expect(dialog.getByRole('button', { name: '确认', exact: true })).toBeEnabled()
      await dialog.getByRole('button', { name: '取消', exact: true }).click()
      await expect(dialog).toHaveCount(0)
      await expect(page.getByLabel('设备ID', { exact: true })).toHaveValue('保留的输入')
      await expect(page.getByRole('button', { name: '设置设备', exact: true })).toBeEnabled()
    }
    expect(await page.evaluate(() => (window as any).__panelSnapshot)).toBeUndefined()
  })

  test('comment pages read their full query before startup and fetch the selected chapter', async ({ page }) => {
    const target = 'https://fixture.test/review?book_id=book%2B1&item_id=chapter-2&source=fixture-source&tag=one&tag=two&panelId=upstream-panel&url=upstream-url#paragraph-4'
    const requests: any[] = []
    const document = readFileSync(fixturePath, 'utf8')
      .replace(/window\.__readerPanelUrl=[^;]*;/, () => 'window.__readerPanelUrl=' + JSON.stringify(target) + ';')
      .replace('<body>', `<body><output id="comment-result"></output><script>
        window.commentStartup = Array.from(new URLSearchParams(window.location.search));
        var query = new URLSearchParams(window.location.search);
        fetch('/api/review/comment/chapter/list?book_id=' + encodeURIComponent(query.get('book_id')) + '&item_id=' + encodeURIComponent(query.get('item_id')))
          .then(function(response) { return response.json(); })
          .then(function(result) { document.getElementById('comment-result').textContent = result.comment; });
      </script>`)
    await page.route('http://reader.localhost/**', async (route) => {
      const url = new URL(route.request().url())
      if (url.pathname === '/sourcePanel') {
        await route.fulfill({ contentType: 'text/html; charset=utf-8', body: document })
      } else if (url.pathname === '/sourcePanelBridge' && route.request().postDataJSON().action === 'fetch') {
        requests.push(route.request().postDataJSON().payload)
        await route.fulfill({ json: { ok: true, value: { code: 200, headers: { 'content-type': 'application/json' }, body: JSON.stringify({ comment: '所选章节的段评' }) } } })
      } else {
        await route.fallback()
      }
    })
    await page.getByRole('button', { name: '设置设备', exact: true }).click()
    await expect(page.getByRole('button', { name: '确认', exact: true })).toBeEnabled()
    const frame = page.frames().find((item) => item.url().includes('/sourcePanel?'))!
    await expect(frame.locator('#comment-result')).toHaveText('所选章节的段评')
    const query = new URLSearchParams(await frame.evaluate(() => (window as any).commentStartup))
    for (const [key, value] of new URL(target).searchParams) expect(query.getAll(key)).toContain(value)
    expect(new URL(frame.url()).hash).toBe('#paragraph-4')
    expect(requests).toHaveLength(1)
    expect(requests[0].url).toBe('https://fixture.test/api/review/comment/chapter/list?book_id=book%2B1&item_id=chapter-2')
    await frame.evaluate(() => history.pushState({}, '', '?book_id=next-book&item_id=next-chapter'))
    expect(await frame.evaluate(() => new URLSearchParams(location.search).get('book_id'))).toBe('next-book')
    expect(await frame.evaluate(() => new URLSearchParams(location.search).has('tag'))).toBe(false)
    await page.getByRole('button', { name: '确认', exact: true }).click()
    await expect(page.locator('.source-browser-panel')).toHaveCount(0)
    expect((await page.evaluate(() => (window as any).__panelResult)).url).toBe('https://fixture.test/review?book_id=next-book&item_id=next-chapter')
  })

  test('applies panel configuration, keeps nested state and rejects foreign messages', async ({ page }) => {
    await page.setViewportSize({ width: 1366, height: 768 })
    await page.getByRole('button', { name: '设置设备', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: '线路设置', exact: true })
    await expect(dialog).toBeVisible()
    await expect(dialog).toHaveCSS('border-top-left-radius', '20px')
    expect((await dialog.boundingBox())!.height).toBeCloseTo(768 * .8, 0)
    await page.locator('.source-browser-mask').click({ position: { x: 2, y: 2 } })
    await expect(dialog).toBeVisible()
    await page.evaluate(() => window.dispatchEvent(new MessageEvent('message', { origin: 'http://reader.localhost', source: window, data: { panelId: 'fixture-settings', type: 'close' } })))
    await expect(dialog).toBeVisible()
    const frame = page.frameLocator('.source-browser-frame:visible')
    await frame.getByLabel('线路', { exact: true }).fill('嵌套前的选择')
    await frame.getByRole('button', { name: '调整高度', exact: true }).click()
    await expect.poll(async () => (await dialog.boundingBox())!.height).toBeCloseTo(768 * .6, 0)
    await frame.getByRole('button', { name: '打开子面板', exact: true }).click()
    await expect(page.getByRole('dialog', { name: '子面板', exact: true })).toBeVisible()
    await page.frameLocator('.source-browser-frame:visible').getByRole('button', { name: '完成', exact: true }).click()
    await expect(dialog).toBeVisible()
    await expect(page.frameLocator('.source-browser-frame:visible').getByLabel('线路', { exact: true })).toHaveValue('嵌套前的选择')
    await dialog.getByRole('button', { name: '取消', exact: true }).click()
  })

  test('closing the login dialog cancels its pending browser', async ({ page }) => {
    await page.getByRole('button', { name: '设置设备', exact: true }).click()
    await expect(page.getByRole('dialog', { name: '线路设置', exact: true })).toBeVisible()
    await page.evaluate(() => (window as any).__setLoginVisible(false))
    await expect(page.locator('.source-browser-panel')).toHaveCount(0)
    const closed = await page.evaluate(() => (window as any).__loginCalls.some((call: any) => call.command === 'close_source_panel' && call.panelId === 'fixture-settings'))
    expect(closed).toBe(true)
    await page.evaluate(() => (window as any).__setLoginVisible(true))
    await expect(page.getByRole('button', { name: '设置设备', exact: true })).toBeEnabled()
  })

  test('keeps assign and replace navigation in the same panel and confirms the final URL', async ({ page }) => {
    await page.getByRole('button', { name: '设置设备', exact: true }).click()
    const confirm = page.getByRole('button', { name: '确认', exact: true })
    await expect(confirm).toBeEnabled()
    const frame = page.frames().find((item) => item.url().includes('/sourcePanel?'))!
    let releaseNavigation!: () => void
    const navigationGate = new Promise<void>((resolve) => { releaseNavigation = resolve })
    await page.route('http://reader.localhost/sourcePanel?**', async (route) => {
      if (new URL(route.request().url()).searchParams.get('__readerPanelUrl') === 'https://fixture.test/assigned') {
        await navigationGate
      }
      await route.fallback()
    })
    try {
      await frame.evaluate(() => location.assign('/assigned'))
      await expect(confirm).toBeDisabled()
    } finally {
      releaseNavigation()
    }
    await frame.waitForURL((url) => url.origin === 'http://reader.localhost' && url.searchParams.get('__readerPanelUrl') === 'https://fixture.test/assigned')
    await expect(confirm).toBeEnabled()
    await frame.evaluate(() => location.replace('replaced'))
    await frame.waitForURL((url) => url.origin === 'http://reader.localhost' && url.searchParams.get('__readerPanelUrl') === 'https://fixture.test/replaced')
    await expect(confirm).toBeEnabled()
    await frame.getByLabel('线路', { exact: true }).fill('跳转后的选择')
    await confirm.click()
    await expect(page.locator('.source-browser-panel')).toHaveCount(0)
    const result = await page.evaluate(() => (window as any).__panelResult)
    expect(result.url).toBe('https://fixture.test/replaced')
    expect(result.body).toContain('value="跳转后的选择"')
  })

  test('history and hash updates return the current real page without reloading it', async ({ page }) => {
    await page.getByRole('button', { name: '设置设备', exact: true }).click()
    const confirm = page.getByRole('button', { name: '确认', exact: true })
    await expect(confirm).toBeEnabled()
    const frame = page.frames().find((item) => item.url().includes('/sourcePanel?'))!
    const navigations: string[] = []
    page.on('request', (request) => {
      if (request.isNavigationRequest() && new URL(request.url()).pathname === '/sourcePanel') navigations.push(request.url())
    })
    await frame.getByLabel('线路', { exact: true }).fill('保留页面输入')
    await frame.evaluate(() => {
      history.pushState({ step: 1 }, '', '/account')
      history.replaceState({ step: 2 }, '', '/account?tab=profile')
      location.hash = 'details'
    })
    await frame.waitForFunction(() => location.hash === '#details')
    await expect(confirm).toBeEnabled()
    await expect(frame.getByLabel('线路', { exact: true })).toHaveValue('保留页面输入')
    await confirm.click()
    await expect(page.locator('.source-browser-panel')).toHaveCount(0)
    expect(navigations).toHaveLength(0)
    expect((await page.evaluate(() => (window as any).__panelResult)).url).toBe('https://fixture.test/account?tab=profile#details')
  })

  test('POST navigation keeps field values and the clicked submit button', async ({ page }) => {
    await page.getByRole('button', { name: '设置设备', exact: true }).click()
    const confirm = page.getByRole('button', { name: '确认', exact: true })
    await expect(confirm).toBeEnabled()
    const frame = page.frames().find((item) => item.url().includes('/sourcePanel?'))!
    await frame.evaluate(() => {
      const form = document.createElement('form')
      form.method = 'post'
      form.action = 'https://fixture.test/register'
      form.innerHTML = '<input name="user" value="读者"><input name="tag" value="one"><input name="tag" value="two"><button name="action" value="register">提交注册</button>'
      document.body.append(form)
    })
    const request = page.waitForRequest((request) => new URL(request.url()).searchParams.get('__readerPanelUrl') === 'https://fixture.test/register')
    await frame.getByRole('button', { name: '提交注册', exact: true }).click()
    const submitted = await request
    expect(submitted.method()).toBe('POST')
    const fields = new URLSearchParams(submitted.postData()!)
    expect(fields.get('user')).toBe('读者')
    expect(fields.getAll('tag')).toEqual(['one', 'two'])
    expect(fields.get('action')).toBe('register')
    await frame.waitForURL((url) => url.searchParams.get('__readerPanelUrl') === 'https://fixture.test/register')
    await expect(confirm).toBeEnabled()
    await confirm.click()
    await expect(page.locator('.source-browser-panel')).toHaveCount(0)
    expect((await page.evaluate(() => (window as any).__panelResult)).url).toBe('https://fixture.test/register')
  })

  test('programmatic multipart POST keeps uploaded file content', async ({ page }) => {
    await page.getByRole('button', { name: '设置设备', exact: true }).click()
    const confirm = page.getByRole('button', { name: '确认', exact: true })
    await expect(confirm).toBeEnabled()
    const frame = page.frames().find((item) => item.url().includes('/sourcePanel?'))!
    await frame.evaluate(() => {
      const form = document.createElement('form')
      form.id = 'upload'
      form.method = 'post'
      form.enctype = 'multipart/form-data'
      form.action = 'https://fixture.test/upload'
      form.innerHTML = '<input name="description" value="附件说明"><input type="file" name="attachment">'
      document.body.append(form)
    })
    await frame.locator('input[type=file]').setInputFiles({ name: 'note.txt', mimeType: 'text/plain', buffer: Buffer.from('fixture file content') })
    const request = page.waitForRequest((request) => new URL(request.url()).searchParams.get('__readerPanelUrl') === 'https://fixture.test/upload')
    await frame.evaluate(() => HTMLFormElement.prototype.submit.call(document.getElementById('upload')))
    const submitted = await request
    expect(submitted.method()).toBe('POST')
    expect(submitted.headers()['content-type']).toContain('multipart/form-data')
    expect(submitted.postData()).toContain('filename="note.txt"')
    expect(submitted.postData()).toContain('fixture file content')
    expect(submitted.postData()).toContain('附件说明')
    await frame.waitForURL((url) => url.searchParams.get('__readerPanelUrl') === 'https://fixture.test/upload')
    await expect(confirm).toBeEnabled()
    await page.getByRole('button', { name: '取消', exact: true }).click()
  })

  test('GET forms retain the panel id while replacing the original action query', async ({ page }) => {
    await page.getByRole('button', { name: '设置设备', exact: true }).click()
    const confirm = page.getByRole('button', { name: '确认', exact: true })
    await expect(confirm).toBeEnabled()
    const frame = page.frames().find((item) => item.url().includes('/sourcePanel?'))!
    await frame.evaluate(() => {
      const form = document.createElement('form')
      const action = new URL(location.href)
      action.searchParams.set('__readerPanelUrl', 'https://fixture.test/search?old=1')
      form.method = 'get'
      form.action = action.href
      form.innerHTML = '<input name="q" value="斗破苍穹"><input name="panelId" value="form-field"><input name="url" value="search-field"><button>搜索页面</button>'
      document.body.append(form)
    })
    await frame.getByRole('button', { name: '搜索页面', exact: true }).click()
    await frame.waitForURL((url) => {
      const target = url.searchParams.get('__readerPanelUrl')
      return url.searchParams.get('__readerPanelId') === 'fixture-settings' && !!target && new URL(target).pathname === '/search'
    })
    const target = new URL(new URL(frame.url()).searchParams.get('__readerPanelUrl')!)
    expect(target.searchParams.get('q')).toBe('斗破苍穹')
    expect(target.searchParams.get('panelId')).toBe('form-field')
    expect(target.searchParams.get('url')).toBe('search-field')
    expect(target.searchParams.has('old')).toBe(false)
    await expect(confirm).toBeEnabled()
    await page.getByRole('button', { name: '取消', exact: true }).click()
  })

  test('a webview without navigation interception reports a recoverable page error', async ({ page }) => {
    await page.addInitScript(() => Object.defineProperty(window, 'navigation', { configurable: true, value: undefined }))
    await page.route('https://fixture.test/unbridged', (route) => route.fulfill({ contentType: 'text/html', body: '<title>登录完成</title><p>完成页面</p>' }))
    await page.getByRole('button', { name: '设置设备', exact: true }).click()
    const confirm = page.getByRole('button', { name: '确认', exact: true })
    await expect(confirm).toBeEnabled()
    const frame = page.frames().find((item) => item.url().includes('/sourcePanel?'))!
    expect(await frame.evaluate(() => Boolean((window as any).navigation))).toBe(false)
    await page.clock.install()
    await frame.evaluate(() => location.assign('https://fixture.test/unbridged'))
    await frame.waitForURL('https://fixture.test/unbridged')
    await expect(confirm).toBeDisabled()
    await page.clock.fastForward(6_000)
    await expect(page.getByRole('alert')).toContainText('当前网页无法确认')
    expect(await page.evaluate(() => (window as any).__panelSnapshot)).toBeUndefined()
    await page.getByRole('button', { name: '重新加载', exact: true }).click()
    await frame.waitForURL((url) => url.origin === 'http://reader.localhost')
    await expect(confirm).toBeEnabled()
    await page.getByRole('button', { name: '取消', exact: true }).click()
  })

  test('confirmation timeout recovers without saving an unacknowledged document', async ({ page }) => {
    await page.addInitScript(() => {
      if (location.hostname !== 'reader.localhost') return
      window.addEventListener('message', (event) => {
        if (event.data?.type !== 'reader:confirm' || sessionStorage.getItem('fixture-confirm-dropped')) return
        sessionStorage.setItem('fixture-confirm-dropped', '1')
        event.stopImmediatePropagation()
      })
    })
    await page.getByRole('button', { name: '设置设备', exact: true }).click()
    const confirm = page.getByRole('button', { name: '确认', exact: true })
    await expect(confirm).toBeEnabled()
    await page.clock.install()
    await confirm.click()
    await expect(page.getByRole('button', { name: '正在保存…', exact: true })).toBeDisabled()
    await page.clock.fastForward(13_000)
    await expect(page.getByRole('alert')).toContainText('网页未响应确认')
    expect(await page.evaluate(() => (window as any).__panelSnapshot)).toBeUndefined()
    const reloaded = page.waitForRequest((request) => request.isNavigationRequest() && new URL(request.url()).pathname === '/sourcePanel')
    await page.getByRole('button', { name: '重新加载', exact: true }).click()
    const reloadUrl = new URL((await reloaded).url())
    expect(reloadUrl.searchParams.get('__readerPanelId')).toBe('fixture-settings')
    expect(reloadUrl.searchParams.has('__readerPanelUrl')).toBe(false)
    await expect(confirm).toBeEnabled()
    await confirm.click()
    await expect(page.locator('.source-browser-panel')).toHaveCount(0)
    expect(await page.evaluate(() => (window as any).__panelSnapshot)).toContain('<!DOCTYPE html>')
  })
})
