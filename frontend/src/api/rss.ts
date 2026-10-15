import { get, post, invokeUpload } from './invoke'
import { FILE_NAME_HEADER, encodeFileName } from './uploadHeader'
import type { RssArticle, RssSource } from '../types'

export function getRssSources() {
  return get<RssSource[]>('/getRssSources').then((r) => r.data)
}

export function saveRssSource(source: RssSource) {
  return post<string>('/saveRssSource', source).then((r) => r.data)
}

export function saveRssSources(sources: RssSource[]) {
  return post<string>('/saveRssSources', sources).then((r) => r.data)
}

export function deleteRssSource(source: Pick<RssSource, 'sourceUrl' | 'sourceName'>) {
  return post<string>('/deleteRssSource', source).then((r) => r.data)
}

export function deleteRssSources(sources: Pick<RssSource, 'sourceUrl' | 'sourceName'>[]) {
  return post<{ deleted: number }>('/deleteRssSources', sources).then((r) => r.data)
}

export function getRssArticles(params: {
  sourceUrl: string
  sortName?: string
  sortUrl?: string
  page?: number
}) {
  return post<{
    first: RssArticle[]
    second: null
    /** `singleUrl` 源: 源地址本身就是"一篇文章", 点开即打开网页(legado 语义)。 */
    singleUrl?: boolean
    /** 该地址是否由源里的 JS 表达式求值得到(用于提示"已自动切换到可用节点")。 */
    urlEvaluated?: boolean
  }>('/getRssArticles', params).then((r) => r.data)
}

export function getRssContent(params: {
  sourceUrl: string
  link: string
  origin: string
}) {
  return post<string>('/getRssContent', params).then((r) => r.data)
}

/** `singleUrl` 源的网页面板信息。 */
export interface RssSourcePanel {
  url: string
  title?: string
  /** 后端托管的面板 id; `external` 为真时没有面板。 */
  panelId?: string
  /** 自定义 scheme(如 `app-scheme://`): 需要交给系统处理, 不走内嵌面板。 */
  external?: boolean
}

/**
 * 为 RSS 源打开网页: 单 URL 源的源地址本身就是"一篇文章"。
 * 后端先抓页面再登记面板, 因此不受"面板必须归属书源"的限制。
 */
export function openRssSourcePanel(params: { sourceUrl: string; url?: string }) {
  return post<RssSourcePanel>('/reader3/openRssSourcePanel', params).then((r) => r.data)
}

export function readRemoteRssSourceFile(url: string) {
  return post<string[]>('/readRemoteRssSourceFile', { url }).then((r) => r.data)
}

/** 读取本地 RSS 源 JSON。字节走 octet-stream 快速通道, 文件名走 IPC header。 */
export async function readRssSourceFile(file: File) {
  return invokeUpload<RssSource[]>(
    'read_rss_source_file',
    { [FILE_NAME_HEADER]: encodeFileName(file.name) },
    file,
  )
}
