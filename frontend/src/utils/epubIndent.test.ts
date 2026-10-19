import { describe, expect, it } from 'vitest'
import SOURCE from '../views/ReaderView.vue?raw'

/**
 * 回归护栏: 本地 EPUB 的首行缩进。
 *
 * 曾经的缺陷: 阅读器兜底 `.chapter-text p { text-indent: 0 }` 特异性(0,2,1)压过
 * 原书 `.epub-chapter-content p { text-indent: 2em }`(0,1,1), 原书缩进被静默清零。
 *
 * 本方案分两条路, 都刻意**不碰分页引擎**:
 *   1. 上下滑动: 用 `:where()` 把兜底特异性归零, 原书排版自然生效 —— 这样
 *      `p.foot`/`.sign` 等刻意 text-indent:0 的例外段也能保持原意。
 *   2. 左右翻页: 分页会把容器展开, 原书选择器全部失效, 只能靠段落上的
 *      `reader-indent` 类, 由分页器既有的 normalizeParagraphHtml 转成内联样式。
 *
 * 之前改过分页的测量与容器包装, 引发了正文被裁; 这次明确约束: 缩进逻辑不得
 * 触及分页的测量代码。
 */
describe('本地 EPUB 首行缩进', () => {
  it('lets the book stylesheet win in scroll mode by zeroing the fallback specificity', () => {
    // :where() 把整条选择器(含 scoped 属性)的特异性压到 0
    expect(SOURCE).toMatch(/:deep\(:where\(\.chapter-text p\)\)\s*\{\s*text-indent:\s*0/)
  })

  it('keeps the book-source default indent in a zero-specificity wrapper', () => {
    // 不能退回未分层的 `.chapter-text p { text-indent: 0 }`
    // —— 那正是压制原书的元凶。
    const styleBlock = SOURCE.match(/<style[^>]*>([\s\S]*?)<\/style>/)?.[1] ?? ''
    const stripped = styleBlock.replace(/\/\*[\s\S]*?\*\//g, '')
    expect(stripped).not.toMatch(/\.chapter-text p\s*\{[^}]*text-indent:\s*0[^!]/)
  })

  it('drives paged mode through the existing reader-indent class', () => {
    // 分页下容器会被展开, 必须靠类把缩进带过去
    expect(SOURCE).toMatch(/READER_INDENT_CLASS = 'reader-indent'/)
    expect(SOURCE).toMatch(/paragraph\.classList\.add\(READER_INDENT_CLASS\)/)
    // 仅在分页模式才加类: 非分页交给原书 CSS, 否则会把例外段也缩进
    expect(SOURCE).toMatch(/config\.value\.firstLineIndent && isHorizontalPageMode\.value/)
  })

  it('forces everything flat when the user turns the switch off', () => {
    expect(SOURCE).toMatch(
      /\.epub-chapter-content\.reader-no-indent p\)\s*\{\s*text-indent:\s*0\s*!important/,
    )
    expect(SOURCE).toMatch(/classList\.toggle\(EPUB_NO_INDENT_CLASS, !config\.value\.firstLineIndent\)/)
  })

  it('does not touch the paging engine', () => {
    // 缩进逻辑放在 ReaderView 里; 一旦有人把它搬进 useHorizontalPaging 的测量
    // 路径, 就会再次面临正文被裁的风险。
    expect(SOURCE).not.toMatch(/preloadChapterImageSizes/)
    expect(SOURCE).not.toMatch(/wrapPageContent/)
  })
})
