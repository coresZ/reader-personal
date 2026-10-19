//! 章节标题与正文写在 CSS 伪元素里的书源，其目录/正文解析回归。
//!
//! 这类站把章节标题与正文**只**写在页面尾部的 CSS 里
//! (`.p_191241001::after{content:'第2章'}`)，DOM 里只有一个空 `<span>`。
//! 书源因此用脚本把标题回写到元素上，并调用了一批 Java 风格 API：
//!
//! - `java.getElements(sel).size()` / `.get(i)`（JSoup `Elements`）
//! - `aEl.text()` 读标题、`aEl.attr('class')` 取 `p_xxxxxx`
//! - `title.isEmpty()` / `class.replaceAll(regex, repl)`（Java String）
//! - `aEl.text(title)` 把标题**写回**元素，供 `chapterName: text` 取值
//!
//! 缺任何一个，`chapterList` 脚本都会抛 "not a function"，目录解析出 0 章；
//! 正文的 `nextContentUrl` 是 `选择器@js:脚本` 组合规则，整条当 CSS 会丢掉分页。

use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js;
use reader_rust::parser::rule_engine::RuleEngine;
use serde_json::json;

/// 目录页片段：三条章节，其中后两条 `<a>` 没有文本，标题只在 CSS 里。
const TOC_HTML: &str = r#"<html><body>
<div id="list-chapterAll"><dl>
<dd><a href="/book/1/100.html" title="第1章" class="chapter-name p_100"><span></span></a></dd>
<dd><a href="/book/1/200.html" title="" class="chapter-name p_200"><span></span></a></dd>
<dd><a href="/book/1/300.html" title="" class="chapter-name p_300"><span></span></a></dd>
</dl></div>
<div class="listpage"><select name="pageselect">
<option value="/book/1_1/" selected>第1 - 60章</option>
<option value="/book/1_2/" >第61 - 120章</option>
</select></div>
<style>
#list-chapterAll .p_200::after{content:'第2章'}
#list-chapterAll .p_300::after{content:'第3章'}
</style>
</body></html>"#;

/// 正文页片段：正文只在 CSS 里，另有「下一页」分页链接。
const CHAPTER_HTML: &str = r#"<html><body>
<div id="htmlContent">
<span class="p_0"></span><span class="p_1"></span>
<style>#htmlContent .p_0::after{content:'第一段正文'}#htmlContent .p_1::after{content:'第二段正文'}</style>
</div>
<p class="readPager"><a id="linkNext" href="100_2.html">下一页</a></p>
</body></html>"#;

fn source() -> reader_rust::model::book_source::BookSource {
    book_source_from_value(json!({
        "bookSourceName": "伪样例源",
        "bookSourceUrl": "https://example.test",
        "ruleToc": {
            // 与真实书源同一套写法: size()/get(i)/isEmpty()/replaceAll()/text(title)
            "chapterList": "@js:\n\
const list = java.getElements(\"#list-chapterAll dl dd a\");\n\
for(let i = 0; i < list.size(); i++){\n\
    let aEl = list.get(i);\n\
    let title = aEl.text().trim();\n\
    if(title.isEmpty()){\n\
        let pClassName = aEl.attr(\"class\").replaceAll(\".*(p_\\\\d+).*\", \"$1\");\n\
        const reg = new RegExp(pClassName + \"::after\\\\s*\\\\{[^}]*content\\\\s*:\\\\s*['\\\"](.*?)['\\\"]\", \"is\");\n\
        const match = String(result).match(reg);\n\
        if (match) { title = match[1].replace(/\\\\(.)/g, \"$1\"); }\n\
    }\n\
    if(!String(title)){ title = \"第\" + (i + 1) + \"章\"; }\n\
    aEl.text(title);\n\
}\n\
list",
            "chapterName": "text",
            "chapterUrl": "href",
            "nextTocUrl": "option:not([selected])@value"
        },
        "ruleContent": {
            "content": "@js:\n\
const reg = new RegExp(\"#htmlContent\\\\s*\\\\.p_\\\\d+::after\\\\s*\\\\{[\\\\s\\\\S]*?content\\\\s*:\\\\s*['\\\"](.*?)['\\\"]\", \"gi\");\n\
const matches = String(result).matchAll(reg);\n\
Array.from(matches, m => m[1].replace(/\\\\(.)/g, \"$1\")).join(\"\\n\");",
            "nextContentUrl": "#linkNext@href@js:\n/_/.test(result) ? result : null"
        }
    }))
    .unwrap()
}

/// `java.getElements` 必须返回带 JSoup `Elements` 方法的数组:
/// `size()` / `isEmpty()` / `get(i)` / `first()` / `last()` / `eachText()`。
#[test]
fn get_elements_exposes_jsoup_elements_methods() {
    let out = js::eval_js(
        "var l = java.getElements('.item');\n\
         [l.size(), l.isEmpty(), l.get(0).attr('data-id'), l.first().attr('data-id'), \
          l.last().attr('data-id'), l.eachText().join('/')].join('|')",
        r#"<div class="item" data-id="7">甲</div><div class="item" data-id="8">乙</div>"#,
        "",
    )
    .unwrap();
    assert_eq!(out, "2|false|7|7|8|甲/乙");
}

/// Java String 的 `isEmpty()` / `replaceAll(regex, repl)` 必须可用,
/// 且 `replaceAll` 的第一个参数按**正则**处理（Java 语义）。
#[test]
fn java_string_helpers_are_available() {
    let out = js::eval_js(
        "[''.isEmpty(), 'x'.isEmpty(), \
          'chapter-name p_123'.replaceAll('.+?(p_\\\\d+).*', '$1')].join('|')",
        "",
        "",
    )
    .unwrap();
    assert_eq!(out, "true|false|p_123");
}

/// `Element.text(value)` / `attr(name, value)` 是 setter, 要能写回元素,
/// 之后 `JSON.stringify` 出来的快照里带着新值。
#[test]
fn element_text_setter_writes_back_to_snapshot() {
    let out = js::eval_js(
        "var l = java.getElements('a');\n\
         l.get(0).text('第三章');\n\
         var raw = JSON.parse(JSON.stringify(l))[0];\n\
         raw.text",
        r#"<a href="/3" class="p_3"><span></span></a>"#,
        "",
    )
    .unwrap();
    assert_eq!(out, "第三章");
}

/// 端到端: 目录脚本能把 CSS 里的标题补回来, 并交出分页地址。
#[test]
fn toc_script_recovers_titles_from_css_and_keeps_paging() {
    let src = source();
    let book_url = "https://example.test/book/1/";
    js::with_source(&src, || {
        let engine = RuleEngine::new().unwrap();
        let (chapters, next) = engine.chapter_list(&src, TOC_HTML, book_url, Some(book_url));
        assert_eq!(
            chapters.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(),
            ["第1章", "第2章", "第3章"],
            "CSS 伪元素里的标题要补回目录"
        );
        assert_eq!(
            chapters.iter().map(|c| c.url.as_str()).collect::<Vec<_>>(),
            [
                "https://example.test/book/1/100.html",
                "https://example.test/book/1/200.html",
                "https://example.test/book/1/300.html",
            ]
        );
        assert_eq!(
            next,
            vec!["https://example.test/book/1_2/".to_string()],
            "`chapterList` 是脚本时 `nextTocUrl` 仍要在原始页面上求值"
        );
        let logs = js::take_js_logs();
        assert!(logs.is_empty(), "目录脚本不该报错: {logs:?}");
    });
}

/// 端到端: 正文从 CSS 里提取, 且 `选择器@js:` 形式的分页规则能取到下一页。
#[test]
fn content_script_extracts_body_and_follows_paging() {
    let src = source();
    let chapter_url = "https://example.test/book/1/100.html";
    js::with_source(&src, || {
        let engine = RuleEngine::new().unwrap();
        let content = engine.content(&src, CHAPTER_HTML, chapter_url, Some(chapter_url));
        assert_eq!(content, "第一段正文\n第二段正文");
        assert_eq!(
            engine.next_content_url(&src, CHAPTER_HTML, chapter_url).as_deref(),
            Some("https://example.test/book/1/100_2.html")
        );
        let logs = js::take_js_logs();
        assert!(logs.is_empty(), "正文脚本不该报错: {logs:?}");
    });
}
