//! `book.durChapterIndex` / `chapter.index` 的默认值回归。
//!
//! legado 里它们是**非空 Int**(`Book.durChapterIndex: Int = 0`,
//! `BookChapter.index: Int = 0`)。我们的模型是 `Option<i32>`, 序列化后为 null,
//! 脚本读到的就是 undefined。对"按章节距离决定是否抓正文"的书源而言这是致命的:
//!
//! ```js
//! let inn = chapter.index - book.durChapterIndex;   // null - null → NaN
//! switch (inn) { case 0: …; case 1: …; default: ii = 0; break; }
//! if (ii) { …抓正文… } else { result = ''; }
//! ```
//!
//! NaN 落到 default, result 被清空 —— 界面报"书源正文解析为空"。
use reader_rust::parser::js::{eval_js_url, with_book_context, with_source, JsBookContext};
use reader_rust::model::book_source::{book_source_from_value, BookSource};
use serde_json::json;

fn plain_source() -> BookSource {
    book_source_from_value(json!({
        "bookSourceUrl": "https://example.test",
        "bookSourceName": "默认值样例源",
        "ruleContent": { "content": "@css:.content@html" }
    }))
    .expect("book source")
}

fn eval_with(book: serde_json::Value, chapter: serde_json::Value, script: &str) -> String {
    let source = plain_source();
    let ctx = JsBookContext { book, chapter };
    with_source(&source, || {
        with_book_context(Some(ctx), || {
            eval_js_url(script, "", "", 1, &source.book_source_url, "https://example.test/read/1/1.html")
        })
    })
    .expect("脚本应执行成功")
}

/// 缺少 durChapterIndex/index 时按 0 补齐, 避免算术得到 NaN。
#[test]
fn chapter_distance_is_numeric_when_fields_missing() {
    let out = eval_with(
        json!({ "bookUrl": "https://example.test/book/1.html" }),
        json!({ "url": "https://example.test/read/1/1.html" }),
        r#"chapter.index - book.durChapterIndex"#,
    );
    assert_eq!(out, "0", "两个字段都应默认 0, 实际: {out}");
}

/// 正常注入时保持原值, 不做覆盖。
#[test]
fn chapter_distance_uses_real_values_when_present() {
    let out = eval_with(
        json!({ "bookUrl": "https://example.test/book/1.html", "durChapterIndex": 5 }),
        json!({ "url": "https://example.test/read/1/6.html", "index": 6 }),
        r#"chapter.index - book.durChapterIndex"#,
    );
    assert_eq!(out, "1", "应使用真实值: {out}");
}

/// 典型的"章节距离守卫"脚本必须走进抓正文分支。
#[test]
fn content_guard_branch_is_taken_when_distance_is_zero() {
    let out = eval_with(
        json!({ "bookUrl": "https://example.test/book/1.html", "name": "样例书" }),
        json!({ "url": "https://example.test/read/1/1.html", "title": "第1章" }),
        r#"
let bin = book.durChapterIndex;
let cin = chapter.index;
let inn = cin - bin;
let ii = 1;
switch (inn) {
  case 0: break;
  case 1: break;
  default: ii = 0; break;
}
ii ? ('抓正文|inn=' + inn) : 'result=""'
"#,
    );
    assert!(
        out.starts_with("抓正文"),
        "章节距离为 0 时必须抓正文, 否则正文为空: {out}"
    );
}

/// `durChapterTitle` 缺省为空串(legado 是 `String? = null`, 但脚本常直接拼接)。
#[test]
fn dur_chapter_title_defaults_to_empty_string() {
    let out = eval_with(
        json!({ "bookUrl": "https://example.test/book/1.html" }),
        json!({ "url": "https://example.test/read/1/1.html" }),
        r#"String(book.durChapterTitle) + '|' + String(book.durChapterPos)"#,
    );
    assert_eq!(out, "|0", "标题默认空串、位置默认 0: {out}");
}
