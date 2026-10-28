import { describe, expect, it } from 'vitest'
import { splitParagraphs, splitParagraphsAroundExisting } from './readerParagraph'

const options = { spacingEm: 0.5, firstLineIndent: true }

describe('splitParagraphs', () => {
  it('splits each <br> into its own paragraph with the configured spacing', () => {
    const html = splitParagraphs('第一段<br>第二段<br>第三段', options)

    expect(html?.match(/<p/g)).toHaveLength(3)
    expect(html).toContain('margin-bottom: 0.5em')
    expect(html).toContain('第一段')
    expect(html).toContain('第三段')
  })

  it('honours the indent setting', () => {
    expect(splitParagraphs('正文<br>下一段', options)).toContain('class="reader-indent"')
    expect(splitParagraphs('正文<br>下一段', { ...options, firstLineIndent: false }))
      .not.toContain('reader-indent')
  })

  it('accepts self-closing and spaced <br> variants', () => {
    expect(splitParagraphs('甲<br/>乙<br />丙', options)?.match(/<p/g)).toHaveLength(3)
  })

  it('drops empty lines produced by consecutive breaks', () => {
    const html = splitParagraphs('甲<br><br>乙<br>   <br>', options)

    expect(html?.match(/<p/g)).toHaveLength(2)
    expect(html).not.toContain('margin-bottom: 0.5em;">   <')
  })

  it('strips leading full-width and ASCII indentation inside each paragraph', () => {
    const html = splitParagraphs('\u3000\u3000首行缩进<br>  半角缩进', options)

    expect(html).toContain('>首行缩进</p>')
    expect(html).toContain('>半角缩进</p>')
  })

  it('keeps inline markup that stays balanced inside one paragraph', () => {
    expect(splitParagraphs('<b>加粗</b>文字<br>普通', options))
      .toContain('<b>加粗</b>文字')
  })

  it('returns an empty string when there is nothing to render', () => {
    expect(splitParagraphs('<br><br>', options)).toBe('')
  })

  // 裸换行是这类书源唯一的分段信号: 整章没有 <p>、没有 <br>,
  // 段尾只追加行内段评气泡 <img>。此时必须按 \n 分段, 否则整章连成一段。
  describe('bare newlines', () => {
    it('splits on \\n so chapters without <br> still get paragraphs', () => {
      const html = splitParagraphs('第一段\n第二段\n第三段', options)

      expect(html?.match(/<p/g)).toHaveLength(3)
      expect(html).toContain('>第一段</p>')
      expect(html).toContain('>第三段</p>')
    })

    it('splits a chapter whose paragraphs carry inline commentary images', () => {
      // 段尾气泡是行内 <img>, 曾经因为 img 不在行内白名单里导致整章不分段。
      const bubble = '<img src="data:image/svg+xml;base64,AAA" style="width: 110px">'
      const html = splitParagraphs(`第一段${bubble}\n第二段${bubble}\n第三段`, options)

      expect(html?.match(/<p/g)).toHaveLength(3)
      expect(html).toContain(`第一段${bubble}`)
    })

    it('splits a <br>-separated chapter that also carries images', () => {
      // issue #17 的第一种形态: 显式 <br> 分段, 段内再嵌图片。
      const html = splitParagraphs('第一段<img src="a.png"><br>第二段<br>第三段', options)

      expect(html?.match(/<p/g)).toHaveLength(3)
      expect(html).toContain('<img src="a.png">')
    })

    it('handles CRLF line endings', () => {
      expect(splitParagraphs('甲\r\n乙\r\n丙', options)?.match(/<p/g)).toHaveLength(3)
    })

    it('mixes windows line endings with <br> without creating empty paragraphs', () => {
      expect(splitParagraphs('甲<br>乙\r\n丙', options)?.match(/<p/g)).toHaveLength(3)
    })

    it('drops blank lines between paragraphs', () => {
      expect(splitParagraphs('甲\n\n\n乙', options)?.match(/<p/g)).toHaveLength(2)
    })

    it('keeps an inline image on its own line as a paragraph', () => {
      const html = splitParagraphs('文字\n<img src="a.png">\n文字', options)

      expect(html?.match(/<p/g)).toHaveLength(3)
      expect(html).toContain('>文字</p>')
    })

    it('does not indent a paragraph that holds only an image', () => {
      // 首行缩进用 text-indent 实现, 套在纯图段落上会把图片整体右推。
      const html = splitParagraphs('文字\n<img src="a.png">', options)

      expect(html).toContain('<p class="reader-indent" style="margin-top: 0; margin-bottom: 0.5em;">文字</p>')
      expect(html).toContain('</p><p style="margin-top: 0; margin-bottom: 0.5em;"><img src="a.png"></p>')
    })
  })

  // 以下为安全闸: 重组会破坏结构时必须返回 null, 让调用方保留原 HTML。
  it('refuses to split when an inline tag spans across a <br>', () => {
    // <b> 跨段会导致每个 <p> 里都有悬空标签。
    expect(splitParagraphs('<b>甲<br>乙</b>', options)).toBeNull()
    expect(splitParagraphs('<span>甲<br>乙', options)).toBeNull()
  })

  it('refuses to split when structured markup is present', () => {
    expect(splitParagraphs('<div>甲</div><br>乙', options)).toBeNull()
    expect(splitParagraphs('<table><tr><td>甲</td></tr></table><br>乙', options)).toBeNull()
    expect(splitParagraphs('甲<br><h1>标题</h1>', options)).toBeNull()
    expect(splitParagraphs('甲<br><hr>', options)).toBeNull()
    expect(splitParagraphs('甲<br><iframe src="x"></iframe>', options)).toBeNull()
    expect(splitParagraphs('甲<br><svg><circle /></svg>', options)).toBeNull()
  })

  it('refuses to split when a bare newline sits inside structured markup', () => {
    // 块级结构里的换行往往只是源码缩进, 不能当成段落边界。
    expect(splitParagraphs('<div>\n  甲\n</div>', options)).toBeNull()
    expect(splitParagraphs('<ul>\n  <li>甲</li>\n</ul>', options)).toBeNull()
  })

  it('refuses to split unbalanced markup', () => {
    expect(splitParagraphs('甲<br><b>未闭合', options)).toBeNull()
  })

  it('is not fooled by a tag name that merely appears inside text', () => {
    // 属性值里的 "<br" 会被 split 误切, 逐段校验会拦下它, 退回安全路径。
    expect(splitParagraphs('<a href="x<br>y">链接</a>', options)).toBeNull()
  })

  it('is not fooled by a newline inside an attribute value', () => {
    // 属性值里的换行会被误切, 逐段校验拦下后整体放弃重组。
    expect(splitParagraphs('<a href="x\ny">链接</a>', options)).toBeNull()
  })

  // img 是空元素, 不做栈平衡校验, 所以"标签是否完整"是它唯一的防线。
  // 以下四种形态都会被分隔符切碎, 必须整体放弃重组而不是切出破图。
  it('refuses to split an img whose attributes span lines', () => {
    expect(splitParagraphs('<img\n  src="a.png"\n  style="width: 10px">', options)).toBeNull()
  })

  it('refuses to split an img holding a newline inside an attribute', () => {
    expect(splitParagraphs('<img src="a\nb.png">', options)).toBeNull()
  })

  it('refuses to split an img holding <br inside an attribute', () => {
    expect(splitParagraphs('<img src="a<br>b.png">', options)).toBeNull()
  })

  it('refuses to split an img holding a quote-adjacent newline in a src path', () => {
    expect(splitParagraphs('甲\n<img\nsrc="x.png"\nstyle="height: 1em">\n乙', options)).toBeNull()
  })

  it('still splits a well-formed multi-image chapter', () => {
    // 真正的正文形态: 每段一个单行 img, 不该被上面的防护误伤。
    const html = splitParagraphs('甲<img src="a.png">\n乙<img src="b.png">', options)

    expect(html?.match(/<p/g)).toHaveLength(2)
    expect(html).toContain('<img src="a.png">')
  })

  it('does not treat a > inside an attribute as the end of the tag', () => {
    // 引号感知: alt="a>b" 里的 > 不是标签边界, 标签仍应被认作完整。
    const html = splitParagraphs('甲<img alt="a>b" src="x.png">\n乙', options)

    expect(html?.match(/<p/g)).toHaveLength(2)
    expect(html).toContain('<img alt="a>b" src="x.png">')
  })
})

describe('splitParagraphsAroundExisting', () => {
  // issue #17: 该源正文是裸文本行, 只在插图下方给署名加一个 <p>。
  // 旧实现「有 <p> 就整章跳过重组」, 导致其余上百行塌成一段。
  it('reflows bare lines even when the chapter already has a <p>', () => {
    const chapter = [
      '第一段正文',
      '第二段正文',
      '<img src="pic.png">',
      "<p style=\"text-align: center;\">——插图署名</p>",
      '第三段正文',
      '第四段正文',
    ].join('\n')

    const html = splitParagraphsAroundExisting(chapter, options)

    // 4 行裸文本各自成段 + 独占一行的图片 + 原有的署名 <p>
    expect(html.match(/<p[ >]/g)).toHaveLength(6)
    expect(html).toContain('>第一段正文</p>')
    expect(html).toContain('>第四段正文</p>')
    // 图片行保留在原位置, 且因为只有图不带首行缩进
    expect(html).toContain(`<p style="margin-top: 0; margin-bottom: 0.5em;"><img src="pic.png"></p>`)
  })

  it('keeps an existing <p> in place instead of moving it to the end', () => {
    const chapter = '甲\n<p style="text-align: center;">署名</p>\n乙'
    const html = splitParagraphsAroundExisting(chapter, options)

    // 顺序必须是 甲 -> 署名 -> 乙, 署名不能被挪到章尾。
    expect(html.indexOf('甲')).toBeLessThan(html.indexOf('署名'))
    expect(html.indexOf('署名')).toBeLessThan(html.indexOf('乙'))
    expect(html).toContain('<p style="text-align: center;">署名</p>')
  })

  it('preserves the existing <p> markup instead of re-wrapping it', () => {
    const existing = '<p class="keep-me" style="color: red;">原样保留</p>'
    const html = splitParagraphsAroundExisting(`甲\n${existing}\n乙`, options)

    expect(html).toContain(existing)
    // 不能出现被二次包裹的 <p><p ...
    expect(html).not.toContain('<p' + ' class="reader-indent" style="margin-top: 0; margin-bottom: 0.5em;"><p')
  })

  it('handles a chapter that is only bare lines', () => {
    const html = splitParagraphsAroundExisting('甲\n乙\n丙', options)

    expect(html.match(/<p[ >]/g)).toHaveLength(3)
  })

  it('handles a chapter that is only existing <p> elements', () => {
    const chapter = '<p>甲</p><p>乙</p>'
    const html = splitParagraphsAroundExisting(chapter, options)

    expect(html).toBe(chapter)
  })

  it('leaves a run alone when it cannot be split safely', () => {
    // 含块级结构: splitParagraphs 返回 null, 该片段必须原样保留。
    const chapter = '<div>甲</div>\n<p>署名</p>'
    const html = splitParagraphsAroundExisting(chapter, options)

    expect(html).toContain('<div>甲</div>')
    expect(html).toContain('<p>署名</p>')
  })

  it('does not mistake a > inside a <p> attribute for the tag end', () => {
    const chapter = '甲\n<p title="a>b">署名</p>\n乙'
    const html = splitParagraphsAroundExisting(chapter, options)

    expect(html).toContain('<p title="a>b">署名</p>')
  })

  it('keeps an inline image line together with its own paragraph', () => {
    const chapter = `甲<img src="a.png">\n乙<p>署名</p>丙`
    const html = splitParagraphsAroundExisting(chapter, options)

    expect(html).toContain('<img src="a.png">')
    expect(html.indexOf('甲')).toBeLessThan(html.indexOf('署名'))
    expect(html.indexOf('署名')).toBeLessThan(html.indexOf('丙'))
  })
})
