//! 发现页(书海)`ruleExplore` 的列表项字段规则回归。
//!
//! legado 在求值**列表项**的字段规则前会 `analyzeRule.setContent(item)`,于是脚本里的
//! `java.getString('h4@text')` / 插件封装的 `gets(...)` 是**在该条目内**取值。
//!
//! 一类书源的发现页规则正是这么写的:
//!
//! ```text
//! bookList = <js>java.getElement('#list>div')</js>
//! name     = <js>result = gets('h4@text'); rs = java.getString('a.0@href'); …</js>
//! bookUrl  = @js:book.bookUrl
//! ```
//!
//! 若不给 `java.getString` 切到当前条目,`h4@text` 会命中**页面上的第一个**
//! `h4`,`java.getString('a.0@href')` 也一样 —— 每个条目的地址都落空,
//! 结果就是书海一片空白(列表一条都出不来)。
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::rule_engine::RuleEngine;
use serde_json::json;

const LIST_HTML: &str = r#"<html><body>
<div id="list">
  <div class="card"><a href="/book/11.html"><h4>甲书</h4></a></div>
  <div class="card"><a href="/book/22.html"><h4>乙书</h4></a></div>
  <div class="card"><a href="/book/33.html"><h4>丙书</h4></a></div>
</div>
</body></html>"#;

/// 与真实书源同构:`name` 用页面作用域的 `java.getString`,并写回 `book.bookUrl`。
fn explore_source() -> reader_rust::model::book_source::BookSource {
    book_source_from_value(json!({
        "bookSourceUrl": "https://example.test",
        "bookSourceName": "发现页样例源",
        "ruleExplore": {
            "bookList": "<js>\nresult = java.getElement('#list>div');\nresult\n</js>",
            "name": "<js>\nlet rs = java.getString('a.0@href') + '';\nlet id = (rs.match(/book\\/(\\d+)/) || ['',''])[1];\njava.put('bkid', id);\nbook.bookUrl = 'data:;base64,' + java.base64Encode(rs) + ',{\"type\":\"\"}';\njava.getString('h4@text') + ''\n</js>",
            "bookUrl": "@js:book.bookUrl"
        }
    }))
    .expect("book source")
}

/// 每个条目必须各自取到自己元素里的书名,而不是页面上第一个。
#[test]
fn explore_items_scope_get_string_to_own_element() {
    let engine = RuleEngine::new().expect("rule engine");
    let source = explore_source();
    let books = engine.explore_books(&source, LIST_HTML, "https://example.test/sort/1.html");

    assert_eq!(books.len(), 3, "三条都要解析出来, 实际: {}", books.len());
    assert_eq!(
        books.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
        ["甲书", "乙书", "丙书"],
        "书名的取值作用域必须是当前条目"
    );
}

/// 条目地址同理:必须来自该条目自己的 `a` 链接。
#[test]
fn explore_items_scope_url_to_own_element() {
    use base64::Engine;

    let engine = RuleEngine::new().expect("rule engine");
    let source = explore_source();
    let books = engine.explore_books(&source, LIST_HTML, "https://example.test/sort/1.html");

    assert_eq!(books.len(), 3);
    for (book, id) in books.iter().zip(["11", "22", "33"]) {
        let payload = book
            .book_url
            .strip_prefix("data:;base64,")
            .and_then(|rest| rest.split_once(',').map(|(b64, _)| b64))
            .unwrap_or_else(|| panic!("地址应是 data:;base64 信封: {}", book.book_url));
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .expect("base64");
        let decoded = String::from_utf8(decoded).expect("utf8");
        assert_eq!(decoded, format!("/book/{id}.html"), "地址应取自该条目自己的 a 链接");
    }
}

/// 设置作用域后,`result.select(...)` 这类元素 API 仍要可用,且两者可以同用。
#[test]
fn explore_items_keep_element_api_available() {
    let source = book_source_from_value(json!({
        "bookSourceUrl": "https://example.test",
        "bookSourceName": "发现页元素 API 样例源",
        "ruleExplore": {
            "bookList": "<js>\nresult = java.getElement('#list>div');\nresult\n</js>",
            // 同时用作用域内的 java.getString 与元素 API 的 result.select。
            "name": "<js>\nlet rs = java.getString('a.0@href') + '';\nlet h = result.select('h4').first();\nString(h.text())\n</js>",
            "bookUrl": "@js:book.bookUrl"
        }
    }))
    .expect("book source");

    let engine = RuleEngine::new().expect("rule engine");
    let books = engine.explore_books(&source, LIST_HTML, "https://example.test/sort/1.html");
    assert_eq!(books.len(), 3, "元素 API 仍应可用");
    assert_eq!(
        books.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
        ["甲书", "乙书", "丙书"]
    );
}