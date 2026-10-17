export interface ReadingPositionSnapshot {
  chapterIndex: number
  progress: number
  paragraphIndex?: number
  paragraphProgress?: number
  updatedAt?: number
}

export const READER_POSITION_PREFIX = 'reader-position:'

export function readSavedReadingPosition(
  bookUrl: string | undefined | null,
): ReadingPositionSnapshot | null {
  if (!bookUrl) return null
  try {
    const raw = localStorage.getItem(`${READER_POSITION_PREFIX}${bookUrl}`)
    if (!raw) return null
    const parsed = JSON.parse(raw) as Partial<ReadingPositionSnapshot>
    if (typeof parsed?.chapterIndex !== 'number') return null
    return {
      chapterIndex: parsed.chapterIndex,
      progress: typeof parsed.progress === 'number'
        ? Math.max(0, Math.min(1, parsed.progress))
        : 0,
      paragraphIndex: typeof parsed.paragraphIndex === 'number' ? parsed.paragraphIndex : undefined,
      paragraphProgress: typeof parsed.paragraphProgress === 'number' ? parsed.paragraphProgress : undefined,
      updatedAt: typeof parsed.updatedAt === 'number' ? parsed.updatedAt : 0,
    }
  } catch {
    return null
  }
}

export function shouldPreferServerReadingPosition(
  local: ReadingPositionSnapshot | null,
  server: ReadingPositionSnapshot | null,
) {
  if (!server) return false
  if (!local) return true
  return Math.abs((server.progress || 0) - (local.progress || 0)) >= 0.02
}
