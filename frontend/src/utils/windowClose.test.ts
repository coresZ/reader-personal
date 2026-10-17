import { describe, expect, it, vi } from 'vitest'
import { resolveWindowClose } from './windowClose'

function createTarget() {
  return {
    hide: vi.fn(async () => undefined),
    destroy: vi.fn(async () => undefined),
  }
}

describe('window close behavior', () => {
  it('hides the window when close-to-tray is enabled', async () => {
    const target = createTarget()

    await resolveWindowClose(target, true)

    expect(target.hide).toHaveBeenCalledOnce()
    expect(target.destroy).not.toHaveBeenCalled()
  })

  it('destroys the window when close-to-tray is disabled', async () => {
    const target = createTarget()

    await resolveWindowClose(target, false)

    expect(target.destroy).toHaveBeenCalledOnce()
    expect(target.hide).not.toHaveBeenCalled()
  })

  /** 保存必须发生在销毁窗口**之前**。
   *
   *  `destroy()` 会立即销毁 WebView, 之后发出的 IPC 一律丢失 —— 这正是
   *  "在阅读页直接关窗口, 下次打开回到章节开头"的根因。 */
  it('persists reading progress before destroying the window', async () => {
    const calls: string[] = []
    const target = {
      hide: vi.fn(async () => {
        calls.push('hide')
      }),
      destroy: vi.fn(async () => {
        calls.push('destroy')
      }),
    }
    const persist = vi.fn(async () => {
      calls.push('persist')
    })

    await resolveWindowClose(target, false, persist)

    expect(persist).toHaveBeenCalledOnce()
    expect(calls).toEqual(['persist', 'destroy'])
  })

  /** 隐藏到托盘同样算"离开阅读页", 保存时机一并前置。 */
  it('persists before hiding to the tray', async () => {
    const calls: string[] = []
    const target = {
      hide: vi.fn(async () => {
        calls.push('hide')
      }),
      destroy: vi.fn(async () => undefined),
    }
    const persist = vi.fn(async () => {
      calls.push('persist')
    })

    await resolveWindowClose(target, true, persist)

    expect(calls).toEqual(['persist', 'hide'])
  })

  /** 保存失败不能把窗口变成关不掉 —— 那是比丢进度更严重的事故。 */
  it('still closes the window when persisting fails', async () => {
    const target = createTarget()
    const persist = vi.fn(async () => {
      throw new Error('ipc failed')
    })

    await resolveWindowClose(target, false, persist)

    expect(persist).toHaveBeenCalledOnce()
    expect(target.destroy).toHaveBeenCalledOnce()
  })

  /** 回执必须晚于窗口动作: 若 hide/destroy 抛错, 看门狗仍可接管结束窗口,
   *  不会出现"回执已发、窗口没关、看门狗又退让"的僵局。 */
  it('acknowledges the close request after the window is gone', async () => {
    const calls: string[] = []
    const target = {
      hide: vi.fn(async () => {
        calls.push('hide')
      }),
      destroy: vi.fn(async () => {
        calls.push('destroy')
      }),
    }
    const persist = vi.fn(async () => {
      calls.push('persist')
    })
    const acknowledge = vi.fn(async () => {
      calls.push('ack')
    })

    await resolveWindowClose(target, false, persist, acknowledge)
    expect(calls).toEqual(['persist', 'destroy', 'ack'])

    calls.length = 0
    await resolveWindowClose(target, true, persist, acknowledge)
    expect(calls).toEqual(['persist', 'hide', 'ack'])
  })

  /** 窗口动作失败时不能先发回执: 看门狗必须还能接管并结束窗口。 */
  it('does not acknowledge when hiding the window fails', async () => {
    const target = {
      hide: vi.fn(async () => {
        throw new Error('hide failed')
      }),
      destroy: vi.fn(async () => undefined),
    }
    const acknowledge = vi.fn(async () => undefined)

    await expect(resolveWindowClose(target, true, undefined, acknowledge)).rejects.toThrow(
      'hide failed',
    )
    expect(acknowledge).not.toHaveBeenCalled()
  })

  /** 回执失败不能阻断关闭, 也不能回滚已完成的窗口动作。 */
  it('still closes the window when the acknowledgement fails', async () => {
    const target = createTarget()
    const acknowledge = vi.fn(async () => {
      throw new Error('ipc failed')
    })

    await resolveWindowClose(target, false, undefined, acknowledge)

    expect(acknowledge).toHaveBeenCalledOnce()
    expect(target.destroy).toHaveBeenCalledOnce()
  })
})
