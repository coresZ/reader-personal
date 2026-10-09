import { describe, expect, it } from 'vitest'
import {
  READER_HOVER_EDGE_PX,
  READER_HOVER_HIDE_DELAY_MS,
  READER_HOVER_SIDE_EXTRA_PX,
  isReaderHoverPointInZone,
  shouldRevealReaderControls,
} from './readerHoverControls'

const VIEWPORT = { width: 1200, height: 800 }
const NO_BLOCKERS = { panelOpen: false, overlayOpen: false }
const CENTER = { x: 600, y: 400 }

describe('isReaderHoverPointInZone', () => {
  it('accepts the top and bottom edges', () => {
    expect(isReaderHoverPointInZone({ x: 600, y: 1 }, VIEWPORT)).toBe(true)
    expect(isReaderHoverPointInZone({ x: 600, y: READER_HOVER_EDGE_PX }, VIEWPORT)).toBe(true)
    expect(isReaderHoverPointInZone({ x: 600, y: VIEWPORT.height - 1 }, VIEWPORT)).toBe(true)
    expect(isReaderHoverPointInZone({ x: 600, y: VIEWPORT.height - READER_HOVER_EDGE_PX }, VIEWPORT)).toBe(true)
  })

  it('accepts the left and right edges with the wider side margin', () => {
    const side = READER_HOVER_EDGE_PX + READER_HOVER_SIDE_EXTRA_PX
    expect(isReaderHoverPointInZone({ x: 1, y: 400 }, VIEWPORT)).toBe(true)
    expect(isReaderHoverPointInZone({ x: side, y: 400 }, VIEWPORT)).toBe(true)
    expect(isReaderHoverPointInZone({ x: VIEWPORT.width - side, y: 400 }, VIEWPORT)).toBe(true)
    // 横向热区比纵向宽: 超出 60px 但在 side 之内仍算靠近。
    expect(isReaderHoverPointInZone({ x: READER_HOVER_EDGE_PX + 1, y: 400 }, VIEWPORT)).toBe(true)
  })

  it('rejects the reading area and points just outside the zone', () => {
    expect(isReaderHoverPointInZone(CENTER, VIEWPORT)).toBe(false)
    expect(isReaderHoverPointInZone({ x: 600, y: READER_HOVER_EDGE_PX + 1 }, VIEWPORT)).toBe(false)
    expect(isReaderHoverPointInZone({ x: 600, y: VIEWPORT.height - READER_HOVER_EDGE_PX - 1 }, VIEWPORT)).toBe(false)
    expect(
      isReaderHoverPointInZone(
        { x: READER_HOVER_EDGE_PX + READER_HOVER_SIDE_EXTRA_PX + 1, y: 400 },
        VIEWPORT,
      ),
    ).toBe(false)
  })

  it('treats an unmeasurable viewport or point as not hovered', () => {
    expect(isReaderHoverPointInZone(CENTER, { width: 0, height: 800 })).toBe(false)
    expect(isReaderHoverPointInZone(CENTER, { width: 1200, height: 0 })).toBe(false)
    expect(isReaderHoverPointInZone({ x: Number.NaN, y: 1 }, VIEWPORT)).toBe(false)
  })
})

describe('shouldRevealReaderControls', () => {
  it('reveals while the pointer sits on an edge', () => {
    expect(shouldRevealReaderControls({ x: 600, y: 4 }, VIEWPORT, NO_BLOCKERS)).toBe(true)
  })

  it('stays hidden while a panel or an overlay is open', () => {
    const point = { x: 600, y: 4 }
    expect(shouldRevealReaderControls(point, VIEWPORT, { panelOpen: true, overlayOpen: false })).toBe(false)
    expect(shouldRevealReaderControls(point, VIEWPORT, { panelOpen: false, overlayOpen: true })).toBe(false)
  })

  it('stays hidden inside the reading area', () => {
    expect(shouldRevealReaderControls(CENTER, VIEWPORT, NO_BLOCKERS)).toBe(false)
  })

  it('collapses as soon as the pointer leaves (no lingering delay)', () => {
    expect(READER_HOVER_HIDE_DELAY_MS).toBe(0)
  })
})
