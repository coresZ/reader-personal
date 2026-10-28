//! 两条「手机端正常、桌面端异常」的书源兼容回归。
//!
//! 1. **JSoup 的 `Element.select()` 把元素自身算进匹配集**。W3C 的
//!    `querySelectorAll` / `scraper` 的 `ElementRef::select` 跳过自身，于是
//!    `class.zjlb@class.lb@ul@li` 这类「第二步落在同一个元素上」的规则在桌面端
//!    恒为空 —— 症状是详情页正常、目录 0 章。
//! 2. **表单型 POST 正文按书源声明的 `charset` 编码**。声明 `"charset": "GBK"`
//!    的站点按 GBK 解码关键词；正文一律按 UTF-8 编码时关键词成了乱码，搜索稳定
//!    返回「无结果」，而发现页（走 GET）一切正常。

use reader_rust::crawler::url_analyzer::analyze_url;
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::{html, rule_engine::RuleEngine};
use serde_json::json;

/// `@` 链式选择器的每一步都要能命中「当前元素自身」。
#[test]
fn chained_selector_matches_the_current_element_itself() {
    let body = r#"<section class="zjlb">
    <ul class="lb fk">
        <li><a href="/1">第1章</a></li>
        <li><a href="/2">第2章</a></li>
        <li><a href="/3">第3章</a></li>
    </ul>
</section>"#;
    let doc = html::parse_document(body);

    // 第二步 `class.lb` 命中的是 `<ul class="lb fk">` 自身，第三步 `ul` 也必须
    // 从它自己开始匹配（页面里没有嵌套的 ul）。
    let items = html::select_list(&doc, "class.zjlb@class.lb@ul@li");
    assert_eq!(items.len(), 3, "`ul` 应命中当前元素自身");

    // 排除首位（`!-1` 是 legado 的「排除最后一个」写法）。
    let kept = html::select_list(&doc, "class.zjlb@class.lb@ul@li!-1");
    assert_eq!(kept.len(), 2, "排除索引之后仍应是 2 条");
}

/// 目录规则走完整链路（`RuleEngine::chapter_list`）也要拿到章节。
#[test]
fn toc_rule_using_self_select_yields_chapters() {
    let source = book_source_from_value(json!({
        "bookSourceUrl": "https://toc.example.test",
        "bookSourceName": "目录自身命中样例源",
        "ruleToc": {
            "chapterList": "class.zjlb@class.lb@ul@li!-1",
            "chapterName": "a@text",
            "chapterUrl": "a@href"
        }
    }))
    .expect("book source");

    let body = r#"<section class="zjlb">
    <ul class="lb fk">
        <li><a href="/1">第1章</a></li>
        <li><a href="/2">第2章</a></li>
        <li><a href="/3">第3章</a></li>
    </ul>
</section>"#;
    let engine = RuleEngine::new().expect("rule engine");
    let (chapters, _) = engine.chapter_list(
        &source,
        body,
        "https://toc.example.test/books/9/",
        None,
    );
    assert_eq!(
        chapters.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(),
        ["第1章", "第2章"],
        "首位应被 `!-1` 排除，其余按原顺序"
    );
    assert_eq!(chapters[0].url, "https://toc.example.test/1");
}

/// 表单型 POST 正文必须按 `charset` 选项编码（legado `analyzeFields` 语义）。
#[test]
fn post_form_body_follows_declared_charset() {
    let source = book_source_from_value(json!({
        "bookSourceUrl": "https://post.example.test",
        "bookSourceName": "GBK 搜索样例源",
        "searchUrl": "https://post.example.test/s.php,{\"body\":\"submit=&type=articlename&s={{key}}\",\"charset\":\"GBK\",\"method\":\"POST\"}"
    }))
    .expect("book source");

    let spec = analyze_url(
        source.search_url.as_deref().unwrap_or_default(),
        "我的",
        1,
        &source.book_source_url,
        &source,
    )
    .expect("analyze url");

    assert_eq!(spec.body.as_deref(), Some("submit=&type=articlename&s=%CE%D2%B5%C4"));
    assert_eq!(spec.charset.as_deref(), Some("GBK"));
}

/// 未声明 charset 时仍走 UTF-8 —— 绝大多数书源属于这一类，不能被改坏。
#[test]
fn post_form_body_defaults_to_utf8() {
    let source = book_source_from_value(json!({
        "bookSourceUrl": "https://post.example.test",
        "bookSourceName": "UTF-8 搜索样例源",
        "searchUrl": "https://post.example.test/s,{\"body\":\"s={{key}}\",\"method\":\"POST\"}"
    }))
    .expect("book source");

    let spec = analyze_url(
        source.search_url.as_deref().unwrap_or_default(),
        "我的",
        1,
        &source.book_source_url,
        &source,
    )
    .expect("analyze url");
    assert_eq!(spec.body.as_deref(), Some("s=%E6%88%91%E7%9A%84"));
}
