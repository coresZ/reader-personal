/** 书源正文的段落间距处理。
 *
 *  背景: ReaderView 的 formatChapterHtml 只给 <p> 写内联 margin-bottom,
 *  于是「有 HTML 但没有 <p>」的正文拿不到「段落间距」与首行缩进。
 *  这里把它们重组成独立 <p>。
 *
 *  两种分段信号:
 *  1. `<br>` —— 显式换行标签。
 *  2. 裸换行(`\n`) —— 一类书源把正文按行返回, 只在段尾插入行内图片
 *     (段评气泡 `<img src="data:image/svg+xml;base64,...">`), 于是整章既没有
 *     `<p>` 也没有 `<br>`, 换行是唯一的分段信号。缺了这条, 含图章节会被
 *     判成"不可拆分", 整章连成一段。
 *
 *  裸换行只在「整段 HTML 全部由行内标签构成」时才当分隔符: 一旦出现
 *  div/table 这类块级结构, 换行往往只是源码格式化缩进, 按它切会凭空多出段落。
 *  isSplittableMarkup 在拆分前后各校验一次, 起的就是这个闸的作用。
 *
 *  重组是字符串级操作, 必须有严格的安全闸: 只有「全部标签都在行内白名单内」
 *  且「每个段落自身标签平衡」时才敢拆。任一不满足就返回 null, 由调用方退回
 *  「只补 margin、不动结构」的安全路径 —— 结构重组出错(悬空标签)的代价远大于
 *  段落间距不生效。
 *
 *  纯字符串逻辑, 不碰 DOM, 便于单测。 */

export interface ParagraphSpacingOptions {
  /** 段落间距, 单位 em(与设置面板一致)。 */
  spacingEm: number
  /** 是否首行缩进 2 字符。 */
  firstLineIndent: boolean
}

/** 允许出现在段落内部的行内标签。名单之外(div/table/hr/h1 等)一律不拆:
 *  黑名单式的排除总会漏新标签, 白名单只会更保守。
 *
 *  `img` 必须在名单里: 书源的段评气泡是行内 `<img>`, 漏掉它会让所有含图
 *  章节落进"不可拆分"分支, 表现为整章不分段。 */
const INLINE_TAGS = new Set([
  'a', 'abbr', 'b', 'bdi', 'bdo', 'br', 'cite', 'code', 'data', 'dfn', 'em',
  'font', 'i', 'img', 'kbd', 'mark', 'q', 'rp', 'rt', 'ruby', 's', 'samp',
  'small', 'span', 'strong', 'sub', 'sup', 'time', 'u', 'var', 'wbr',
])

/** 无需闭合的行内空元素。`img` 与 `br` 一样是空元素, 没有闭合标签。 */
const VOID_INLINE_TAGS = new Set(['br', 'wbr', 'img'])

/** 段落分隔符: 显式 `<br>` 与裸换行。 */
const PARAGRAPH_BREAK = /<br\s*\/?>|\r?\n/i

/** 段落里除标签外是否还有可见文字。
 *
 *  首行缩进是给文字用的: 纯图片段落(整段只有 `<img>`, 常见于插图章)
 *  一旦带上 `reader-indent`, 样式里的 `text-indent: 2em` 会把行内图片整体
 *  右推 2em。这类段落照常给间距, 但不给缩进。 */
function hasVisibleText(paragraph: string): boolean {
  const text = paragraph.replace(new RegExp(TAG_SOURCE, 'g'), '').replace(/&nbsp;/gi, ' ')
  return text.trim().length > 0
}

/** 一个完整标签的正则源。**必须引号感知**。
 *
 *  `img` 是空元素, 走 VOID 分支时不做栈平衡校验, 于是"标签是否完整"
 *  成了唯一的防线。若标签解析不认引号, `[^>]*` 会把 `src="a<br>b.png"`
 *  整段当成标签内容, 拆分后得到 `<img src="a` + `b.png">`; 而残缺片段
 *  因为没有 `>` 又匹配不到任何标签, 栈依然平衡, 校验便全部放行 ——
 *  正文被切坏。引号感知让属性值里的 `>`、`<br`、换行都不再是标签边界。 */
const TAG_SOURCE = String.raw`<\/?([a-zA-Z][a-zA-Z0-9-]*)\b(?:[^>"']|"[^"]*"|'[^']*')*?(\/?)>`

const TAG_PATTERN = new RegExp(TAG_SOURCE, 'g')

/** 形如标签开头的残缺片段: 有 `<` 和字母, 却没能被 TAG_PATTERN 完整匹配。 */
const BROKEN_TAG_HEAD = /<[a-zA-Z]/

/** 扫描标签, 校验「白名单 + 平衡」。返回 false 表示这段 HTML 不能安全重组。 */
function isSplittableMarkup(html: string): boolean {
  const stack: string[] = []
  TAG_PATTERN.lastIndex = 0
  let match: RegExpExecArray | null
  let consumed = 0
  while ((match = TAG_PATTERN.exec(html)) !== null) {
    // 两个标签之间若残留"像标签开头"的 `<`, 说明有标签被分隔符切碎了。
    // 残缺片段匹配不到 TAG_PATTERN, 只看栈平衡会把这种情况误判为安全。
    if (BROKEN_TAG_HEAD.test(html.slice(consumed, match.index))) return false
    consumed = match.index + match[0].length

    const tagName = match[1].toLowerCase()
    if (!INLINE_TAGS.has(tagName)) return false
    if (VOID_INLINE_TAGS.has(tagName) || match[2] === '/') continue
    if (match[0].startsWith('</')) {
      if (stack.pop() !== tagName) return false
    } else {
      stack.push(tagName)
    }
  }
  if (BROKEN_TAG_HEAD.test(html.slice(consumed))) return false
  return stack.length === 0
}

/**
 * 把以 `<br>` 或裸换行分段的 HTML 拆成独立 `<p>`, 并附上间距与缩进。
 *
 * @returns 拆好的 HTML; 空串表示没有可渲染内容;
 *          **null 表示这段内容不能安全重组**(含结构化标签, 或有标签跨过分隔符,
 *          例如 `<b>甲<br>乙</b>` 拆开会留下悬空标签), 调用方应放弃重组。
 */
export function splitParagraphs(
  html: string,
  options: ParagraphSpacingOptions,
): string | null {
  // 整体先过一遍: 拦掉结构化标签与源本身就不平衡的 HTML。
  if (!isSplittableMarkup(html)) return null

  const paragraphs = html
    .split(PARAGRAPH_BREAK)
    .map((line) => line.replace(/^[\u3000\u00A0 \t]+/, '').trimEnd())
    .filter((line) => line.trim())
  if (!paragraphs.length) return ''

  // 逐段再校验: 整体平衡不代表每段平衡(`<b>甲<br>乙</b>` 整体是平的)。
  // 属性值里混入 "<br" 或换行造成的误切也会在这里被拦下。
  if (!paragraphs.every((paragraph) => isSplittableMarkup(paragraph))) return null

  const spacing = `${options.spacingEm}em`
  return paragraphs
    .map((paragraph) => {
      const indent = options.firstLineIndent && hasVisibleText(paragraph)
        ? ' class="reader-indent"'
        : ''
      return `<p${indent} style="margin-top: 0; margin-bottom: ${spacing};">${paragraph}</p>`
    })
    .join('')
}

/** 一个已有段落: `<p ...>...</p>`, 非贪婪。属性里的 `>` 用引号感知的写法跳过。
 *  HTML 里嵌套 `<p>` 本就不合法(浏览器会自动闭合), 按非贪婪匹配切分足够。 */
const EXISTING_PARAGRAPH = new RegExp(
  String.raw`<p\b(?:[^>"']|"[^"]*"|'[^']*')*>[\s\S]*?<\/p\s*>`,
  'gi',
)

/**
 * 把整章 HTML 按「已有 `<p>`」切段, 只对**不含 `<p>` 的片段**做段落重组。
 *
 *  为什么需要它: 一类书源只在插图下方给署名加一个 `<p>`, 正文主体仍是裸文本行。
 *  调用方原先以「章节里有没有 `<p>`」作二选一条件, 于是只要出现任何一个 `<p>`,
 *  剩余几十上百行裸文本就完全跳过分段处理, 整章塌成一大段。
 *
 *  这里按原顺序处理: 已有 `<p>` 原样保留(调用方已给它们补好间距与缩进),
 *  其余片段交给 `splitParagraphs` 重组, 再拼回去 —— 插图署名留在原地,
 *  不会被挪到章尾。
 *
 *  @returns 重组后的完整 HTML; 某片段不能安全重组时, 该片段原样保留。
 */
export function splitParagraphsAroundExisting(
  html: string,
  options: ParagraphSpacingOptions,
): string {
  const output: string[] = []
  let cursor = 0

  const flushRun = (end: number) => {
    const run = html.slice(cursor, end)
    if (!run.trim()) {
      output.push(run)
      return
    }
    const split = splitParagraphs(run, options)
    output.push(split === null ? run : split)
  }

  EXISTING_PARAGRAPH.lastIndex = 0
  let match: RegExpExecArray | null
  while ((match = EXISTING_PARAGRAPH.exec(html)) !== null) {
    flushRun(match.index)
    output.push(match[0])
    cursor = match.index + match[0].length
  }
  flushRun(html.length)
  return output.join('')
}
