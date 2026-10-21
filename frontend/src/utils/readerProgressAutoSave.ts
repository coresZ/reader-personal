type TimerHandle = ReturnType<typeof setTimeout> | number

export interface ReaderProgressAutoSaveSchedulerOptions {
  intervalMs: number
  flush: () => Promise<void> | void
  setTimer?: (callback: () => void, delay: number) => TimerHandle
  clearTimer?: (timer: TimerHandle) => void
}

export interface ReaderProgressExitSaverOptions {
  disposeAutoSave: () => void
  savePosition: () => void
  flushToServerKeepalive: () => Promise<void> | void
  flushToLegado?: () => Promise<void> | void
}

export function createReaderProgressAutoSaveScheduler({
  intervalMs,
  flush,
  setTimer = (callback, delay) => window.setTimeout(callback, delay),
  clearTimer = (timer) => window.clearTimeout(timer as ReturnType<typeof setTimeout>),
}: ReaderProgressAutoSaveSchedulerOptions) {
  let timer: TimerHandle | null = null
  let flushPromise: Promise<void> | null = null

  function clearScheduledFlush() {
    if (timer == null) return
    clearTimer(timer)
    timer = null
  }

  function runFlush() {
    if (flushPromise) return flushPromise
    const current = Promise.resolve(flush())
      .catch(() => undefined)
      .finally(() => {
        if (flushPromise === current) {
          flushPromise = null
        }
      })
    flushPromise = current
    return current
  }

  return {
    schedule() {
      if (timer != null) return
      timer = setTimer(() => {
        timer = null
        void runFlush()
      }, Math.max(0, intervalMs))
    },

    async flushNow() {
      clearScheduledFlush()
      await runFlush()
    },

    dispose() {
      clearScheduledFlush()
    },
  }
}

/**
 * 离开阅读页时的进度收尾。
 *
 * 关键约定: **导航不能被进度写入拖住**。
 *
 * 本地位置(`localStorage`)在 `beginPersist()` 里同步落盘, 这一步已经保证了
 * 「下次打开这本书落在原处」; 服务端与 WebDAV 那两份是尽力而为 —— 它们经 IPC /
 * 网络往返, 慢一次就是几百毫秒到几十秒。
 *
 * 旧实现让路由守卫 `await` 这条链, 并用 1200ms 上限兜底, 于是「退出到书架」
 * 的耗时随写入速度在 0~1.2s 之间抖动: 命中缓存时秒出, 后端慢一次就明显卡顿
 * (issue #19「有时候需要几秒钟才能退出来, 有时候也能秒出」)。守卫里还先等了
 * 最多 800ms 让 keepalive 落地, 才轮到那一次写入 —— 慢的时候两段叠加。
 *
 * 现在的做法: 本地位置同步写, 服务端/WebDAV 的两次写入发起后立刻放行导航。
 * 阅读页卸载后 store 仍常驻, IPC 与上传都会继续跑完; 真要立刻关窗时,
 * App.vue 的 close-requested 路径还会再 flush 一次。
 */
export function createReaderProgressExitSaver({
  disposeAutoSave,
  savePosition,
  flushToServerKeepalive,
  flushToLegado,
}: ReaderProgressExitSaverOptions) {
  let routeFlushed = false
  let autoSaveDisposed = false
  let keepaliveSent = false

  function beginPersist() {
    if (!autoSaveDisposed) {
      autoSaveDisposed = true
      disposeAutoSave()
    }
    savePosition()
  }

  /** 发起后台写入: 一次 keepalive 进度写入, 完成后再同步 WebDAV 进度。全程不等待。
   *
   *  keepalive 版本就是为"页面即将消失"准备的: 一次 IPC 往返即返回。这里仍然
   *  不 await —— 导航不该为它排队。`keepaliveSent` 负责去重: 同一批退出钩子
   *  (路由离开 / pagehide / 组件卸载)会连着触发, 只写一次就够。 */
  function startBackgroundFlush() {
    if (keepaliveSent) return
    keepaliveSent = true
    void Promise.resolve(flushToServerKeepalive())
      .then(() => flushToLegado?.())
      .catch(() => undefined)
  }

  return {
    /** 路由离开钩子: 本地位置同步落盘, 服务端/网盘写入发起后立即放行。
     *
     *  改动前这里 await 整条写入链(含 800ms keepalive 等待与 1200ms 上限),
     *  于是"退出到书架"的耗时随后端快慢在 0~1.2s 之间抖动 —— 见 issue #19。 */
    async flushBeforeRouteLeave() {
      if (routeFlushed) {
        beginPersist()
        return
      }
      routeFlushed = true
      beginPersist()
      startBackgroundFlush()
    },

    /** 页面隐藏/卸载: 本地落盘 + 一次 keepalive 写入(同样不阻塞)。 */
    flushKeepalive() {
      beginPersist()
      startBackgroundFlush()
    },

    /** 切到后台(可能马上被系统冻结): 落盘并发一次 keepalive, 不做更多。 */
    flushTemporaryKeepalive() {
      savePosition()
      void Promise.resolve(flushToServerKeepalive()).catch(() => undefined)
    },
  }
}
