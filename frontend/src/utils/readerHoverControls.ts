/**
 * 阅读页「鼠标靠近边缘自动展开菜单栏」的判定规则。
 *
 * 抽成纯函数是为了能单测: 热区、以及「面板/浮层打开时不该展开」这两条规则
 * 互相牵连, 放在视图里只能靠手点验证。
 */

/** 顶部/底部触发自动展开的边缘热区厚度(px)。 */
export const READER_HOVER_EDGE_PX = 60

/**
 * 左右边缘热区额外留出的余量(px)。
 *
 * 左右悬浮钮本身 40px 宽、贴边 16px 摆放, 指针要能停在按钮上而不滑出热区,
 * 所以横向热区比纵向宽一档。
 */
export const READER_HOVER_SIDE_EXTRA_PX = 24

/**
 * 指针离开热区后延迟多久收起(ms)。
 *
 * 取 0: 指针一离开就直接收起, 与 readest 的 HeaderBar/FooterBar 一致 ——
 * 它们的 onMouseLeave 都是立即隐藏、不留延迟(唯一的 100ms 是给 macOS 红绿灯
 * 按钮换目标时的过渡, 与菜单栏主体无关)。
 *
 * 之所以不需要"够到菜单栏"的余量: 指针若从正文划向菜单栏, 途中必然经过热区,
 * 在热区内的每次移动都会取消收起; 而停在菜单栏上时按 hoverPointerOnControls
 * 判定为"没走开"。因此这里留延迟只会让菜单栏在正文上多赖一会儿。
 */
export const READER_HOVER_HIDE_DELAY_MS = 0

export interface ReaderHoverPoint {
  x: number
  y: number
}

export interface ReaderHoverViewport {
  width: number
  height: number
}

export interface ReaderHoverBlockers {
  /** 阅读面板(目录/设置等抽屉)已打开: 菜单栏不该再被悬停弹出来。 */
  panelOpen: boolean
  /** 搜索面板/朗读面板/书详情等浮层已打开。 */
  overlayOpen: boolean
}

/**
 * 指针是否落在「靠近边缘」的热区内。
 *
 * 顶部/底部各 60px, 左右各 60+24px。宽高任一项无效(0 或非有限数)时判定为不在热区,
 * 避免窗口尚未布局完成时把菜单栏弹出来。
 */
export function isReaderHoverPointInZone(
  point: ReaderHoverPoint,
  viewport: ReaderHoverViewport,
): boolean {
  const { width, height } = viewport
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) return false
  const { x, y } = point
  if (!Number.isFinite(x) || !Number.isFinite(y)) return false

  const side = READER_HOVER_EDGE_PX + READER_HOVER_SIDE_EXTRA_PX
  return x <= side || x >= width - side || y <= READER_HOVER_EDGE_PX || y >= height - READER_HOVER_EDGE_PX
}

/**
 * 悬停是否可以把菜单栏展开。面板与浮层占用界面时不展开 —— 那时正文已经被
 * 抽屉/浮层挡住, 再弹一条菜单栏只是添乱。
 */
export function shouldRevealReaderControls(
  point: ReaderHoverPoint,
  viewport: ReaderHoverViewport,
  blockers: ReaderHoverBlockers,
): boolean {
  if (blockers.panelOpen || blockers.overlayOpen) return false
  return isReaderHoverPointInZone(point, viewport)
}
