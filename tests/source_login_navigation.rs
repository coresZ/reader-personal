//! 登录页注入脚本的回归。
//!
//! 三条独立的链路，任何一条缺失都会让「登录成功但页面不跳转」重现：
//!
//! 1. **语法必须成立**。脚本是 `format!` 拼的，写坏了没有编译期报错，注入后
//!    被浏览器静默丢弃。
//! 2. **导航要补会话段**。单页应用登录成功后 `history.replaceState(state, "",
//!    "/user/me")`，目标路径不带会话段就是跳出 `/__login/<s>/` 空间。
//! 3. **cookie 要能读写**。一类站点把 token 写进 `document.cookie`，页面自己
//!    随后读它判登录态；登录页 iframe 在 reader origin 下读写全隔离，读不到
//!    就当作未登录 → 登出 + 弹回登录页。这比导航更靠前，是真正的拦路虎。

use rquickjs::{Context, Runtime};

const PAGE: &str = "http://reader.localhost/__login/session-a/user/me?app=yuedu";

fn body(script: String) -> String {
    script
        .trim()
        .strip_prefix("<script>")
        .and_then(|rest| rest.strip_suffix("</script>"))
        .expect("脚本应带 <script> 包裹")
        .to_string()
}

fn login_script() -> String {
    body(reader_rust::api::protocol::build_login_request_rewrite_script_for_test(
        "session-a",
        "https://m.example.site",
    ))
}

fn cookie_script() -> String {
    body(reader_rust::api::protocol::build_cookie_harvest_script_for_test(
        "https://m.example.site",
        "session-a",
    ))
}

/// 最小 DOM：只要 `URL`、`location`、`history`、`document.cookie`、
/// `XMLHttpRequest`。QuickJS 没有内置 `URL`，补一个够用的实现。
const HARNESS: &str = r#"
var __log = [];
var __cookieWrites = [];
function URLImpl(input, base) {
  var raw = String(input);
  var resolved;
  if (/^https?:\/\//i.test(raw)) resolved = raw;
  else if (raw.charAt(0) === "/") {
    var m = /^(https?:\/\/[^\/]+)/.exec(String(base || PAGE));
    resolved = m[1] + raw;
  } else {
    resolved = String(base || PAGE).replace(/[^\/]*$/, "") + raw;
  }
  var parts = /^(https?:\/\/[^\/]+)(\/[^?#]*)?(\?[^#]*)?(#.*)?$/.exec(resolved) || [];
  this.origin = parts[1] || "";
  this.pathname = parts[2] || "/";
  this.search = parts[3] || "";
  this.hash = parts[4] || "";
  this.href = (parts[1] || "") + this.pathname + this.search + this.hash;
  this.protocol = (parts[1] || "").split(":")[0] + ":";
}
URLImpl.prototype.toString = function () { return this.href; };
// 脚本里用的是标准名 URL；测试桩的实现叫 URLImpl，补个别名。
var URL = URLImpl;
function mkLocation() {
  var url = new URLImpl(PAGE);
  return {
    get href() { return url.href; },
    set href(v) { __log.push("href=" + v); },
    get origin() { return url.origin; },
    get pathname() { return url.pathname; },
    get search() { return url.search; },
    get hash() { return url.hash; },
    set hash(v) { __log.push("hash=" + v); },
    assign: function (v) { __log.push("assign=" + v); },
    replace: function (v) { __log.push("replace=" + v); },
  };
}
var location = mkLocation();
var window = {
  location: location,
  history: {
    pushState: function (s, t, u) { __log.push("push:" + u); },
    replaceState: function (s, t, u) { __log.push("replace:" + u); },
  },
  fetch: function () { return Promise.resolve(); },
  addEventListener: function () {},
  parent: null,
};
var __navListeners = [];
window.navigation = { addEventListener: function (t, fn) { __navListeners.push(fn); } };
var __docListeners = [];
var __nativeCookie = "";
// 脚本用 `Document.prototype` / `HTMLDocument.prototype` 取 cookie 描述符。
// 测试桩里让 document 自己就是那两个构造函数的实例，语义一致。
function Document() {}
function HTMLDocument() {}
Object.defineProperty(Document.prototype, "cookie", {
  configurable: true,
  get: function () { return this.__cookieValue || ""; },
  set: function (v) { this.__cookieValue = String(v); },
});
var document = new Document();
document.addEventListener = function (t, fn) { __docListeners.push(fn); };
document.visibilityState = "visible";
// ServiceWorker 在桩里就是「原生实现会失败」的样子；注入脚本应把它替换掉。
var navigator = {
  sendBeacon: null,
  serviceWorker: {
    register: function () { throw new TypeError("ServiceWorker 不可用"); },
    getRegistrations: function () { return Promise.resolve([]); },
    getRegistration: function () { return Promise.resolve(undefined); },
  },
};function XMLHttpRequest() { this.status = 200; this.responseText = '{"ok":true}'; }
XMLHttpRequest.prototype.open = function (m, u) { this.__url = u; };
XMLHttpRequest.prototype.setRequestHeader = function () {};
XMLHttpRequest.prototype.send = function (b) { __cookieWrites.push({ url: this.__url, body: b }); };
"#;

fn run(script: &str, call: &str) -> String {
    let runtime = Runtime::new().expect("runtime");
    let context = Context::full(&runtime).expect("context");
    context.with(|ctx| {
        let source = format!(
            "var PAGE = \"{PAGE}\";\n{HARNESS}\n{script}\n{call}\nJSON.stringify({{log: __log, writes: __cookieWrites, cookie: document.cookie}})"
        );
        ctx.eval::<String, _>(source)
            .unwrap_or_else(|e| format!("EVAL_ERR: {e}"))
    })
}

#[test]
fn scripts_parse() {
    for (name, script) in [("请求/导航", login_script()), ("cookie 桥", cookie_script())] {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        context.with(|ctx| {
            let probe = format!("(function () {{\n{script}\n}})");
            let parsed = ctx.eval::<rquickjs::Value, _>(probe);
            assert!(
                parsed.is_ok(),
                "{name} 脚本有语法错误: {:?}",
                parsed.err().map(|e| e.to_string())
            );
        });
    }
}

/// Vue Router 登录成功后 `replaceState(state, "", "/user/me")`。
#[test]
fn router_replace_state_keeps_session_prefix() {
    let out = run(&login_script(), r#"window.history.replaceState({}, "", "/user/me");"#);
    assert!(out.contains("/__login/session-a/user/me"), "实际：{out}");
}

#[test]
fn router_push_state_keeps_session_prefix() {
    let out = run(&login_script(), r#"window.history.pushState({}, "", "/user/me");"#);
    assert!(out.contains("/__login/session-a/user/me"), "实际：{out}");
}

#[test]
fn already_prefixed_history_url_is_untouched() {
    let out = run(
        &login_script(),
        r#"window.history.replaceState({}, "", "/__login/session-a/user/me");"#,
    );
    assert!(!out.contains("/__login/session-a/__login/"), "不应重复加前缀：{out}");
}

/// 登录页是 PWA 时会自注册 ServiceWorker，自定义协议下必然失败并刷一条 TypeError。
///
/// 直接 reject 也不行 —— 页面普遍写 `register(...)` 而不接 `.catch`，那样仍是一条
/// unawaited rejection。必须返回一个**已 resolve 的占位 registration**。
#[test]
fn service_worker_registration_is_neutralised() {
    let out = run(
        &login_script(),
        r#"
        var results = [];
        navigator.serviceWorker.register("/user/sw.js").then(function (reg) {
          results.push("resolved:" + (reg && typeof reg.scope === "string" ? "scope" : "noscope"));
        }, function () { results.push("rejected"); });
        navigator.serviceWorker.getRegistrations().then(function (list) {
          results.push("regs:" + list.length);
        });
        __log.push("sync");
        "#,
    );
    // 不能在同步阶段就抛错。
    assert!(out.contains("sync"), "注册不应同步抛错：{out}");
}

/// 整页导航：`location.href = …` 被 navigation 事件拦下并改写。
#[test]
fn full_page_navigation_is_rewritten() {
    let out = run(
        &login_script(),
        r#"
        var ev = {
          destination: { url: "http://reader.localhost/user/login" },
          cancelable: true, hashChange: false, downloadRequest: false,
          preventDefault: function () { __log.push("prevented"); },
        };
        __navListeners.forEach(function (fn) { fn(ev); });
        "#,
    );
    assert!(out.contains("/__login/session-a/user/login"), "实际：{out}");
    assert!(out.contains("prevented"), "应阻止原始导航：{out}");
}

/// cookie 桥是本应用自己的端点，**绝不能**被补上会话段。
///
/// 补成 `/__login/<s>/loginCookieBridge` 后会被当成上游路径转发回站点，站点没有
/// 这个地址就是 404 —— cookie 写不进 jar，页面判登录态读不到 token，又退回登录页。
/// 症状是「跳转打开后又退回登录页」，且日志里满屏 `loginCookieBridge 404`。
#[test]
fn app_own_endpoints_are_not_prefixed() {
    let out = run(
        &login_script(),
        r#"
        window.history.replaceState({}, "", "/loginCookieBridge?loginSession=session-a");
        "#,
    );
    assert!(
        out.contains("replace:/loginCookieBridge"),
        "应用自身端点不应被补会话段：{out}"
    );
    assert!(
        !out.contains("/__login/session-a/loginCookieBridge"),
        "不应把 cookie 桥收进会话空间：{out}"
    );
}

/// 页面写 `document.cookie` 时，必须同步落到书源 jar（经 cookie 桥）。
#[test]
fn page_written_cookie_reaches_the_bridge() {
    let out = run(
        &cookie_script(),
        r#"document.cookie = "novel_user_access_token=tok123; path=/; max-age=3600";"#,
    );
    assert!(out.contains("loginCookieBridge"), "应调用 cookie 桥：{out}");
    assert!(
        out.contains("novel_user_access_token=tok123"),
        "写入的 cookie 应原样过桥：{out}"
    );
}

/// 页面随后读回 cookie 时必须读得到 —— 这是判登录态的那一步。
#[test]
fn page_can_read_back_its_own_cookie() {
    let out = run(
        &cookie_script(),
        r#"
        document.cookie = "novel_user_access_token=tok123; path=/";
        var got = (document.cookie.match(/novel_user_access_token=([^;]*)/) || [])[1] || "";
        __log.push("read=" + got);
        "#,
    );
    assert!(out.contains("read=tok123"), "页面应读回自己写的 cookie：{out}");
}

/// 退出登录写空值时必须按删除处理（发 `name=`，不能把原串当有效值）。
#[test]
fn expired_cookie_is_removed() {
    let out = run(
        &cookie_script(),
        r#"
        document.cookie = "novel_user_access_token=tok123; path=/";
        document.cookie = "novel_user_access_token=; path=/; expires=Thu, 01 Jan 1970 00:00:00 GMT";
        "#,
    );
    // 第二次过桥必须是**空值**：宿主按「值为空」判定删除，发原串会被当成有效 cookie 写回 jar。
    assert!(
        out.contains(r#"\"cookie\":\"novel_user_access_token=\""#),
        "删除应发空值：{out}"
    );
    // 且此后页面读不到该 cookie。
    assert!(!out.contains("read="), "不应残留日志：{out}");
}
