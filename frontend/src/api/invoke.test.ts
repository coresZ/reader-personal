import { describe, expect, it, vi } from 'vitest'
import { toError } from './invoke'

/**
 * Tauri command 失败时, Rust `AppError` 的自定义 `Serialize` 把错误写成
 * **字符串**(`serializer.serialize_str(&self.to_string())`), 所以前端 catch
 * 到的是 string 而不是 Error。
 *
 * 曾经的写法 `(error as Error).message || '备份失败'` 在这种情况下恒为
 * `undefined`, 于是界面永远只显示兜底文案 —— issue #15 里"备份失败"四个字
 * 背后真正的原因(条目超限 / 路径非法 / 网络错误)一个字都没传到用户眼前。
 */
describe('toError', () => {
  it('keeps the original message when Tauri rejects with a string', () => {
    const error = toError('备份条目 reader-rust.json 超过 64 MB')

    expect(error).toBeInstanceOf(Error)
    expect(error.message).toBe('备份条目 reader-rust.json 超过 64 MB')
  })

  it('returns the same Error instance unchanged', () => {
    const original = new Error('无法连接 WebDAV 服务器')

    expect(toError(original)).toBe(original)
  })

  it('falls back when the rejection carries no usable text', () => {
    // undefined / 空串 / 空对象: 都没有可展示的信息
    expect(toError(undefined, '备份失败').message).toBe('备份失败')
    expect(toError('', '备份失败').message).toBe('备份失败')
    expect(toError('   ', '备份失败').message).toBe('备份失败')
    expect(toError({}, '备份失败').message).toBe('备份失败')
  })

  it('reads a message off a non-Error rejection object', () => {
    expect(toError({ message: '网络超时' }).message).toBe('网络超时')
  })

  it('uses a generic default when no fallback is supplied', () => {
    expect(toError(null).message).toBe('请求失败')
  })
})

/**
 * 上传通道的契约测试。
 *
 * 背景: 传输层刚抽象出来时, `invokeUpload` 试图用「command → `/reader3/*`
 * 路径」反查表来定位接口。但上传类命令里只有 `upload_txt_book` /
 * `upload_epub_book` / `upload_pdf_book` 有 HTTP 路由, 背景图、自定义字体、
 * WebDAV、RSS 源导入这些**根本没有**。结果是桌面端点「添加背景图」直接弹
 * 「未知上传接口: upload_background_image」。
 *
 * 这里必须真的断言"传下去了什么"。早先的版本只写
 * `expect(typeof tauriTransport.upload).toBe('function')`, 那种断言在
 * 任何实现下都成立 —— 包括把它写成 `async () => { throw new Error(...) }`,
 * 等于没有测试。
 */
describe('transport upload contract', () => {
  it('desktop upload forwards the command name verbatim, never a path', async () => {
    // 必须 mock 模块之后再 import, 否则拿到的是真的 invoke(会去连 Tauri)。
    vi.resetModules()
    const invoke = vi.fn(async () => 'reader://files?path=x.png')
    vi.doMock('@tauri-apps/api/core', () => ({ invoke, Channel: class {} }))

    const { tauriTransport } = await import('./transport/tauri')
    const file = new Blob([new Uint8Array([1, 2, 3])], { type: 'image/png' })

    const result = await tauriTransport.upload(
      'upload_background_image',
      file,
      '背景图.png',
      { 'x-reader-background-kind': 'desktop' },
    )

    expect(result).toBe('reader://files?path=x.png')
    expect(invoke).toHaveBeenCalledTimes(1)

    const [command, payload, options] = invoke.mock.calls[0] as unknown as [
      string,
      Uint8Array,
      { headers: Record<string, string> },
    ]
    // 命令名原样透传: 没有"未知上传接口"这一层, 也不需要 HTTP 路由存在。
    expect(command).toBe('upload_background_image')
    // 载荷是**顶层**二进制, 不是 `{ req: ... }` 里的字节数组。
    expect(payload).toBeInstanceOf(Uint8Array)
    expect(Array.from(payload)).toEqual([1, 2, 3])
    // 文件名走 header 且做 percent-encode(header 只能是 ASCII)。
    expect(options.headers['x-reader-file-name']).toBe(encodeURIComponent('背景图.png'))
    expect(options.headers['x-reader-background-kind']).toBe('desktop')

    vi.doUnmock('@tauri-apps/api/core')
  })

  it('web upload maps the three local-book commands to their routes', async () => {
    vi.resetModules()
    const fetchMock = vi.fn(async () => new Response(
      JSON.stringify({ isSuccess: true, errorMsg: '', data: { bookUrl: 'x' } }),
      { status: 200, headers: { 'content-type': 'application/json' } },
    ))
    vi.stubGlobal('fetch', fetchMock)
    vi.doMock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), Channel: class {} }))

    const { httpTransport } = await import('./transport/http')
    await httpTransport.upload(
      'upload_txt_book',
      new Blob([new Uint8Array([1])]),
      '小说.txt',
    )

    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    // 路径**不能**再补一次 /reader3 前缀(历史上出现过 /reader3/reader3/... 全 405)
    expect(url).toBe('/reader3/uploadTxtBook')
    expect(init.method).toBe('POST')
    const form = init.body as FormData
    expect(form.get('file')).toBeInstanceOf(Blob)

    vi.unstubAllGlobals()
    vi.doUnmock('@tauri-apps/api/core')
  })

  it('web upload rejects commands without an HTTP route', async () => {
    const { httpTransport } = await import('./transport/http')
    await expect(
      httpTransport.upload('upload_background_image', new Blob([new Uint8Array()]), 'a.png'),
    ).rejects.toThrow(/仅桌面应用可用|不支持/)
  })
})
