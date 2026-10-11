/** 左右分页的**正文行宽**计算。
 *
 *  两个宽度必须分开, 这是这个文件的全部意义:
 *  - **屏宽**(= 容器宽, JS 里的 `horizontalPageStep`)：一页容器与翻页位移的宽度。
 *    一屏恰好一页, 所以宽窗口下不会露出下一页的半页内容。
 *  - **行宽**(= 设置里的「页面宽度」)：一页内正文列的宽度。窗口比它宽时正文居中、
 *    两侧留白; 比它窄时按可用宽度收敛, 避免正文溢出被裁。
 *
 *  早期实现把两者混为一谈(屏宽 = min(容器宽, 页面宽度)), 于是 1920 宽窗口里每页只有
 *  800px, 一屏并排露出当前页与下一页左半 —— 用户只能把窗口拖窄来回避。
 */

/** 一页内正文列的宽度。
 *
 *  「页面宽度」是行宽(上限), 所以取**可用宽度**与它的较小者:
 *  窗口更宽 → 用设置值(正文居中留白); 窗口更窄 → 用可用宽度(正文铺满不溢出)。
 *
 *  `availableWidth` 应当是页容器扣掉左右 padding 之后的宽度 —— 与正文测量和实际
 *  渲染用的必须是同一个值, 否则测量出的分页与实际排版会不一致。 */
export function getReaderTextWidth(availableWidth: number, pageWidth: number): number {
  const safeAvailable = Number.isFinite(availableWidth) && availableWidth > 0
    ? availableWidth
    : 1
  if (!Number.isFinite(pageWidth) || pageWidth <= 0) return safeAvailable
  return Math.min(safeAvailable, pageWidth)
}
