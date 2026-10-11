import { readSourceLiteral } from './sourceLiteral'

const RAW_TEXT_TAGS = new Set(['script', 'style', 'textarea', 'title', 'iframe', 'xmp', 'noembed', 'noframes'])

/**
 * Source rules place a literal options object inside a quoted img src.
 * Repair its attribute quotes before the HTML tokenizer can truncate the URL.
 */
export function repairSourceImageHtml(html: string): string {
  if (!/<img[\s/>]/i.test(html)) return html
  const lower = html.toLowerCase()
  const changes: Array<{ start: number; end: number; value: string }> = []

  const tagEnd = (start: number) => {
    let quote = ''
    for (let index = start; index < html.length; index++) {
      const character = html[index]
      if (quote) {
        if (character === quote) quote = ''
      } else if (character === '"' || character === "'") {
        quote = character
      } else if (character === '>') {
        return index + 1
      }
    }
    return html.length
  }

  let position = 0
  while (position < html.length) {
    const start = html.indexOf('<', position)
    if (start < 0) break
    if (html.startsWith('<!--', start)) {
      const end = html.indexOf('-->', start + 4)
      position = end < 0 ? html.length : end + 3
      continue
    }
    if (html.startsWith('<![CDATA[', start)) {
      const end = html.indexOf(']]>', start + 9)
      position = end < 0 ? html.length : end + 3
      continue
    }
    const tag = /^<(\/?)([a-z][\w:-]*)(?=[\s/>])/i.exec(html.slice(start))
    if (!tag) { position = start + 1; continue }
    const name = tag[2].toLowerCase()
    position = start + tag[0].length
    if (tag[1] || name !== 'img') {
      position = tagEnd(position)
      if (!tag[1] && name === 'plaintext') break
      if (!tag[1] && RAW_TEXT_TAGS.has(name)) {
        const end = lower.indexOf('</' + name, position)
        position = end < 0 ? html.length : tagEnd(end + name.length + 2)
      }
      continue
    }

    while (position < html.length && html[position] !== '>') {
      if (/[\s/]/.test(html[position])) { position++; continue }
      const attribute = /^[^\s=/>]+/.exec(html.slice(position))
      if (!attribute) { position++; continue }
      const attributeName = attribute[0].toLowerCase()
      position += attribute[0].length
      while (/\s/.test(html[position] || '') && position < html.length) position++
      if (html[position] !== '=') continue
      position++
      while (/\s/.test(html[position] || '') && position < html.length) position++
      const quote = html[position]
      if (quote !== '"' && quote !== "'") {
        while (position < html.length && !/[\s>]/.test(html[position])) position++
        continue
      }
      const valueStart = ++position
      const normalEnd = html.indexOf(quote, valueStart)
      if (normalEnd < 0) { position = html.length; break }
      let valueEnd = normalEnd
      if (attributeName === 'src') {
        const prefix = html.slice(valueStart, normalEnd)
        const separators = [...prefix.matchAll(/,\s*(?=\{)/g)].reverse()
        for (const separator of separators) {
          const objectStart = valueStart + separator.index + separator[0].length
          const parsed = readSourceLiteral(html, objectStart)
          if (!parsed || !parsed.value || typeof parsed.value !== 'object' || Array.isArray(parsed.value)) continue
          let end = parsed.end
          while (end < html.length && /\s/.test(html[end])) end++
          if (html[end] !== quote || !/[\s/>]/.test(html[end + 1] || '>')) continue
          valueEnd = end
          if (end !== normalEnd) {
            const value = html.slice(valueStart, end)
              .replaceAll(quote, quote === '"' ? '&quot;' : '&#39;')
              .replaceAll('<', '&lt;')
            changes.push({ start: valueStart, end, value })
          }
          break
        }
      }
      position = valueEnd + 1
    }
    position++
  }

  let result = ''
  let copied = 0
  for (const change of changes) {
    result += html.slice(copied, change.start) + change.value
    copied = change.end
  }
  return changes.length ? result + html.slice(copied) : html
}
