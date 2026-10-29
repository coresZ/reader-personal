//! 从独立登录窗口读回 Cookie 与 localStorage，写进书源的会话。
//!
//! 独立窗口打开的是**上游真实站点**，页面在那个域下写的 Cookie 只存在于
//! WebView 自己的存储里；书源后续请求走的是我们自己维护的 cookie jar，两边
//! 不通 —— 表现就是「窗口里登录成功了，回到面板还是显示未登录」。
//!
//! 手机端 legado 也是同一原理，只是 Android 提供同步的
//! `CookieManager.getCookie(url)`。
//!
//! ## Cookie：用 Tauri 的跨平台接口，不再手写 WebView2 COM
//!
//! 早先这里用 `ICoreWebView2CookieManager` + `GetCookiesCompletedHandler` 手动读
//! Cookie，并把整块关在 `#[cfg(target_os = "windows")]` 里。那套写法的代价是
//! macOS / Linux 没有实现（登录态不会回传），而调用方又无条件引用这些符号，
//! 非 Windows 直接编译失败。
//!
//! Tauri 2 的 `WebviewWindow::cookies()` 已经跨平台，底层由 wry 分平台实现：
//!   - Windows: `ICoreWebView2_2::CookieManager().GetCookies`
//!   - macOS:   `WKHTTPCookieStore.getAllCookies`
//!   - Linux:   WebKitGTK `CookieManager::all_cookies`
//!
//! 三者在等待异步回调时都会自己泵消息循环（`with_with_bump` /
//! `wait_for_blocking_operation` / `gtk::main_iteration`），因此可以从
//! `on_window_event` 这种同步回调里直接调，不会死锁 —— 和原先手写 COM 的
//! 做法本质相同，但一份代码覆盖三平台。
//!
//! `document.cookie` 那条路不可行：登录态最常见的是 `HttpOnly` Cookie。

/// 一条 Cookie 的必需字段。
#[derive(Debug, Clone)]
pub struct CookieRecord {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
}

impl CookieRecord {
    /// 拼成 `name=value`，供 `Cookie` 请求头使用。
    pub fn pair(&self) -> String {
        format!("{}={}", self.name, self.value)
    }
}

/// 读出窗口**当前可见的全部** Cookie（跨所有域）。
///
/// 取全部而不是按单个 URL 取：登录流程会跨多个子域写 Cookie。
pub fn read_all(window: &tauri::WebviewWindow) -> anyhow::Result<Vec<CookieRecord>> {
    let cookies = window
        .cookies()
        .map_err(|err| anyhow::anyhow!("读取 Cookie 失败: {err}"))?;
    Ok(cookies
        .into_iter()
        .filter(|cookie| !cookie.name().is_empty())
        .map(|cookie| CookieRecord {
            name: cookie.name().to_string(),
            value: cookie.value().to_string(),
            // `domain` / `path` 在个别平台可能为空串；调用方会退回按 URL 推导。
            domain: cookie.domain().unwrap_or_default().to_string(),
            path: cookie.path().unwrap_or("/").to_string(),
            secure: cookie.secure().unwrap_or(false),
            http_only: cookie.http_only().unwrap_or(false),
        })
        .collect())
}

/// 读出窗口**当前页面 origin 下**的 localStorage（`key=value` 对）。
///
/// 为什么需要它：一类书源的「段评 / 本章说」页面是 SPA，鉴权要两样东西 ——
/// `Authorization`（在 Cookie 里）和 `X-Device-ID`（在 `localStorage` 里）。
/// 独立窗口跑在**上游真实域名**下，localStorage 天然是对的；但段评面板挂在
/// `reader.localhost` origin 下，localStorage 与上游完全隔离，SPA 取不到
/// device id 就现生成一个假的，服务端判定异常返回空。
///
/// 因此窗口关闭时把 localStorage 一起收走，之后渲染面板时再注入回去。
///
/// 这条路目前**只有 Windows 实现**：wry 没有跨平台的「同步读脚本结果」接口，
/// macOS 要 `WKWebView.evaluateJavaScript`、Linux 要 webkit2gtk 的异步回调，
/// 各自需要单独适配。它只影响段评面板能否读到上游设备 id，不影响登录态回传
/// （那是上面的 `read_all`，三平台都支持）。
#[cfg(target_os = "windows")]
pub fn read_local_storage(window: &tauri::WebviewWindow) -> anyhow::Result<Vec<(String, String)>> {
    use std::sync::{Arc, Mutex};

    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
    use windows::core::{HSTRING, PCWSTR};

    // `{k:v,...}` 形式回传，宿主侧再解析。
    const SCRIPT: &str = r#"(function () {
  try {
    var out = {};
    for (var i = 0; i < localStorage.length; i++) {
      var key = localStorage.key(i);
      if (key === null) continue;
      out[key] = localStorage.getItem(key);
    }
    return JSON.stringify(out);
  } catch (e) { return '{}'; }
})()"#;

    let collected: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let failure: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let slot_out = Arc::clone(&collected);
    let slot_err = Arc::clone(&failure);
    let outer_out = Arc::clone(&collected);
    let outer_err = Arc::clone(&failure);

    window.with_webview(move |platform| unsafe {
        let core: ICoreWebView2 = match platform.controller().CoreWebView2() {
            Ok(core) => core,
            Err(err) => {
                *slot_err.lock().unwrap() = Some(format!("CoreWebView2: {err}"));
                return;
            }
        };
        let script = HSTRING::from(SCRIPT);
        let inner_out = Arc::clone(&slot_out);
        let inner_err = Arc::clone(&slot_err);
        let outcome = webview2_com::ExecuteScriptCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| {
                core.ExecuteScript(PCWSTR(script.as_ptr()), &handler)
                    .map_err(webview2_com::Error::from)
            }),
            // 回调签名是 `(Result<(), Error>, String)`：第二个参数已经是解好的
            // 脚本返回值（JSON 字面量，带外层引号）。
            Box::new(move |error_code: windows::core::Result<()>, value: String| {
                if let Err(err) = error_code {
                    *inner_err.lock().unwrap() = Some(format!("ExecuteScript: {err}"));
                    return Ok(());
                }
                if !value.is_empty() {
                    *inner_out.lock().unwrap() = Some(value);
                }
                Ok(())
            }),
        );
        if let Err(err) = outcome {
            *slot_err.lock().unwrap() = Some(format!("等待 localStorage 回调失败: {err}"));
        }
    })?;

    if let Some(err) = outer_err.lock().unwrap().clone() {
        anyhow::bail!(err);
    }
    // `ExecuteScript` 的结果本身是 JSON 字符串字面量（含引号），先按 JSON 解一次。
    let raw = outer_out.lock().unwrap().clone().unwrap_or_default();
    let decoded: String = serde_json::from_str(&raw).unwrap_or(raw);
    let parsed: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&decoded).unwrap_or_default();
    Ok(parsed
        .into_iter()
        .filter_map(|(key, value)| match value {
            serde_json::Value::String(text) => Some((key, text)),
            serde_json::Value::Null => None,
            other => Some((key, other.to_string())),
        })
        .collect())
}

/// 非 Windows：localStorage 读取暂未实现，返回空表让调用方走默认路径。
#[cfg(not(target_os = "windows"))]
pub fn read_local_storage(_window: &tauri::WebviewWindow) -> anyhow::Result<Vec<(String, String)>> {
    Ok(Vec::new())
}
