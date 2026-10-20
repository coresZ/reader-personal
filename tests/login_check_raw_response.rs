//! `StrResponse.raw().priorResponse()` 与 `isRedirect()` 的兼容性回归。
//!
//! 一类书源的 loginCheckJs 用它识别"被 302 到年龄确认页", 并记住跳转后的站点:
//!
//! ```js
//! var pres = result.raw().priorResponse();
//! var isRedirect = pres != null && pres.isRedirect();
//! if (isRedirect) { … source.putLoginInfo(…) }
//! ```
//!
//! legado 的 `StrResponse.raw()` 返回 okhttp `Response`。缺 `raw` 时脚本在
//! `result.raw().priorResponse()` 处抛 "not a function", 整个 loginCheckJs 失败。

use reader_rust::parser::js::{eval_js_with_bindings, resp_binding};
use serde_json::{json, Value};
use std::collections::HashMap;

fn run(script: &str, envelope: Value) -> String {
    let mut bindings = HashMap::new();
    bindings.insert("result".to_string(), resp_binding(&envelope));
    eval_js_with_bindings(script, "", "https://example.test/content", &bindings)
        .expect("loginCheckJs 应执行成功")
}

/// 未发生重定向时 `priorResponse()` 返回 null, `isRedirect()` 为 false。
#[test]
fn raw_prior_response_is_null_without_redirect() {
    let out = run(
        r#"
var pres = result.raw().priorResponse();
var isRedirect = pres != null && pres.isRedirect();
String(isRedirect) + '|' + String(pres === null) + '|' + result.code()
"#,
        json!({
            "body": "正文内容",
            "url": "https://example.test/content?item_id=1",
            "code": 200,
            "headers": {}
        }),
    );
    assert_eq!(out, "false|true|200", "未重定向时不应误判: {out}");
}

/// 原始请求地址与最终地址不同 → 视为发生过重定向, 且 `priorResponse()`
/// 给出上一跳的地址与状态码。
#[test]
fn raw_prior_response_detects_redirect() {
    let out = run(
        r#"
var pres = result.raw().priorResponse();
var isRedirect = pres != null && pres.isRedirect();
String(isRedirect) + '|' + (pres ? pres.url() : '') + '|' + (pres ? pres.code() : 0)
"#,
        json!({
            "body": "已成年确认后的正文",
            "url": "https://site-b.test/content?item_id=1",
            "__requestUrl": "https://site-a.test/content?item_id=1",
            "code": 200,
            "headers": {}
        }),
    );
    assert_eq!(
        out,
        "true|https://site-a.test/content?item_id=1|302",
        "应识别为重定向并给出上一跳: {out}"
    );
}

/// `raw()` 自身的常用取值方法要与 okhttp `Response` 一致。
#[test]
fn raw_exposes_response_accessors() {
    let out = run(
        r#"
var r = result.raw();
String(r.code()) + '|' + String(r.url()) + '|' + String(r.isSuccessful()) + '|' + String(r.isRedirect())
"#,
        json!({
            "body": "x",
            "url": "https://example.test/a",
            "code": 200,
            "headers": {}
        }),
    );
    assert_eq!(out, "200|https://example.test/a|true|false");
}

/// 原书源 loginCheckJs 的完整形态(脱敏)必须能跑通并原样返回响应。
#[test]
fn age_gate_login_check_script_runs() {
    let script = r#"
(function(){
  try {
    var u = String(result.url());
    var m = u.match(/^https?:\/\/[^\/]+/);
    if (m) {
      var org = m[0];
      var ck = String(cookie.getCookie(org) || '');
      if (ck.indexOf('verify_ok') < 0) {
        cookie.setCookie(org, ck ? (ck + '; verify_ok=1') : 'verify_ok=1');
      }
    }
  } catch(e) {}
  try {
    var b = String(result.body ? (typeof result.body==='function'?result.body():result.body) : '');
    if (b.indexOf('年龄确认') >= 0 && b.indexOf('已成年') >= 0) {
      java.log('被年龄确认拦截：' + String(result.url()));
    }
  } catch(e2) {}
})();

var pres = result.raw().priorResponse();
var isRedirect = pres!=null&&pres.isRedirect();
if (isRedirect) {
  let url=String(result.url()).match(/^https?:\/\/[^\/]+\//)[0];
  if (!/sample-host/.test(url)) {
    let Info=source.getLoginInfoMap()||{};
    Info['网址'] = url;
    source.putLoginInfo(JSON.stringify(Info));
  }
}
var code = result.code();
if (code == 403) { java.toast('\n访问被拒绝'); }
else if (code == 520) { java.toast('\nIP被拉黑'); }
result
"#;
    let out = run(
        script,
        json!({
            "body": "<p>正文</p>",
            "url": "https://example.test/content?item_id=1",
            "code": 200,
            "headers": {}
        }),
    );
    let envelope: Value = serde_json::from_str(&out).expect("应返回响应信封");
    assert_eq!(
        envelope.get("body").and_then(Value::as_str),
        Some("<p>正文</p>"),
        "正文必须原样返回: {out}"
    );
}

