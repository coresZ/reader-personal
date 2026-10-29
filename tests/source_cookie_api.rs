//! `java.*` 的 Cookie 读写接口。
//!
//! 书源常用 `loginUi` 里的按钮做「查看 / 清除 Cookie」：
//!
//! ```js
//! checkCookie = () => { var c = java.getCookie('https://example.test'); ... }
//! clearCookie = () => { java.clearCookie('https://example.test'); ... }
//! ```
//!
//! `clearCookie` **不在 legado 的 `JsExtensions` 里**（那边只有 `getCookie`，
//! 清除是书源编辑页的菜单项），但书源确实在写。缺失时的现象是点按钮弹
//! `internal error: JS 异常: not a function at clearCookie (...)`，
//! 用户只会觉得「这个源坏了」。
//!
//! 这里验证的是**这些名字在 JS 环境里是可调用的函数**，不深究底层 jar 的
//! 域匹配细节（那部分由 `source_session` 自己的测试覆盖）。
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js::{eval_js, with_source};

fn source() -> reader_rust::model::book_source::BookSource {
    book_source_from_value(serde_json::json!({
        "bookSourceName": "示例源",
        "bookSourceUrl": "https://example.test",
    }))
    .expect("source")
}

fn eval(script: &str) -> String {
    let source = source();
    with_source(&source, || eval_js(script, "", "")).expect("求值应成功")
}

/// `java.clearCookie` 必须存在且可调用（书源 `loginUi` 的「清除 Cookie」按钮）。
#[test]
fn clear_cookie_is_callable() {
    let out = eval(
        r#"
var t = typeof java.clearCookie;
var called;
try { called = java.clearCookie('https://example.test'); }
catch (e) { called = 'THREW: ' + e; }
t + '|' + called
"#,
    );
    assert_eq!(
        out.split('|').next(),
        Some("function"),
        "java.clearCookie 应为函数，实际 {out}"
    );
    assert!(
        !out.contains("THREW"),
        "调用 java.clearCookie 不应抛异常，实际 {out}"
    );
}

/// `java.getCookie` 支持单参（整个 Cookie 串）与双参（取某个键）。
#[test]
fn get_cookie_supports_one_and_two_args() {
    let out = eval(
        r#"
var t = typeof java.getCookie;
var one, two;
try { one = String(java.getCookie('https://example.test')); } catch (e) { one = 'THREW: ' + e; }
try { two = String(java.getCookie('https://example.test', 'sid')); } catch (e) { two = 'THREW: ' + e; }
t + '|' + (one.indexOf('THREW') < 0) + '|' + (two.indexOf('THREW') < 0)
"#,
    );
    assert_eq!(
        out, "function|true|true",
        "java.getCookie 的两种调用都应为函数且不抛异常，实际 {out}"
    );
}

/// 一次性核对「登录面板常用的一批 `java.*`」都在。
///
/// 单独断言每个名字太碎，这里用一张表把它们一起探一遍 —— 缺哪个会在
/// 失败信息里直接点名。
#[test]
fn login_panel_java_api_surface_is_complete() {
    let out = eval(
        r#"
var names = [
  'startBrowser', 'startBrowserAwait', 'showBrowser',
  // `getCookie` 是 legado 官方 API；`clearCookie` 是书源自造的（legado 没有），
  // 但确实有源在用，所以也补上。`setCookie` / `removeCookie` 只在宿主侧的
  // `cookie` 对象上，不冒充 `java.*`。
  'getCookie', 'clearCookie',
  'toast', 'longToast', 'log',
  'ajax', 'get', 'post',
  'base64Encode', 'base64Decode',
  'encodeURI', 'encodeURIComponent'
];
var missing = [];
for (var i = 0; i < names.length; i++) {
  if (typeof java[names[i]] !== 'function') missing.push(names[i]);
}
missing.join(',')
"#,
    );
    assert!(
        out.is_empty(),
        "以下 java.* 缺失或不是函数: {out}"
    );
}
