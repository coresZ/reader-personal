//! 从独立登录窗口读回 Cookie，写进书源的 cookie jar。
//!
//! 独立窗口打开的是**上游真实站点**，页面在那个域下写的 Cookie 只存在于
//! WebView2 的 profile 里；书源后续请求走的是我们自己维护的 cookie jar，两边
//! 不通 —— 表现就是「窗口里登录成功了，回到面板还是显示未登录」。
//!
//! 手机端 legado 也是同一原理，只是 Android 提供同步的
//! `CookieManager.getCookie(url)`；WebView2 没有等价物，`GetCookies` 是异步
//! 回调。这里用 `GetCookiesCompletedHandler::wait_for_async_operation` 把它
//! 同步化 —— 它会泵 WebView2 的消息循环，**不能**用 condvar 干等，否则回调
//! 永远派发不出来。
//!
//! `document.cookie` 那条路不可行：登录态最常见的是 `HttpOnly` Cookie。
#[cfg(target_os = "windows")]
mod imp {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Cookie, ICoreWebView2CookieList, ICoreWebView2_2,
    };
    use webview2_com::GetCookiesCompletedHandler;
    use windows::core::{Interface, HSTRING, PCWSTR, PWSTR};

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
        use std::sync::{Arc, Mutex};

        let collected: Arc<Mutex<Vec<CookieRecord>>> = Arc::new(Mutex::new(Vec::new()));
        let failure: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let slot_out = Arc::clone(&collected);
        let slot_err = Arc::clone(&failure);
        // 外层再克隆一份: 闭包里的 `slot_err` 会被回调移走, 之后还要读它。
        let outer_err = Arc::clone(&failure);
        let outer_out = Arc::clone(&collected);

        window.with_webview(move |platform| unsafe {
            let inner_err = Arc::clone(&slot_err);
            let inner_out = Arc::clone(&slot_out);
            let core = match platform.controller().CoreWebView2() {
                Ok(core) => core,
                Err(err) => {
                    *slot_err.lock().unwrap() = Some(format!("CoreWebView2: {err}"));
                    return;
                }
            };
            // `CookieManager()` 在 `ICoreWebView2_2` 上，基础接口没有。
            let core2: ICoreWebView2_2 = match core.cast() {
                Ok(core2) => core2,
                Err(err) => {
                    *slot_err.lock().unwrap() = Some(format!("接口版本不足: {err}"));
                    return;
                }
            };
            let manager = match core2.CookieManager() {
                Ok(manager) => manager,
                Err(err) => {
                    *slot_err.lock().unwrap() = Some(format!("CookieManager: {err}"));
                    return;
                }
            };
            // 空 URI = 不按地址过滤，返回 profile 里全部 Cookie。
            let empty = HSTRING::new();
            let outcome = GetCookiesCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| {
                    manager
                        .GetCookies(PCWSTR(empty.as_ptr()), &handler)
                        .map_err(webview2_com::Error::from)
                }),
                Box::new(
                    move |result: windows::core::Result<()>,
                          list: Option<ICoreWebView2CookieList>| {
                        if let Err(err) = result {
                            *inner_err.lock().unwrap() = Some(format!("GetCookies: {err}"));
                            return Ok(());
                        }
                        let Some(list) = list else { return Ok(()) };
                        let mut count = 0u32;
                        if list.Count(&mut count).is_err() {
                            return Ok(());
                        }
                        let mut out = inner_out.lock().unwrap();
                        for index in 0..count {
                            let Ok(cookie) = list.GetValueAtIndex(index) else { continue };
                            if let Some(record) = read_one(&cookie) {
                                out.push(record);
                            }
                        }
                        Ok(())
                    },
                ),
            );
            if let Err(err) = outcome {
                *slot_err.lock().unwrap() = Some(format!("等待 Cookie 回调失败: {err}"));
            }
        })?;

        if let Some(err) = outer_err.lock().unwrap().clone() {
            anyhow::bail!(err);
        }
        let result = outer_out.lock().unwrap().clone();
        Ok(result)
    }

    unsafe fn read_one(cookie: &ICoreWebView2Cookie) -> Option<CookieRecord> {
        let name = read_pwstr(|p| cookie.Name(p));
        if name.is_empty() {
            return None;
        }
        let mut secure = windows::core::BOOL(0);
        let mut http_only = windows::core::BOOL(0);
        let _ = cookie.IsSecure(&mut secure);
        let _ = cookie.IsHttpOnly(&mut http_only);
        Some(CookieRecord {
            name,
            value: read_pwstr(|p| cookie.Value(p)),
            domain: read_pwstr(|p| cookie.Domain(p)),
            path: read_pwstr(|p| cookie.Path(p)),
            secure: secure.as_bool(),
            http_only: http_only.as_bool(),
        })
    }

    /// 读出窗口**当前页面 origin 下**的 localStorage（`key=value` 对）。
    ///
    /// 为什么需要它：一类书源的「段评 / 本章说」页面是 SPA，鉴权要两样东西 ——
    /// `Authorization`（在 Cookie 里）和 `X-Device-ID`（在 `localStorage` 里）。
    /// 独立窗口跑在**上游真实域名**下，localStorage 天然是对的；但段评面板挂在
    /// `reader.localhost` origin 下，localStorage 与上游完全隔离，SPA 取不到
    /// device id 就现生成一个假的，服务端判定异常返回空 —— 表现就是
    /// 「手机端能看段评，桌面端点开一片空白」。
    ///
    /// 因此窗口关闭时把 localStorage 一起收走，之后渲染面板时再注入回去。
    /// WebView2 没有 localStorage 的 COM 接口，只能 `ExecuteScript` 读。
    pub fn read_local_storage(window: &tauri::WebviewWindow) -> anyhow::Result<Vec<(String, String)>> {
        use std::sync::{Arc, Mutex};

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
            let core = match platform.controller().CoreWebView2() {
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
                // 回调签名是 `(HRESULT, PCWSTR)`：`PCWSTR` 指向脚本返回值的
                // JSON 字面量（含外层引号），空指针表示脚本抛错或无返回值。
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

    /// 调用 `out PWSTR` 形式的 getter，读出字符串并归还 COM 内存。
    unsafe fn read_pwstr(mut get: impl FnMut(*mut PWSTR) -> windows::core::Result<()>) -> String {
        let mut ptr = PWSTR::null();
        if get(&mut ptr).is_err() || ptr.is_null() {
            return String::new();
        }
        let text = ptr.to_string().unwrap_or_default();
        windows::Win32::System::Com::CoTaskMemFree(Some(ptr.0 as *const _));
        text
    }
}

#[cfg(target_os = "windows")]
pub use imp::{read_all, read_local_storage, CookieRecord};
