// 上传类命令共用的文件名 header 约定。
//
// 为什么文件名走 header 而不是和字节一起放进 payload：Tauri 只在 IPC 载荷
// **本身是二进制**时才走 octet-stream 快速通道（tauri 的
// process-ipc-message-fn.js: 顶层 ArrayBuffer / ArrayBufferView / Array 才命中）。
// 一旦把 `Uint8Array` 塞进对象（如 `{ fileName, file }`），就落进
// `JSON.stringify` 分支，那里对 Uint8Array 做 `Array.from(val)` —— 每个字节
// 变成一个十进制数字、平均 3.57 个字符。一本几十 MB 的书会因此涨到上百 MB
// 字符串再被 Rust 解析回字节。参见 `invokeUpload`。
//
// 与 Rust 侧 `crate::api::commands::FILE_NAME_HEADER` 必须保持一致。

/** 承载上传文件名的 IPC header 名。 */
export const FILE_NAME_HEADER = 'x-reader-file-name'

/** header 值只能承载 ASCII，中文名先 percent-encode，Rust 侧解码还原。 */
export function encodeFileName(name: string) {
  return encodeURIComponent(name)
}
