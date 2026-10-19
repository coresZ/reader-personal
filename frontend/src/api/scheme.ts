// reader:// 自定义协议的 origin 因 webview 后端而异:
//   WebView2 (Windows/Android)  → http://reader.localhost
//   WebKitGTK (Linux) / WKWebView (macOS) → reader://localhost
// Tauri 注入的 convertFileSrc 用的正是这套映射(默认 protocol 为 asset),
// 借它探测而不是嗅探 window.location —— dev 模式下所有平台的页面都跑在
// http://localhost:5173, 无法据此区分。单测环境没有 Tauri, 退回 Windows 形式。
import { convertFileSrc } from '@tauri-apps/api/core'

export const readerOrigin: string = (() => {
  try {
    // 只取 origin, 不要按 "/probe" 后缀切片: 某些 WebView 后端返回的探测地址
    // 带路径前缀(形如 http://reader.localhost/user/probe), 切片后剩下的是
    // `http://reader.localhost/user` —— 用它拼 `/bookSourceProxy` 会得到
    // `/user/bookSourceProxy`, 登录代理页因此落到错误路径上(表现为白屏)。
    const probe = convertFileSrc('probe', 'reader')
    return new URL(probe).origin
  } catch {
    /* 不在 Tauri 内运行(单元测试) */
  }
  return 'http://reader.localhost'
})()
