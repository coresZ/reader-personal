// HTTP 传输实现(浏览器 / Web 阅读模式)。
//
// 与 Tauri 版一一对应, 只是把 IPC 换成 fetch。后端 `src/server/router.rs`
// 暴露的路由前缀与路径完全一致, 因此这里不需要任何映射表。
//
// 鉴权: **服务端没有鉴权**(见 `docs/web-reader.md` 的「安全」)。这里不拼
// Authorization, 也没有登录跳转; `credentials: 'same-origin'` 只是为了让
// 同源的 cookie(书源登录代理用得上)照常带上。

import type { Transport } from './index'

/**
 * 统一的请求入口。
 *
 * `path` 已经带 `/reader3` 前缀(由 `invoke.ts` 的 `normalizePath` 补齐),
 * 直接拼在 origin 后面即可 —— 早先这里又补了一次前缀, 实际请求变成
 * `/reader3/reader3/xxx`, 全部落到 405。这类错误只在真实浏览器里暴露,
 * 单测与直接 curl 都不会经过前端这层。
 */
async function request(path: string, init: RequestInit): Promise<Response> {
  const response = await fetch(path, {
    credentials: 'same-origin',
    ...init,
  })
  return response
}

export const httpTransport: Transport = {
  async call<T>(path: string, payload: unknown, method: 'GET' | 'POST'): Promise<T> {
    // GET 的查询参数走 query string，PUT/POST 走 JSON body。
    // 后端把读接口注册在 GET 上(与 Legado 的 HttpServer 一致)，方法不匹配会
    // 直接 405 —— 这一点只有真实浏览器会踩到。
    let url = path
    let init: RequestInit = { method }
    if (method === 'GET') {
      const params = payload as Record<string, unknown> | undefined
      const query = new URLSearchParams()
      for (const [key, value] of Object.entries(params ?? {})) {
        if (value === undefined || value === null) continue
        query.set(key, typeof value === 'object' ? JSON.stringify(value) : String(value))
      }
      const qs = query.toString()
      if (qs) url = `${url}?${qs}`
    } else {
      init = {
        method,
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(payload ?? {}),
      }
    }
    const response = await request(url, init)
    return readJson<T>(response, path)
  },

  async upload<T>(
    command: string,
    file: Blob,
    fileName: string,
    _headers?: Record<string, string>,
  ): Promise<T> {
    // Web 端的上传只支持"本地书"这三种 —— 它们有真实的 HTTP 路由。
    // 背景图、自定义字体、WebDAV 这些是桌面专属能力, 浏览器里没有等价路由,
    // 直接给出明确提示, 而不是发一个注定 404 的请求。
    const path = UPLOAD_ROUTES[command]
    if (!path) {
      throw new Error('Web 端不支持该上传操作（仅桌面应用可用）')
    }
    const form = new FormData()
    // 后端(`read_upload`)只认字段名 `file`, 文件名取自 multipart 的
    // filename 部分 —— 中文无需 percent-encode, multipart 本身支持 UTF-8。
    form.append('file', file, fileName)
    const response = await request(path, { method: 'POST', body: form })
    return readJson<T>(response, path)
  },
}

/** Web 端支持的上传命令 → 路由。 */
const UPLOAD_ROUTES: Record<string, string> = {
  upload_txt_book: '/reader3/uploadTxtBook',
  upload_epub_book: '/reader3/uploadEpubBook',
  upload_pdf_book: '/reader3/uploadPdfBook',
}

async function readJson<T>(response: Response, path: string): Promise<T> {
  // 401 只可能来自上游书源(登录代理页), 本服务自身不会发 —— 鉴权已移除。
  if (response.status === 401) {
    throw new Error('书源要求登录: 请先在书架里打开该书源并完成登录')
  }
  const text = await response.text()
  if (!text) {
    if (!response.ok) throw new Error(`请求失败(${response.status}): ${path}`)
    return undefined as T
  }
  try {
    return JSON.parse(text) as T
  } catch {
    throw new Error(
      response.ok ? `响应不是 JSON: ${path}` : `请求失败(${response.status}): ${path}`,
    )
  }
}
