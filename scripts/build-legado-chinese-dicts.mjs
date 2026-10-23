// 从 legado 手机端同款词表生成前端的简繁转换词典模块。
//
// 用法:
//   1. 取 quick-transfer-core-0.2.17.jar（Maven Central / JitPack）
//   2. 解出其中的 tc/t2s.txt、tc/s2t.txt（UTF-8，每行 `源=目标`）
//   3. node scripts/build-legado-chinese-dicts.mjs <t2s.txt> <s2t.txt> <输出目录>
//
// 生成 `legadoT2sDict.js`（繁 → 简）与 `legadoS2tDict.js`（简 → 繁）。
// 条目会以 opencc-js 的序列化词典格式（`源 目标|源 目标|…`）写入，
// 运行时由 opencc-js 的 Trie 做最长匹配 —— 与 quick-transfer 的
// BasicDictionary 一致（同长度时先加入者生效，因此保持原文件顺序）。
//
// 繁 → 简方向额外合并 legado ChineseUtils.fixT2sDict() 的排除表：
// legado 用它在启动时把这些词从词典里删掉，等价于让它们「原样保留」。
import fs from 'node:fs'
import path from 'node:path'

// legado app/src/main/java/io/legado/app/utils/ChineseUtils.kt 的 fixT2sDict()，
// 逐字照抄（原表里有重复项，重复写入 Map 无副作用）。
const LEGADO_EXCLUDE = [
  '槃',
  '划槳', '列根', '雪梨', '雪糕', '多士', '起司', '芝士', '沙芬', '母音',
  '华乐', '民乐', '晶元', '晶片', '映像', '明覆', '明瞭', '新力', '新喻',
  '零錢', '零钱', '離線', '碟片', '模組', '桌球', '案頭', '機車', '電漿',
  '鳳梨', '魔戒', '載入', '菲林', '整合', '變數', '解碼', '散钱', '插水',
  '房屋', '房价', '快取', '德士', '建立', '常式', '席丹', '布殊', '布希',
  '巴哈', '巨集', '夜学', '向量', '半形', '加彭', '列印', '函式', '全形',
  '光碟', '介面', '乳酪', '沈船', '永珍', '演化', '牛油', '相容', '磁碟',
  '菲林', '規則', '酵素', '雷根', '饭盒',
  '路易斯', '非同步', '出租车', '周杰倫', '马铃薯', '馬鈴薯', '機械人', '電單車',
  '電扶梯', '音效卡', '飆車族', '點陣圖', '個入球', '顆進球', '沃尒沃', '晶片集',
  '斯瓦巴', '斜角巷', '战列舰', '快速面', '希特拉', '太空梭', '吐瓦魯', '吉布堤',
  '吉布地', '史太林', '南冰洋', '区域网', '波札那', '解析度', '酷洛米', '金夏沙',
  '魔獸紀元', '高空彈跳', '铁达尼号', '太空战士', '埃及妖后', '吉里巴斯', '附加元件',
  '魔鬼終結者', '純文字檔案', '奇幻魔法Melody', '列支敦斯登',
]

const [, , t2sArg, s2tArg, outDirArg] = process.argv
if (!t2sArg || !s2tArg || !outDirArg) {
  console.error('用法: node scripts/build-legado-chinese-dicts.mjs <t2s.txt> <s2t.txt> <输出目录>')
  process.exit(1)
}

/**
 * 解析 quick-transfer 的 `源=目标` 词典。
 * DictionaryFactory 会跳过空行、`#` 注释与不含 `=` 的行；这里照做，
 * 同时挡住 opencc-js 序列化格式里会歧义的空格与 `|`。
 */
function parseDictionary(file) {
  const map = new Map()
  let skipped = 0
  for (const line of fs.readFileSync(file, 'utf8').split(/\r?\n/)) {
    if (!line || line.startsWith('#')) continue
    const eq = line.indexOf('=')
    if (eq < 0) {
      skipped++
      continue
    }
    const source = line.slice(0, eq)
    const target = line.slice(eq + 1)
    if (!source || !target || /[ |]/.test(source) || /[ |]/.test(target)) {
      skipped++
      continue
    }
    map.set(source, target)
  }
  return { map, skipped }
}

function writeModule(file, map, comment) {
  const serialized = [...map.entries()].map(([source, target]) => `${source} ${target}`).join('|')
  const header = `// ⚠️ 自动生成，请勿手改（改动会被下次生成覆盖）。
//
// ${comment}
//
// 词表出处：https://github.com/liuyueyi/quick-chinese-transfer
// （transfer-core 的词典转自 HanLP 与 luhuiguo/chinese-utils，均为 Apache-2.0；
//   手机端 legado 用的就是这一份，因此这里照搬以保持两端一致。）
//
// 条目格式与 opencc-js 序列化词典一致：\`源 目标\` 以 \`|\` 分隔，
// 由 opencc-js 的 Trie 做最长匹配（等价于 quick-transfer 的 BasicDictionary）。
export default `
  fs.writeFileSync(file, header + JSON.stringify(serialized) + '\n', 'utf8')
  return { count: map.size, bytes: Buffer.byteLength(serialized, 'utf8') }
}

const outDir = path.resolve(outDirArg)
fs.mkdirSync(outDir, { recursive: true })

const t2s = parseDictionary(t2sArg)
for (const word of LEGADO_EXCLUDE) t2s.map.set(word, word)
const t2sResult = writeModule(
  path.join(outDir, 'legadoT2sDict.js'),
  t2s.map,
  'legado「繁 → 简」词典：quick-transfer-core 0.2.17 的 tc/t2s.txt，\n'
  + '// 已合并 legado ChineseUtils.fixT2sDict() 的排除表。\n'
  + '//\n'
  + '// 为什么不用 OpenCC 自带的 t2s：它的 TSCharacters 把「乾」无条件映射成\n'
  + '// 「干」，只有 51 个短语例外，于是「乾國」在简体档下变成「干国」\n'
  + '// （issue #25）。手机端 legado 的这张表里「乾」没有单字映射，只在\n'
  + '// 「乾燥/乾淨/乾杯…」这类短语里转换，因此「乾國/乾隆/乾坤」保持原样。',
)

const s2t = parseDictionary(s2tArg)
const s2tResult = writeModule(
  path.join(outDir, 'legadoS2tDict.js'),
  s2t.map,
  'legado「简 → 繁」词典：quick-transfer-core 0.2.17 的 tc/s2t.txt。\n'
  + '// 与手机端一致：不做地区用词替换（「软件」→「軟件」而不是「軟體」）。',
)

console.log(`t2s: ${t2sResult.count} 条 / ${t2sResult.bytes} 字节（跳过 ${t2s.skipped} 行）`)
console.log(`s2t: ${s2tResult.count} 条 / ${s2tResult.bytes} 字节（跳过 ${s2t.skipped} 行）`)
console.log(`已写入 ${outDir}`)
