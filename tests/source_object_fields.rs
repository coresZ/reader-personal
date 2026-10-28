//! JS 侧 `source` 对象的字段与返回值语义（对齐 legado 的 `BookSource`）。
//!
//! 一类书源把登录脚本**只放在 `loginUrl`** 里，并在 `exploreUrl` 末尾复用：
//!
//! ```js
//! eval(String(source.loginUrl));
//! login();          // ← 上面 eval 进来的函数
//! ```
//!
//! `source.loginUrl` 取不到时，`login` 就是未定义 —— 日志只有一行
//! `JS 异常: login is not defined`，**发现页整块失效**。
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js::{eval_js, with_source};

fn source_with(login_url: &str, login_ui: &str) -> reader_rust::model::book_source::BookSource {
    book_source_from_value(serde_json::json!({
        "bookSourceName": "示例源",
        "bookSourceUrl": "https://example.test",
        "loginUrl": login_url,
        "loginUi": login_ui,
    }))
    .expect("source")
}

/// `source.loginUrl` / `source.loginUi` 必须可读，且能被 `eval` 出其中的函数。
#[test]
fn source_exposes_login_url_for_eval() {
    let source = source_with(
        "function login() { return 'ok'; }",
        "[]",
    );
    let out = with_source(&source, || {
        eval_js(
            r#"
eval(String(source.loginUrl));
typeof login === 'function' ? login() : 'NOT_DEFINED'
"#,
            "",
            "",
        )
    })
    .expect("求值应成功");
    assert_eq!(out, "ok", "source.loginUrl 应可读且其中的函数可被调用");
}

/// 没有 `loginUrl` 时不应报错，读到空串即可（书源可能只配了 `loginUi`）。
#[test]
fn missing_login_url_reads_empty() {
    let source = source_with("", "");
    let out = with_source(&source, || {
        eval_js("String(source.loginUrl) + '|' + String(source.loginUi)", "", "")
    })
    .expect("求值应成功");
    assert_eq!(out, "|", "空 loginUrl/loginUi 应读成空串");
}

/// `getVariable()` 无值时必须是**空串**，书源靠 `v == ""` 判首次初始化。
///
/// 返回 `undefined` 时判空不成立，紧接着 `JSON.parse(v)` 抛
/// `unexpected token: 'undefined'`。
#[test]
fn get_variable_returns_empty_string_when_unset() {
    let source = source_with("", "");
    let out = with_source(&source, || {
        eval_js(
            r#"
source.setVariable('');           // 显式清空，避免受历史状态影响
var v = source.getVariable();
(typeof v) + '|' + (v === '') + '|' + (v == '')
"#,
            "",
            "",
        )
    })
    .expect("求值应成功");
    assert_eq!(out, "string|true|true", "无值时应是空串且能通过 == ''");
}

/// 首次初始化链路：空变量 → 写入 → 立刻读回 → 可 JSON.parse。
#[test]
fn variable_round_trip_supports_first_run_init() {
    let source = source_with("", "");
    let out = with_source(&source, || {
        eval_js(
            r#"
source.setVariable('');
var v = source.getVariable();
if (v == '') source.setVariable('[{"fl":[{"k":"v"}]}]');
var parsed = JSON.parse(source.getVariable());
parsed.length
"#,
            "",
            "",
        )
    })
    .expect("求值应成功");
    assert_eq!(out, "1", "首次初始化后应能解析出 1 项");
}
