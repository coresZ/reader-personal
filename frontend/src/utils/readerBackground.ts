const SUPPORTED_IMAGE_TYPES = new Set([
  'image/jpeg',
  'image/png',
  'image/webp',
  'image/bmp',
])

const MAX_SOURCE_BYTES = 20 * 1024 * 1024
const MAX_STORED_LENGTH = 2_000_000

export const DEFAULT_READER_BACKGROUND_OPACITY = 0.35

/** 透明度滑块的步长(1%)。旧版是 5%, 用户反馈太粗。 */
export const READER_BACKGROUND_OPACITY_STEP = 0.01

export function normalizeReaderBackgroundOpacity(value: unknown) {
  const numeric = typeof value === 'number' ? value : Number(value)
  if (!Number.isFinite(numeric)) return DEFAULT_READER_BACKGROUND_OPACITY
  return Math.max(0, Math.min(1, numeric))
}

/** 存储值(0~1) → 界面上的百分比整数。 */
export function readerOpacityToPercent(opacity: unknown): number {
  return Math.round(normalizeReaderBackgroundOpacity(opacity) * 100)
}

/** 界面上的百分比 → 存储值(0~1, 保留两位小数)。
 *
 *  百分比输入框允许用户键入非法内容(空串、字母、"abc%"), 因此非数字一律返回
 *  null 由调用方保持原值, 而不是回落到默认透明度 —— 那会让用户只是误敲一个
 *  字符, 背景就突然变成 35%。 */
export function percentToReaderOpacity(percent: unknown): number | null {
  // 空串与纯空白必须先挡掉: Number('') === 0, 否则用户清空输入框时背景会
  // 突然变成全透明, 而不是保持原值等用户重新输入。
  if (typeof percent === 'string' && !percent.trim()) return null
  if (percent === null || percent === undefined) return null
  const numeric = typeof percent === 'number' ? percent : Number(String(percent).trim())
  if (!Number.isFinite(numeric)) return null
  const clamped = Math.max(0, Math.min(100, numeric))
  return Math.round(clamped) / 100
}

export async function prepareReaderBackgroundImage(file: File) {
  if (!SUPPORTED_IMAGE_TYPES.has(file.type)) {
    throw new Error('请选择 JPG、PNG、WebP 或 BMP 图片')
  }
  if (file.size > MAX_SOURCE_BYTES) {
    throw new Error('背景图片不能超过 20 MB')
  }

  let bitmap: ImageBitmap
  try {
    bitmap = await createImageBitmap(file)
  } catch {
    throw new Error('无法读取该图片，请更换图片后重试')
  }
  try {
    if (!bitmap.width || !bitmap.height) {
      throw new Error('无法读取该图片')
    }

    const attempts = [
      { maxDimension: 2400, quality: 0.86 },
      { maxDimension: 1920, quality: 0.72 },
      { maxDimension: 1600, quality: 0.58 },
    ]
    for (const attempt of attempts) {
      const dataUrl = renderBackgroundImage(bitmap, attempt.maxDimension, attempt.quality)
      if (dataUrl.length <= MAX_STORED_LENGTH) return dataUrl
    }
  } finally {
    bitmap.close()
  }

  throw new Error('图片内容过大，请选择更简单或尺寸更小的图片')
}

function renderBackgroundImage(bitmap: ImageBitmap, maxDimension: number, quality: number) {
  const scale = Math.min(1, maxDimension / Math.max(bitmap.width, bitmap.height))
  const width = Math.max(1, Math.round(bitmap.width * scale))
  const height = Math.max(1, Math.round(bitmap.height * scale))
  const canvas = document.createElement('canvas')
  canvas.width = width
  canvas.height = height
  const context = canvas.getContext('2d')
  if (!context) throw new Error('当前环境无法处理图片')
  context.drawImage(bitmap, 0, 0, width, height)
  return canvas.toDataURL('image/webp', quality)
}
