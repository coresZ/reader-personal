//! `java.encodeURI(text, charset)` 的字符集语义。
//!
//! 这类书源的搜索规则把关键词按 GBK 编码后塞进表单：
//!
//! ```js
//! var kb = java.encodeURI(key, 'gbk');
//! var op = { method: 'POST', body: 'searchkey=' + kb, charset: 'gbk', … };
//! "https://example.test/search.html" + ',' + JSON.stringify(op)
//! ```
//!
//! 第二参数被忽略时（此前恒按 UTF-8），站点按 GBK 解码得到乱码关键词，搜索
//! 稳定 0 条；而发现页走 GET、不经过这里，所以只有搜索坏掉。
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js::{eval_js, with_source};

fn source_with_lib(lib: &str) -> reader_rust::model::book_source::BookSource {
    book_source_from_value(serde_json::json!({
        "bookSourceName": "示例源",
        "bookSourceUrl": "https://example.test",
        "jsLib": lib,
    }))
    .expect("source")
}

fn encode(keyword: &str, charset: &str) -> String {
    let source = source_with_lib("");
    let script = format!(
        r#"java.encodeURI({keyword:?}, {charset:?})"#,
        keyword = keyword,
        charset = charset
    );
    with_source(&source, || eval_js(&script, "", "")).expect("求值应成功")
}

/// GBK 关键词必须按 GBK 字节编码。
///
/// 「剑来」的 GBK 是 `BD A3 C0 B4`，UTF-8 是 `E5 89 91 E6 9D A5` —— 两者
/// 完全不同，站点按 GBK 解码时只有前者能得到正确的两个字。
#[test]
fn gbk_keyword_is_encoded_as_gbk() {
    assert_eq!(encode("剑来", "gbk"), "%BD%A3%C0%B4");
    // 同一词按 UTF-8 编出的必须是另一串，用来证明 charset 真的生效了。
    assert_eq!(encode("剑来", "utf-8"), "%E5%89%91%E6%9D%A5");
}

/// 不传 charset 时按 UTF-8（legado 的单参数重载）。
#[test]
fn missing_charset_defaults_to_utf8() {
    let source = source_with_lib("");
    let out = with_source(&source, || eval_js(r#"java.encodeURI("剑来")"#, "", ""))
        .expect("求值应成功");
    assert_eq!(out, "%E5%89%91%E6%9D%A5");
}

/// 纯 ASCII 关键词在两种字符集下结果一致（不能因为改了编码器就动到它们）。
#[test]
fn ascii_keyword_is_stable_across_charsets() {
    for charset in ["utf-8", "gbk", "gb2312", "big5"] {
        assert_eq!(encode("Monkey", charset), "Monkey", "charset={charset}");
    }
}

/// Java `URLEncoder` 的 form 语义：空格是 `+`，`%` 会被再编码一次。
///
/// 后者是它与 `encodeURIComponent` 的关键差别 —— 调用方不该提前自己编码。
#[test]
fn url_encoder_uses_form_semantics() {
    assert_eq!(encode("a b", "utf-8"), "a+b");
    assert_eq!(encode("100%", "utf-8"), "100%25");
}
/// 完整链路：书源的 `searchUrl` 脚本 → `URL,选项` → `analyze_url` 解析出的请求。
///
/// 这条用例用真实书源的写法把三处串起来，任何一处退化都会让它失败：
///   1. `java.encodeURI(key,'gbk')` 必须按 GBK 编关键词（否则站点解码成乱码）；
///   2. JS 返回的字符串必须保留 GBK 百分号形式；
///   3. `analyze_url` 解析 `地址,{…}` 时，body 已声明 `Content-Type` 应原样发送。
#[test]
fn form_search_keeps_gbk_body_end_to_end() {
    use reader_rust::crawler::url_analyzer::analyze_url;

    let source = book_source_from_value(serde_json::json!({
        "bookSourceName": "示例源",
        "bookSourceUrl": "https://example.test",
        // 与真实书源一致：<js> 包住一段返回「地址,选项」的脚本
        "searchUrl": r#"<js>var kb=java.encodeURI(key,'gbk');var op={method:'POST',body:'searchkey='+kb,charset:'gbk',headers:{'Content-Type':'application/x-www-form-urlencoded'}};"https://example.test/search.html"+','+JSON.stringify(op)</js>"#,
    }))
    .expect("source");

    let spec = analyze_url(
        source.search_url.as_deref().expect("searchUrl"),
        "剑来",
        1,
        &source.book_source_url,
        &source,
    )
    .expect("analyze_url 应成功");

    assert!(
        spec.url.contains("/search.html"),
        "搜索地址应指向 search.html，实际 {}",
        spec.url
    );
    let body = spec.body.as_deref().unwrap_or_default();
    assert_eq!(
        body, "searchkey=%BD%A3%C0%B4",
        "POST 正文必须是 GBK 百分号编码的关键词"
    );
}
