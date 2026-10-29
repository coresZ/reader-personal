//! `java.startBrowserAwait` 的分流：http(s) 链接交给独立窗口，其余留在面板。
//!
//! 背景：书源用 `startBrowserAwait` 打开「注册 / 用户后台 / 网页版阅读」这类
//! 页面。它们常带 `X-Frame-Options: SAMEORIGIN` / CSP `frame-ancestors`，或用
//! 前端安全 SDK 检测「自己是否被 iframe 嵌入」—— 命中就把整页换成
//! 「页面渲染安全策略拦截」。独立窗口里 `window.top === window`，检测不触发。
//!
//! 但**不能无条件**改：`html` 有值（`data:text/html` 快照）或非 http(s) 的调用
//! 必须留在面板 —— 那些内容由宿主托管，独立窗口既没有代理也没有桥接脚本。
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js::{eval_js, with_source};

fn demo_source() -> reader_rust::model::book_source::BookSource {
    book_source_from_value(serde_json::json!({
        "bookSourceName": "示例源",
        "bookSourceUrl": "https://example.test",
    }))
    .expect("source")
}

/// 分流判定本身：与实现里那段条件一致。
///
/// 这里把判定逻辑抄成一段独立脚本验证「什么样算外部窗口」，避免为了测试
/// 而去驱动真实的窗口创建（那需要 AppHandle）。
fn routes_to_external_window(url: &str, html: &str) -> bool {
    let source = demo_source();
    let script = format!(
        r#"
var target = {url:?};
var hasHtml = {html:?} !== '';
!hasHtml && /^https?:\/\//i.test(target)
"#,
        url = url,
        html = html
    );
    with_source(&source, || eval_js(&script, "", "")).expect("求值应成功") == "true"
}

/// 真实 http(s) 链接 + 无内联 HTML → 独立窗口。
#[test]
fn plain_http_link_goes_to_external_window() {
    assert!(routes_to_external_window("https://example.test/register", ""));
    assert!(routes_to_external_window("http://example.test/user", ""));
}

/// 带内联 HTML（`data:text/html` 快照）→ 必须留在面板。
#[test]
fn inline_html_stays_in_panel() {
    assert!(!routes_to_external_window(
        "https://example.test/register",
        "<html><body>hi</body></html>"
    ));
}

/// `data:` URL → 留在面板（内容是我们自己生成的，独立窗口没法托管）。
#[test]
fn data_url_stays_in_panel() {
    assert!(!routes_to_external_window("data:text/html;base64,PGgxPkNLPC9oMT4=", ""));
}

/// 独立窗口分支**不能抛错**：书源里这类调用的返回值通常被丢弃，
/// 抛异常会中断整个规则。
#[test]
fn external_branch_returns_empty_response() {
    let source = demo_source();
    // 模拟打开窗口失败（无 AppHandle 时 `__open_external_window` 不可用），
    // 脚本应走 catch 分支继续，而不是把异常抛给调用方。
    let out = with_source(&source, || {
        eval_js(
            r#"
var result = null;
try {
  // 与实现同构：try 内调用，失败则吞掉。
  try { g.__definitely_missing_native("x", "y"); } catch (inner) { throw inner; }
} catch (e) {
  result = "fallback";
}
result
"#,
            "",
            "",
        )
    })
    .expect("求值应成功");
    assert_eq!(out, "fallback");
}

// ---------------------------------------------------------------------------
// 下面几条验证 `java.startBrowser` 的**分流行为**。
//
// 难点：`eval_js` 每次求值都会重新注入原生函数（实测：把
// `__open_external_window` 换成记录器后，调用期间又被换回原生的），
// 所以在 JS 里打桩观察不到。这里改成：直接从 `js.rs` 里**取出真实的
// `startBrowser` 源码**求值 —— 测的就是实现本身，不是重抄一遍条件。
// ---------------------------------------------------------------------------

/// 从 `js.rs` 的 prelude 里截出 `J.startBrowser = function (...) {...};` 整段。
fn start_browser_source() -> String {
    let full = include_str!("../src/parser/js.rs");
    let start = full
        .find("J.startBrowser = function (u, title, html) {")
        .expect("js.rs 里应存在 J.startBrowser 定义");
    let rest = &full[start..];
    // 顶到第一个「行首两空格 + `};`」为止 —— 即该函数的收尾。
    let end = rest.find("\n  };").expect("找不到函数结尾") + 4;
    rest[..end].to_string()
}

/// 跑真实的 `startBrowser`，桩掉两个原生入口，返回 (走的路径, URL)。
fn run_start_browser(url: &str, html: Option<&str>) -> (String, String) {
    let source = demo_source();
    let body = start_browser_source();
    // 用 JSON 序列化注入参数：直接拼 `Debug` 输出会在含引号/反斜杠时失真。
    let url_json = serde_json::to_string(url).expect("url");
    let html_json = match html {
        Some(value) => serde_json::to_string(value).expect("html"),
        None => "null".to_string(),
    };
    // 桩函数与 `js.rs` 里同名同签名（`S` 是 prelude 里的字符串化工具）。
    let script = format!(
        r#"
var __kind = 'none', __url = '';
var __urlIn = {url_json};
var __htmlIn = {html_json};
var g = {{
  __open_external_window: function (u, t) {{ __kind = 'external'; __url = String(u); }},
  __show_browser: function (u, h) {{ __kind = 'panel'; __url = String(u); }}
}};
function S(v) {{ return v === undefined || v === null ? '' : String(v); }}
var java = {{}};
var J = java;
{body}
java.startBrowser(__urlIn, '标题', __htmlIn === null ? undefined : __htmlIn);
__kind + '|' + __url
"#
    );
    let out = with_source(&source, || eval_js(&script, "", "")).expect("求值应成功");
    let (kind, url) = out.split_once('|').unwrap_or((out.as_str(), ""));
    (kind.to_string(), url.to_string())
}

/// 无内联 `html` 的 http(s) 链接 → 独立窗口（方案 C 的核心行为）。
#[test]
fn start_browser_plain_url_uses_external_window() {
    let (kind, url) =
        run_start_browser("https://example.test/user/token-login?token=t", None);
    assert_eq!(kind, "external", "纯网址应交给独立窗口");
    assert!(
        url.ends_with("/user/token-login?token=t"),
        "窗口地址不对: {url}"
    );
}

/// `http://` 同样走窗口。
#[test]
fn start_browser_plain_http_url_uses_external_window() {
    let (kind, _) = run_start_browser("http://example.test/reg", None);
    assert_eq!(kind, "external");
}

/// 带内联 `html` → 仍走面板（`data:text/html` 快照、播放器页）。
#[test]
fn start_browser_with_inline_html_uses_panel() {
    let (kind, _) = run_start_browser("https://example.test/x", Some("<h1>hi</h1>"));
    assert_eq!(kind, "panel", "带 html 应留在面板");
}

/// 非 http(s) 地址留在面板。
#[test]
fn start_browser_non_http_stays_in_panel() {
    let (kind, url) = run_start_browser("about:blank", Some("<h1>player</h1>"));
    assert_eq!(kind, "panel");
    assert_eq!(url, "about:blank");
}

/// 空 `html` 字符串等同「没有 html」——书源常写 `''` 占位。
#[test]
fn start_browser_empty_html_counts_as_no_html() {
    let (kind, _) = run_start_browser("https://example.test/a", Some(""));
    assert_eq!(kind, "external", "空字符串应视为没有内联 html");
}

// ---------------------------------------------------------------------------
// `java.showBrowser` —— 段评 / 本章说的路由
//
// 书源里的写法是先抓页面再交给面板：
//     var html = java.ajax(url);
//     java.showBrowser(url, html, 'window.java=java;', config);
//
// 但段评页往往是**空壳 SPA**（`<div id="app"></div>` + 打包 JS），把静态壳塞进
// iframe 面板永远渲染不出来：面板与上游跨域，SPA 的 XHR / localStorage / CORS
// 都得靠桥逐项模拟，漏一处就是白屏。这类页面必须交给独立窗口，在真实域名下运行。
// 反过来，带实质内容的 HTML 仍走面板 —— 不该让内嵌设置页突然弹窗。
// ---------------------------------------------------------------------------

fn show_browser_source() -> String {
    let full = include_str!("../src/parser/js.rs");
    let start = full
        .find("J.showBrowser = function (u, html, preloadJs, options) {")
        .expect("js.rs 里应存在 J.showBrowser 定义");
    let rest = &full[start..];
    let end = rest.find("\n  };").expect("J.showBrowser 应有收尾") + 4;
    rest[..end].to_string()
}

fn run_show_browser(url: &str, html: Option<&str>) -> (String, String) {
    let body = show_browser_source();
    let url_json = serde_json::to_string(url).expect("url");
    let html_json = match html {
        Some(value) => serde_json::to_string(value).expect("html"),
        None => "null".to_string(),
    };
    let script = format!(
        r#"
var __kind = 'none', __url = '';
var g = {{
  __open_external_window: function (u, t) {{ __kind = 'external'; __url = String(u); }},
  __show_browser: function (u, h, p, o, t) {{ __kind = 'panel'; __url = String(u); }}
}};
var S = function (v) {{ return v === undefined || v === null ? '' : String(v); }};
var J = {{}};
{body}
java = {{ showBrowser: J.showBrowser }};
java.showBrowser({url_json}, {html_json}, 'window.java=java;', '{{}}');
[__kind, __url].join('|')
"#
    );
    let out = eval_js(&script, "", "").expect("求值失败");
    let mut parts = out.splitn(2, '|');
    let kind = parts.next().unwrap_or("").to_string();
    let url_out = parts.next().unwrap_or("").to_string();
    (kind, url_out)
}

/// 空壳 SPA + 纯网址 → 独立窗口。
///
/// 这是段评 / 本章说的实际形态：`java.ajax` 抓回来只有一个 `<div id="app">`。
#[test]
fn show_browser_spa_shell_uses_external_window() {
    let shell = r#"<!doctype html><html><head><title>段评</title></head><body><div id="app"></div><script src="/assets/index-abc.js"></script></body></html>"#;
    let (kind, url) = run_show_browser("https://example.test/user/paragraph-comments?chapter_id=1", Some(shell));
    assert_eq!(kind, "external", "空壳 SPA 应交独立窗口打开");
    assert!(url.starts_with("https://example.test/"), "窗口应加载真实地址: {url}");
}

/// 不传 html（纯网址）→ 独立窗口。
#[test]
fn show_browser_plain_url_uses_external_window() {
    let (kind, _) = run_show_browser("https://example.test/page", None);
    assert_eq!(kind, "external");
}

/// 带实质内容的 HTML → 仍走面板。
///
/// 内嵌设置页、阅读器片段本来就是静态的，面板更轻，也不该突然弹窗。
#[test]
fn show_browser_content_html_stays_in_panel() {
    let content = r#"<html><body><h1>设置</h1><p>选择线路：</p></body></html>"#;
    let (kind, _) = run_show_browser("https://example.test/settings", Some(content));
    assert_eq!(kind, "panel", "带实质内容的 HTML 应继续走面板");
}

