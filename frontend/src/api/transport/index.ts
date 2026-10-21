// 传输层抽象。
//
// 桌面端与 Web 端跑的是**同一份前端代码**, 差异只在"怎么把 `/reader3/xxx`
// 这个调用送到后端":
//
//   - Tauri: `invoke(commandName, { req })`, 走 IPC, 返回 `ApiResponse` 信封;
//   - Web:   `POST /reader3/xxx`, 走 HTTP, 返回同一个信封。
//
// 之所以不在构建期用 alias 替换整个 `invoke.ts`, 是因为那样会出现两份解包逻辑
// (信封判定、`NEED_LOGIN` 分发、错误归一化), 任何一边改了另一边就会静默失配。
// 这里只抽象最底层的两个动作, 上层解包代码保持唯一。

import type { ApiResponse } from '../../types'

/**
 * 底层调用结果: 已经解过信封, 直接给业务数据。
 */
export interface Transport {
  /**
   * 调用一个 `/reader3/*` 接口。
   * @param path 形如 `/reader3/getBookshelf`
   * @param payload 请求体
   * @param method 语义上的 HTTP 方法。Tauri 侧忽略(IPC 没有方法之分);
   *   HTTP 侧据此选 GET/POST —— 后端的读接口注册在 GET 上, 用 POST 打会 405。
   */
  call<T>(path: string, payload: unknown, method: 'GET' | 'POST'): Promise<T>

  /**
   * 上传文件。
   *
   * 注意参数是 **command 名**而非路径: 上传类接口的 command 与 `/reader3/*`
   * 路径不是一一对应(`upload_background_image`、`upload_custom_font`、
   * `upload_webdav_...` 这些根本没有 HTTP 路由), 硬做反查表只会漏项 ——
   * 早先的版本就是这么把「桌面背景图上传」搞挂的。
   *
   * @param command Rust 侧的命令名, 如 `upload_txt_book`
   * @param file 文件内容
   * @param fileName 原始文件名(含中文与扩展名)
   * @param headers 额外的 IPC header(如背景图类型)
   */
  upload<T>(
    command: string,
    file: Blob,
    fileName: string,
    headers?: Record<string, string>,
  ): Promise<T>
}

/**
 * 运行时是否跑在 Tauri 里。
 *
 * **全项目唯一的判定入口**。别处请 import 这个函数, 不要再写
 * `'__TAURI_INTERNALS__' in window` —— 项目里曾同时存在五份字面量判断和
 * 一份 `@tauri-apps/api` 自带的 `isTauri()`, 它们在"单测/SSR/浏览器"这几档上
 * 的答案并不一致, 改一处漏一处就会冒出"桌面端按浏览器逻辑跑"这种怪事。
 */
export function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/**
 * 产品语义上的「Web 精简模式」: 非 Tauri 即视为浏览器端。
 *
 * 与 [`isWebReaderMode`] 的差别: 这个只回答"是不是桌面应用", 用于决定
 * **界面显示哪些入口**; 后者还额外要求"后端真的在身边", 用于决定
 * **资源地址怎么拼**。两者别混用 —— 在 Vite dev server 里前者为真
 * (按 Web 端精简界面), 后者为假(仍按 Tauri 的绝对地址拼, 方便配合真机调试)。
 */
export function isWebUiMode(): boolean {
  return typeof window !== 'undefined' && !isTauriRuntime()
}

/**
 * 是否由**本项目自己的 HTTP 服务**托管(Web 阅读模式)。
 *
 * 这问的和 [`isTauriRuntime`] 不是同一件事: Vitest(无 window.location 端口)
 * 和 Vite dev server(5173, 只有前端没有后端)都"不是 Tauri", 但它们也没有
 * `/reader3/*` 与 `/cover` 这些同源路由可用。只有真被 `src/server` 托管时,
 * 资源地址才能写成相对路径。
 */
export function isWebReaderMode(): boolean {
  if (typeof window === 'undefined') return false
  if (isTauriRuntime()) return false
  const { protocol, port } = window.location
  if (protocol !== 'http:' && protocol !== 'https:') return false
  // Vite dev server 只是前端热更新, 没有后端。
  return port !== '5173'
}

/**
 * 判断一个响应是否是 `ApiResponse` 信封。
 *
 * 有些二进制接口(如 AI 代理)直接返回裸数据, 不能按信封解。判据与旧实现一致:
 * 只要对象里带 `isSuccess` 就当作信封。
 */
export function isEnvelope(value: unknown): value is ApiResponse<unknown> {
  return typeof value === 'object' && value !== null && 'isSuccess' in value
}
