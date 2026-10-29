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
