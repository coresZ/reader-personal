import { invokeEnvelope, invokeUpload } from './invoke'
import { FILE_NAME_HEADER, encodeFileName } from './uploadHeader'

/** 桌面 / 阅读页背景图。
 *
 *  图片真实文件由后端写入应用数据目录(`storage/assets/default/backgrounds/`),
 *  前端只保存返回的 URL —— 与自定义封面同一思路。以前整张图以 base64 dataURL
 *  存进 localStorage, 受 5MB 配额限制, 只能缩到 1600px 再压 0.58 质量 webp,
 *  4K 屏上肉眼可见地糊(issue #20 第 1 条)。 */

export type BackgroundKind = 'desktop' | 'reader'

/** 与后端 `BACKGROUND_KIND_HEADER` 一致。
 *
 *  背景图类型必须走 header: `invokeUpload` 把整个 payload 当二进制 body,
 *  普通参数放不进去(与文件名同一原因, 见 uploadHeader.ts)。 */
const BACKGROUND_KIND_HEADER = 'x-reader-background-kind'

/** 上传背景图, 返回可直接存进 `readConfig` 的 URL。
 *
 *  字节走 Tauri 的 octet-stream 快速通道, 文件名与类型走 IPC header —— 详见
 *  `invokeUpload` 与 `uploadHeader.ts` 的说明。 */
export async function uploadBackgroundImage(kind: BackgroundKind, file: Blob, fileName: string) {
  return invokeUpload<string>(
    'upload_background_image',
    {
      [FILE_NAME_HEADER]: encodeFileName(fileName),
      [BACKGROUND_KIND_HEADER]: kind,
    },
    file,
  )
}

/** 删除本应用存下的背景图文件。
 *  传 dataURL(旧版存法)或任意其它地址都是安全的 —— 后端只认自己生成的 URL。 */
export function deleteBackgroundImage(url: string) {
  return invokeEnvelope<null>('delete_background_image', { url })
}
