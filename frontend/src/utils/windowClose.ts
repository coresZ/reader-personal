export interface WindowCloseTarget {
  hide: () => Promise<void>
  destroy: () => Promise<void>
}

/** 关闭窗口前等待前端保存的预算。
 *
 *  给足时间让一次 IPC 往返落地, 又不至于让"点了没反应"变成明显的卡顿。 */
const CLOSE_PERSIST_TIMEOUT_MS = 1500

/** 关窗前的收尾动作(保存阅读进度等)。
 *
 *  必须在这里做, 不能依赖 `beforeunload` 或 Vue 的 `onUnmounted`:
 *  `destroy()` 会立即销毁 WebView, 两者都不保证执行, 正在进行的异步 IPC 也会
 *  被一起终止 —— 表现就是"在阅读页直接关窗口, 下次打开回到章节开头"。
 *
 *  与 legado 的 `ReadBookActivity.onPause` 同一语义: 关闭/切走的那一刻把当前位置
 *  同步落盘, 而不是等下一次翻页或自动保存周期。 */
export type WindowClosePersist = () => Promise<void> | void

/** 告诉外壳"这一轮关闭请求已处理完"。
 *
 *  外壳在拦截关闭后会启动看门狗: 宽限期内收不到回执就直接销毁窗口, 免得
 *  `close-requested` 没人处理时窗口关不掉。
 *
 *  回执放在 hide/destroy **之后**: 若窗口动作抛错, 看门狗仍会接管并结束窗口,
 *  不会出现"回执已发、窗口没关、看门狗又退让"的僵局。宽限期(3s)远大于保存
 *  上限(1.5s)加一次 IPC 往返, 顺序放在后面仍在预算内。
 *
 *  回执失败不影响关闭: 它只决定看门狗是否退让, 不决定窗口能否关闭。 */
export type WindowCloseAcknowledge = () => Promise<void> | void

export async function resolveWindowClose(
  target: WindowCloseTarget,
  closeToTray: boolean,
  persist?: WindowClosePersist,
  acknowledge?: WindowCloseAcknowledge,
) {
  if (persist) {
    // 失败不能阻断关闭: 保存不上是遗憾, 关不掉窗口是事故。
    await Promise.race([
      Promise.resolve()
        .then(() => persist())
        .catch(() => undefined),
      new Promise<void>((resolve) => {
        setTimeout(resolve, CLOSE_PERSIST_TIMEOUT_MS)
      }),
    ])
  }
  if (closeToTray) {
    await target.hide()
  } else {
    await target.destroy()
  }
  if (acknowledge) {
    try {
      await acknowledge()
    } catch {
      // 走到这里窗口已隐藏/销毁, 关闭本身已完成; 回执只影响看门狗是否退让。
    }
  }
}
