import { invokeEnvelope, invokeUpload } from './invoke'
import { FILE_NAME_HEADER, encodeFileName } from './uploadHeader'

export interface CustomFontEntry {
  id: string
  name: string
  url: string
}

export function listCustomFonts() {
  return invokeEnvelope<CustomFontEntry[]>('list_custom_fonts', {})
}

/** 上传自定义字体。字节走 octet-stream 快速通道, 文件名走 IPC header。 */
export async function uploadCustomFont(file: File) {
  return invokeUpload<CustomFontEntry>(
    'upload_custom_font',
    { [FILE_NAME_HEADER]: encodeFileName(file.name) },
    file,
  )
}

export function deleteCustomFont(id: string) {
  return invokeEnvelope<null>('delete_custom_font', { id })
}
