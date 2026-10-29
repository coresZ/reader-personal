//! `java.showBrowser(url, html, ...)` 的面板**带内联 HTML** 这条路径。
//!
//! 一类书源的段评 / 本章说这么用：
//!
//! ```js
//! var htmlContent = java.ajax(url);                       // 先把页面抓下来
//! java.showBrowser(url, htmlContent, 'window.java=java;', config);
//! ```
//!
//! 与「只给 URL」不同，这里宿主已经拿到了完整 HTML，面板应当**静态渲染**它，
//! 而不是再去上游请求一次 —— 那次请求没有脚本里的认证头，只会拿回登录页。
//!
//! 这里的断言集中在一个容易静默坏掉的地方：**桥脚本必须真的进了 HTML**。
//! 桥缺失时面板能显示内容，但宿主收不到 `ready`，前端等 5 秒后报
//! 「当前网页无法确认，请重新加载或取消后重试」—— 也就是用户看到的空白面板。
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js::{eval_js, with_source};
use reader_rust::service::book_service::ShowBrowserRequest;

fn source() -> reader_rust::model::book_source::BookSource {
    book_source_from_value(serde_json::json!({
        "bookSourceName": "示例源",
        "bookSourceUrl": "https://example.test",
    }))
    .expect("source")
}

/// 触发 `java.showBrowser(url, html, ...)`，返回面板请求。
///
/// `showBrowser`（非 await）走 `set_show_browser_request` 交给前端拉取，不是
/// `BROWSER_CONTEXT` 那条 handler —— 后者只服务 `startBrowserAwait`/独立窗口。
fn open_panel_with_html(url: &str, html: &str) -> ShowBrowserRequest {
    let source = source();
    let script = format!(
        r#"java.showBrowser({url:?}, {html:?}, 'window.java=java;', JSON.stringify({{heightPercentage:0.8}}))"#
    );
    with_source(&source, || eval_js(&script, "", "")).expect("求值应成功");

    let request =
        reader_rust::service::book_service::take_show_browser_request().expect("应产生面板请求");

    // 生产链路里是前端拿到这条 IPC 后再向宿主换一个 panelId（见
    // `register_browser_request`）；测试里直接完成这一步。
    let source_url = source.book_source_url.clone();
    reader_rust::service::source_browser::register_browser_request(
        request,
        reader_rust::service::source_browser::source_panel_entry(&source_url)
            .expect("面板条目"),
    )
    .expect("注册面板")
}

/// 带内联 HTML 时，宿主应当把这个 HTML 存进面板（而不是留空等着去抓上游）。
///
/// 留空的后果：`source_panel_route` 会发现 `entry.html.is_empty()` 于是**重新请求上游**，
/// 而那次请求没有脚本里的 `Authorization` 头 —— 段评页是登录后才能看的，
/// 于是拿回登录页/空页，面板一片空白。
#[test]
fn inline_html_is_kept_on_the_panel() {
    let body = r#"<html><head><title>段评</title></head><body><div id="comments">共 3 条</div></body></html>"#;
    let request = open_panel_with_html("https://example.test/user/paragraph-comments?x=1", body);

    // 面板 HTML 由后端托管，IPC 只回 panelId，因此这里按 id 取回渲染结果。
    let page = reader_rust::api::protocol::build_source_panel_page(
        request.panel_id.as_deref().expect("应有 panelId"),
    )
    .expect("面板应可渲染");

    assert!(
        page.contains(r#"id="comments">共 3 条"#),
        "面板应静态渲染传入的 HTML（否则会退回去请求需要登录的上游）"
    );
}

/// 面板 HTML 必须带宿主桥脚本 —— 缺了它，前端收不到 `ready`，
/// 5 秒后报「当前网页无法确认」，表现就是这个源点开段评一片空白。
#[test]
fn panel_page_injects_bridge_script() {
    let request = open_panel_with_html(
        "https://example.test/user/paragraph-comments?x=1",
        "<html><head><title>段评</title></head><body>hi</body></html>",
    );
    let page = reader_rust::api::protocol::build_source_panel_page(
        request.panel_id.as_deref().expect("应有 panelId"),
    )
    .expect("面板应可渲染");

    // 桥脚本的关键标记：ready 上报与同步回调入口。
    assert!(
        page.contains("reader:ready"),
        "桥脚本应处理宿主的 reader:ready 握手"
    );
    assert!(
        page.contains("sourcePanelBridge"),
        "桥脚本应指向 /sourcePanelBridge 同步接口"
    );
    // 桥必须排在页面自己的脚本之前，否则页面脚本先跑就把状态改了。
    let bridge_at = page.find("sourcePanelBridge").expect("桥位置");
    let title_at = page.find("<title>").expect("标题位置");
    assert!(
        bridge_at < title_at,
        "桥脚本应注入到 <head> 起始处（在页面脚本之前）"
    );
}

/// `preload_js`（`window.java=java;`）要一起注入：段评页里的内联脚本靠它拿到 `java`。
#[test]
fn panel_page_injects_preload_js() {
    let request = open_panel_with_html(
        "https://example.test/user/paragraph-comments?x=1",
        "<html><head></head><body>hi</body></html>",
    );
    let page = reader_rust::api::protocol::build_source_panel_page(
        request.panel_id.as_deref().expect("应有 panelId"),
    )
    .expect("面板应可渲染");

    assert!(
        page.contains("window.java=java"),
        "preload_js 应被注入到面板页"
    );
}


/// 上游站点的 `localStorage` 必须被注入到面板里。
///
/// 段评页是 SPA，鉴权 = Cookie 里的 token + `localStorage` 里的设备 id。
/// 面板跑在 reader origin 下，localStorage 与上游隔离，不注入就只能现生成
/// 一个假 id，服务端判定异常返回空 —— 也就是「手机端能看段评、桌面端点开空白」。
#[test]
fn panel_injects_upstream_local_storage() {
    use reader_rust::service::book_service::set_source_window_storage_for_test;

    // 用 example.test 这个 origin 作为「上游」。
    set_source_window_storage_for_test(
        "https://example.test",
        &[
            ("app_device_id".to_string(), "device-abc".to_string()),
            ("theme".to_string(), "dark".to_string()),
        ],
    );

    let request = open_panel_with_html(
        "https://example.test/user/paragraph-comments?x=1",
        "<html><head></head><body>hi</body></html>",
    );
    let page = reader_rust::api::protocol::build_source_panel_page(
        request.panel_id.as_deref().expect("应有 panelId"),
    )
    .expect("面板应可渲染");

    assert!(
        page.contains("app_device_id"),
        "面板应注入上游 localStorage 的键"
    );
    assert!(
        page.contains("device-abc"),
        "面板应注入上游 localStorage 的值"
    );
    assert!(
        page.contains("localStorage.setItem"),
        "注入应当真的写进 localStorage（缺才写，不覆盖页面自己的值）"
    );
}


/// 桥必须同时拦截 `fetch` **和** `XMLHttpRequest`。
///
/// axios 在浏览器环境里默认走 XHR（只有显式 `adapter: 'fetch'` 才用 fetch）。
/// 段评 / 本章说这类页面是 SPA，全部用 axios —— 只重写 `fetch` 拦不住，
/// 请求会从 reader origin 直连上游：CORS 不放行、jar 里的 Cookie 也带不过去，
/// 服务端返空，面板一片空白。这条测试盯的就是这个缺口。
#[test]
fn panel_bridge_intercepts_xhr_and_fetch() {
    let script = reader_rust::api::protocol::build_source_panel_bridge_script(
        "panel-xhr",
        false,
        &[],
    );

    assert!(
        script.contains("window.fetch ="),
        "桥应重写 fetch"
    );
    assert!(
        script.contains("window.XMLHttpRequest = BridgedXHR"),
        "桥应重写 XMLHttpRequest（axios 默认用它）"
    );
    // 按行过滤掉注释，避免「把调用注释掉」也能通过。
    let effective: String = script
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        effective.contains("  installXhrBridge();"),
        "XHR 桥应被真正安装（不是注释掉的）"
    );
    // 桥自身的同步 XHR 必须走原生实现，否则会自己拦自己。
    assert!(
        script.contains("var NativeXHR = window.XMLHttpRequest"),
        "桥应保留原生 XHR 引用，避免递归"
    );
    assert!(
        script.contains("new NativeXHR()"),
        "同源/桥自身请求应回退到原生 XHR"
    );
}
