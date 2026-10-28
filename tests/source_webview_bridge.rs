//! `java.webView(...)` 的降级实现回归。
//!
//! legado 里 `java.webView(html, url, js)` 把 HTML 交给后台 WebView 渲染后再取
//! 内容，书源用它应付「正文由脚本注入」的站点。桌面端没有从 Rust 规则沙盒同步
//! 驱动 WebView2 的通道，但**必须仍然存在于 `java` 命名空间**：缺失时脚本第一行
//! 就是 `not a function`，整条正文规则中断，界面表现成「正文永远加载不出来」
//! （日志里只有一行 `[正文 JS] JS 异常: not a function`）。
//!
//! 这里断言三点：
//! 1. `java.webView` 是函数，不抛错；
//! 2. 传进来的 HTML 已含正文时，按 `js` 选择器能取到；
//! 3. `webView` 的默认行为（`js` 为空）返回整份 HTML。

use reader_rust::parser::js::eval_js;

const PAGE: &str = r#"<html><body>
<div id="pageReadWrap">
  <content class="alt-read-content">
    <article><section class="read-section">
      <h3>第一章</h3>
      <div class="read-content main-text">
        <p>第一段正文。</p>
        <p>第二段正文。</p>
      </div>
    </section></article>
  </content>
</div>
</body></html>"#;

/// 与书源里的典型写法同构：先 `java.webView`，再 `Jsoup.parse` + `select`。
#[test]
fn content_rule_using_web_view_resolves_paragraphs() {
    let script = r#"(function() {
    var html = java.webView(result, "https://example.test/book/1/2.html", null);
    var doc = org.jsoup.Jsoup.parse(html);
    var nodes = doc.select("div.read-content p");
    var res = [];

    for (var i = 0; i < nodes.size(); i++) {
        var text = nodes.get(i).text().trim();
        if (text.length < 1) { continue; }
        res.push(text);
    }
    return res.join("\n");
})();"#;

    let out = eval_js(script, PAGE, "https://example.test/book/1/2.html").expect("webView 不应抛错");
    assert_eq!(out, "第一段正文。\n第二段正文。");
}

/// `js` 为空时返回整份 HTML（legado `BackstageWebView.JS` 的
/// `document.documentElement.outerHTML` 语义）。
#[test]
fn web_view_without_script_returns_html() {
    let out = eval_js(
        "java.webView(result, null, null)",
        PAGE,
        "https://example.test/book/1/2.html",
    )
    .expect("webView 不应抛错");
    assert!(out.contains("第一段正文。"), "应原样返回 HTML: {out}");
}

/// `webViewGetSource` 按正则从渲染结果里取地址；无匹配返回空串而不是抛错。
#[test]
fn web_view_get_source_extracts_matching_url() {
    let html = r#"<html><body><img src="https://img.example.test/cover.jpg"></body></html>"#;
    let out = eval_js(
        r#"java.webViewGetSource(result, null, null, "https://img\\.example\\.test/[^\"]+")"#,
        html,
        "https://example.test/",
    )
    .expect("webViewGetSource 不应抛错");
    assert_eq!(out, "https://img.example.test/cover.jpg");

    let none = eval_js(
        r#"java.webViewGetSource(result, null, null, "https://none\\.example/[^\"]+")"#,
        html,
        "https://example.test/",
    )
    .expect("无匹配也要返回空串");
    assert!(none.is_empty());
}
