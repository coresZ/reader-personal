// 阅读页的简繁转换。
//
// 引擎: opencc-js 的 Trie（纯 JavaScript，无 WASM / 无原生二进制），
// 词表: **手机端 legado 的同款词表**（quick-transfer-core 0.2.17），
//       由 scripts/build-legado-chinese-dicts.mjs 生成，不要手改。
//
// 为什么不用 opencc-js 自带的 preset —— 曾经用 from.tw → to.cn（纯字形），
// 也试过 from.twp → to.cn（带台湾用词）。两者都是 OpenCC 的词表，和手机端
// legado 用的词表**不是同一份**，于是同一本书在两端显示不同：
//
//   issue #12: twp 词表把「序列」本地化成「串行」；手机端没有这条。
//   issue #25: OpenCC 的 TSCharacters 把「乾」无条件映射成「干」，
//              只有 51 个短语例外，于是「乾國」→「干国」；
//              手机端 legado 的词表里「乾」没有单字映射，
//              只在「乾燥/乾淨/乾杯…」这类短语里转换，「乾國/乾隆/乾坤」原样保留。
//
// 「只做字形转换」这条规则本身挡不住 #25 —— 地区用词和异体字是两回事，
// 而 OpenCC 的单字表里就有「乾→干」这种需要语境才能判定的映射。真正的
// 判据是**换用手机端那份词表**：它逐条维护了语境（乾燥→干燥、乾隆→乾隆），
// legado 还额外用 fixT2sDict() 的排除表挡住 100 多个词（列印/光碟/介面…）。
// 生成脚本已把排除表合并进 legadoT2sDict.js。
//
// 两个方向都与手机端 legado 逐字一致（CJK 基本区 20992 字 + GB2312 全表
// 做过逐字比对，见 chinese.test.ts 的回归用例）。因此手机端会做的地区词
// 转换（雷射→激光、滑鼠→鼠标）这里也会做 —— 这是刻意的，不是 bug：
// 两端显示不一致才是 bug。
//
// 「关闭」档（chineseMode = 'off'）直接返回原文，与 legado 的「关闭」一致。

import * as OpenCC from 'opencc-js/core'
import legadoS2tDict from './legadoS2tDict.js'
import legadoT2sDict from './legadoT2sDict.js'

// ConverterFactory 参数是「词典组」: 传数组包一层才是 DictGroup。
const toSimplified = OpenCC.ConverterFactory([legadoT2sDict])
const toTraditional = OpenCC.ConverterFactory([legadoS2tDict])

/* ─── HTML 感知的转换 ───
 *
 *  正文里可能就是 HTML(例: `廣告\n正文漢字<img src="comment.png"/>`),
 *  而直接对整串做替换会把**标签属性值**也换掉:
 *      <p class="繁體類名">  →  <p class="繁体类名">   // CSS 选择器失效
 *      <a href=".../書">      →  <a href=".../书">       // 链接被改写
 *      <img alt="繁體圖片">   →  <img alt="繁体图片">    // 这个倒是想要的
 *
 *  readest 的做法可借鉴: 只对文本节点做转换(它用 TreeWalker(SHOW_TEXT))。
 *  这里不引入 DOM(转换发生在 store 的 computed 里, 且要能单测), 改用「切分
 *  标签与文本」的方式达到同样效果: 跳过 `<...>` 与 `&...;`, 只把其余文本交给
 *  词表。正则 `/<[^>]*>/g` 对正文里的良构标签足够; 未闭合的 `<` 会被当作
 *  文本处理(交给词表也无害)。 */
const TAG_OR_ENTITY = /(<[^>]*>|&[a-zA-Z#][a-zA-Z0-9]*;)/g

function convertHtmlAware(text, convert) {
  // 没有标签时走快路径, 省掉切分开销(纯文本正文是最常见的情况)。
  if (text.indexOf('<') < 0 && text.indexOf('&') < 0) {
    return convert(text)
  }
  let out = ''
  let last = 0
  TAG_OR_ENTITY.lastIndex = 0
  let match
  while ((match = TAG_OR_ENTITY.exec(text)) !== null) {
    out += convert(text.slice(last, match.index))
    out += match[0]   // 标签/实体原样保留
    last = match.index + match[0].length
  }
  out += convert(text.slice(last))
  return out
}

/** 繁体 → 简体（legado 词表）。 */
export const simplified = function (orgStr) {
  if (!orgStr) return orgStr
  return convertHtmlAware(orgStr, toSimplified)
}

/** 简体 → 繁体（legado 词表）。 */
export const traditionalized = function (orgStr) {
  if (!orgStr) return orgStr
  return convertHtmlAware(orgStr, toTraditional)
}
