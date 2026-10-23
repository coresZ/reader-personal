import { computed, nextTick, ref } from 'vue'
import type { ComputedRef, Ref } from 'vue'
import type { useReaderStore } from '../stores/reader'
import { getReaderTextWidth } from '../utils/readerTextWidth'
import { READER_PAGE_VERTICAL_PADDING_MAX } from '../stores/reader'
import { parseReaderParagraph } from '../utils/readerParagraphRange'

type ReaderStore = ReturnType<typeof useReaderStore>
const HORIZONTAL_PAGE_SIDE_PADDING = 24
const HORIZONTAL_PAGE_MIN_BOTTOM_GUARD = 18
const HORIZONTAL_PAGE_BOTTOM_GUARD_LINES = 0.5
const NON_STARTING_PUNCTUATION = new Set([
  '，',
  '。',
  '、',
  '：',
  '；',
  '！',
  '？',
  ',',
  '.',
  ':',
  ';',
  '!',
  '?',
  '”',
  '’',
  '）',
  '】',
  '》',
  '」',
  '』',
])
const SPLIT_BACKTRACK_BREAKERS = new Set(['，', '。', '；', '！', '？', ',', '.', ';', '!', '?', ' '])
const PUNCTUATION_ONLY_CHARS = new Set([...NON_STARTING_PUNCTUATION, '“', '‘', '（', '【', '《', '「', '『', '…', '—'])
const MAX_PUNCTUATION_BACKTRACK = 12

/** 块容器里可以独占一块的原子媒体。 */
const MEDIA_ELEMENT_SELECTOR = 'img, svg, math, video, audio, iframe, canvas, object, embed'
/** `MEDIA_ELEMENT_SELECTOR` 的廉价字符串预检: 命中才值得做一次 DOM 解析。 */
const MEDIA_HINT_PATTERN = /<(?:img|svg|math|video|audio|iframe|canvas|object|embed)[\s/>]/i
/** 容器里出现这些结构时整块保留: 拦腰拆开会破坏表格/列表/代码块的完整语义。 */
const MEDIA_SPLIT_BLOCKER_SELECTOR = 'table, thead, tbody, tr, td, th, pre, ul, ol, li, dl, dt, dd, p, div, section, article, blockquote, h1, h2, h3, h4, h5, h6'
/** 书源常常把插图的高度写死(如 `height="932"`、`style="height:100%"`)。
 *  横向分页里页高固定, 这类声明会让图片纵向拉伸并把图后文字挤出可视区。
 *  这里只清掉**决定高度与定位/缩放的声明**, 书源自己的宽度设置(如 `,{width}` 选项)
 *  保持原样 —— 宽度超限由 max-width 兜住, 不影响画面比例。 */
const IMAGE_HEIGHT_STYLE_PROPERTIES = [
  'height',
  'min-height',
  'max-height',
  'aspect-ratio',
  'min-width',
  'position',
  'top',
  'right',
  'bottom',
  'left',
  'inset',
  'float',
  'object-fit',
  'transform',
  'zoom',
] as const
/** 段评气泡等行内小图标: 靠 zoom/em 尺寸维持小图, 不能按插图规则缩放。 */
const INLINE_ICON_IMAGE_SELECTOR = 'img.source-inline-bubble'
/** 拆块时**不能**复制到新段落的声明。
 *  尺寸/定位类属性原本作用于整个容器, 复制到分段后每个段落都会撑成一整页,
 *  页数会成倍膨胀且每页只剩一行字 —— 探针实测这种写法会把 14 页涨到 20 页。 */
const SPLIT_COPY_BLOCKED_STYLE_PROPERTIES = new Set([
  'width',
  'height',
  'min-width',
  'min-height',
  'max-width',
  'max-height',
  'aspect-ratio',
  'position',
  'top',
  'right',
  'bottom',
  'left',
  'inset',
  'float',
  'display',
  'flex',
  'flex-basis',
  'flex-grow',
  'flex-shrink',
  'align-self',
  'justify-self',
  'grid-column',
  'grid-row',
  'overflow',
  'overflow-x',
  'overflow-y',
  'box-sizing',
  'object-fit',
  'transform',
  'zoom',
])

/** 按顶层 `;` 切分 CSS 声明串。
 *
 *  不能直接 `split(';')`：`url(data:image/svg+xml;base64,…)` 这类值本身含分号，
 *  切开会产出 `background: url(data:image/svg+xml` 这种残缺声明 —— 结果是拆出的
 *  段落丢掉整条背景（浏览器忽略非法声明），甚至把 base64 尾巴当成新属性名。
 *  这里跟踪括号与引号深度，只在深度为 0 处切分。 */
function splitStyleDeclarations(styleText: string): string[] {
  const parts: string[] = []
  let current = ''
  let depth = 0
  let quote: string | null = null
  for (let i = 0; i < styleText.length; i++) {
    const ch = styleText[i]
    if (quote) {
      current += ch
      if (ch === quote && styleText[i - 1] !== '\\') quote = null
      continue
    }
    if (ch === '"' || ch === "'") {
      quote = ch
      current += ch
      continue
    }
    if (ch === '(') depth++
    else if (ch === ')') depth = Math.max(0, depth - 1)
    else if (ch === ';' && depth === 0) {
      parts.push(current)
      current = ''
      continue
    }
    current += ch
  }
  parts.push(current)
  return parts
}

/** 过滤容器样式: 只保留段落级排版声明(边距/缩进/对齐等)。 */
function filterBlockStyle(styleText: string): string {
  return splitStyleDeclarations(styleText)
    .map((item) => item.trim())
    .filter(Boolean)
    .filter((item) => {
      const idx = item.indexOf(':')
      if (idx <= 0) return false
      const property = item.slice(0, idx).trim().toLowerCase()
      return property.length > 0 && !SPLIT_COPY_BLOCKED_STYLE_PROPERTIES.has(property)
    })
    .join('; ')
}

/** 清掉一个元素上会把图片拉高/移出正常流的写法, 让页面高度只由内容决定。
 *
 *  **`width`/`height` 属性必须保留** —— 它们是浏览器在图片**下载完成之前**
 *  按比例预留占位的唯一依据(HTML 规范: 有这两个属性即可算出 aspect ratio)。
 *  删掉它们, 图片在加载前只有一行 broken-image 的高度(约 16px), 加载后突然
 *  变成几百像素, 正文在页内整块位移 —— 这正是 issue #21 的「带图章节排版延迟」。
 *  书源写的绝对像素确实不该决定最终显示尺寸, 但这件事交给 CSS 完成:
 *  下面的 `max-width: 100%` + `max-height` 已经接管了实际尺寸,
 *  属性只承担「加载前占位」这一个职责。
 *
 *  CSS 里的高度声明仍要清掉: 书源常写 `style="height:100%"`, 那会把图纵向拉满。 */
function stripImageLayout(element: Element) {
  if (!(element instanceof HTMLElement) && !(element instanceof SVGElement)) return
  for (const property of IMAGE_HEIGHT_STYLE_PROPERTIES) {
    element.style.removeProperty(property)
  }
}

/** 该元素是否只承载一个媒体(可能带空白文本), 用于判断它是不是纯媒体包裹层。 */
function hasOnlyImageContent(element: Element) {
  const elements = Array.from(element.children)
  if (elements.length !== 1 || !elements[0].matches(MEDIA_ELEMENT_SELECTOR)) return false
  return !Array.from(element.childNodes).some(
    (node) => node.nodeType === Node.TEXT_NODE && (node.textContent || '').trim(),
  )
}

/** 去掉段落首尾的空 <br>: 媒体边界切出来的碎段若带着分隔用的 <br>, 会多出一行空行。 */
function trimEdgeBreaks(paragraph: HTMLElement) {
  let first = paragraph.firstChild
  while (first instanceof HTMLBRElement) {
    const next = first.nextSibling
    first.remove()
    first = next
  }
  let last = paragraph.lastChild
  while (last instanceof HTMLBRElement) {
    const previous = last.previousSibling
    last.remove()
    last = previous
  }
}

/** 把「块容器 + 行内文字 + 媒体」拆成若干段落级块: 媒体独占一块, 前后文字各自成段。
 *
 *  横向分页只能按段落内的 token 切分, 撞上块级容器里的媒体时只能整块塞进一页 ——
 *  图片过高时, 图后的文字就被 `.horizontal-page-content { overflow: hidden }` 裁掉,
 *  这正是 issue #10。先按媒体边界把容器拆开, 媒体和前后文字就能各自参与分页。
 *
 *  只在容器内容是纯行内流时拆分; 结构化内容(表格/列表/嵌套块)保持原样。
 *  返回 null 表示无需或不适合拆分。 */
function splitBlockAtMedia(block: Element): string[] | null {
  if (!block.querySelector(MEDIA_ELEMENT_SELECTOR)) return null
  if (block.querySelector(MEDIA_SPLIT_BLOCKER_SELECTOR)) return null

  // 容器样式只在能被段落安全继承时带过去, 否则拆出来的每个段落都会撑满整页。
  const style = filterBlockStyle(block.getAttribute('style') || '')
  const blocks: string[] = []
  const flush = (nodes: Node[]) => {
    if (!nodes.some((node) => node instanceof Element || (node.textContent || '').trim())) return
    const paragraph = document.createElement('p')
    nodes.forEach((node) => paragraph.appendChild(node.cloneNode(true)))
    trimEdgeBreaks(paragraph)
    if (!paragraph.childNodes.length) return
    if (style) paragraph.setAttribute('style', style)
    blocks.push(paragraph.outerHTML)
  }

  let inline: Node[] = []
  block.childNodes.forEach((node) => {
    if (node instanceof Element && node.matches(MEDIA_ELEMENT_SELECTOR)) {
      flush(inline)
      inline = []
      flush([node])
      return
    }
    inline.push(node)
  })
  flush(inline)

  return blocks.length > 1 ? blocks : null
}

/** 已解码插图的真实尺寸: `src -> "宽x高"`。
 *
 *  为什么需要缓存: 每次重建分页都会从 HTML 字符串重新创建一遍 `<img>`, 那是全新的
 *  元素, `naturalWidth/Height` 要等它自己解码完才有效。重建却必须当场算出比例,
 *  否则这一轮又读不到尺寸。书源插图绝大多数不带 `width`/`height` 属性(实测缓存里
 *  2323 张有 2321 张没有), 这个缓存就是它们唯一的尺寸来源。 */
const decodedImageSizes = new Map<string, string>()

/** 记录一张已解码插图的真实尺寸。由阅读页在图片 `load` 时调用。 */
export function rememberDecodedImageSize(
  src: string,
  width: number,
  height: number,
): void {
  if (!src || !(width > 0) || !(height > 0)) return
  const size = `${width}x${height}`
  if (decodedImageSizes.get(src) === size) return
  decodedImageSizes.set(src, size)
}

/** 读取缓存的真实尺寸; 没有记录时返回 null。 */
function readDecodedImageSize(src: string): { width: number; height: number } | null {
  const cached = decodedImageSizes.get(src)
  if (!cached) return null
  const [width, height] = cached.split('x').map(Number)
  if (!(width > 0) || !(height > 0)) return null
  return { width, height }
}

/** 给横向分页的媒体元素设一页之内的最大高度。
 *
 *  竖长图原本按原始比例铺满行宽, 高度可以远超一屏; 页容器 `overflow: hidden` 会把
 *  超出部分连同图片下方的文字一起裁掉, 这正是 issue #10 的现象。压上高度上限后,
 *  浏览器按比例缩小图片, 图后文字也会被切到后续页而不是被裁。
 *
 *  高度要**留出余量**(两行), 不能顶满整页: 块自身还有段落外边距, 而分页器按字符
 *  token 二分切片时, 只要图片能和至少一个字符同页, 剩下的文字才会流到下一页。
 *  图片顶满一页则一个字符都放不下, 整块会被判成不可切分而塞进同一页, 图后文字
 *  又被裁掉 —— 修了缩放却留下同一个症状。
 *
 *  约束对象与拆块用的媒体集合一致（不止 `img`）: 过高的 `svg`/`video`/`iframe` 等
 *  会撑满整页并把后续文字裁掉, 与竖长图是同一个症状。
 *
 *  测量与渲染必须用同一份结果: 分页器把这里的输出同时喂给测量器和 v-html,
 *  否则测量高度与实际排版不符, 页边界会再次错位。 */
function constrainBlockMedia(blockHtml: string, pageHeightPx: number, lineHeightPx: number): string {
  // 绝大多数块不含媒体: 先用字符串挡掉, 避免每个块都做一次 DOM 解析。
  if (!MEDIA_HINT_PATTERN.test(blockHtml)) return blockHtml
  const wrapper = document.createElement('div')
  wrapper.innerHTML = blockHtml
  // 显式给出返回类型: `querySelectorAll` 默认返回 `Element`, 而 `style` 由 lib.dom
  // 的 `ElementCSSInlineStyle` 混入提供, `Element` 上没有声明 —— 下面三行会报
  // TS2339。选择器只命中可替换元素, 它们恰好分属这三个接口, 都实现了该混入。
  // 这里只是补类型, 运行时对象与改动前完全一致(不做 instanceof 过滤, 免得顺带
  // 把 `math` 排除在约束之外)。
  const media = Array.from(
    wrapper.querySelectorAll<HTMLElement | SVGElement | MathMLElement>(MEDIA_ELEMENT_SELECTOR),
  )
  if (!media.length) return blockHtml
  // 下限兜底: 页面很矮时也别把媒体压得只剩一条缝。
  const maxHeight = Math.max(pageHeightPx * 0.5, pageHeightPx - lineHeightPx * 2)
  for (const element of media) {
    // 段评气泡等行内小图标靠 em 尺寸维持在行内, 不参与插图缩放。
    if (element.matches(INLINE_ICON_IMAGE_SELECTOR)) continue
    // 没有 `width`/`height` 属性时, 用已解码的真实尺寸补上这两个属性。
    // 下面是按属性推导宽高比的(`height: auto`), 属性缺省就推不出比例 —— 补上之后
    // 这类插图才和带属性的插图一样, 在加载前就能占住正确高度。
    const declaredWidth = Number(element.getAttribute('width'))
    const declaredHeight = Number(element.getAttribute('height'))
    if (!(declaredWidth > 0 && declaredHeight > 0)) {
      const cached = readDecodedImageSize(element.getAttribute('src') || '')
      if (cached) {
        element.setAttribute('width', String(cached.width))
        element.setAttribute('height', String(cached.height))
      }
    }
    // 书源常给插图写死布局(如 `style="width:100%;height:100%"`)。页容器高度固定,
    // 那个 height 会把画面撑满整页并纵向拉伸, 图后文字随即被挤出可视区 —— 而 EPUB
    // 路径进过清洗器, 这些属性早被剥掉, 所以同样的插图在 EPUB 里是正常的。
    // 这里先清掉书源的布局声明, 只留下按比例缩放用的两条约束。
    stripImageLayout(element)
    element.style.maxWidth = '100%'
    element.style.maxHeight = `${maxHeight}px`
    // `height: auto` 让宽高比由 `<img>` 自带的 width/height 属性推导:
    // 加载前就能占住正确高度(不塌成 broken-image 的一行), 加载后比例也对。
    // 缺了它, `max-height` 会把图按属性绝对高度直接截断, 画面被压扁。
    element.style.height = 'auto'
    // 兜底: 万一还有别的来源给死了宽高, contain 至少不会把画面压扁。
    // 只对可替换元素设置: `svg`/`iframe` 用不上该属性, 设了也无害。
    element.style.objectFit = 'contain'

    // 书源也常把布局写在图片的包裹层上(如 `<div style="height:100%"><img/></div>`)。
    // 包裹层若只剩图片, 它的 height 同样会把这一格撑满整页, 把图后文字挤出可视区。
    let ancestor = element.parentElement
    while (ancestor && ancestor !== wrapper && hasOnlyImageContent(ancestor)) {
      stripImageLayout(ancestor)
      ancestor = ancestor.parentElement
    }
  }
  return wrapper.innerHTML
}

export function useHorizontalPaging(
  store: ReaderStore,
  config: ComputedRef<{
    fontSize: number
    fontWeight: number
    lineHeight: number
    pageWidth: number
    pageVerticalPadding: number
    paragraphSpacing: number
    firstLineIndent: boolean
  }>,
  currentFontFamily: ComputedRef<string>,
  formattedContent: ComputedRef<string>,
  isHorizontalPageMode: ComputedRef<boolean>,
  scrollContainerRef: Ref<HTMLElement | undefined>,
) {
  const horizontalPageIndex = ref(0)
  /** **屏宽**(= 容器宽)。一屏恰好一页, 翻页位移 = 索引 × 屏宽。
   *  注意这不是行宽: 行宽由「页面宽度」设置在页内单独控制(见 `horizontalTextWidth`)。 */
  const horizontalPageStep = ref(1)
  /** 一页内正文列的宽度(行宽), 由设置里的「页面宽度」决定。 */
  const horizontalTextWidth = ref(0)
  const horizontalPageStepStyle = computed(() => `${Math.max(1, horizontalPageStep.value)}px`)
  const horizontalPages = ref<string[]>([])
  /** 最近一次分页实际用到的正文快照。换章后 `horizontalPages` 还是**上一章**的
   *  分页结果, 而 `formattedContent` 已经是新章的 —— 两者不等就说明分页过期。
   *  搜索跳转要靠它决定"能否直接按现有页定位", 否则会落到上一章的页上。 */
  let pagedContentSnapshot: string | null = null
  /** 最近一次分页使用的尺寸/排版签名。窗口最大化或改排版时，旧页在新尺寸下
   *  会短暂露出底部残留，必须在重建前撤下。 */
  let pagedLayoutSignature = ''

  /** 最后一页的索引。一屏一页, 所以末页索引就是页数 - 1。 */
  const horizontalLastPageStart = computed(() =>
    Math.max(0, horizontalPages.value.length - 1),
  )

  function escapeHtml(input: string) {
    return input
      .replaceAll('&', '&amp;')
      .replaceAll('<', '&lt;')
      .replaceAll('>', '&gt;')
  }

  function parseInlineStyle(styleText: string) {
    const entries = styleText
      .split(';')
      .map((item) => item.trim())
      .filter(Boolean)
      .map((item) => {
        const idx = item.indexOf(':')
        if (idx <= 0) return null
        const key = item.slice(0, idx).trim().toLowerCase()
        const value = item.slice(idx + 1).trim()
        if (!key || !value) return null
        return [key, value] as const
      })
      .filter(Boolean) as Array<readonly [string, string]>
    return Object.fromEntries(entries) as Record<string, string>
  }

  function buildInlineStyle(styleObj: Record<string, string>) {
    return Object.entries(styleObj)
      .map(([key, value]) => `${key}: ${value}`)
      .join('; ')
  }

  function removeIndentClass(className: string) {
    return className
      .split(/\s+/)
      .filter(Boolean)
      .filter((name) => name !== 'reader-indent')
      .join(' ')
  }

  function normalizeParagraphHtml(paragraphHtml: string) {
    const wrapper = document.createElement('div')
    wrapper.innerHTML = paragraphHtml
    const paragraph = wrapper.querySelector('p')
    if (!paragraph) return paragraphHtml
    const style = paragraph.getAttribute('style') || ''
    const styleObj = parseInlineStyle(style)
    if (paragraph.classList.contains('reader-indent') && !styleObj['text-indent']) {
      styleObj['text-indent'] = '2em'
    }
    const styleText = buildInlineStyle(styleObj)
    if (styleText) paragraph.setAttribute('style', styleText)
    return paragraph.outerHTML
  }

  function firstVisibleChar(text: string) {
    return text.trimStart().charAt(0)
  }

  function isPunctuationOnlyText(text: string) {
    const value = text.trim()
    return value.length > 0 && value.length <= 3 && Array.from(value).every((char) => PUNCTUATION_ONLY_CHARS.has(char))
  }

  function adjustFitCountForReadableStart(text: string[], fitCount: number) {
    if (fitCount <= 0 || fitCount >= text.length) return fitCount
    if (!NON_STARTING_PUNCTUATION.has(firstVisibleChar(text.slice(fitCount).join('')))) return fitCount

    const minIndex = Math.max(1, fitCount - MAX_PUNCTUATION_BACKTRACK)
    for (let idx = fitCount - 1; idx >= minIndex; idx -= 1) {
      if (SPLIT_BACKTRACK_BREAKERS.has(text[idx])) {
        return idx + 1
      }
    }
    return Math.max(1, fitCount - Math.min(4, fitCount - 1))
  }

  function extractTextFromHtml(html: string) {
    const wrapper = document.createElement('div')
    wrapper.innerHTML = html
    return {
      text: wrapper.textContent || '',
      hasMedia: Boolean(wrapper.querySelector('img, svg, math, video, audio, iframe, canvas, object, embed, table')),
    }
  }

  function prependTextToFirstParagraph(html: string, text: string) {
    const wrapper = document.createElement('div')
    wrapper.innerHTML = html
    const paragraph = wrapper.querySelector('p')
    if (!paragraph) return `${escapeHtml(text)}${html}`
    paragraph.insertBefore(document.createTextNode(text), paragraph.firstChild)
    return wrapper.innerHTML
  }

  function appendTextToLastParagraph(html: string, text: string) {
    const wrapper = document.createElement('div')
    wrapper.innerHTML = html
    const paragraphs = wrapper.querySelectorAll('p')
    const paragraph = paragraphs[paragraphs.length - 1]
    if (!paragraph) return `${html}${escapeHtml(text)}`
    paragraph.appendChild(document.createTextNode(text))
    return wrapper.innerHTML
  }

  function mergeOrphanPunctuationPages(sourcePages: string[]) {
    const pendingPages = [...sourcePages]
    const mergedPages: string[] = []
    for (let idx = 0; idx < pendingPages.length; idx += 1) {
      const page = pendingPages[idx]
      const { text, hasMedia } = extractTextFromHtml(page)
      if (!hasMedia && isPunctuationOnlyText(text)) {
        const punctuation = text.trim()
        if (mergedPages.length) {
          mergedPages[mergedPages.length - 1] = appendTextToLastParagraph(mergedPages[mergedPages.length - 1], punctuation)
          continue
        }
        if (idx + 1 < pendingPages.length) {
          pendingPages[idx + 1] = prependTextToFirstParagraph(pendingPages[idx + 1], punctuation)
          continue
        }
      }
      mergedPages.push(page)
    }
    return mergedPages
  }

  function buildHorizontalBlocks() {
    const root = document.createElement('div')
    root.innerHTML = formattedContent.value
    const blocks: string[] = []
    let inline: Node[] = []
    const blockTags = new Set(['DIV', 'SECTION', 'ARTICLE', 'BLOCKQUOTE', 'UL', 'OL', 'TABLE', 'PRE', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6'])
    const flushInline = () => {
      if (inline.some((node) => node instanceof Element || (node.textContent || '').trim())) {
        const paragraph = document.createElement('p')
        inline.forEach((node) => paragraph.appendChild(node.cloneNode(true)))
        blocks.push(normalizeParagraphHtml(paragraph.outerHTML))
      }
      inline = []
    }
    const visit = (node: Node) => {
      if (!(node instanceof Element)) { inline.push(node); return }
      if (node.tagName === 'P') {
        flushInline()
        blocks.push(normalizeParagraphHtml(node.outerHTML))
      } else if (node.querySelector('p')) {
        flushInline()
        node.childNodes.forEach(visit)
        flushInline()
      } else if (blockTags.has(node.tagName)) {
        flushInline()
        // 块级容器里可能混着行内文字和图片: 先按媒体边界拆开, 图与文字才能分别分页。
        const split = splitBlockAtMedia(node)
        if (split) {
          blocks.push(...split)
        } else {
          blocks.push(node.outerHTML)
        }
      } else {
        inline.push(node)
      }
    }
    root.childNodes.forEach(visit)
    flushInline()
    return blocks
  }

  /** 屏宽恒等于容器宽(一屏一页), 并算出页内正文行宽。
   *
   *  这里修的是一个**语义混淆**: 旧实现把「页面宽度」同时当成了屏宽和行宽 ——
   *  屏宽 = min(容器宽, 设置值), 于是 1920 宽的窗口里每页只有 800px, 一屏会同时
   *  露出当前页和下一页的左半, 右边还留一大片空白, 用户只能把窗口拖窄来回避。
   *
   *  现在两者分开: 屏宽 = 容器宽(一屏恰好一页), 「页面宽度」只管页内正文列多宽;
   *  窗口比它宽时正文居中、两侧留白(居中由 CSS 的 margin auto 完成)。 */
  function updateHorizontalMetrics() {
    const container = scrollContainerRef.value
    if (!container || !isHorizontalPageMode.value) return
    const containerWidth = Math.max(1, container.clientWidth)
    horizontalPageStep.value = containerWidth
    // 可用宽度 = 屏宽扣掉页容器左右 padding。与 .horizontal-page 的 padding 一致,
    // 也必须是测量正文时用的同一个值, 否则分页与实际排版会不一致。
    const availableWidth = Math.max(1, containerWidth - HORIZONTAL_PAGE_SIDE_PADDING * 2)
    horizontalTextWidth.value = getReaderTextWidth(availableWidth, config.value.pageWidth)
  }

  function getHorizontalPageMeasure(container: HTMLElement) {
    const lineHeightPx = config.value.fontSize * config.value.lineHeight
    const verticalPadding = Math.min(
      READER_PAGE_VERTICAL_PADDING_MAX,
      Math.max(0, Math.round(config.value.pageVerticalPadding)),
    )
    // 上下边距为 0 是用户的明确选择，此时不能再偷偷扣一段“保护距离”，
    // 否则正文底部永远贴不到边界。默认边距下仍保留最后一行保护，避免
    // 分页测量偏差把末尾行裁掉。
    const bottomGuard = verticalPadding > 0
      ? Math.max(
          HORIZONTAL_PAGE_MIN_BOTTOM_GUARD,
          Math.ceil(lineHeightPx * HORIZONTAL_PAGE_BOTTOM_GUARD_LINES),
        )
      : 0
    // 正文按**行宽**测量(不是屏宽): 页容器两侧的 padding 已在 availableWidth 里
    // 扣掉, 所以这里直接用行宽作为测量宽度, 与 .horizontal-page-content 的实际
    // 宽度保持一致。
    const textWidth = horizontalTextWidth.value > 0
      ? horizontalTextWidth.value
      : Math.max(120, horizontalPageStep.value - HORIZONTAL_PAGE_SIDE_PADDING * 2)
    return {
      innerWidth: Math.max(120, textWidth),
      pageHeight: Math.max(160, container.clientHeight - verticalPadding * 2 - bottomGuard),
    }
  }

  async function rebuildHorizontalPages() {
    if (!isHorizontalPageMode.value) {
      horizontalPages.value = []
      pagedContentSnapshot = null
      pagedLayoutSignature = ''
      return
    }
    await nextTick()
    const container = scrollContainerRef.value
    if (!container) return

    updateHorizontalMetrics()

    const { innerWidth, pageHeight } = getHorizontalPageMeasure(container)
    const contentSnapshot = formattedContent.value
    const layoutSignature = [
      innerWidth,
      pageHeight,
      config.value.fontSize,
      config.value.fontWeight,
      config.value.lineHeight,
      config.value.paragraphSpacing,
      config.value.firstLineIndent,
      config.value.pageWidth,
      config.value.pageVerticalPadding,
    ].join('::')
    // 正文换章或页尺寸变化时先把上一版页撤下，等新页构建完成再赋值。
    // 这样最大化窗口下不会在底部闪出旧页最后一行。
    if (
      pagedContentSnapshot !== null
      && (pagedContentSnapshot !== contentSnapshot || pagedLayoutSignature !== layoutSignature)
    ) {
      horizontalPages.value = []
      pagedContentSnapshot = null
      pagedLayoutSignature = ''
      await nextTick()
    }
    const title = (store.currentChapter?.title || '加载中...').trim()
    const titleHtml = `<h1 class="horizontal-flow-title">${escapeHtml(title)}</h1>`
    // 媒体高度上限 = 一页可用高度。分页与渲染共用同一份 HTML, 保证测量与实际排版
    // 一致(见 constrainBlockMedia)。
    const lineHeightPx = config.value.fontSize * config.value.lineHeight
    const paragraphs = buildHorizontalBlocks().map((block) =>
      constrainBlockMedia(block, pageHeight, lineHeightPx),
    )

    const measurer = document.createElement('div')
    measurer.className = 'chapter-text horizontal-page-content'
    for (const attribute of Array.from(container.attributes)) {
      if (attribute.name.startsWith('data-v-')) measurer.setAttribute(attribute.name, attribute.value)
    }
    measurer.style.position = 'fixed'
    measurer.style.left = '-99999px'
    measurer.style.top = '0'
    measurer.style.width = `${innerWidth}px`
    measurer.style.height = `${pageHeight}px`
    measurer.style.overflow = 'hidden'
    measurer.style.visibility = 'hidden'
    measurer.style.pointerEvents = 'none'
    measurer.style.boxSizing = 'border-box'
    measurer.style.fontSize = `${config.value.fontSize}px`
    measurer.style.fontWeight = String(config.value.fontWeight)
    measurer.style.lineHeight = String(config.value.lineHeight)
    measurer.style.fontFamily = currentFontFamily.value || ''
    measurer.style.wordBreak = 'normal'
    measurer.style.overflowWrap = 'break-word'
    measurer.style.textAlign = 'left'
    document.body.appendChild(measurer)

    const pages: string[] = []
    let currentParts: string[] = [titleHtml]

    const measureContentHeight = (parts: string[]) => {
      measurer.innerHTML = parts.join('')
      const children = Array.from(measurer.children) as HTMLElement[]
      if (!children.length) return 0
      const top = measurer.getBoundingClientRect().top
      return children.reduce((bottom, child, idx) => {
        const rect = child.getBoundingClientRect()
        const style = window.getComputedStyle(child)
        const isLastParagraph = idx === children.length - 1 && child.tagName.toLowerCase() === 'p'
        const marginBottom = isLastParagraph ? 0 : parseFloat(style.marginBottom) || 0
        return Math.max(bottom, rect.bottom - top + marginBottom)
      }, 0)
    }

    const overflows = (parts: string[]) => {
      return measureContentHeight(parts) > measurer.clientHeight
    }

    const flushPage = () => {
      if (!currentParts.length) return
      pages.push(currentParts.join(''))
      currentParts = []
    }

    const buildSegmentStyle = (style: string, isContinuation: boolean, hasMoreText: boolean) => {
      const styleObj = parseInlineStyle(style)
      if (hasMoreText) delete styleObj['margin-bottom']
      if (isContinuation) delete styleObj['text-indent']
      return buildInlineStyle(styleObj)
    }

    const fitParagraphSegment = (
      blockHtml: string,
      options: { isContinuation: boolean; minRemainingLines?: number },
    ) => {
      const parsed = parseReaderParagraph(blockHtml)
      if (!parsed || parsed.tokens.length <= 1) return null

      const { style, tokens: text, className } = parsed
      const currentHeight = measureContentHeight(currentParts)
      const remainingHeight = measurer.clientHeight - currentHeight
      const minRemainingHeight = (options.minRemainingLines || 0) * config.value.fontSize * config.value.lineHeight
      if (remainingHeight < minRemainingHeight) return null

      const segmentClassName = options.isContinuation ? removeIndentClass(className) : className
      let left = 1
      let right = text.length
      let fitCount = 0
      while (left <= right) {
        const mid = Math.floor((left + right) / 2)
        const tryStyle = buildSegmentStyle(style, options.isContinuation, mid < text.length)
        const tryHtml = parsed.slice(0, mid, tryStyle, segmentClassName)
        if (!overflows([...currentParts, tryHtml])) {
          fitCount = mid
          left = mid + 1
        } else {
          right = mid - 1
        }
      }

      if (fitCount <= 0) return null

      fitCount = adjustFitCountForReadableStart(text, fitCount)
      if (fitCount <= 0 || (fitCount < text.length && isPunctuationOnlyText(text.slice(0, fitCount).join('')))) return null

      const hasMoreText = fitCount < text.length
      const fitStyle = buildSegmentStyle(style, options.isContinuation, hasMoreText)
      return {
        html: parsed.slice(0, fitCount, fitStyle, segmentClassName),
        remainingHtml: hasMoreText
          ? parsed.slice(fitCount, text.length, style, removeIndentClass(className))
          : '',
      }
    }

    const appendOversizedParagraph = (blockHtml: string, isContinuation = false) => {
      const parsed = parseReaderParagraph(blockHtml)
      if (!parsed || parsed.tokens.length <= 1) {
        pages.push(blockHtml)
        return
      }

      let pending = blockHtml
      let continuation = isContinuation
      while (pending) {
        const fitted = fitParagraphSegment(pending, { isContinuation: continuation })
        if (fitted) {
          currentParts = [...currentParts, fitted.html]
          pending = fitted.remainingHtml
          continuation = true
          if (pending) flushPage()
          continue
        }

        if (currentParts.length) {
          flushPage()
          continue
        }

        pages.push(pending)
        pending = ''
      }
    }

    const appendBlock = (blockHtml: string) => {
      const withBlock = [...currentParts, blockHtml]
      if (!overflows(withBlock)) {
        currentParts = withBlock
        return
      }

      if (currentParts.length) {
        const fitted = fitParagraphSegment(blockHtml, { isContinuation: false })
        if (fitted) {
          currentParts = [...currentParts, fitted.html]
          if (fitted.remainingHtml) {
            flushPage()
            appendOversizedParagraph(fitted.remainingHtml, true)
          }
          return
        }

        flushPage()
        if (!overflows([blockHtml])) {
          currentParts = [blockHtml]
          return
        }
      }

      appendOversizedParagraph(blockHtml)
    }

    for (const paragraph of paragraphs) {
      appendBlock(paragraph)
    }

    if (currentParts.length) {
      flushPage()
    }

    if (!pages.length) {
      pages.push(titleHtml)
    } else if (pages.length === 1 && !pages[0].includes('horizontal-flow-title')) {
      pages[0] = `${titleHtml}${pages[0]}`
    }

    const mergedPages = mergeOrphanPunctuationPages(pages)

    document.body.removeChild(measurer)
    horizontalPages.value = mergedPages
    pagedContentSnapshot = contentSnapshot
    pagedLayoutSignature = layoutSignature
    // 重建后页数可能变少: 夹进有效范围, 否则索引会指向不存在的页(画面停在空白处)。
    horizontalPageIndex.value = Math.max(
      0,
      Math.min(horizontalLastPageStart.value, horizontalPageIndex.value),
    )

    container.scrollTo({ left: 0, behavior: 'auto' })
    updateHorizontalMetrics()
  }

  function alignHorizontalToNearestPage(touchMoving: boolean) {
    const container = scrollContainerRef.value
    if (!container || !isHorizontalPageMode.value || touchMoving) return
    updateHorizontalMetrics()
    horizontalPageIndex.value = Math.max(
      0,
      Math.min(horizontalLastPageStart.value, horizontalPageIndex.value),
    )
    if (container.scrollLeft !== 0) {
      container.scrollTo({ left: 0, behavior: 'auto' })
    }
  }

  function resetHorizontalPagePosition() {
    horizontalPageIndex.value = 0
    const container = scrollContainerRef.value
    if (!container) return
    updateHorizontalMetrics()
    container.scrollTo({ left: 0, behavior: 'auto' })
  }

  /** 当前 `horizontalPages` 是否还是别的内容分出来的页(换章/改排版后重建之前)。 */
  function isHorizontalPagesStale() {
    return pagedContentSnapshot !== formattedContent.value
  }

  return {
    horizontalPageIndex,
    horizontalPageStep,
    horizontalPageStepStyle,
    horizontalTextWidth,
    horizontalPages,
    horizontalLastPageStart,
    isHorizontalPagesStale,
    rebuildHorizontalPages,
    updateHorizontalMetrics,
    alignHorizontalToNearestPage,
    resetHorizontalPagePosition,
  }
}
