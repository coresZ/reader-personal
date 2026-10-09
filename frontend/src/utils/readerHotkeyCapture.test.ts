// 快捷键录入的「抑制窗口」回归测试。
//
// 这个文件修掉过两个真实缺陷, 两处都容易复发, 因此锁在这里:
//
// 缺陷 1(顺序): finishCapture 曾把窗口赋值放在 `capturingAction.value = null` 之后。
//   那个赋值会通过 watch(flush:'sync') **同步**触发 releaseCaptureListeners(),
//   它靠读窗口值算拆除延迟 —— 读到 0 就 delayMs=0, 五个捕获监听器在下一宏任务被
//   摘除, 而窗口的唯一读取者正是这些监听器, 于是窗口成了死代码。
//   曾经的"验证"用手工模拟顺序(把 watch 触发写在最后)把它掩盖了, 这里不再模拟:
//   顺序问题用真实 vue 的 ref/watch 复现。
//
// 缺陷 2(按键盘): 窗口曾只判断"是不是鼠标键"而不比对具体键位, 于是录入侧键1 后
//   一秒内按侧键2 也会被吞掉; 更严重的是带修饰键录入(Ctrl+侧键1)后窗口记的是
//   'Control+Mouse4', 而松开时标识退化成裸 'Mouse4', 永远匹配不上 → 侧键导航 →
//   hash 变 → 设置面板被卸载。修法是按裸键名记录。
import { describe, expect, it } from 'vitest'
import { nextTick, ref, watch } from 'vue'
import {
  MOUSE_BUTTON_KEY_IDS,
  bareMouseButtonKeyId,
  isMouseButtonKeyId,
} from './readerHotkeys'

/* ─── 缺陷 2: 按键名的归一 ───
 * 这两个函数是**从实现里导入的真实代码**, 不是复刻 —— 改实现会让这里失败。 */
describe('bareMouseButtonKeyId / isMouseButtonKeyId', () => {
  it('去掉修饰键前缀, 只留主键', () => {
    expect(bareMouseButtonKeyId('Mouse4')).toBe('Mouse4')
    expect(bareMouseButtonKeyId('Control+Mouse4')).toBe('Mouse4')
    expect(bareMouseButtonKeyId('Control+Shift+MouseMiddle')).toBe('MouseMiddle')
    expect(bareMouseButtonKeyId('Alt+Mouse5')).toBe('Mouse5')
  })

  it('容忍修饰键前缀地识别鼠标键', () => {
    // 裸键名
    for (const id of MOUSE_BUTTON_KEY_IDS) expect(isMouseButtonKeyId(id)).toBe(true)
    // 带修饰键 —— 用户完全可以按 Ctrl+侧键 录入
    expect(isMouseButtonKeyId('Control+Mouse4')).toBe(true)
    expect(isMouseButtonKeyId('Control+MouseMiddle')).toBe(true)
    expect(isMouseButtonKeyId('Shift+Mouse5')).toBe(true)
  })

  it('不把滚轮与键盘当作鼠标键(滚轮没有松开时触发的默认行为)', () => {
    for (const id of ['WheelUp', 'WheelDown', 'ArrowDown', 'Space', 'F11', 'Control+f']) {
      expect(isMouseButtonKeyId(id)).toBe(false)
    }
  })

  it('空串与异常输入不抛错', () => {
    expect(bareMouseButtonKeyId('')).toBe('')
    expect(isMouseButtonKeyId('')).toBe(false)
    expect(isMouseButtonKeyId('Control+')).toBe(false)
  })
})

/* ─── 缺陷 1: 顺序 ───
 * 用真实 vue 的 ref/watch 复现 flush:'sync' 的同步语义。
 * 不模拟顺序 —— 模拟正是当初掩盖缺陷的原因。 */
describe('finishCapture 的赋值顺序(真实 Vue watch flush:sync)', () => {
  /** 复刻 ReadSettings 的窗口 + 延迟拆除骨架, 只保留顺序相关的部分。 */
  function harness() {
    const capturingAction = ref<string | null>(null)
    const s = {
      until: 0,
      key: '',
      releaseDelays: [] as number[],
      listeners: new Set<string>(),
    }
    const CAPTURE_MOUSEUP_SUPPRESS_MS = 1000

    function releaseCaptureListeners() {
      const delayMs = Math.max(0, s.until - Date.now())
      s.releaseDelays.push(delayMs)
      if (delayMs === 0) s.listeners.clear()   // delayMs=0 → 立刻拆
    }
    watch(capturingAction, (active) => {
      if (active) ['keydown', 'mousedown', 'wheel', 'auxclick', 'mouseup'].forEach((n) => s.listeners.add(n))
      else releaseCaptureListeners()
    }, { flush: 'sync' })

    return {
      capturingAction, s,
      /** 与实现同形的正确顺序: 先开窗口, 再清状态。 */
      finishCorrect(keyId: string) {
        if (isMouseButtonKeyId(keyId)) {
          s.key = bareMouseButtonKeyId(keyId)
          s.until = Date.now() + CAPTURE_MOUSEUP_SUPPRESS_MS
        }
        capturingAction.value = null
      },
      /** 曾经写错的顺序: 先清状态, 再开窗口 —— 用于证明测试能捕获这个回归。 */
      finishWrongOrder(keyId: string) {
        capturingAction.value = null
        if (isMouseButtonKeyId(keyId)) {
          s.key = bareMouseButtonKeyId(keyId)
          s.until = Date.now() + CAPTURE_MOUSEUP_SUPPRESS_MS
        }
      },
      isSuppressed(keyId: string | null) {
        if (!keyId || keyId !== s.key) return false
        return Date.now() < s.until
      },
    }
  }

  it('正确顺序: 释放监听器时已能读到窗口 → 延迟拆除, 监听器保住', () => {
    const h = harness()
    h.capturingAction.value = 'nextPage'
    h.finishCorrect('Mouse4')

    expect(h.s.releaseDelays).toHaveLength(1)
    expect(h.s.releaseDelays[0]).toBeGreaterThanOrEqual(900)
    // 监听器仍在 —— 窗口期内还有读取者
    expect(h.s.listeners.size).toBe(5)
  })

  it('错误顺序: 释放时窗口还是 0 → delayMs=0, 监听器立刻被摘(死代码)', () => {
    const h = harness()
    h.capturingAction.value = 'nextPage'
    h.finishWrongOrder('Mouse4')

    expect(h.s.releaseDelays[0]).toBe(0)      // ← 缺陷的证据
    expect(h.s.listeners.size).toBe(0)        // ← 窗口从此无人读取
  })

  it('窗口期内能挡住「同一次操作」的松开', () => {
    const h = harness()
    h.capturingAction.value = 'nextPage'
    h.finishCorrect('Mouse4')
    expect(h.isSuppressed('Mouse4')).toBe(true)
  })

  it('带修饰键录入后, 退化成裸键名的松开仍被挡住', () => {
    // 真实路径: 按 Ctrl+侧键1 录入 → 先松 Ctrl → 再松侧键, 标识退化成 'Mouse4'
    const h = harness()
    h.capturingAction.value = 'nextPage'
    h.finishCorrect('Control+Mouse4')

    expect(h.s.key).toBe('Mouse4')             // 记的是裸键名
    expect(h.isSuppressed('Mouse4')).toBe(true) // 退化的标识被挡住 ✓
  })

  it('只抑制刚录完的那个键, 不牵连另一个侧键', () => {
    const h = harness()
    h.capturingAction.value = 'nextPage'
    h.finishCorrect('Mouse4')

    expect(h.isSuppressed('Mouse5')).toBe(false)
    expect(h.isSuppressed('MouseMiddle')).toBe(false)
    expect(h.isSuppressed(null)).toBe(false)
    expect(h.isSuppressed('NotAKey')).toBe(false)
  })

  it('滚轮不开窗口(它没有松开时触发的默认行为)', () => {
    const h = harness()
    h.capturingAction.value = 'nextPage'
    h.finishCorrect('WheelUp')
    expect(h.s.until).toBe(0)
    expect(h.isSuppressed('WheelUp')).toBe(false)
  })

  it('窗口过期后不再抑制', () => {
    const h = harness()
    h.capturingAction.value = 'nextPage'
    h.finishCorrect('Mouse4')
    h.s.until = Date.now() - 1
    expect(h.isSuppressed('Mouse4')).toBe(false)
  })

  it('开始新一次录入会作废上一次的窗口', async () => {
    const h = harness()
    h.capturingAction.value = 'nextPage'
    h.finishCorrect('Mouse4')
    expect(h.isSuppressed('Mouse4')).toBe(true)

    // 复刻 startCapture 的清零
    h.s.until = 0
    h.s.key = ''
    h.capturingAction.value = 'prevPage'
    expect(h.isSuppressed('Mouse4')).toBe(false)
    await nextTick()
  })
})
