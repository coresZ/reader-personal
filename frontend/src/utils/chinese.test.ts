// 阅读页简繁转换的回归测试(legado 同款词表)。
//
// 背景: issue #6 第 6 条「简繁转换对于繁体字的本地文件无效」。
// 根因是「简体」档从未真正转换 —— 当时只有一张简→繁表, 繁体字不在表里,
// 转换等于什么都没做, 于是繁体内容(常见于本地导入的书)选「简体」毫无变化。
//
// 引擎演进:
//   1. 自建字符表 —— 覆盖常用字, 但短语与地区词处理不了。
//   2. opencc-js 的 twp 预设 —— 会把「序列」本地化成「串行」(issue #12)。
//   3. opencc-js 的 tw 预设(纯字形) —— 仍与手机端不一致: OpenCC 的
//      TSCharacters 把「乾」无条件映射成「干」, 「乾國」因此变成「干国」
//      (issue #25)。
//   4. **现在**: 直接用手机端 legado 的同款词表(quick-transfer-core 0.2.17
//      的 tc/t2s.txt、tc/s2t.txt + legado 的 fixT2sDict() 排除表), 由
//      scripts/build-legado-chinese-dicts.mjs 生成。两端逐字一致。
//
// 这组测试同时锁住三件事:
//   1. 转换本身正确(含 issue #12 / #25 报告的两个方向)
//   2. **HTML 属性不被改写** —— 从整串替换改成文本节点级替换修掉的真实缺陷
//   3. 与 legado 一致: 该转的转(雷射→激光), 不该转的不转(列印/乾國)
import { describe, expect, it } from 'vitest'
import { simplified, traditionalized } from './chinese.js'

describe('简繁转换: 繁体 → 简体', () => {
  it('把常用繁体字转成简体(issue #6 第 6 条的直接诉求)', () => {
    expect(simplified('這是一段繁體中文內容，用來測試轉換。'))
      .toBe('这是一段繁体中文内容，用来测试转换。')
    expect(simplified('漢字轉換測試')).toBe('汉字转换测试')
    expect(simplified('體國學書閱讀轉換臺灣萬與為這個說時間'))
      .toBe('体国学书阅读转换台湾万与为这个说时间')
  })

  it('覆盖原本缺失的常用字', () => {
    // 自建表时代这些字前端转不了(靠后端表补); legado 词表一并覆盖。
    expect(simplified('丟佈係偽僕儘僱儂倖')).toBe('丢布系伪仆尽雇侬幸')
    expect(simplified('後爲啓夥屍擡')).toBe('后为启伙尸抬')
  })

  it('不做台湾用词本地化: 序列/程序保持原文(issue #12)', () => {
    expect(simplified('序列')).toBe('序列')
    expect(simplified('程序')).toBe('程序')
    expect(simplified('序列之爭')).toBe('序列之争')
    expect(simplified('進程')).toBe('进程')
  })

  it('按 legado 排除表原样保留地区词', () => {
    // legado 的 fixT2sDict() 把这些词从词典里删掉, 手机端就是这么显示的。
    for (const word of ['列印', '光碟', '介面', '乳酪', '相容']) {
      expect(simplified(word)).toBe(word)
    }
    // 不在排除表里的地区词照常转换(与手机端一致)。
    expect(simplified('雷射')).toBe('激光')
    expect(simplified('滑鼠')).toBe('鼠标')
    expect(simplified('軟體')).toBe('软件')
    expect(simplified('資訊')).toBe('信息')
  })

  it('语境相关: 同一个字在不同词里结果不同(issue #25)', () => {
    // 「乾」在干燥里要转, 在乾隆/乾國里不能转 —— OpenCC 的单字表做不到,
    // legado 的词表可以。这正是 #25 的根因。
    expect(simplified('乾燥')).toBe('干燥')
    expect(simplified('乾淨')).toBe('干净')
    expect(simplified('乾隆')).toBe('乾隆')
    expect(simplified('乾坤')).toBe('乾坤')
    expect(simplified('乾國')).toBe('乾国')
    // 单独一个字时保持原样(legado 词表里没有「乾」的单字映射)。
    expect(simplified('乾')).toBe('乾')
    // 「髮」与「發」同简化为「发」, 反向时靠词判定。
    expect(simplified('頭髮')).toBe('头发')
    expect(simplified('發財')).toBe('发财')
  })

  it('已是简体或非汉字的内容原样返回(幂等)', () => {
    for (const text of ['这是一段简体', 'abc123', '', '！？，。']) {
      expect(simplified(text)).toBe(text)
    }
    // 简体「干」不会被改写成「乾」: 词表里干=干, 这条身份映射挡住
    // 「干杯→乾杯」之类反向污染。
    expect(simplified('他干了一杯酒，干净利落地把活干完了。'))
      .toBe('他干了一杯酒，干净利落地把活干完了。')
  })

  it('空值与非法输入不抛异常', () => {
    // 正文可能还没加载(undefined/null) —— 转换必须原样返回而不是崩掉,
    // 否则 displayContent 这个同步 computed 会直接把渲染打断。
    for (const value of ['', undefined, null]) {
      expect(() => simplified(value as string)).not.toThrow()
      expect(simplified(value as string)).toBe(value)
    }
  })
})

describe('简繁转换: 简体 → 繁体', () => {
  it('把常用简体字转成繁体', () => {
    expect(traditionalized('汉字转换测试')).toBe('漢字轉換測試')
    expect(traditionalized('这是一段简体中文内容，用来测试转换。'))
      .toBe('這是一段簡體中文內容，用來測試轉換。')
  })

  it('与手机端一致: 不做台湾地区用词替换', () => {
    // legado 用的是「字形 + 少量常用词」的通用繁体表, 不是台灣用词表。
    expect(traditionalized('计算机')).toBe('計算機')
    expect(traditionalized('打印机')).toBe('打印機')
    expect(traditionalized('激光')).toBe('激光')
    expect(traditionalized('鼠标')).toBe('鼠標')
    expect(traditionalized('软件')).toBe('軟件')
    expect(traditionalized('信息')).toBe('信息')
    // 「发」的多义由词决定: 头发→頭髮, 发财→發財
    expect(traditionalized('头发')).toBe('頭髮')
    expect(traditionalized('发财')).toBe('發財')
    expect(traditionalized('序列')).toBe('序列')
    expect(traditionalized('程序')).toBe('程序')
  })

  it('已是繁体或非汉字的内容原样返回(幂等)', () => {
    for (const text of ['這是一段繁體', 'abc123', '', '！？，。']) {
      expect(traditionalized(text)).toBe(text)
    }
  })

  it('空值与非法输入不抛异常', () => {
    // 与 simplified 同理: 两个方向都要挡住空值, 否则会打断同步 computed。
    for (const value of ['', undefined, null]) {
      expect(() => traditionalized(value as string)).not.toThrow()
      expect(traditionalized(value as string)).toBe(value)
    }
  })
})

describe('简繁转换: HTML 安全', () => {
  // 正文里可能就是 HTML(真实样例见 readerReadConfig.test.ts:
  // '廣告\n正文漢字<img src="comment.png"/>')。
  // 曾经对整个字符串做替换, 于是 class/href 被改写、CSS 选择器失效。
  // 现在只转换标签与实体之外的文本。

  it('不改写文本内容以外的部分: class / href / alt 一律保留', () => {
    expect(simplified('<p class="繁體類名">文字</p>'))
      .toBe('<p class="繁體類名">文字</p>')
    expect(simplified('<a href="https://example.com/書">連結</a>'))
      .toBe('<a href="https://example.com/書">链接</a>')
    expect(simplified('<img src="a.png" alt="圖片說明">'))
      .toBe('<img src="a.png" alt="圖片說明">')
  })

  it('标签之间的文本仍被转换', () => {
    expect(simplified('<p>繁體段落</p>')).toBe('<p>繁体段落</p>')
    expect(simplified('廣告\n正文漢字<img src="comment.png"/>'))
      .toBe('广告\n正文汉字<img src="comment.png"/>')
    expect(simplified('<div class="章節"><span>標題</span><img src="x.png" title="圖"/></div>'))
      .toBe('<div class="章節"><span>标题</span><img src="x.png" title="圖"/></div>')
  })

  it('HTML 实体不被破坏', () => {
    expect(simplified('&amp;繁體&nbsp;內容')).toBe('&amp;繁体&nbsp;内容')
  })

  it('未闭合的尖括号按文本处理(不吞内容)', () => {
    // 正文有时带残缺标记, 不能因此丢掉后面的字。
    expect(simplified('a<b 繁體')).toBe('a<b 繁体')
  })
})
