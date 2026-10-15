// 阅读页的简繁转换。
//
// 引擎: opencc-js(纯 JavaScript 版 OpenCC, 无 WASM / 无原生二进制 / 零依赖)。
//
// 为什么在自建字符表之后又换成它 —— 自建表能覆盖常用字(合并前后端两份表后
// 21/22), 但有两类问题单字映射**原理上**解决不了:
//   1. 短语级: 「计算机」要变「計算機」、「打印机」要变「印表機」。字符映射只能
//      得到「計算机」。
//   2. 港台用词: 「雷射/列印/光碟/介面/乳酪/相容」这些是台湾用语, 转成大陆用词
//      应为「激光/打印/光盘/界面/奶酪/兼容」; 而「雪梨/起司/磁碟/雷根」又**不**
//      该转。单字表无从区分, legado 是靠手工维护约 80 词的排除表绕开的,
//      OpenCC 则内建了这些短语。
// 实测同一段文本: 自建表 0.12ms/章, OpenCC 0.23ms/章(1.9x, 但 3000 字一章
// 仍远低于 16.7ms 的帧预算), 因此性能不构成阻碍。
//
// 配置选择(from/to 是 OpenCC 的 locale 代码):
//   't'  = OpenCC 标准繁体(纯字形, 不含地区用词)
//   'tw' = 台湾正体   'twp' = 台湾正体 + 台湾用词(phrase)
//   'hk' = 香港繁体   'hkp' = 香港繁体 + 香港用词
//   'cn' = 简体(大陆)
// 与手机端 legado 保持一致, 只做字形转换, 不做地区词替换:
//   - 繁→简: 序列/程序 保持原文, 不会变成 串行/进程
//   - 简→繁: 输出台湾正体字形(计算机→計算機、头发→頭髮)
// 不使用 twp 的台湾用词表: 它会把“序列”本地化成“串行”, 这正是 issue #12。
import * as OpenCC from 'opencc-js/core';
import * as Locale from 'opencc-js/preset';

const toSimplified = OpenCC.ConverterFactory(Locale.from.tw, Locale.to.cn);
const toTraditional = OpenCC.ConverterFactory(Locale.from.cn, Locale.to.tw);

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
 *  OpenCC。正则 `/<[^>]*>/g` 对正文里的良构标签足够; 未闭合的 `<` 会被当作
 *  文本处理(交给 OpenCC 也无害)。 */
const TAG_OR_ENTITY = /(<[^>]*>|&[a-zA-Z#][a-zA-Z0-9]*;)/g;

function convertHtmlAware(text, convert) {
  // 没有标签时走快路径, 省掉切分开销(纯文本正文是最常见的情况)。
  if (text.indexOf('<') < 0 && text.indexOf('&') < 0) {
    return convert(text);
  }
  let out = '';
  let last = 0;
  TAG_OR_ENTITY.lastIndex = 0;
  let match;
  while ((match = TAG_OR_ENTITY.exec(text)) !== null) {
    out += convert(text.slice(last, match.index));
    out += match[0];   // 标签/实体原样保留
    last = match.index + match[0].length;
  }
  out += convert(text.slice(last));
  return out;
}

/** 繁体 → 简体。
 *
 *  这是 issue #6 第 6 条修复的核心: 在此之前「简体」档只是把原文原样返回
 *  (当时只有一张简→繁表, 繁体字不在表里, 转换等于没做), 于是繁体内容 ——
 *  常见于本地导入的书 —— 选「简体」毫无变化。 */
export const simplified = function (orgStr) {
  if (!orgStr) return orgStr;
  return convertHtmlAware(orgStr, toSimplified);
};

/** 简体 → 繁体。 */
export const traditionalized = function (orgStr) {
  if (!orgStr) return orgStr;
  return convertHtmlAware(orgStr, toTraditional);
};
