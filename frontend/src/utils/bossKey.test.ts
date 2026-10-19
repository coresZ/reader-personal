import { describe, expect, it } from 'vitest'
import { captureBossKey, formatBossKey, normalizeBossKeyShortcut } from './bossKey'

function keyboardEvent(overrides: Partial<KeyboardEvent>): KeyboardEvent {
  return {
    key: '',
    code: '',
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    metaKey: false,
    ...overrides,
  } as KeyboardEvent
}

describe('boss key capture', () => {
  it('converts a modified key into a global shortcut string', () => {
    expect(captureBossKey(keyboardEvent({ key: 'h', code: 'KeyH', ctrlKey: true, shiftKey: true })))
      .toEqual({ shortcut: 'Control+Shift+H' })
  })

  it('allows F1 through F12 without a modifier', () => {
    expect(captureBossKey(keyboardEvent({ key: 'F9', code: 'F9' })))
      .toEqual({ shortcut: 'F9' })
  })

  it('allows editing and navigation keys without a modifier', () => {
    expect(captureBossKey(keyboardEvent({ key: 'Home', code: 'Home' })))
      .toEqual({ shortcut: 'Home' })
    expect(captureBossKey(keyboardEvent({ key: 'End', code: 'End' })))
      .toEqual({ shortcut: 'End' })
    expect(captureBossKey(keyboardEvent({ key: 'Delete', code: 'Delete' })))
      .toEqual({ shortcut: 'Delete' })
    expect(captureBossKey(keyboardEvent({ key: 'PageDown', code: 'PageDown' })))
      .toEqual({ shortcut: 'PageDown' })
    expect(captureBossKey(keyboardEvent({ key: 'ArrowLeft', code: 'ArrowLeft' })))
      .toEqual({ shortcut: 'ArrowLeft' })
  })

  it('still accepts a modifier combination on an editing key', () => {
    expect(captureBossKey(keyboardEvent({ key: 'Home', code: 'Home', ctrlKey: true })))
      .toEqual({ shortcut: 'Control+Home' })
  })

  it('supports the space key with a modifier', () => {
    expect(captureBossKey(keyboardEvent({ key: ' ', code: 'Space', altKey: true })))
      .toEqual({ shortcut: 'Alt+Space' })
  })

  it('silently ignores the Windows key and its combinations', () => {
    expect(captureBossKey(keyboardEvent({ key: 'Meta', code: 'MetaLeft', metaKey: true })))
      .toEqual({})
    expect(captureBossKey(keyboardEvent({ key: 'h', code: 'KeyH', metaKey: true })))
      .toEqual({})
  })

  it('rejects unmodified regular keys', () => {
    expect(captureBossKey(keyboardEvent({ key: 'q', code: 'KeyQ' })).error)
      .toContain('Ctrl')
    // 字母/数字/空格单按会抢输入, 仍需组合键。
    expect(captureBossKey(keyboardEvent({ key: ' ', code: 'Space' })).error)
      .toContain('Ctrl')
  })

  it('formats stored shortcuts for display', () => {
    expect(formatBossKey('CommandOrControl+Shift+H')).toBe('Ctrl / Command + Shift + H')
    expect(formatBossKey('ArrowLeft')).toBe('←')
    expect(formatBossKey('PageDown')).toBe('PgDn')
    expect(formatBossKey('Control+Home')).toBe('Ctrl + Home')
  })

  it('replaces an old Windows-key shortcut with the default shortcut', () => {
    expect(normalizeBossKeyShortcut('Super+H')).toBe('CommandOrControl+Shift+H')
    expect(normalizeBossKeyShortcut('Control+Alt+H')).toBe('Control+Alt+H')
  })
})
