import { describe, expect, it } from 'vitest'
import { repairSourceImageHtml } from './sourceImageHtml'
import { parseSourceImageOptions } from './sourceImageOptions'

describe('source image HTML repair', () => {
  it('protects the options inside a raw source-generated src without changing other attributes', () => {
    const html = `<p>正文<img src="data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=,{"click":"showCmt('https://site/?a=1&amp;b=2')","style":"text"}" alt="气泡" /></p>`
    expect(repairSourceImageHtml(html)).toBe(
      html.replace('{"click":"showCmt(\'https://site/?a=1&amp;b=2\')","style":"text"}',
        '{&quot;click&quot;:&quot;showCmt(\'https://site/?a=1&amp;b=2\')&quot;,&quot;style&quot;:&quot;text&quot;}'),
    )
  })

  it('finds the object end through nested values, escaped quotes and greater-than signs', () => {
    const option = String.raw`{"click":"if (a > 1) showCmt('it\\\'s } ,{')","nested":{"items":[{"value":"<p>quoted \"text\"</p>"}]}}`
    const html = '<img src="https://site/icon.svg,' + option + '" title="保留">'
    const fixed = repairSourceImageHtml(html)
    expect(fixed).toBe('<img src="https://site/icon.svg,' + option.replaceAll('"', '&quot;').replaceAll('<', '&lt;') + '" title="保留">')
  })

  it('repairs single-quoted attributes and is stable when applied again', () => {
    const html = `<img src='https://site/icon.svg,{"click":"showCmt('url')","style":"text"}'>`
    const fixed = repairSourceImageHtml(html)
    expect(fixed).toContain('showCmt(&#39;url&#39;)')
    expect(repairSourceImageHtml(fixed)).toBe(fixed)
  })

  it('leaves ordinary images, valid HTML attributes and malformed options unchanged', () => {
    for (const html of [
      `<img alt='a"b' src='https://site/image.png?caption="quoted"'>`,
      '<img src="https://site/a,b.png" data-other=\'{"value":">"}\'>',
      '<img src="https://site/a.svg,{&quot;click&quot;:&quot;open()&quot;}">',
      '<img src="https://site/a.svg,{click:execute()}">',
      '<img src="https://site/a.svg,{oops" alt="later">',
      '<img src="https://site/a.svg,{width:2}suffix">',
    ]) expect(repairSourceImageHtml(html)).toBe(html)
  })

  it('does not rewrite apparent image tags inside comments or raw text elements', () => {
    const image = '<img src="https://site/icon.svg,{"click":"open()"}">'
    for (const html of [
      '<!-- ' + image + ' -->',
      '<script>const example = ' + JSON.stringify(image) + ';</script>',
      '<style>p:before{content:' + JSON.stringify(image) + '}</style>',
      '<textarea>' + image + '</textarea>',
    ]) expect(repairSourceImageHtml(html)).toBe(html)
  })
})

describe('source image options with Legado literals', () => {
  it('reads the actual mixed-quote chapter-review shape', () => {
    const raw = `https://site/chapter.svg,{'type':'qtbzs',"click":"showCmt('https://site/review?a=1&b=2','源甲','本章说' )",'style':'FULL'}`
    expect(parseSourceImageOptions(raw)).toMatchObject({
      url: 'https://site/chapter.svg',
      click: "showCmt('https://site/review?a=1&b=2','源甲','本章说' )",
      style: 'full',
    })
  })

  it('retains commas and object-looking text in the image URL and click string', () => {
    const raw = `https://site/icon?value={id:1},{click:"showCmt('text ,{ value > 1')",extra:{items:[{x:1}]}}`
    expect(parseSourceImageOptions(raw)).toMatchObject({
      url: 'https://site/icon?value={id:1}',
      click: "showCmt('text ,{ value > 1')",
    })
  })
})
