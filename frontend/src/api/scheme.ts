// reader:// 自定义协议的 origin。
//
// 三种运行环境的映射:
//   WebView2 (Windows)      → http://reader.localhost
//   WebKitGTK / WKWebView   → reader://localhost
//   浏览器 (Web 阅读模式)   → 空串, 资源走同源的 `/cover`、`/pdf` 等路由
//
// 浏览器里没有自定义协议, 但服务端 (`src/server/router.rs`) 把 `/cover`
// `/pdf` `/epub` `/files` 挂在了同一批路径上, 因此这里返回空串, 让调用方
// 拼出 `/cover?path=...` 这样的同源相对地址即可。
//
// 单测环境没有 Tauri 也没有服务端, 维持原来的 Windows 形式, 避免已有断言
// (bookshelf.test.ts / localEpubAsset.test.ts) 集体改期望值。

import { convertFileSrc } from '@tauri-apps/api/core'
import { isTauriRuntime, isWebReaderMode } from './transport'

export const readerOrigin: string = (() => {
  // 浏览器(Web 阅读模式): 资源走同源相对路径, 前缀为空。
  if (isWebReaderMode()) return ''
  if (!isTauriRuntime()) {
    // 单测 / SSR: 维持历史上的 Windows 形式, 让已有断言继续成立。
    return 'http://reader.localhost'
  }
  try {
    // 只取 origin, 不要按 "/probe" 后缀切片: 某些 WebView 后端返回的探测地址
    // 带路径前缀(形如 http://reader.localhost/user/probe), 切片后剩下的是
    // `http://reader.localhost/user` —— 用它拼 `/bookSourceProxy` 会得到
    // `/user/bookSourceProxy`, 登录代理页因此落到错误路径上(表现为白屏)。
    const probe = convertFileSrc('probe', 'reader')
    return new URL(probe).origin
  } catch {
    /* 不在 Tauri 内运行 */
  }
  return 'http://reader.localhost'
})()

/**
 * 拼一个资源地址。
 *
 * `readerOrigin` 为空(浏览器)时返回**相对路径**, 由同源服务端处理;
 * 非空时返回绝对地址, 供 WebView 的自定义协议拦截。
 */
export function assetUrl(pathAndQuery: string): string {
  const suffix = pathAndQuery.startsWith('/') ? pathAndQuery : `/${pathAndQuery}`
  return readerOrigin ? `${readerOrigin}${suffix}` : suffix
}
