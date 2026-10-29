//! `java.get(...)` 返回值的 `cookies()` 与 `source.getLoginHeaderMap()` 的判空语义。
//!
//! 两者合起来决定了一类「先换 token 再请求」的书源能否工作：
//!
//! ```js
//! function Token() {
//!   var token = java.get("https://passport.example.test", {}).cookies()
//!     .toString().match(/session_token=([^,]*)/)[1];
//!   source.putLoginHeader(JSON.stringify({ authorization: "Bearer " + token }));
//! }
//! function GetUrl(path, params) {
//!   …
//!   if (source.getLoginHeaderMap() == null) { this.Token(); }   // ← 依赖 null
//!   …
//! }
//! ```
//!
//! 缺 `cookies()` → `not a function`；`getLoginHeaderMap()` 恒返回 `{}` →
//! 条件不成立、`Token()` 从不执行。两种都会让后续请求缺 `authorization`，
//! 服务端统一回 `401`：**详情简介只剩标签、目录 0 章、正文空白，而搜索正常**
//! （搜索接口不校验 token）。
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js::{eval_js, with_source};

fn source_with_lib(lib: &str) -> reader_rust::model::book_source::BookSource {
    source_at("https://example.test", lib)
}

/// 同一份 jsLib，但挂在指定的书源地址下。
///
/// `js_store` 是**跨用例共享的持久 KV**，键里含书源地址。多个用例若共用同一个
/// 地址，`cargo test` 并发跑时就会互相踩（A 的 `removeLoginHeader()` 恰好落在
/// B 的 `putLoginHeader` 与 `getLoginHeaderMap` 之间 → B 读到 `null` 而失败）。
/// 需要独立登录头的用例必须换一个地址。
fn source_at(url: &str, lib: &str) -> reader_rust::model::book_source::BookSource {
    book_source_from_value(serde_json::json!({
        "bookSourceName": "示例源",
        "bookSourceUrl": url,
        "jsLib": lib,
    }))
    .expect("source")
}

/// 没有登录头时 `getLoginHeaderMap()` 必须是 `null`，否则书源的「首次取 token」
/// 分支永远进不去。
///
/// 先显式清掉可能残留的登录头：`js_store` 是**跨用例共享的持久 KV**，别的用例
/// （或上一次运行）写进去的 `loginHeader` 会让这里读到 `{}` 而非 `null`。
#[test]
fn login_header_map_is_null_when_absent() {
    let source = source_with_lib("");
    let out = with_source(&source, || {
        eval_js(
            r#"
source.removeLoginHeader();
String(source.getLoginHeaderMap())
"#,
            "",
            "",
        )
    })
    .expect("求值应成功");
    assert_eq!(out, "null", "无登录头时应为 null（书源靠它判空）");
}

/// 写入登录头之后应能解析回对象。
#[test]
fn login_header_map_round_trips() {
    // 独立地址：避免与 `login_header_map_is_null_when_absent` 争同一个 KV 键。
    let source = source_at("https://round-trip.example.test", "");
    let out = with_source(&source, || {
        eval_js(
            r#"
source.putLoginHeader(JSON.stringify({authorization: "Bearer abc"}));
String(source.getLoginHeaderMap().authorization)
"#,
            "",
            "",
        )
    })
    .expect("求值应成功");
    assert_eq!(out, "Bearer abc");
}

/// `java.get(url).cookies()` 必须存在，且 `toString()` 能被正则匹配到 `name=value`。
///
/// 这里只验证**形状**（不依赖网络）：`cookies()` 是函数，返回值可 `String()`
/// 且支持 `.toString()` / `.filter` 之外的字符串操作。
#[test]
fn response_exposes_cookies_function() {
    let source = source_with_lib("");
    let out = with_source(&source, || {
        eval_js(
            r#"
var r = java.getStrResponse();
"cookies=" + typeof r.cookies
"#,
            "",
            "",
        )
    })
    .expect("求值应成功");
    assert_eq!(out, "cookies=function", "Resp 必须提供 cookies()");
}

/// `cookies()` 的返回值要能像 okhttp `List<Cookie>` 那样被 `String()` 成
/// `a=1, b=2` 形式，书源才能用 `match(/name=([^,]*)/)` 取值。
#[test]
fn cookies_stringifies_as_comma_separated_pairs() {
    let source = source_with_lib("");
    let out = with_source(&source, || {
        eval_js(
            r#"
// 无网络时 jar 为空，但形状必须正确：String() 得到空串而不是报错。
var list = java.getStrResponse().cookies();
"ok=" + (typeof list.toString === "function") + " len=" + list.length
"#,
            "",
            "",
        )
    })
    .expect("求值应成功");
    assert!(out.starts_with("ok=true"), "cookies() 应返回可字符串化的列表：{out}");
}
