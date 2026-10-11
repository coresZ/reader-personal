export type SourceLiteral = string | number | boolean | null | SourceLiteral[] | { [key: string]: SourceLiteral }

/** Read data literals used in Legado options; strings can use either quote. */
export function readSourceLiteral(
  input: string,
  start = 0,
): { value: SourceLiteral; end: number } | null {
  let position = start
  const fail = (): never => { throw new Error('Invalid source option literal') }
  const space = () => {
    while (position < input.length) {
      if (/\s/.test(input[position])) { position++; continue }
      if (input.startsWith('//', position)) {
        const end = input.indexOf('\n', position + 2)
        position = end < 0 ? input.length : end + 1
      } else if (input.startsWith('/*', position)) {
        const end = input.indexOf('*/', position + 2)
        if (end < 0) fail()
        position = end + 2
      } else break
    }
  }
  const quoted = (): string => {
    const quote = input[position++]
    let value = ''
    while (position < input.length) {
      const character = input[position++]
      if (character === quote) return value
      if (character !== '\\') {
        if (character.charCodeAt(0) < 32) fail()
        value += character
        continue
      }
      const escaped = input[position++]
      const escapes: Record<string, string> = {
        '"': '"', "'": "'", '\\': '\\', '/': '/', b: '\b', f: '\f',
        n: '\n', r: '\r', t: '\t', v: '\v',
      }
      if (Object.hasOwn(escapes, escaped)) {
        value += escapes[escaped]
      } else if (escaped === 'u' || escaped === 'x') {
        const length = escaped === 'u' ? 4 : 2
        const digits = input.slice(position, position + length)
        if (digits.length !== length || !/^[\da-f]+$/i.test(digits)) fail()
        value += String.fromCharCode(Number.parseInt(digits, 16))
        position += length
      } else if (escaped === '\n') {
        continue
      } else if (escaped === '\r') {
        if (input[position] === '\n') position++
      } else {
        fail()
      }
    }
    return fail()
  }
  const read = (depth: number): SourceLiteral => {
    if (depth > 64) return fail()
    space()
    const character = input[position]
    if (character === '"' || character === "'") return quoted()
    if (character === '{') {
      position++
      const value: Record<string, SourceLiteral> = Object.create(null)
      space()
      if (input[position] === '}') { position++; return value }
      while (position < input.length) {
        space()
        let key: string
        if (input[position] === '"' || input[position] === "'") {
          key = quoted()
        } else {
          const identifier = /^[\p{L}_$][\p{L}\p{N}_$]*/u.exec(input.slice(position))
          if (!identifier) return fail()
          key = identifier[0]
          position += key.length
        }
        space()
        if (input[position++] !== ':') return fail()
        value[key] = read(depth + 1)
        space()
        const delimiter = input[position++]
        if (delimiter === '}') return value
        if (delimiter !== ',') return fail()
        space()
        if (input[position] === '}') { position++; return value }
      }
      return fail()
    }
    if (character === '[') {
      position++
      const value: SourceLiteral[] = []
      space()
      if (input[position] === ']') { position++; return value }
      while (position < input.length) {
        value.push(read(depth + 1))
        space()
        const delimiter = input[position++]
        if (delimiter === ']') return value
        if (delimiter !== ',') return fail()
        space()
        if (input[position] === ']') { position++; return value }
      }
      return fail()
    }
    for (const [keyword, value] of [['true', true], ['false', false], ['null', null]] as const) {
      if (input.startsWith(keyword, position)) {
        position += keyword.length
        return value
      }
    }
    const number = /^[+-]?(?:0x[\da-f]+|(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?)/i.exec(input.slice(position))
    if (number) {
      const value = Number(number[0])
      if (!Number.isFinite(value)) return fail()
      position += number[0].length
      return value
    }
    return fail()
  }
  try {
    const value = read(0)
    return { value, end: position }
  } catch {
    return null
  }
}
