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

  it('saves local position on every exit hook but writes to the server once', async () => {
    const disposeAutoSave = vi.fn()
    const savePosition = vi.fn()
    const flushToServerKeepalive = vi.fn()
    const flushToLegado = vi.fn().mockResolvedValue(undefined)
    const saver = createReaderProgressExitSaver({
      disposeAutoSave,
      savePosition,
      flushToServerKeepalive,
      flushToLegado,
    })

    await saver.flushBeforeRouteLeave()
    await saver.flushBeforeRouteLeave()
    await saver.flushKeepalive()

    expect(disposeAutoSave).toHaveBeenCalledTimes(1)
    expect(savePosition).toHaveBeenCalledTimes(3)
    expect(flushToServerKeepalive).toHaveBeenCalledTimes(1)
    await vi.waitFor(() => expect(flushToLegado).toHaveBeenCalledTimes(1))
  })

  /** issue #19: 退出到书架的耗时不能随后端快慢抖动。
   *
   *  旧实现让路由守卫 await 整条写入链(800ms keepalive 等待 + 1200ms 上限),
   *  后端一回慢就卡住导航。这里用一个**永不 settle** 的写入把这条约束钉死:
   *  守卫必须在本地落盘后立刻放行。 */
  it('never blocks route leave on a stalled progress write', async () => {
    const flushToServerKeepalive = vi.fn().mockReturnValue(new Promise<void>(() => {}))
    const savePosition = vi.fn()
    const saver = createReaderProgressExitSaver({
      disposeAutoSave: vi.fn(),
      savePosition,
      flushToServerKeepalive,
    })

    let settled = false
    const navigation = saver.flushBeforeRouteLeave().then(() => {
      settled = true
    })

    // 本地位置同步落盘: 这一步才是"下次打开落在原处"的保证。
    expect(savePosition).toHaveBeenCalledTimes(1)
    await vi.waitFor(() => expect(settled).toBe(true))
    await navigation

    expect(flushToServerKeepalive).toHaveBeenCalledTimes(1)
  })

  it('starts the background write without awaiting it', async () => {
    // 用 definite assignment: Promise executor 是同步执行的, 走到断言时一定已赋值。
    let resolveKeepalive!: () => void
    const flushToLegado = vi.fn().mockResolvedValue(undefined)
    const flushToServerKeepalive = vi.fn().mockReturnValue(
      new Promise<void>((resolve) => {
        resolveKeepalive = resolve
      }),
    )
    const saver = createReaderProgressExitSaver({
      disposeAutoSave: vi.fn(),
      savePosition: vi.fn(),
      flushToServerKeepalive,
      flushToLegado,
    })

    await saver.flushBeforeRouteLeave()
    expect(flushToServerKeepalive).toHaveBeenCalledTimes(1)
    // 导航已放行, 但写入链还没跑完 —— WebDAV 同步排在它后面。
    expect(flushToLegado).not.toHaveBeenCalled()

    resolveKeepalive()
    await vi.waitFor(() => expect(flushToLegado).toHaveBeenCalledTimes(1))
  })

  it('re-saves local position but sends only one keepalive when exit hooks overlap', async () => {
    const savePosition = vi.fn()
    const flushToServerKeepalive = vi.fn()
    const saver = createReaderProgressExitSaver({
      disposeAutoSave: vi.fn(),
      savePosition,
      flushToServerKeepalive,
    })

    await saver.flushBeforeRouteLeave()
    await saver.flushKeepalive()
    await Promise.resolve()

    expect(savePosition).toHaveBeenCalledTimes(2)
    expect(flushToServerKeepalive).toHaveBeenCalledTimes(1)
  })

  it('does not send a second keepalive after the route leave already started one', async () => {
    const flushToServerKeepalive = vi.fn()
    const saver = createReaderProgressExitSaver({
      disposeAutoSave: vi.fn(),
      savePosition: vi.fn(),
      flushToServerKeepalive,
    })

    await saver.flushBeforeRouteLeave()
    saver.flushKeepalive()
    await Promise.resolve()

    expect(flushToServerKeepalive).toHaveBeenCalledTimes(1)
  })

  it('writes again on exit after a temporary background keepalive', async () => {
    const savePosition = vi.fn()
    const flushToServerKeepalive = vi.fn()
    const saver = createReaderProgressExitSaver({
      disposeAutoSave: vi.fn(),
      savePosition,
      flushToServerKeepalive,
    })

    saver.flushTemporaryKeepalive()
    await saver.flushBeforeRouteLeave()

    expect(savePosition).toHaveBeenCalledTimes(2)
    // 两次写入是**故意**的: 切后台那次是当时的进度, 离开时可能已经又读了很久,
    // 不能因为后台写过一次就跳过退出这一步。
    expect(flushToServerKeepalive).toHaveBeenCalledTimes(2)
  })
})
