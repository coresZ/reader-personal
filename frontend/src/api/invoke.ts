// Drop-in replacement for the old axios `http` client. The 14 api/*.ts modules
// keep their exact signatures — `http.get<T>('/x').then(r => r.data)` becomes
// `get<T>('/x').then(r => r.data)` — only the import changes.
//
// 传输层可替换 (`api/transport/`): 桌面端走 Tauri IPC, 浏览器走 HTTP。
// 本文件只保留**协议无关**的部分 —— 路径归一化、信封解包、错误归一化。
// 两份实现共用这一段, 避免行为分叉。

import { invoke } from '@tauri-apps/api/core'
import type { ApiResponse } from '../types'
import { httpTransport } from './transport/http'
import { isEnvelope, isTauriRuntime } from './transport'
import { COMMAND_BY_PATH, tauriTransport } from './transport/tauri'

/**
 * 当前传输实现。运行时判定而非构建期替换:
 * 同一份 `dist` 既能装进 Tauri, 也能被浏览器直接打开。
 */
const transport = isTauriRuntime() ? tauriTransport : httpTransport

/** 把 `/getBookshelf` 归一化成 `/reader3/getBookshelf`。 */
function normalizePath(path: string): string {
  return path.startsWith('/reader3') ? path : `/reader3${path}`
}

/**
 * command 名 → `/reader3/*` 路径(仅 Web 端需要)。
 *
 * 复用 `COMMAND_BY_PATH` 这一份表, 避免再维护一张会漂移的反向映射。
 */
const PATH_BY_COMMAND: Record<string, string> = Object.fromEntries(
  Object.entries(COMMAND_BY_PATH).map(([path, command]) => [command, path]),
)

function pathForCommand(command: string): string | undefined {
  return PATH_BY_COMMAND[command]
}

/**
 * 把 Tauri command 的 reject 值统一成 `Error`。
 *
 * Rust 侧的 `AppError` 自定义了 `Serialize`（`serializer.serialize_str(...)`），
 * 因此 `invoke()` 被 `Err(AppError)` 拒绝时拿到的是**字符串**而非 `Error`。
 * 调用方写 `(error as Error).message` 只会得到 `undefined`，真实原因被兜底文案
 * 吞掉 —— 用户只能看到「备份失败」，无从判断是超限、路径还是网络问题。
 */
export function toError(value: unknown, fallback = '请求失败'): Error {
  if (value instanceof Error) return value
  if (typeof value === 'string' && value.trim()) return new Error(value)
  if (value && typeof value === 'object' && typeof (value as { message?: unknown }).message === 'string') {
    const message = (value as { message: string }).message
    if (message.trim()) return new Error(message)
  }
  return new Error(fallback)
}

let lastNeedLoginDispatchAt = 0

function dispatchNeedLogin() {
  const now = Date.now()
  if (now - lastNeedLoginDispatchAt < 1500) return
  lastNeedLoginDispatchAt = now
  window.dispatchEvent(new CustomEvent('need-login'))

  // 只有桌面端有监听者(App.vue 弹 WebDAV 登录框)。
  //
  // 这里**曾经**在 Web 端跳 `/__login` —— 那是服务端 HTTP Basic / cookie 会话
  // 时代的登录页, 鉴权移除后那条路由也不存在了, 跳过去只会 404。Web 端现在
  // 能产生 `NEED_LOGIN` 的只剩 WebDAV(且该功能在 Web 端已隐藏), 因此不再分支。
}

/** 统一解信封。Tauri 与 HTTP 两条路径共用, 保证错误处理只有一份。 */
function unwrap<T>(res: ApiResponse<T> | T): T {
  if (isEnvelope(res)) {
    const box = res as ApiResponse<T>
    if (!box.isSuccess) {
      if (box.errorMsg === 'NEED_LOGIN' || box.data === 'NEED_LOGIN') {
        dispatchNeedLogin()
      }
      throw new Error(box.errorMsg || '请求失败')
    }
    return box.data as T
  }
  return res as T // raw payload (binary endpoints that bypass the envelope)
}

async function call<T>(path: string, payload: unknown, method: 'GET' | 'POST' = 'POST'): Promise<T> {
  const fullPath = normalizePath(path)
  try {
    return unwrap<T>((await transport.call(fullPath, payload, method)) as ApiResponse<T> | T)
  } catch (err) {
    throw toError(err)
  }
}

/** GET with query params → command args. */
export function get<T>(path: string, opts?: { params?: Record<string, unknown> }): Promise<{ data: T }> {
  return call<T>(path, opts?.params, 'GET').then((data) => ({ data }))
}

/** POST with a JSON body → command `req` arg. */
export function post<T>(path: string, body?: unknown, _config?: unknown): Promise<{ data: T }> {
  return call<T>(path, body).then((data) => ({ data }))
}

/**
 * 直接调用底层传输(给需要自定义参数的调用点用, 如带 Channel 的命令)。
 *
 * Web 端不存在 Channel, 传进来的 `args` 里的函数会被忽略 —— 因此调用方必须
 * 保证"没有 Channel 也能工作"。
 */
export async function invokeRaw<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauriRuntime()) {
    return invoke<T>(command, args)
  }
  const path = pathForCommand(command)
  if (!path) throw new Error(`Web 端不支持直接调用 ${command}`)
  return call<T>(path, args?.req ?? args)
}

/**
 * Invoke a command that takes top-level args (not wrapped in `req`) and returns
 * an ApiResponse envelope. Used by upload commands whose Rust params are
 * `file`/`file_name`/`path` etc. rather than a single `req` struct.
 */
export async function invokeEnvelope<T>(
  command: string,
  args: Record<string, unknown>,
): Promise<T> {
  const res = await invokeRaw<ApiResponse<T>>(command, args).catch((err) => {
    throw toError(err)
  })
  if (!res.isSuccess) throw new Error(res.errorMsg || '请求失败')
  return res.data as T
}

/**
 * 上传二进制文件专用。
 *
 * 桌面端: 把字节作为**顶层** payload 交给 Tauri —— 它只在载荷本身是二进制时
 * 才走 octet-stream 通道。一旦把 `Uint8Array` 塞进对象(如 `{ fileName, file }`),
 * 就落进 `JSON.stringify` 分支, 那里对 Uint8Array 做 `Array.from(val)` ——
 * 每个字节变成十进制数字、平均 3.57 个字符。一本 50MB 的 TXT 会因此涨到约
 * 178MB 字符串再被 Rust 解析回字节, 纯编码就要 1.8s 以上。
 *
 * Web 端: 走标准 multipart, 没有这个开销。
 */
export async function invokeUpload<T>(
  command: string,
  commandHeaders: Record<string, string>,
  file: Blob,
): Promise<T> {
  const fileName = decodeURIComponent(commandHeaders['x-reader-file-name'] ?? '未命名')

  try {
    // 桌面端直接把 command 交给 IPC —— 上传类命令不一定有 HTTP 路由,
    // 拿它去反查路径会漏项(早期版本因此让「背景图上传」报"未知上传接口")。
    const res = isTauriRuntime()
      ? await tauriTransport.upload<T>(command, file, fileName, commandHeaders)
      : await httpTransport.upload<T>(command, file, fileName, commandHeaders)
    return unwrap<T>(res as ApiResponse<T> | T)
  } catch (err) {
    throw toError(err)
  }
}

export default { get, post, invokeRaw, invokeEnvelope, invokeUpload, toError }
