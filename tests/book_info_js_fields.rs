//! 详情页规则求值的回归：多行 `{{…}}`、纯 JS 字段、`init` 回退。
//!
//! 一类书源的 `ruleBookInfo` 三个字段恰好各踩一个坑，合起来就是
//! 「书名/作者能出来，简介却始终为空」：
//!
//! ```text
//! init  = <js>… if (!i) { result = java.ajax(…) } … java.setContent(result); result</js>
//! name  = <js>let j = Packages.org.jsoup.Jsoup.parse(String(result)); …</js>
//! intro = {{
//!           let r = java.getString('#full-intro@textNodes');
//!           r
//!         }}
//! ```
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::html;
use reader_rust::parser::rule_engine::RuleEngine;
use serde_json::json;

const DETAIL_HTML: &str = r#"<html><body>
<div class="book-head">
  <h4 class="title"><span class="fr fss"><a href="/claim.php">>>>认领</a></span>《示例书名》</h4>
</div>
<div class="list-body"><div id="short-intro">摘要..</div>
  <div id="full-intro" style="display:none;">第一段完整简介。<br />
第二段完整简介。</div>
</div>
</body></html>"#;

fn source_with(rule_book_info: serde_json::Value) -> reader_rust::model::book_source::BookSource {
    book_source_from_value(json!({
        "bookSourceUrl": "https://example.test",
        "bookSourceName": "详情页样例源",
        "ruleBookInfo": rule_book_info,
    }))
    .expect("book source")
}

fn parse(source: &reader_rust::model::book_source::BookSource) -> reader_rust::model::book::Book {
    RuleEngine::new()
        .expect("rule engine")
        .book_info(source, DETAIL_HTML, "https://example.test/book/1.html", "https://example.test/book/1.html")
}

/// 多行 `{{…}}` 必须被当作 JS 求值。
///
/// `inline_js_lazy_re` 早先是 `\{\{(.*?)\}\}`，而 Rust `regex` 的 `.` 默认不匹配换行：
/// 多行块整段匹配不上，会被当成 CSS 选择器解析，结果恒为空。
#[test]
fn multiline_template_block_is_evaluated() {
    let source = source_with(json!({
        "name": ".book-head h4@text",
        "intro": "{{\nlet prefix = '简介:';\nlet body = java.getString('#full-intro@textNodes');\nprefix + body\n}}"
    }));
    let book = parse(&source);
    let intro = book.intro.expect("简介不应为空");
    assert!(intro.starts_with("简介:"), "多行模板应被求值: {intro}");
    assert!(intro.contains("第一段完整简介"), "应取到完整简介: {intro}");
}

/// 纯 JS 的详情字段规则里，`result` 必须是**页面内容**。
///
/// 早先 `eval_field_html_doc_with_ctx` 在 `pure` 为空时把 `text` 设成空串，于是
/// `Jsoup.parse(String(result))` 解析的是空文档，`select(...)` 必然选不到。
#[test]
fn plain_js_field_receives_page_content() {
    let source = source_with(json!({
        "name": "<js>\nlet j = Packages.org.jsoup.Jsoup.parse(String(result));\nlet e = j.select('h4.title').first();\nString(e.ownText())\n</js>"
    }));
    let book = parse(&source);
    assert_eq!(book.name, "《示例书名》", "纯 JS 字段应拿到整页内容");
}

/// 字段规则能读 `java.getString(规则, 元素)` 里的 CSS 规则，且优先用完整简介。
#[test]
fn js_field_can_read_css_rules_from_content() {
    let source = source_with(json!({
        "name": ".book-head h4@text",
        "intro": "{{\njava.getString('#full-intro@textNodes')\n}}"
    }));
    let book = parse(&source);
    let intro = book.intro.expect("简介不应为空");
    assert!(intro.contains("第二段完整简介"), "应取到 #full-intro 的完整内容: {intro}");
}

/// `init` 返回空串或抛错时必须**保留原始响应**，否则详情页所有字段一起变空。
#[test]
fn empty_init_output_keeps_original_body() {
    let source = source_with(json!({
        "init": "<js>\n''\n</js>",
        "name": ".book-head h4@text",
        "intro": "{{\njava.getString('#full-intro@textNodes')\n}}"
    }));
    let book = parse(&source);
    assert!(
        !book.name.trim().is_empty(),
        "init 返回空串时仍应从原响应解析出书名, 实际: {:?}",
        book.name
    );
    assert!(book.intro.is_some(), "简介也应能解析出来");
}

/// `init` 抛错时同样回退到原始响应。
#[test]
fn failing_init_keeps_original_body() {
    let source = source_with(json!({
        "init": "<js>\nthrow new Error('boom');\n</js>",
        "name": ".book-head h4@text"
    }));
    let book = parse(&source);
    assert!(!book.name.trim().is_empty(), "init 失败不应毁掉书名");
}

/// `init` 把内容换成无关页面时，用原响应兜底重新解析。
#[test]
fn unrelated_init_output_falls_back_to_original_body() {
    let source = source_with(json!({
        // 模拟 init 内部 ajax 打到别处（拿到的是无关页面）。
        "init": "<js>\n'<html><body><p>站点首页</p></body></html>'\n</js>",
        "name": ".book-head h4@text"
    }));
    let book = parse(&source);
    assert!(
        book.name.contains("《示例书名》"),
        "init 换成了无关内容时应回退原响应, 实际: {:?}",
        book.name
    );
    assert!(
        !book.name.contains("站点首页"),
        "不应采用 init 换进去的无关页面, 实际: {:?}",
        book.name
    );
}

/// 属性选择器里含多字节字符时不能按字节切片（会切在字符中间 panic）。
#[test]
fn multibyte_attribute_selector_does_not_panic() {
    let doc = html::parse_document(
        r#"<html><body><p title="中文标题">甲</p><p title="other">乙</p></body></html>"#,
    );
    let out = html::select_text_list(&doc, "[title=中文标题]@text");
    assert_eq!(out, vec!["甲".to_string()], "含中文的属性选择器应正常工作");
}


