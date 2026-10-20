type TimerHandle = ReturnType<typeof setTimeout> | number
const KEEPALIVE_SETTLE_TIMEOUT_MS = 800

/**
 * 离开阅读页时等待进度写入的上限。
 *
 * 这段逻辑挂在路由守卫里, 守卫 resolve 之前页面不会真正切走 —— 任何一次后端慢请求
 * 都会变成「点了返回却还停在阅读页」。最坏的一种是目录缓存缺失时同步联网抓目录:
 * 单请求 15s 超时 × 3 次尝试, 分页目录还要逐页串行, 一分钟内都回不了书架。
 *
 * 进度写入只是尽力而为: 本地位置在 beginPersist 里已经同步落盘, 服务端那份晚到或
 * 丢失都能接受, 卡住导航不能接受。因此到点无条件放行。
 */
const ROUTE_LEAVE_TIMEOUT_MS = 1200

export interface ReaderProgressAutoSaveSchedulerOptions {
  intervalMs: number
  flush: () => Promise<void> | void
  setTimer?: (callback: () => void, delay: number) => TimerHandle
  clearTimer?: (timer: TimerHandle) => void
}

export interface ReaderProgressExitSaverOptions {
  disposeAutoSave: () => void
  savePosition: () => void
  flushToServer: () => Promise<void> | void
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

export function createReaderProgressExitSaver({
  disposeAutoSave,
  savePosition,
  flushToServer,
  flushToServerKeepalive,
  flushToLegado,
}: ReaderProgressExitSaverOptions) {
  let routeFlushPromise: Promise<void> | null = null
  let routeFlushed = false
  let keepaliveSent = false
  let autoSaveDisposed = false
  let keepaliveThenFinal: Promise<void> | null = null
  let finalFlushCompleted = false

  function beginPersist() {
    if (!autoSaveDisposed) {
      autoSaveDisposed = true
      disposeAutoSave()
    }
    savePosition()
    return true
  }

  return {
    async flushBeforeRouteLeave() {
      if (routeFlushPromise) return routeFlushPromise
      if (routeFlushed) {
        beginPersist()
        return
      }
      routeFlushed = true
      beginPersist()
      const previous = keepaliveThenFinal
      let settleTimer: TimerHandle | null = null
      const flush = Promise.race([
        Promise.resolve(previous),
        new Promise<void>((resolve) => {
          settleTimer = setTimeout(resolve, KEEPALIVE_SETTLE_TIMEOUT_MS)
        }),
      ])
        .then(() => flushToServer())
        .then(() => {
          finalFlushCompleted = true
        })
        .then(() => flushToLegado?.())
        .then(() => undefined)
        .catch(() => undefined)
        .finally(() => {
          if (settleTimer != null) clearTimeout(settleTimer as ReturnType<typeof setTimeout>)
        })
      // 守卫等的是这个 Promise: 到点必须放行, 否则后端慢一次就把用户锁在阅读页。
      // 未完成的 flush 仍会继续跑完, 只是不再阻塞导航。
      let routeLeaveTimer: TimerHandle | null = null
      routeFlushPromise = Promise.race([
        flush,
        new Promise<void>((resolve) => {
          routeLeaveTimer = setTimeout(resolve, ROUTE_LEAVE_TIMEOUT_MS)
        }),
      ]).then(() => {
        if (routeLeaveTimer != null) clearTimeout(routeLeaveTimer as ReturnType<typeof setTimeout>)
      }).finally(() => {
        routeFlushPromise = null
      })
      await routeFlushPromise
    },

    flushKeepalive() {
      beginPersist()
      if (keepaliveSent || finalFlushCompleted) return
      keepaliveSent = true
      keepaliveThenFinal = Promise.resolve(flushToServerKeepalive())
        .then(() => flushToLegado?.())
        .then(() => undefined)
    },

    flushTemporaryKeepalive() {
      savePosition()
      flushToServerKeepalive()
    },
  }
}
