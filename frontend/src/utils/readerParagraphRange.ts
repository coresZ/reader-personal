const ATOMIC_ELEMENTS = new Set([
  'IMG', 'BR', 'HR', 'SVG', 'MATH', 'VIDEO', 'AUDIO', 'IFRAME',
  'CANVAS', 'OBJECT', 'EMBED', 'TABLE',
])

interface Boundary { node: Node; offset: number }

/** A paragraph can be split only between characters or complete media nodes. */
export function parseReaderParagraph(html: string) {
  const root = document.createElement('div')
  root.innerHTML = html
  const paragraph = root.firstElementChild
  if (!(paragraph instanceof HTMLParagraphElement)) return null

  const tokens: string[] = []
  const boundaries: Boundary[] = []
  const visit = (node: Node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      let offset = 0
      for (const character of node.textContent || '') {
        offset += character.length
        tokens.push(character)
        boundaries.push({ node, offset })
      }
      return
    }
    if (!(node instanceof Element)) return
    if (ATOMIC_ELEMENTS.has(node.tagName) || !node.childNodes.length) {
      const parent = node.parentNode!
      tokens.push('\uFFFC')
      boundaries.push({ node: parent, offset: Array.prototype.indexOf.call(parent.childNodes, node) + 1 })
      return
    }
    node.childNodes.forEach(visit)
  }
  paragraph.childNodes.forEach(visit)

  return {
    tokens,
    style: paragraph.getAttribute('style') || '',
    className: paragraph.className,
    slice(start: number, end: number, style: string, className: string): string {
      const range = document.createRange()
      const first = start === 0 ? { node: paragraph, offset: 0 } : boundaries[start - 1]
      const last = end === tokens.length
        ? { node: paragraph, offset: paragraph.childNodes.length }
        : boundaries[end - 1]
      range.setStart(first.node, first.offset)
      range.setEnd(last.node, last.offset)
      const segment = paragraph.cloneNode(false) as HTMLParagraphElement
      segment.appendChild(range.cloneContents())
      if (style) segment.setAttribute('style', style)
      else segment.removeAttribute('style')
      if (className) segment.className = className
      else segment.removeAttribute('class')
      if (start > 0) segment.removeAttribute('id')
      return segment.outerHTML
    },
  }
}
