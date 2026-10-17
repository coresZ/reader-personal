type TimerHandle = ReturnType<typeof setTimeout> | number
const KEEPALIVE_SETTLE_TIMEOUT_MS = 800

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
      routeFlushPromise = Promise.race([
        Promise.resolve(previous),
        new Promise<void>((resolve) => {
          setTimeout(resolve, KEEPALIVE_SETTLE_TIMEOUT_MS)
        }),
      ])
        .then(() => flushToServer())
        .then(() => {
          finalFlushCompleted = true
        })
        .then(() => flushToLegado?.())
        .finally(() => {
          routeFlushPromise = null
        })
        .catch(() => undefined)
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
