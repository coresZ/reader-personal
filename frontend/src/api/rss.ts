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
  return post<{ first: RssArticle[]; second: null }>('/getRssArticles', params).then((r) => r.data)
}

export function getRssContent(params: {
  sourceUrl: string
  link: string
  origin: string
}) {
  return post<string>('/getRssContent', params).then((r) => r.data)
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
