/** 从书源面板里点「一键导入」链接到真正入库的完整流程。
 *
 *  为什么需要它: 源仓库的「3.0 一键导入」是 `yuedu://booksource/importonline?src=...`
 *  (Legado 是 `legado://import/...?src=...`)。手机端靠系统协议注册接管,
 *  PC 端没有这一层, 于是点下去要么交给系统浏览器(打不开)、要么毫无反应。
 *  现在由书源面板的桥脚本截获, 交给这里完成「解析链接 → 拉 JSON → 入库」。
 *
 *  拉取走后端的 `readRemoteSourceFile`: 直接在前端 fetch 会被源仓库的 CORS
 *  拦下, 而这个命令本来就在后端用 reqwest 抓, 顺带复用代理配置。
 */
import { useAppStore } from '../stores/app'
import { saveBookSources, readRemoteSourceFile } from '../api/source'
import type { BookSource } from '../types'
import { describeImportLinkFailure, parseImportLink } from '../utils/sourceImportLink'

export function useSourceImport() {
  const appStore = useAppStore()

  /** 处理一条导入链接。返回导入成功的书源数量; 失败返回 0 并已提示用户。 */
  async function importFromLink(raw: string): Promise<number> {
    const link = parseImportLink(raw)
    if (!link) return 0

    if (!link.sourceUrl) {
      appStore.showToast(describeImportLinkFailure(link), 'warning')
      return 0
    }

    try {
      const payload = await readRemoteSourceFile(link.sourceUrl)
      const sources = payload.flatMap((item) => {
        try {
          const value = JSON.parse(item)
          return Array.isArray(value) ? value : [value]
        } catch {
          return []
        }
      }) as BookSource[]

      if (!sources.length) {
        appStore.showToast('这个链接里没有可导入的书源', 'warning')
        return 0
      }

      await saveBookSources(sources)
      appStore.showToast(`成功导入 ${sources.length} 个书源`, 'success')
      return sources.length
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error)
      appStore.showToast(message || '书源导入失败', 'error')
      return 0
    }
  }

  return { importFromLink }
}
