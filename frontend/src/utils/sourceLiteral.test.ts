import { describe, expect, it } from 'vitest'
import { readSourceLiteral } from './sourceLiteral'

describe('source option data literals', () => {
  it('reads mixed quotes, escaped characters and nested data without stopping on string delimiters', () => {
    const input = String.raw`{'type':'qtbzs',"click":"showCmt('https://site/?a=1&b=2','本章说')", extra:{text:'it\'s > } ,{', values:[1,true,null,{'quoted':'\"'}],},}`
    const parsed = readSourceLiteral(input)
    expect(parsed?.end).toBe(input.length)
    expect(parsed?.value).toMatchObject({
      type: 'qtbzs',
      click: "showCmt('https://site/?a=1&b=2','本章说')",
      extra: { text: "it's > } ,{", values: [1, true, null, { quoted: '"' }] },
    })
  })

  it('decodes string escapes and returns the end of just the object', () => {
    const input = String.raw`{"text":"\u6bb5\u8bc4\n\t\\","width":"2em"}" alt="outside">`
    const parsed = readSourceLiteral(input)
    expect(parsed?.value).toMatchObject({ text: '段评\n\t\\', width: '2em' })
    expect(input.slice(parsed!.end)).toBe('" alt="outside">')
  })

  it('rejects expressions, accessors and malformed strings', () => {
    for (const input of [
      '{click: function(){}}', '{click: (globalThis.changed = true)}',
      '{get click(){return "run()"}}', '{click: globalThis.value}',
      String.raw`{"click":"invalid\q"}`, '{click: "unfinished}', '[1,,2]',
    ]) expect(readSourceLiteral(input)).toBeNull()
  })

  it('stores prototype-like keys as plain data', () => {
    const parsed = readSourceLiteral('{"__proto__":{"changed":true},"click":"open()"}')!
    expect(Object.getPrototypeOf(parsed.value)).toBeNull()
    expect(Object.prototype).not.toHaveProperty('changed')
    expect(Object.getOwnPropertyDescriptor(parsed.value, '__proto__')?.value).toEqual({ changed: true })
  })
})
