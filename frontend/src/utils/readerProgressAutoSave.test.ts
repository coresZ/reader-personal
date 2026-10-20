import { describe, expect, it, vi } from 'vitest'
import { createReaderProgressAutoSaveScheduler, createReaderProgressExitSaver } from './readerProgressAutoSave'

describe('readerProgressAutoSave', () => {
  it('coalesces repeated schedule calls into one timed flush', () => {
    const timers = new Map<number, () => void>()
    let nextTimerId = 1
    const flush = vi.fn()

    const scheduler = createReaderProgressAutoSaveScheduler({
      intervalMs: 10000,
      flush,
      setTimer: (callback) => {
        const id = nextTimerId++
        timers.set(id, () => {
          timers.delete(id)
          callback()
        })
        return id
      },
      clearTimer: (id) => {
        timers.delete(id as number)
      },
    })

    scheduler.schedule()
    scheduler.schedule()

    expect(timers.size).toBe(1)
    timers.get(1)?.()

    expect(flush).toHaveBeenCalledTimes(1)
    expect(timers.size).toBe(0)
  })

  it('flushes immediately before navigation and clears a pending timer', async () => {
    const timers = new Map<number, () => void>()
    let nextTimerId = 1
    const flush = vi.fn()

    const scheduler = createReaderProgressAutoSaveScheduler({
      intervalMs: 10000,
      flush,
      setTimer: (callback) => {
        const id = nextTimerId++
        timers.set(id, callback)
        return id
      },
      clearTimer: (id) => {
        timers.delete(id as number)
      },
    })

    scheduler.schedule()
    await scheduler.flushNow()

    expect(timers.size).toBe(0)
    expect(flush).toHaveBeenCalledTimes(1)
  })

  it('saves local position on every exit hook but blocks server flush once', async () => {
    const disposeAutoSave = vi.fn()
    const savePosition = vi.fn()
    const flushToServer = vi.fn()
    const flushToServerKeepalive = vi.fn()
    const flushToLegado = vi.fn().mockResolvedValue(undefined)
    const saver = createReaderProgressExitSaver({
      disposeAutoSave,
      savePosition,
      flushToServer,
      flushToServerKeepalive,
      flushToLegado,
    })

    await saver.flushBeforeRouteLeave()
    await saver.flushBeforeRouteLeave()
    saver.flushKeepalive()

    expect(disposeAutoSave).toHaveBeenCalledTimes(1)
    expect(savePosition).toHaveBeenCalledTimes(3)
    expect(flushToServer).toHaveBeenCalledTimes(1)
    expect(flushToServerKeepalive).not.toHaveBeenCalled()
    expect(flushToLegado).toHaveBeenCalledTimes(1)
  })

  it('re-saves local position but sends only one keepalive when exit hooks overlap', async () => {
    const disposeAutoSave = vi.fn()
    const savePosition = vi.fn()
    const flushToServer = vi.fn()
    const flushToServerKeepalive = vi.fn()
    const saver = createReaderProgressExitSaver({
      disposeAutoSave,
      savePosition,
      flushToServer,
      flushToServerKeepalive,
    })

    saver.flushKeepalive()
    saver.flushKeepalive()
    await Promise.resolve()

    expect(disposeAutoSave).toHaveBeenCalledTimes(1)
    expect(savePosition).toHaveBeenCalledTimes(2)
    expect(flushToServer).not.toHaveBeenCalled()
    expect(flushToServerKeepalive).toHaveBeenCalledTimes(1)
  })

  it('runs the final blocking flush after a gesture navigation keepalive', async () => {
    const disposeAutoSave = vi.fn()
    const savePosition = vi.fn()
    const flushToServer = vi.fn()
    const flushToServerKeepalive = vi.fn()
    const saver = createReaderProgressExitSaver({
      disposeAutoSave,
      savePosition,
      flushToServer,
      flushToServerKeepalive,
    })

    saver.flushKeepalive()
    await saver.flushBeforeRouteLeave()

    expect(disposeAutoSave).toHaveBeenCalledTimes(1)
    expect(savePosition).toHaveBeenCalledTimes(2)
    expect(flushToServer).toHaveBeenCalledTimes(1)
    expect(flushToServerKeepalive).toHaveBeenCalledTimes(1)
  })

  it('allows a final navigation save after temporary background keepalive', async () => {
    const disposeAutoSave = vi.fn()
    const savePosition = vi.fn()
    const flushToServer = vi.fn()
    const flushToServerKeepalive = vi.fn()
    const saver = createReaderProgressExitSaver({
      disposeAutoSave,
      savePosition,
      flushToServer,
      flushToServerKeepalive,
    })

    saver.flushTemporaryKeepalive()
    await saver.flushBeforeRouteLeave()

    expect(disposeAutoSave).toHaveBeenCalledTimes(1)
    expect(savePosition).toHaveBeenCalledTimes(2)
    expect(flushToServerKeepalive).toHaveBeenCalledTimes(1)
    expect(flushToServer).toHaveBeenCalledTimes(1)
  })

  it('unsticks navigation when the progress write never settles', async () => {
    vi.useFakeTimers()
    try {
      const disposeAutoSave = vi.fn()
      const savePosition = vi.fn()
      // 后端卡死(例如目录缓存缺失时同步联网抓目录): 这个 Promise 永远不 resolve。
      const flushToServer = vi.fn().mockReturnValue(new Promise<void>(() => {}))
      const flushToServerKeepalive = vi.fn()
      const saver = createReaderProgressExitSaver({
        disposeAutoSave,
        savePosition,
        flushToServer,
        flushToServerKeepalive,
      })

      let settled = false
      const navigation = saver.flushBeforeRouteLeave().then(() => {
        settled = true
      })

      // 本地位置必须已经落盘, 即使服务端那份还没写完。
      expect(savePosition).toHaveBeenCalledTimes(1)
      expect(settled).toBe(false)

      await vi.advanceTimersByTimeAsync(1200)
      await navigation

      expect(settled).toBe(true)
      expect(flushToServer).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('does not wait for the route timeout when the flush settles quickly', async () => {
    vi.useFakeTimers()
    try {
      const flushToServer = vi.fn().mockResolvedValue(undefined)
      const saver = createReaderProgressExitSaver({
        disposeAutoSave: vi.fn(),
        savePosition: vi.fn(),
        flushToServer,
        flushToServerKeepalive: vi.fn(),
      })

      let settled = false
      const navigation = saver.flushBeforeRouteLeave().then(() => {
        settled = true
      })

      await vi.advanceTimersByTimeAsync(0)
      await navigation

      expect(settled).toBe(true)
      expect(flushToServer).toHaveBeenCalledTimes(1)
      // 计时器要清掉, 不能留一个 1.2s 的悬挂定时器。
      expect(vi.getTimerCount()).toBe(0)
    } finally {
      vi.useRealTimers()
    }
  })
})
