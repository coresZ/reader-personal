/** 书源仓库「一键导入」链接的解析。
 *
 *  手机端 Legado 靠自定义协议接管这类链接(`OnLineImportActivity`), 格式是
 *  `{scheme}://{host}/{path}?src={json 地址}`:
 *
 *  | 链接                                              | 来源                     |
 *  |---------------------------------------------------|--------------------------|
 *  | `yuedu://booksource/importonline?src=...`          | yck 系源仓库的「3.0 一键导入」 |
 *  | `legado://import/booksource?src=...`               | Legado 自己的分享格式     |
 *  | `legado://import/auto?src=...`                     | 旧版通用导入(需按内容判类型) |
 *
 *  PC 端没有系统级协议注册, 这些链接由书源面板的桥脚本截获后交给本模块,
 *  这里只负责「取出可下载的 http(s) 地址」, 真正的拉取与入库由调用方完成。
 */

/** 可以接管导入的协议。 */
const IMPORT_PROTOCOLS = new Set(['legado:', 'yuedu:'])

export interface ImportLink {
  /** 原始链接(用于提示与排查)。 */
  raw: string
  /** 协议名(小写, 带冒号): `legado:` 或 `yuedu:`。 */
  protocol: string
  /** 待下载的 http(s) 地址; 链接里没有合法地址时为 null。 */
  sourceUrl: string | null
}

/** 解析导入链接; 不是导入协议时返回 null。
 *
 *  不抛异常: 这条路径由网页点击触发, 拼错的链接应该被安静忽略(或由调用方
 *  提示), 而不是让面板报错。
 */
export function parseImportLink(raw: string | null | undefined): ImportLink | null {
  if (!raw) return null
  let url: URL
  try {
    url = new URL(raw)
  } catch {
    return null
  }
  const protocol = url.protocol.toLowerCase()
  if (!IMPORT_PROTOCOLS.has(protocol)) return null

  // 手机端读的是 `src`; 少数源仓库用 `url`。两者都认, `src` 优先。
  const candidate = url.searchParams.get('src') || url.searchParams.get('url') || ''
  return {
    raw,
    protocol,
    sourceUrl: normalizeSourceUrl(candidate),
  }
}

/** 只接受 http(s): 导入地址来自第三方网页, 不能让它变成本地文件读取。 */
function normalizeSourceUrl(value: string): string | null {
  const trimmed = value.trim()
  if (!trimmed) return null
  try {
    const parsed = new URL(trimmed)
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return null
    return parsed.href
  } catch {
    return null
  }
}

/** 给用户看的失败原因。 */
export function describeImportLinkFailure(link: ImportLink): string {
  return link.sourceUrl
    ? ''
    : '这个导入链接里没有有效的书源地址，请回到源仓库页面点「一键导入」重新获取'
}
