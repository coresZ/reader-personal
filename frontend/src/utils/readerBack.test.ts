import { describe, expect, it } from 'vitest'
import { resolveReaderBackTarget, type ReaderBackLayers } from './readerBack'

const idle: ReaderBackLayers = {
  panelOpen: false,
  selectionMenuVisible: false,
  searchOpen: false,
  ttsOpen: false,
  bookInfoOpen: false,
}

describe('resolveReaderBackTarget', () => {
  it('leaves the reader when nothing is open', () => {
    expect(resolveReaderBackTarget(idle)).toBe('leave')
  })

  it('closes the drawer before anything else', () => {
    expect(resolveReaderBackTarget({ ...idle, panelOpen: true, searchOpen: true })).toBe('panel')
  })

  it('walks the overlay cascade in order', () => {
    expect(resolveReaderBackTarget({ ...idle, selectionMenuVisible: true, searchOpen: true }))
      .toBe('selectionMenu')
    expect(resolveReaderBackTarget({ ...idle, searchOpen: true, ttsOpen: true })).toBe('search')
    expect(resolveReaderBackTarget({ ...idle, ttsOpen: true, bookInfoOpen: true })).toBe('tts')
    expect(resolveReaderBackTarget({ ...idle, bookInfoOpen: true })).toBe('bookInfo')
  })

  /** issue #19: 用户按 Esc 期望一次回书架。
   *
   *  旧实现把工具栏也算一层, 于是第一次 Esc 只收起菜单栏("第一次会先隐藏顶层
   *  菜单栏"), 第二次才退出。工具栏与正文同层, 不该占用「返回」。 */
  it('never spends a back press on the reader toolbar', () => {
    // 工具栏可见时(悬停展开或点击呼出)仍然直接退出: 它根本不是入参的一部分。
    expect(resolveReaderBackTarget({
      ...idle,
      // 旧实现会读这个字段; 现在多传了也不影响结果。
      ...({ controlsVisible: true } as unknown as ReaderBackLayers),
    })).toBe('leave')
  })

  it('still leaves immediately after an overlay was dismissed', () => {
    expect(resolveReaderBackTarget({ ...idle, panelOpen: false })).toBe('leave')
  })
})
