import { readerOrigin } from '../api/scheme'

/**
 * 后端为本地 EPUB 生成的资源地址前缀。
 *
 * 真正的资源由 `reader://` 自定义协议提供(`${readerOrigin}/epub?...`), 而章节
 * HTML 里写的是这个后端路径, 前端渲染前必须改写, 否则图片、SVG 封面和 CSS
 * 背景图都会 404。
 */
export const LOCAL_EPUB_ASSET_PREFIX = '/reader3/localEpubAsset'

/** 把后端资源地址改写成自定义协议地址; 其他地址原样返回。 */
export function toReaderAssetUrl(value: string) {
  if (!value.startsWith(LOCAL_EPUB_ASSET_PREFIX)) return value
  const query = value.includes('?') ? value.slice(value.indexOf('?')) : ''
  return `${readerOrigin}/epub${query}`
}

/** 内联 `<style>` 里的 `url(...)` 同样指向资源接口, 需要整体改写。 */
export function rewriteCssAssetUrls(css: string) {
  if (!css.includes(LOCAL_EPUB_ASSET_PREFIX)) return css
  return css.split(LOCAL_EPUB_ASSET_PREFIX).join(`${readerOrigin}/epub`)
}
