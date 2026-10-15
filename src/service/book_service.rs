use crate::crawler::{
    fetcher::{fetch_with_client, FetchResponse, RequestSpec, StrResponse},
    http_client::{HttpClient, ProxyMode, ProxyStatus},
    url_analyzer::{analyze_url, split_url_options},
};
use crate::error::error::AppError;
use crate::model::{
    book::Book,
    book_chapter::BookChapter,
    book_source::{BookSource, ExploreKind},
    search::SearchBook,
};
use crate::parser::js::{
    eval_js_with_bindings, take_js_logs, with_book_context, with_source, with_source_key,
    JsBookContext,
};
use crate::parser::rule_engine::RuleEngine;
use crate::storage::cache::file_cache::FileCache;
use crate::service::local_pdf_book::{is_local_pdf_origin, is_local_pdf_url};
use crate::util::hash::md5_hex;
use crate::util::text::{normalize_source_url, repair_encoded_url};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{sleep, Duration, Instant};

/// 书源 JS 调用 `java.showBrowser(url, html)` 时捕获的请求, 供前端弹出面板。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowBrowserRequest {
    pub url: String,
    pub html: Option<String>,
    pub preload_js: Option<String>,
    pub options: Option<serde_json::Value>,
    pub title: Option<String>,
    #[serde(default)]
    pub await_result: bool,
    /// 后端托管的面板 id(前端按 `${readerOrigin}/sourcePanel?panelId=...` 加载)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panel_id: Option<String>,
}

thread_local! {
    /// `showBrowser` 由 JS 同步调用, 结果先落在线程本地, 由调用方取走。
    static SHOW_BROWSER_REQUEST: std::cell::RefCell<Option<ShowBrowserRequest>> =
        const { std::cell::RefCell::new(None) };
}

/// 供 js.rs 的原生 `__show_browser` 原语写入。
pub fn set_show_browser_request(request: ShowBrowserRequest) {
    SHOW_BROWSER_REQUEST.with(|cell| *cell.borrow_mut() = Some(request));
}

/// 取走当前线程捕获的 `showBrowser` 请求(取走即清空, 不跨次调用泄漏)。
pub fn take_show_browser_request() -> Option<ShowBrowserRequest> {
    SHOW_BROWSER_REQUEST.with(|cell| cell.borrow_mut().take())
}

fn clear_show_browser_request() {
    SHOW_BROWSER_REQUEST.with(|cell| *cell.borrow_mut() = None);
}

/// 已打开的 `java.showBrowser` 面板: 面板 HTML 由后端托管, 前端只需按 id 加载
/// `${readerOrigin}/sourcePanel?panelId=...`, 面板内的桥脚本即可同源回调宿主。
#[derive(Clone, Default)]
pub struct SourcePanelEntry {
    pub url: String,
    pub html: String,
    pub preload_js: Option<String>,
    pub http: Option<crate::crawler::source_session::SourceHttpContext>,
    /// 归属书源, 面板内 `window.run` 需要在其 jsLib 作用域执行。
    pub book_source_url: String,
    pub book_url: String,
    pub chapter_url: String,
    /// `book` / `chapter` 绑定(JSON), 面板脚本 `writeConfig(chapter, ...)` 需要。
    pub book_json: String,
    pub chapter_json: String,
    /// 面板内的**二级链接**是否交给系统浏览器打开。
    ///
    /// RSS 单 URL 源的面板只托管首屏 HTML(在 `reader` origin 下渲染, 所以能正常
    /// 显示); 页面里的链接指向上游站点, 若在 iframe 内直接导航, 上游的
    /// `X-Frame-Options` / CSP `frame-ancestors` 会让 WebView 拒绝渲染 —— 表现为
    /// **白屏**。
    ///
    /// 书源登录面板必须保持 `false`: 登录/注册流程依赖 iframe 内导航。
    pub external_links: bool,
}

static SOURCE_PANELS: std::sync::LazyLock<std::sync::Mutex<Vec<(String, SourcePanelEntry)>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Vec::new()));

/// 同时保留的面板数上限(插件通常只有 1~2 层弹层)。
const MAX_SOURCE_PANELS: usize = 8;

/// 登记一个面板并返回其 id。
pub fn register_source_panel(entry: SourcePanelEntry) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut panels = SOURCE_PANELS.lock().unwrap_or_else(|e| e.into_inner());
    // 按插入顺序淘汰最旧的(用 Vec 而非 HashMap: 后者无序, 淘汰会变成随机的,
    // 可能剔掉正在显示的面板导致其后续交互 404)
    if panels.len() >= MAX_SOURCE_PANELS {
        let (old_id, _) = panels.remove(0);
        crate::service::source_browser::cancel_browser(&old_id);
    }
    panels.push((id.clone(), entry));
    id
}

pub fn remove_source_panel(panel_id: &str) {
    SOURCE_PANELS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(id, _)| id != panel_id);
}

pub fn get_source_panel(panel_id: &str) -> Option<SourcePanelEntry> {
    SOURCE_PANELS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(id, _)| id == panel_id)
        .map(|(_, entry)| entry.clone())
}

/// 把面板页写入的 `document.cookie` 落到书源 cookie jar。
///
/// 面板页(如某聚合源的登录页)用 `document.cookie = 'user_token=…'` 记录会话,
/// 而上游后续页面由**服务端**读 Cookie 判断登录态。只写浏览器的话这份 cookie
/// 留在面板 origin 下, 转发请求带不上, 上游就把页面踢回登录页(用户看到
/// 「登录过期」)。这里按面板当前的上游地址写入 jar, 让后续转发带上它。
///
/// 目标地址必须落在书源域内 —— 否则面板页脚本就能把会话写到任意域。
pub(crate) fn set_panel_cookie(entry: &SourcePanelEntry, raw_url: &str, cookie: &str) -> Result<(), String> {
    let head = cookie.split(';').next().unwrap_or("").trim();
    let Some((name, value)) = head.split_once('=') else {
        return Err("Cookie 格式无效".to_string());
    };
    let name = name.trim();
    if name.is_empty() {
        return Err("Cookie 格式无效".to_string());
    }
    // 控制字符在写入处就拒掉, 不依赖下层只做 header 值校验。
    if name.chars().any(char::is_control) || value.chars().any(char::is_control) {
        return Err("Cookie 含控制字符".to_string());
    }
    let http = entry.http.clone().map(Ok).unwrap_or_else(|| {
        crate::parser::js::source_http_context(&entry.book_source_url)
    }).map_err(|e| e.to_string())?;
    // 以面板当前的上游地址为域写入; 页面可能在子路径下, 域名与端口仍取自它。
    let target = if raw_url.is_empty() { entry.url.clone() } else { raw_url.to_string() };
    let url = crate::api::protocol::resolve_panel_cookie_url(&target, &entry.url)
        .map_err(|e| e.to_string())?;
    if !crate::api::protocol::proxy_target_matches_source(url.as_str(), &entry.url) {
        return Err("Cookie 目标地址不在书源域内".to_string());
    }
    // Cookie 不区分 scheme: 把 https 会话写进 http 目标会让后续明文请求带上它。
    // 只在此处拦降级, 不改共享的域判定函数(那会影响既有代理行为)。
    let source_is_https = crate::api::protocol::resolve_panel_cookie_url(&entry.url, &entry.url)
        .map(|u| u.scheme() == "https").unwrap_or(false);
    if source_is_https && url.scheme() == "http" {
        return Err("不允许把 https 会话写入 http 目标".to_string());
    }
    // 空值代表页面删除了该 cookie(退出登录), 必须从 jar 移除, 否则空值会被当成
    // 有效值发给上游。
    if value.trim().is_empty() {
        return http.session.cookies.remove(&url, name).map_err(|e| e.to_string());
    }
    http.session.cookies.set(&url, head).map_err(|e| e.to_string())
}

/// 处理面板内 `window.java` / `window.cache` / `window.run` 的同步桥请求。
///
/// 与 legado `WebJsExtensions` 对齐: `ajax`/`get`/`post`/`connect` 走书源网络层,
/// `run` 在书源 jsLib 作用域执行脚本, `cache` 复用书源变量存储。
pub async fn handle_source_panel_bridge(
    book_source_service: &crate::service::book_source_service::BookSourceService,
    entry: &SourcePanelEntry,
    action: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let text_of = |key: &str| -> String {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };

    match action {
        "log" => {
            crate::parser::js::push_js_log(format!("[panel] {}", text_of("message")));
            Ok(serde_json::Value::Null)
        }
        "toast" => {
            let message = text_of("message");
            crate::parser::js::push_js_log(format!("[panel.toast] {message}"));
            Ok(serde_json::Value::String(message))
        }
        "close" => {
            Ok(serde_json::Value::Null)
        }
        "sourceGet" | "sourceSet" => {
            let key = text_of("key");
            if !matches!(key.as_str(), "variable" | "loginInfo" | "loginHeader") {
                return Err("未知的书源状态".to_string());
            }
            let http = entry.http.clone().map(Ok).unwrap_or_else(|| {
                crate::parser::js::source_http_context(&entry.book_source_url)
            }).map_err(|e| e.to_string())?;
            if action == "sourceSet" {
                let value = payload.get("value").and_then(|v| v.as_str());
                http.session.put(&key, value).map_err(|e| e.to_string())?;
                Ok(json!(true))
            } else {
                Ok(json!(http.session.get(&key)))
            }
        }
        // 面板页 JS 用 `document.cookie = 'k=v'` 记录登录态(某聚合源的登录页写
        // `user_token`), 而上游后续页面由**服务端**读 Cookie 判断登录, 只留在浏览器里
        // 转发请求带不上, 上游会当作未登录并把页面踢回登录页(表现为「登录过期」)。
        // 这里把它落到书源 jar, 随后的转发请求就能带上。
        "cookieSet" => match set_panel_cookie(entry, &text_of("url"), &text_of("cookie")) {
            Ok(()) => Ok(json!(true)),
            Err(error) => {
                // 域校验拒绝/格式错误等要留痕: 「登录过期」类问题的现场诊断依赖这条日志。
                crate::parser::js::push_js_log(format!("[panel.cookieSet] {error}"));
                Err(error)
            }
        },
        "cacheGet" => {
            let key = text_of("key");
            Ok(crate::parser::js::get_panel_cache(&entry.book_source_url, &key)
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null))
        }
        "cachePut" => {
            let key = text_of("key");
            let value = text_of("value");
            crate::parser::js::put_panel_cache(&entry.book_source_url, &key, &value);
            Ok(serde_json::Value::Null)
        }
        "run" => {
            let script = text_of("script");
            let source = book_source_service
                .get("default", &entry.book_source_url)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "bookSource not found".to_string())?;
            let entry = entry.clone();
            // 面板脚本形如 `writeConfig(chapter, 'k', v, java)`, 需要 book/chapter 绑定;
            // 直接用面板登记时的书籍上下文, 否则脚本里的 `chapter` 会是空对象。
            let book_ctx = crate::parser::js::JsBookContext {
                book: serde_json::from_str(&entry.book_json).unwrap_or_else(|_| json!({})),
                chapter: serde_json::from_str(&entry.chapter_json).unwrap_or_else(|_| json!({})),
            };
            let output = tokio::task::spawn_blocking(move || {
                let execute = || crate::parser::js::with_book_context(Some(book_ctx), || {
                    crate::parser::js::with_source(&source, || {
                        crate::parser::js::with_source_key(Some(&entry.book_url), || {
                            take_js_logs();
                            clear_show_browser_request();
                            crate::parser::js::take_login_effects();
                            let result = crate::parser::js::eval_js(&script, "", &entry.url);
                            let browser = take_show_browser_request();
                            let logs = take_js_logs();
                            let effects = crate::parser::js::take_login_effects();
                            let result = result?;
                            let browser = browser.map(|request| {
                                crate::service::source_browser::register_browser_request(request, entry.clone())
                            }).transpose()?;
                            Ok::<_, anyhow::Error>(json!({"result":result,"showBrowser":browser,"logs":logs,"loginEffects":effects}))
                        })
                    })
                });
                match entry.http.clone() {
                    Some(http) => crate::parser::js::with_http_context(http, execute),
                    None => execute(),
                }
            })
            .await
            .map_err(|e| e.to_string())?;
            output.map_err(|e| format!("脚本执行失败: {e}"))
        }
        "ajax" | "connect" | "http" | "fetch" => {
            let source = book_source_service
                .get("default", &entry.book_source_url)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "bookSource not found".to_string())?;
            let action = action.to_string();
            let payload = payload.clone();
            let entry = entry.clone();
            let result = tokio::task::spawn_blocking(move || {
                let execute = || {
                    if action == "fetch" {
                        crate::parser::js::run_panel_fetch(&source, &entry.url, &action, &payload)
                    } else {
                        crate::parser::js::run_panel_request(&source, &entry.url, &action, &payload).map(serde_json::Value::String)
                    }
                };
                match entry.http.clone() {
                    Some(http) => crate::parser::js::with_http_context(http, execute),
                    None => execute(),
                }
            })
            .await
            .map_err(|e| e.to_string())?;
            result.map_err(|e| e.to_string())
        }
        // 面板内再次调 `java.showBrowser`: 登记为新面板并回传 panelId,
        // 由桥通过 postMessage 通知宿主换成新面板(嵌套弹层)。
        "showBrowser" => {
            let options = payload.get("options").and_then(|value| {
                if let Some(text) = value.as_str() { serde_json::from_str::<serde_json::Value>(text).ok() }
                else { Some(value.clone()) }
            }).filter(|value| value.is_object());
            let request = crate::service::source_browser::register_browser_request(ShowBrowserRequest {
                url: text_of("url"),
                html: payload.get("html").and_then(|v| v.as_str()).map(str::to_string),
                preload_js: payload.get("preloadJs").and_then(|v| v.as_str()).map(str::to_string),
                options, title: None, await_result: false, panel_id: None,
            }, entry.clone()).map_err(|e| e.to_string())?;
            Ok(json!(request))
        }
        // 面板签名用的对称加密(与书源规则的 `java.createSymmetricCrypto` 同一实现)。
        // 部分书源接口靠它签请求, 缺了这个插件只能降级到 CryptoJS 注入, 容易失败。
        "crypto" => crate::parser::js::panel_symmetric_crypto(&payload)
            .map(serde_json::Value::String)
            .map_err(|e| e.to_string()),
        "digest" => crate::parser::js::panel_digest(&payload)
            .map(serde_json::Value::String)
            .map_err(|e| e.to_string()),
        other => Err(format!("未知的面板桥操作: {other}")),
    }
}

/// 章节内容加载阶段: 让前端显示"正在做什么", 避免长时间无反馈看起来像卡死。
///
/// 只用于**阅读时的单章加载**。缓存是并发多章的(见 `cache_book_sse`),
/// 同一时刻不同章节处在不同阶段, 单一阶段对它没有意义。
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentLoadStage {
    /// 正在请求书源章节页
    Fetching,
    /// 正在执行正文规则(含书源脚本与段评解析), 通常是最耗时的一步
    Parsing,
    /// 正在写入本地缓存
    Caching,
}

/// State for background chapter fetching
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ChapterPagination {
    pub user_ns: String,
    pub source: BookSource,
    pub toc_url: String,
    pub visited_urls: Vec<String>,
    pub pending_urls: Vec<String>,
    pub seen_chapter_urls: Vec<String>,
    pub next_index: i32,
}

/// 取消信号: 测试书源等后台任务用它中断在途请求。
///
/// 用 `AtomicBool` 而不是 `CancellationToken`, 与既有的 `CACHE_BOOK_CANCELLATIONS`
/// / `SEARCH_CANCELLATIONS` 保持同一套模式。
pub type CancelSignal = Arc<std::sync::atomic::AtomicBool>;

/// 检查取消标志(集中在两处 loop 里用, 避免每处都写一遍 `load(Relaxed)`)。
fn is_cancelled(signal: Option<&CancelSignal>) -> bool {
    signal.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
}

/// 取消错误标记: 让调用方能把它与「站点请求失败」区分开。
///
/// 测试书源必须区分这两者 —— 把用户主动中止当成检测失败会把可用书源误标为失效。
pub const CANCELLED_ERR: &str = "cancelled by user";

#[derive(Clone)]
pub struct BookService {
    http: HttpClient,
    parser: RuleEngine,
    cache: FileCache,
    storage_dir: PathBuf,
    login_sessions: Arc<RwLock<HashMap<String, SourceLoginSession>>>,
    rate_states: Arc<RwLock<HashMap<String, RateState>>>,
    bookshelf_write_lock: Arc<Mutex<()>>,
}

#[derive(Clone, Default)]
struct RateState {
    in_flight: bool,
    last_start: Option<Instant>,
    window_starts: Vec<Instant>,
}

/// 发现页控件 `action` 的执行结果。
///
/// `action` 脚本通常只产生**副作用**（刷新发现页、打开登录面板、发起搜索），
/// 返回值往往为空，所以真正要传回前端的是这里的一组意图标志。
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreActionOutcome {
    /// 脚本的返回值（多数 action 为空串）。
    pub value: String,
    /// `java.refreshExplore()` 被调用：前端应重新拉分类与列表。
    pub explore_refresh: bool,
    /// `java.reLoginView()` / `source.login()`：前端应重开登录面板。
    pub login_refresh: bool,
    pub login: bool,
    /// `java.open(...)` 请求打开的面板，前端据此跳转。
    pub open_urls: Option<String>,
    /// `java.open` / `java.searchBook` / `java.openBook` 登记的待执行动作，
    /// 形如 `{"kind":"search","payload":{...}}`。
    pub intents: Vec<serde_json::Value>,
    /// `java.startBrowser` / `java.showBrowser` 弹出的面板（legado 的
    /// BottomWebViewDialog）。
    ///
    /// 缺了它，"更新书源""书源设置""刷新发现页设置"这类按钮点了毫无反应 ——
    /// 脚本已经把面板 HTML 准备好了，只是没人把它交给前端渲染。
    pub show_browser: Option<ShowBrowserRequest>,
    /// 脚本写入的 UI 值（如 `java.upLoginData`），前端可回填到控件。
    pub values: serde_json::Map<String, serde_json::Value>,
    /// 脚本产生的提示（`java.toast` / `java.longToast`）。
    ///
    /// "更新配置""已是最新配置"这类反馈全靠它 —— 早先被丢掉，用户以为按钮坏了。
    pub messages: Vec<String>,
    pub logs: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookSourceAvailability {
    pub book_source_url: String,
    pub book_source_name: String,
    pub valid: bool,
    pub search_ok: bool,
    pub explore_ok: bool,
    pub keyword: String,
    pub explore_url: Option<String>,
    pub search_error: Option<String>,
    pub explore_error: Option<String>,
    /// 用户主动中止导致本次检测未完成。
    ///
    /// 调用方必须跳过"按结果写库": 这不是"站点不可用", 把它当失效会把好源标坏。
    /// 统计时也要排除, 否则中止会让"失效数"虚高。
    #[serde(default)]
    pub cancelled: bool,
}

/// 构造一个"已取消"的占位结果(不写库、不计入有效/失效)。
fn cancelled_availability(source: &BookSource, keyword: &str) -> BookSourceAvailability {
    BookSourceAvailability {
        book_source_url: source.book_source_url.clone(),
        book_source_name: source.book_source_name.clone(),
        valid: false,
        search_ok: false,
        explore_ok: false,
        keyword: keyword.to_string(),
        explore_url: None,
        search_error: None,
        explore_error: None,
        cancelled: true,
    }
}

/// A single step of the source debugger: what URL was fetched, the raw response
/// (truncated) and the parsed result.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugTrace {
    pub request_url: String,
    pub status: u16,
    pub body: String,
    pub result: serde_json::Value,
    /// 反爬/异常特征提示, 帮助区分「站点拦截」与「规则写错」。
    pub warnings: Vec<String>,
    /// 响应头(Set-Cookie / Location 等), 辅助判断 UA 切换与重定向。
    pub headers: Vec<(String, String)>,
}

#[derive(Clone)]
struct SourceLoginSession {
    source_url: String,
    created_at: Instant,
}

/// Detect anti-crawler / degradation markers so the debugger can tell「被站点拦截」
/// apart from「规则写错」. Factors collected from the Qidian debugging session:
/// - 202 + `var buid` = PC 站验证壳(要浏览器指纹)
/// - 验证码页 / WAF tunnel
/// - 移动站被重定向到 PC 站(`source=m_jump`)
/// - 非 HTML 的极小响应(代理网关 / 错误页)
fn detect_anti_crawler(
    status: u16,
    body: &str,
    headers: &[(String, String)],
) -> Vec<String> {
    let mut warnings = Vec::new();
    let lower = body.to_lowercase();

    if status >= 400 {
        warnings.push(format!("HTTP {status}, 疑似被拦截或页面异常"));
    }
    if status == 202 || lower.contains("var buid") {
        warnings.push("反爬验证壳(202 / var buid), 站点要求浏览器指纹, 普通请求必被拦".to_string());
    }
    if lower.contains("turing.captcha") || lower.contains("tcaptcha") || lower.contains("验证码")
    {
        warnings.push("触发验证码(WAF), 需要真人验证".to_string());
    }
    if lower.contains("source=m_jump") || lower.contains("m_jump") {
        warnings.push("被重定向到 PC 站(UA 触发移动→PC 跳转)".to_string());
    }
    let is_html = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-type") && v.to_lowercase().contains("text/html")
    });
    if !is_html && body.len() < 2000 {
        warnings.push("响应非 HTML 且过小, 可能被代理/网关拦截".to_string());
    }
    warnings
}

/// Build a DebugTrace from a fetched response, attaching anti-crawler warnings
/// and the response headers for the debugger UI.
fn debug_trace_from(res: &FetchResponse, result: serde_json::Value) -> DebugTrace {
    let warnings = detect_anti_crawler(res.status, &res.body, &res.headers);
    DebugTrace {
        request_url: res.url.clone(),
        status: res.status,
        body: truncate_trace_body(&res.body),
        result,
        warnings,
        headers: res.headers.clone(),
    }
}

/// Keep the raw response readable in the debugger UI without flooding it.
fn truncate_trace_body(body: &str) -> String {
    const MAX: usize = 50_000;
    if body.len() <= MAX {
        body.to_string()
    } else {
        // body[..MAX] 若 MAX 落在多字节 UTF-8 字符中间会 panic。
        // 用 floor_char_boundary 回退到最近的字符边界。
        let mut end = MAX;
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
        let mut s = body[..end].to_string();
        s.push_str("\n…[截断]");
        s
    }
}

impl BookService {
    pub fn new(http: HttpClient, parser: RuleEngine, cache: FileCache, storage_dir: &str) -> Self {
        let storage_dir = PathBuf::from(storage_dir);
        http.set_source_storage_dir(storage_dir.join("source-state"));
        Self {
            http,
            parser,
            cache,
            storage_dir,
            login_sessions: Arc::new(RwLock::new(HashMap::new())),
            rate_states: Arc::new(RwLock::new(HashMap::new())),
            bookshelf_write_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn http_client(&self) -> reqwest::Client {
        self.http.client()
    }

    /// Return a client isolated to one book source. Its cookie jar is never
    /// shared with another source or with the app's generic HTTP client.
    pub async fn source_http_client(
        &self, user_ns: &str, source_url: &str, proxy: Option<&str>,
    ) -> Result<reqwest::Client, AppError> {
        Ok(self.http.source_context(user_ns, source_url, proxy).map_err(AppError::Internal)?.client)
    }

    pub fn source_scope<T>(&self, user_ns: &str, source: &BookSource, f: impl FnOnce() -> T) -> Result<T, AppError> {
        let context = self.http.source_context(user_ns, &source.book_source_url, None).map_err(AppError::Internal)?;
        Ok(crate::parser::js::with_http_context(context, || with_source(source, f)))
    }

    async fn run_source_task<T: Send + 'static>(
        &self, user_ns: &str, source: &BookSource,
        f: impl FnOnce(&BookSource) -> T + Send + 'static,
    ) -> Result<T, AppError> {
        let context = self.http.source_context(user_ns, &source.book_source_url, None).map_err(AppError::Internal)?;
        let source = source.clone();
        tokio::task::spawn_blocking(move || crate::parser::js::with_http_context(context, || with_source(&source, || f(&source))))
            .await.map_err(|err| AppError::Internal(anyhow::anyhow!("书源任务失败: {err}")))
    }

    async fn analyze_request(&self, user_ns: &str, source: &BookSource, rule: &str, key: &str, page: i32, base: &str) -> Result<RequestSpec, AppError> {
        let (rule, key, base) = (rule.to_string(), key.to_string(), base.to_string());
        self.run_source_task(user_ns, source, move |source| crate::parser::js::with_search_context(&key, page, || analyze_url(&rule, &key, page, &base, source))).await?
    }

    async fn parse_toc_page(&self, user_ns: &str, source: &BookSource, res: &FetchResponse, book_url: &str) -> Result<(Vec<BookChapter>, Vec<String>), AppError> {
        let mut book_ctx = self.js_book_context(user_ns, book_url, "").await.unwrap_or_default();
        book_ctx.book["tocUrl"] = json!(book_url);
        let source_key = book_ctx.book["bookUrl"].as_str().unwrap_or(book_url).to_string();
        let (body, url) = (res.body.clone(), res.url.clone());
        self.run_source_task(user_ns, source, move |source| {
            with_book_context(Some(book_ctx), || {
                RuleEngine::new().unwrap().chapter_list(source, &body, &url, Some(&source_key))
            })
        }).await
    }

    async fn parse_info_page(&self, user_ns: &str, source: &BookSource, res: &FetchResponse, book_url: &str) -> Result<Book, AppError> {
        // 详情响应为空时给出可诊断的提示: 规则里的 `result` 会是空串,
        // 表现为书源脚本自己抛出的公式化异常(如"详情bookUrl为空:"),
        // 掩盖了真正原因(线路失效/站点返回空页/被限流)。
        if res.body.trim().is_empty() {
            return Err(AppError::BadRequest(format!(
                "详情页响应为空（HTTP {}，{}）。请检查书源线路是否可用或稍后重试",
                res.status,
                if res.url.is_empty() { book_url.to_string() } else { res.url.clone() }
            )));
        }
        let book_ctx = self.js_book_context(user_ns, book_url, "").await.unwrap_or_default();
        let read_config = book_ctx.book.get("readConfig").cloned()
            .and_then(|value| serde_json::from_value(value).ok());
        // 书架里已有的书名：详情规则取不到名字时用它兜底（见下方说明）。
        let shelf_name = book_ctx.book.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let (body, url, book_url) = (res.body.clone(), res.url.clone(), book_url.to_string());
        self.run_source_task(user_ns, source, move |source| {
            take_js_logs();
            let mut book = with_book_context(Some(book_ctx), || {
                RuleEngine::new().unwrap().book_info(source, &body, &url, &book_url)
            });
            book.read_config = read_config;
            apply_book_read_config_override(&mut book);
            let logs = take_js_logs();

            // 书名取不到时的兜底与报错策略。
            //
            // legado 的 `BookInfo.analyzeBookInfo`（BookInfo.kt:68-73）在详情规则
            // 拿不到名字时**保留书籍原有名字**，不把整次解析判为失败：
            //   `if (it.isNotEmpty() && (mCanReName || book.name.isEmpty())) book.name = it`
            //
            // 书源的详情规则常在开头做登录态假设，例如该源 `ruleBookInfo.name` 的
            // 第一行 `result.match(/Ark.CSRF_TOKEN = '([^']+)'/)[1]` —— 未登录时
            // 抛 `cannot read property of null`，脚本中断、名字为空。此时若直接报
            // 「详情解析失败」，用户就会「打开详细页就报错」；而目录、正文其实都能
            // 正常取到（`tocUrl` 是独立字段）。
            //
            // 所以：能从书架拿到名字就用它，否则退回 URL 尾段；真的什么都拿不到
            //（且脚本报错）才提示失败。
            if book.name.trim().is_empty() {
                if !shelf_name.trim().is_empty() {
                    book.name = shelf_name;
                } else {
                    // 退回 URL 的**路径末段**（先剥掉 query/fragment 再取最后一段）。
                    // 顺序反了会拿到 `?dcs=…` 这种查询串、剥完变空串，兜底就失效了。
                    let without_query = book_url
                        .split(['?', '#'])
                        .next()
                        .unwrap_or("")
                        .trim_end_matches('/');
                    let fallback = without_query.rsplit('/').next().unwrap_or("").to_string();
                    if !fallback.is_empty() {
                        book.name = fallback;
                    } else if let Some(error) = first_js_error(&logs) {
                        return Err(AppError::BadRequest(format!("书籍详情解析失败: {error}")));
                    }
                }
            }
            Ok(book)
        }).await?
    }

    async fn check_source_response(&self, user_ns: &str, source: &BookSource, res: FetchResponse) -> Result<FetchResponse, AppError> {
        if source.login_check_js.as_deref().unwrap_or("").trim().is_empty() { return Ok(res); }
        let retry_url = res.url.clone();
        let (checked, login_requested) = self.run_source_task(user_ns, source, move |source| apply_login_check_js(source, res)).await??;
        // `source.login()`: 校验脚本已请求重新登录(典型: 响应 401/40101)。
        // 用已保存的登录信息重跑 loginUrl, 再用同一个 URL 重取一次。
        if !login_requested { return Ok(checked); }
        if let Err(err) = self.run_source_login(source, Some("login".into()), None).await {
            tracing::warn!("source.login() 重新登录失败: {err:?}");
            return Ok(checked);
        }
        let spec = match crate::crawler::url_analyzer::analyze_url(&retry_url, "", 1, &retry_url, source) {
            Ok(spec) => spec,
            Err(err) => {
                tracing::warn!("source.login() 重取请求构造失败: {err:?}");
                return Ok(checked);
            }
        };
        match self.fetch_with_rate(user_ns, source, spec).await {
            Ok(retried) => Ok(retried),
            Err(err) => {
                tracing::warn!("source.login() 重取响应失败: {err:?}");
                Ok(checked)
            }
        }
    }

    pub async fn create_source_login_session(&self, source_url: &str) -> String {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let source_url = normalize_source_url(source_url);
        let mut sessions = self.login_sessions.write().await;
        sessions.retain(|_, session| {
            session.source_url != source_url
                && session.created_at.elapsed() <= Duration::from_secs(30 * 60)
        });
        sessions.insert(
            token.clone(),
            SourceLoginSession {
                source_url,
                created_at: Instant::now(),
            },
        );
        token
    }

    pub async fn source_for_login_session(&self, token: &str) -> Option<String> {
        let mut sessions = self.login_sessions.write().await;
        sessions.retain(|_, session| session.created_at.elapsed() <= Duration::from_secs(30 * 60));
        sessions.get(token).map(|session| session.source_url.clone())
    }

    pub async fn configure_network_proxy(
        &self,
        mode: ProxyMode,
        proxy: Option<&str>,
    ) -> anyhow::Result<ProxyStatus> {
        let status = self.http.configure_proxy(mode, proxy)?;
        // A proxy switch invalidates any in-flight login preview tokens. This
        // prevents a session created under one network path from being reused
        // after the request routing has changed.
        self.login_sessions.write().await.clear();
        Ok(status)
    }

    pub async fn set_source_cookie(&self, user_ns: &str, source_url: &str, cookie: &str) -> Result<(), AppError> {
        let context = self.http.source_context(user_ns, source_url, None).map_err(AppError::Internal)?;
        let url = url::Url::parse(source_url).map_err(|_| AppError::BadRequest("书源地址无效".to_string()))?;
        context.session.cookies.replace(&url, cookie).map_err(AppError::Internal)?;
        Ok(())
    }

    pub async fn clear_source_cookie(&self, user_ns: &str, source_url: &str) {
        if let Ok(context) = self.http.source_context(user_ns, source_url, None) {
            let _ = context.session.cookies.clear();
            let _ = context.session.put("loginHeader", None);
        }
        let source_url = normalize_source_url(source_url);
        self.login_sessions.write().await.retain(|_, session| session.source_url != source_url);
    }

    pub async fn clear_source_cookies(&self, user_ns: &str, source_urls: &[String]) {
        for url in source_urls { self.clear_source_cookie(user_ns, url).await; }
    }

    /// Validate in an isolated session. A search without matches cannot tell
    /// whether the candidate cookie is valid; only an explicit login check can.
    pub async fn validate_source_cookie(
        &self, user_ns: &str, source: &BookSource, cookie: &str, keyword: &str,
    ) -> Result<bool, AppError> {
        let candidate_http = HttpClient::new(30, self.http.active_proxy()).map_err(AppError::Internal)?;
        let context = candidate_http.source_context(&format!("cookie-check-{}", uuid::Uuid::new_v4()), &source.book_source_url, None).map_err(AppError::Internal)?;
        let url = url::Url::parse(&source.book_source_url).map_err(|_| AppError::BadRequest("书源地址无效".into()))?;
        context.session.cookies.set(&url, cookie).map_err(AppError::Internal)?;
        let current = self.http.source_context(user_ns, &source.book_source_url, None).map_err(AppError::Internal)?;
        for key in ["variable", "loginInfo"] {
            if let Some(value) = current.session.get(key) { context.session.put(key, Some(&value)).map_err(AppError::Internal)?; }
        }
        let Some(rule) = source.search_url.as_ref().filter(|v| !v.trim().is_empty()).cloned() else { return Ok(false) };
        let source_copy = source.clone();
        let keyword = keyword.to_string();
        let analyze_context = context.clone();
        let mut spec = tokio::task::spawn_blocking(move || crate::parser::js::with_http_context(analyze_context, || {
            analyze_url(&rule, &keyword, 1, &source_copy.book_source_url, &source_copy)
        })).await.map_err(|err| AppError::Internal(err.into()))??;
        spec.headers.retain(|(name, _)| !name.eq_ignore_ascii_case("cookie"));
        spec.headers.push(("Cookie".into(), cookie.trim().to_string()));
        let client = if let Some(proxy) = spec.proxy.as_deref() {
            crate::crawler::http_client::build_source_client(context.timeout_secs, Some(proxy), context.session.cookies.clone()).map_err(AppError::Internal)?
        } else { context.client.clone() };
        let holds_slot = self.wait_for_rate(source).await;
        let result = fetch_with_client(&client, spec).await;
        if holds_slot { self.finish_rate(source).await; }
        let res = result.map_err(AppError::Internal)?;
        if !res.is_successful || res.status == 202 || res.body.contains("var buid") {
            return Err(AppError::BadRequest(format!("站点未接受验证请求（HTTP {}），请检查网页登录状态", res.status)));
        }
        if source.login_check_js.as_deref().unwrap_or("").trim().is_empty() { return Ok(false); }
        let source_copy = source.clone();
        tokio::task::spawn_blocking(move || crate::parser::js::with_http_context(context, || apply_login_check_js(&source_copy, res)))
            .await.map_err(|err| AppError::Internal(err.into()))??;
        Ok(true)
    }

    async fn fetch_source_url(
        &self,
        user_ns: &str,
        source: &BookSource,
        url_rule: &str,
        base_url: &str,
        key: &str,
    ) -> Result<FetchResponse, AppError> {
        let spec = self.analyze_request(user_ns, source, url_rule, key, 1, base_url).await?;
        let res = self.fetch_with_rate(user_ns, source, spec).await?;
        self.check_source_response(user_ns, source, res).await
    }

    async fn fetch_with_rate(
        &self,
        user_ns: &str,
        source: &BookSource,
        spec: RequestSpec,
    ) -> anyhow::Result<FetchResponse> {
        self.fetch_with_rate_cancellable(user_ns, source, spec, None)
            .await
    }

    /// 与 `fetch_with_rate` 相同, 但可被取消信号中断在途请求。
    ///
    /// 关键点: 用 `select!` 包住 `fetch_with_client` 的 future, 取消时该 future 被
    /// **drop** —— reqwest 的连接随之关闭, 这才是"真断", 而不是等它跑到 30s 超时。
    /// 取消返回 `Err(CANCELLED_ERR)`, 调用方据此把结果标为"已取消"而不是"失效",
    /// 否则把用户主动中止当成站点不可用, 会把好源误标失效。
    async fn fetch_with_rate_cancellable(
        &self,
        user_ns: &str,
        source: &BookSource,
        spec: RequestSpec,
        cancel: Option<&CancelSignal>,
    ) -> anyhow::Result<FetchResponse> {
        if is_cancelled(cancel) {
            return Err(anyhow::anyhow!(CANCELLED_ERR));
        }
        // 限速等待也必须可取消: 配了 `concurrentRate: 2000` 这类较大间隔时,
        // 傻等限速窗口会让"中止"延迟数秒才生效, 与 100ms 响应上限的意图不符。
        //
        // `wait_for_rate` 返回**是否占用了 rate slot**, 据此精确决定要不要归还 ——
        // 只对"自己占到的 slot"调 finish_rate, 避免把别的在途请求的 slot 误释放
        // (那会让限速形同虚设)。
        let holds_slot;
        if let Some(flag) = cancel {
            let waited = self.wait_for_rate(source);
            tokio::pin!(waited);
            loop {
                tokio::select! {
                    acquired = &mut waited => {
                        holds_slot = acquired;
                        break;
                    }
                    _ = sleep(Duration::from_millis(100)) => {
                        if flag.load(std::sync::atomic::Ordering::Relaxed) {
                            // 提前 drop: 未占用时不归还(别人的 slot 不能动)。
                            return Err(anyhow::anyhow!(CANCELLED_ERR));
                        }
                    }
                }
            }
        } else {
            holds_slot = self.wait_for_rate(source).await;
        }
        if is_cancelled(cancel) {
            if holds_slot {
                self.finish_rate(source).await;
            }
            return Err(anyhow::anyhow!(CANCELLED_ERR));
        }
        let request_context = match self.http
            .source_context(user_ns, &source.book_source_url, spec.proxy.as_deref())
        {
            Ok(context) => context,
            Err(err) => {
                // 必须归还 in_flight: 用 `?` 直接返回会让这个源永久停在
                // in_flight, 之后所有请求都在 wait_for_serial_rate 里死等。
                if holds_slot {
                    self.finish_rate(source).await;
                }
                return Err(anyhow::anyhow!(err.to_string()));
            }
        };

        let result = match cancel {
            Some(flag) => {
                // 100ms 轮询而不是"监听一次": 取消标志是 AtomicBool, 没有可等待的
                // 唤醒机制, 用间隔轮询即可 —— 取消响应延迟上限就是这 100ms。
                let fetch = request_context.session.request(&request_context.client, spec, source.concurrent_rate.as_deref());
                tokio::pin!(fetch);
                loop {
                    tokio::select! {
                        result = &mut fetch => break result,
                        _ = sleep(Duration::from_millis(100)) => {
                            if flag.load(std::sync::atomic::Ordering::Relaxed) {
                                // fetch 在此处被 drop → 连接关闭。
                                if holds_slot {
                                    self.finish_rate(source).await;
                                }
                                return Err(anyhow::anyhow!(CANCELLED_ERR));
                            }
                        }
                    }
                }
            }
            None => request_context.session.request(&request_context.client, spec, source.concurrent_rate.as_deref()).await,
        };
        if holds_slot {
            self.finish_rate(source).await;
        }
        result
    }

    /// 等待限速窗口并**占用 rate slot**(返回时 `in_flight == true` 表示已占)。
    ///
    /// 返回 `true` 表示本次调用占用了 slot, 调用方结束请求时必须 `finish_rate` 归还;
    /// 返回 `false` 表示未占用(无需归还)。
    async fn wait_for_rate(&self, source: &BookSource) -> bool {
        let Some(rate) = source.concurrent_rate.as_deref().map(str::trim) else {
            return false;
        };
        if rate.is_empty() || rate == "0" {
            return false;
        }
        if let Some((limit, window_ms)) = parse_window_rate(rate) {
            return self
                .wait_for_window_rate(&source.book_source_url, limit, window_ms)
                .await;
        }
        let Ok(delay_ms) = rate.parse::<u64>() else {
            return false;
        };
        self.wait_for_serial_rate(&source.book_source_url, delay_ms)
            .await
    }

    async fn wait_for_serial_rate(&self, source_key: &str, delay_ms: u64) -> bool {
        let delay = Duration::from_millis(delay_ms);
        loop {
            let wait = {
                let mut states = self.rate_states.write().await;
                let state = states.entry(source_key.to_string()).or_default();
                let now = Instant::now();
                if state.in_flight {
                    delay
                } else if let Some(last_start) = state.last_start {
                    let elapsed = now.saturating_duration_since(last_start);
                    if elapsed < delay {
                        delay - elapsed
                    } else {
                        state.in_flight = true;
                        state.last_start = Some(now);
                        return true;
                    }
                } else {
                    state.in_flight = true;
                    state.last_start = Some(now);
                    return true;
                }
            };
            sleep(wait).await;
        }
    }

    async fn wait_for_window_rate(&self, source_key: &str, limit: usize, window_ms: u64) -> bool {
        if limit == 0 || window_ms == 0 {
            return false;
        }
        let window = Duration::from_millis(window_ms);
        loop {
            let wait = {
                let mut states = self.rate_states.write().await;
                let state = states.entry(source_key.to_string()).or_default();
                let now = Instant::now();
                state
                    .window_starts
                    .retain(|start| now.saturating_duration_since(*start) <= window);
                if state.window_starts.len() >= limit {
                    state
                        .window_starts
                        .first()
                        .map(|start| window.saturating_sub(now.saturating_duration_since(*start)))
                        .unwrap_or(window)
                } else {
                    state.window_starts.push(now);
                    return true;
                }
            };
            sleep(wait).await;
        }
    }

    async fn finish_rate(&self, source: &BookSource) {
        let mut states = self.rate_states.write().await;
        if let Some(state) = states.get_mut(&source.book_source_url) {
            state.in_flight = false;
        }
    }

    pub async fn search_book(
        &self,
        user_ns: &str,
        source: &BookSource,
        key: &str,
        page: i32,
    ) -> Result<Vec<SearchBook>, AppError> {
        self.search_book_cancellable(user_ns, source, key, page, None)
            .await
    }

    /// 可取消的搜索。解析段放到阻塞线程池, 避免 `js:` 规则里的同步 HTTP 独占
    /// tokio worker(那会让取消命令自己排不上队, 表现为"点了没反应")。
    pub async fn search_book_cancellable(
        &self,
        user_ns: &str,
        source: &BookSource,
        key: &str,
        page: i32,
        cancel: Option<&CancelSignal>,
    ) -> Result<Vec<SearchBook>, AppError> {
        if is_cancelled(cancel) {
            return Err(AppError::BadRequest(CANCELLED_ERR.to_string()));
        }
        let search_url = source
            .search_url
            .clone()
            .ok_or_else(|| AppError::BadRequest("missing search_url".to_string()))?;
        tracing::info!(
            "searching book from {}: key={}, page={}, url={}",
            source.book_source_name,
            key,
            page,
            search_url
        );
        let spec = self.analyze_request(user_ns, source, &search_url, key, page, &source.book_source_url).await
            .map_err(|e| {
                tracing::error!("analyze_url failed: {:?}", e);
                e
            })?;


        tracing::debug!("search request prepared");
        let res = self
            .fetch_with_rate_cancellable(user_ns, source, spec, cancel)
            .await
            .map_err(|e| {
                tracing::error!("fetch failed: {:?}", e);
                e
            })?;
        let res = self.check_source_response(user_ns, source, res).await?;
        tracing::debug!("fetch success, body length: {}", res.body.len());
        let books = self
            .parse_search_books_blocking(user_ns, source, &res.body, &res.url, key, page)
            .await?;
        if books.is_empty() && !res.is_successful {
            return Err(AppError::BadRequest(format!(
                "书源搜索请求失败（HTTP {}），请检查站点访问状态或网络设置", res.status
            )));
        }
        tracing::info!("found {} books", books.len());
        Ok(books)
    }

    /// 搜索解析放到阻塞线程池: 规则里可能带 `js:` 且 JS 内会发起同步 HTTP
    /// (reqwest::blocking, 30s 超时), 留在 async worker 上会把它整个占住。
    ///
    /// **解析器 panic 必须继续向上传播**, 不能吞成空 `Vec`: 空结果会被上层当成
    /// "这个源搜不到书" → `search_ok=false` → `valid=false` → 书源被标为失效并写库,
    /// 把一个完全正常的源标坏。原实现里 `parser.search_books` 是同步调用, panic 会
    /// 直接终止该检测任务(不产生 outcome、不写库), 这里用 `resume_unwind` 保持同一
    /// 行为 —— `spawn_blocking` 只是把 panic 包成了 `JoinError`, 并非真的"没有结果"。
    async fn parse_search_books_blocking(&self, user_ns: &str, source: &BookSource, body: &str, url: &str, key: &str, page: i32) -> Result<Vec<SearchBook>, AppError> {
        self.parse_book_list_blocking(user_ns, source, body, url, false, key.to_string(), page).await
    }

    async fn parse_explore_books_blocking(&self, user_ns: &str, source: &BookSource, body: &str, url: &str, page: i32) -> Result<Vec<SearchBook>, AppError> {
        self.parse_book_list_blocking(user_ns, source, body, url, true, String::new(), page).await
    }

    async fn parse_book_list_blocking(&self, user_ns: &str, source: &BookSource, body: &str, url: &str, explore: bool, key: String, page: i32) -> Result<Vec<SearchBook>, AppError> {
        let context = self.http.source_context(user_ns, &source.book_source_url, None).map_err(AppError::Internal)?;
        let parser = self.parser.clone();
        let (source, body, url) = (source.clone(), body.to_string(), url.to_string());
        match tokio::task::spawn_blocking(move || crate::parser::js::with_http_context(context, || {
            take_js_logs();
            let books = crate::parser::js::with_search_context(&key, page, || if explore { parser.explore_books(&source, &body, &url) } else { parser.search_books(&source, &body, &url) });
            (books, take_js_logs())
        })).await {
            Ok((books, logs)) => {
                if books.is_empty() {
                    if let Some(error) = first_js_error(&logs) { return Err(AppError::BadRequest(format!("书源解析失败: {error}"))); }
                }
                Ok(books)
            }
            Err(err) if err.is_cancelled() => Ok(Vec::new()),
            Err(err) => std::panic::resume_unwind(err.into_panic()),
        }
    }

    pub async fn explore_book(
        &self,
        user_ns: &str,
        source: &BookSource,
        rule_find_url: &str,
        page: i32,
    ) -> Result<Vec<SearchBook>, AppError> {
        self.explore_book_cancellable(user_ns, source, rule_find_url, page, None)
            .await
    }

    /// 可取消的发现页搜索(同 `search_book_cancellable`)。
    pub async fn explore_book_cancellable(
        &self,
        user_ns: &str,
        source: &BookSource,
        rule_find_url: &str,
        page: i32,
        cancel: Option<&CancelSignal>,
    ) -> Result<Vec<SearchBook>, AppError> {
        if is_cancelled(cancel) {
            return Err(AppError::BadRequest(CANCELLED_ERR.to_string()));
        }
        if rule_find_url.trim().is_empty() {
            return Err(AppError::BadRequest("ruleFindUrl required".to_string()));
        }
        let spec = self.analyze_request(user_ns, source, rule_find_url, "", page, &source.book_source_url).await?;


        let res = self.fetch_with_rate_cancellable(user_ns, source, spec, cancel).await?;
        let res = self.check_source_response(user_ns, source, res).await?;
        self.parse_explore_books_blocking(user_ns, source, &res.body, &res.url, page).await
    }

    /// 求值书源的发现页分类（exploreUrl）。
    ///
    /// `info_map` 是发现页控件的当前取值：JS 型 exploreUrl 会读它来决定生成哪些
    /// 分类（如"发现页来源=源甲"），也会写回默认值。缺省用空 map。
    pub fn explore_kinds_with_info(
        &self,
        source: &BookSource,
        info_map: &HashMap<String, String>,
    ) -> Result<Vec<ExploreKind>, AppError> {
        self.source_scope("default", source, || {
            // 与 explore_action 一样先清空线程局部的副作用槽位: exploreUrl 的
            // JS 里也可能出现 java.refreshExplore() / java.open(...)，不清理会
            // 让它们残留到下一次调用（例如被无辜的 action 当成自己的意图）。
            crate::parser::js::take_login_effects();
            crate::parser::js::take_explore_intents();
            parse_explore_kinds(source, info_map)
        })?
    }

    pub fn explore_kinds(&self, source: &BookSource) -> Result<Vec<ExploreKind>, AppError> {
        self.explore_kinds_with_info(source, &HashMap::new())
    }

    /// 求值发现页控件的 `action`（按钮/下拉/输入框回调）。
    ///
    /// 书源脚本里的副作用无法在 WebView 侧产生，统一随返回值交给调用方：
    /// - `java.refreshExplore()` / `java.open` / `java.searchBook` → 意图标志
    /// - `java.startBrowser` / `java.showBrowser` → `show_browser` 面板请求
    /// - `java.toast` / `java.longToast` → `messages` 提示文案
    ///
    /// `show_browser` 必须在这里登记成可渲染的面板：某书源的「更新书源」「书源设置」
    /// 都靠它弹出 HTML，早先只回传意图而丢掉面板，按钮点了毫无反应。
    pub fn explore_action(
        &self,
        source: &BookSource,
        action: &str,
        info_map: &HashMap<String, String>,
    ) -> Result<ExploreActionOutcome, AppError> {
        let action = action.to_string();
        let source_key = source.book_source_url.clone();
        let info_map = info_map.clone();
        let source_for_panel = source.clone();
        self.source_scope("default", source, || {
            take_js_logs();
            crate::parser::js::take_login_effects();
            crate::parser::js::take_explore_intents();
            // 面板是"本次动作"的产物：先清掉上一轮残留，避免把旧面板当成本次结果。
            clear_show_browser_request();
            let value = crate::parser::js::eval_explore_action(&action, &source_key, &info_map);
            let effects = crate::parser::js::take_login_effects();
            let intents = crate::parser::js::take_explore_intents();
            let browser = take_show_browser_request();
            let logs = take_js_logs();

            let messages = logs
                .iter()
                .filter_map(|line| line.strip_prefix("[toast] "))
                .map(str::to_string)
                .collect::<Vec<_>>();

            let show_browser = match browser {
                Some(request) => {
                    let entry = crate::service::source_browser::source_panel_entry(
                        &source_for_panel.book_source_url,
                    )?;
                    Some(crate::service::source_browser::register_browser_request(
                        request, entry,
                    )?)
                }
                None => None,
            };

            match value {
                Ok(value) => Ok(ExploreActionOutcome {
                    value,
                    explore_refresh: effects.explore_refresh,
                    login_refresh: effects.refresh,
                    login: effects.login,
                    open_urls: effects
                        .values
                        .get("__open")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    intents: intents
                        .into_iter()
                        .map(|(kind, payload)| {
                            json!({
                                "kind": kind,
                                "payload": serde_json::from_str::<serde_json::Value>(&payload)
                                    .unwrap_or(serde_json::Value::String(payload)),
                            })
                        })
                        .collect(),
                    show_browser,
                    values: effects.values,
                    messages,
                    logs,
                }),
                Err(err) => Err(AppError::BadRequest(format!("书源动作执行失败: {err}"))),
            }
        })?
    }

    pub async fn test_book_source_availability(
        &self,
        user_ns: &str,
        source: &BookSource,
        keyword: Option<&str>,
    ) -> BookSourceAvailability {
        self.test_book_source_availability_cancellable(user_ns, source, keyword, None)
            .await
    }

    /// 可取消的书源可用性检测。
    ///
    /// 与不可取消版本的区别: 单次请求可被中断(连接真断), 且取消时返回的可用性对象
    /// 带 `cancelled = true`。调用方必须据此**跳过写库** —— 否则把"用户中止"当成
    /// "站点不可用", 会把好源误标成失效。
    pub async fn test_book_source_availability_cancellable(
        &self,
        user_ns: &str,
        source: &BookSource,
        keyword: Option<&str>,
        cancel: Option<&CancelSignal>,
    ) -> BookSourceAvailability {
        let keyword = keyword
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            // 不给关键词时用源自己的 checkKeyWord, 否则回退默认值(见 check_keyword
            // 的注释: 默认值是"我的"而不是"测试", 避免把好源误判失效)。
            .unwrap_or_else(|| source.check_keyword());

        // 取消时立即返回一个"已取消"的占位结果: 不写库, 也不计入有效/失效统计。
        if is_cancelled(cancel) {
            return cancelled_availability(source, &keyword);
        }

        let (search_ok, search_error) = if source
            .search_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            && source.rule_search.is_some()
        {
            match self
                .search_book_cancellable(user_ns, source, &keyword, 1, cancel)
                .await
            {
                Ok(books) => (!books.is_empty(), None),
                Err(err) => (false, Some(format!("{err:?}"))),
            }
        } else {
            (false, Some("missing searchUrl or ruleSearch".to_string()))
        };
        // 搜索阶段被取消: 不再发发现页请求, 直接返回取消结果。
        if is_cancelled(cancel) {
            return cancelled_availability(source, &keyword);
        }

        let explore_url = self.explore_kinds(source).ok().and_then(|kinds| {
            kinds
                .into_iter()
                .filter_map(|kind| kind.url)
                .map(|url| url.trim().to_string())
                .find(|url| !url.is_empty())
        });
        let (explore_ok, explore_error) = if let Some(url) = explore_url.as_deref() {
            match self
                .explore_book_cancellable(user_ns, source, url, 1, cancel)
                .await
            {
                Ok(books) => (!books.is_empty(), None),
                Err(err) => (false, Some(format!("{err:?}"))),
            }
        } else {
            (false, Some("missing explore category url".to_string()))
        };
        if is_cancelled(cancel) {
            return cancelled_availability(source, &keyword);
        }

        BookSourceAvailability {
            book_source_url: source.book_source_url.clone(),
            book_source_name: source.book_source_name.clone(),
            valid: search_ok || explore_ok,
            search_ok,
            explore_ok,
            keyword,
            explore_url,
            search_error,
            explore_error,
            cancelled: false,
        }
    }

    /// Run a single book-source parsing step for the source debugger.
    ///
    /// Unlike the normal crawl path this always fetches fresh (no cache) and
    /// returns the request URL, the raw response body and the parsed result so
    /// the UI can show what the source actually produced.
    pub async fn debug_source_step(
        &self,
        user_ns: &str,
        source: &BookSource,
        step: &str,
        keyword: &str,
        book_url: &str,
        chapter_url: &str,
    ) -> Result<DebugTrace, AppError> {
        match step {
            "search" => {
                let search_url = source
                    .search_url
                    .clone()
                    .ok_or_else(|| AppError::BadRequest("missing searchUrl".to_string()))?;
                let kw = if keyword.trim().is_empty() {
                    source
                        .rule_search
                        .as_ref()
                        .and_then(|r| r.check_key_word.clone())
                        .unwrap_or_default()
                } else {
                    keyword.to_string()
                };
                if kw.trim().is_empty() {
                    return Err(AppError::BadRequest("请输入搜索关键词".to_string()));
                }
                let res = self
                    .fetch_source_url(user_ns, source, &search_url, &source.book_source_url, &kw)
                    .await?;
                let books = self.parse_search_books_blocking(user_ns, source, &res.body, &res.url, &kw, 1).await?;
                Ok(debug_trace_from(
                    &res,
                    serde_json::to_value(books).unwrap_or_default(),
                ))
            }
            "bookInfo" => {
                let url = if book_url.trim().is_empty() {
                    source
                        .rule_book_info
                        .as_ref()
                        .and_then(|r| r.toc_url.clone())
                        .unwrap_or_default()
                } else {
                    book_url.to_string()
                };
                if url.trim().is_empty() {
                    return Err(AppError::BadRequest("请输入书籍链接".to_string()));
                }
                let res = self
                    .fetch_source_url(user_ns, source, &url, &source.book_source_url, "")
                    .await?;
                let info = self.parse_info_page(user_ns, source, &res, &url).await?;
                Ok(debug_trace_from(
                    &res,
                    serde_json::to_value(info).unwrap_or_default(),
                ))
            }
            "toc" => {
                let toc_url = book_url.trim();
                if toc_url.is_empty() {
                    return Err(AppError::BadRequest("请输入目录链接".to_string()));
                }
                let res = self
                    .fetch_source_url(user_ns, source, toc_url, &source.book_source_url, "")
                    .await?;
                let (chapters, _next_urls) =
                    self.parse_toc_page(user_ns, source, &res, toc_url).await?;
                let mut index = 0i32;
                let chapters: Vec<BookChapter> = chapters
                    .into_iter()
                    .map(|mut ch| {
                        ch.index = index;
                        index += 1;
                        ch
                    })
                    .collect();
                Ok(debug_trace_from(
                    &res,
                    serde_json::to_value(chapters).unwrap_or_default(),
                ))
            }
            "content" => {
                let url = chapter_url.trim();
                if url.is_empty() {
                    return Err(AppError::BadRequest("请输入章节链接".to_string()));
                }
                let res = self
                    .fetch_source_url(user_ns, source, url, &source.book_source_url, "")
                    .await?;
                let (content, js_logs) =
                    self.parse_content_blocking(user_ns, source, &res, url, url, None).await?;
                let next = self.source_scope(user_ns, source, || self.parser.next_content_url(source, &res.body, &res.url))?;
                Ok(debug_trace_from(
                    &res,
                    serde_json::json!({
                        "content": content,
                        "nextContentUrl": next,
                        "jsLogs": js_logs,
                    }),
                ))
            }
            other => Err(AppError::BadRequest(format!("未知调试步骤: {other}"))),
        }
    }

    pub async fn login_book_source(&self, source: &BookSource) -> Result<serde_json::Value, AppError> {
        self.run_source_login(source, None, None).await
    }

    pub async fn run_source_login(&self, source: &BookSource, action: Option<String>, values: Option<serde_json::Map<String, serde_json::Value>>) -> Result<serde_json::Value, AppError> {
        self.run_source_login_with_browser(source, action, values, None).await
    }

    pub async fn run_source_login_with_browser(&self, source: &BookSource, action: Option<String>, values: Option<serde_json::Map<String, serde_json::Value>>, handler: Option<crate::service::source_browser::BrowserHandler>) -> Result<serde_json::Value, AppError> {
        self.run_source_task("default", source, move |source| {
            crate::service::source_browser::with_browser_handler(source, handler, || {
                crate::service::source_login::run(source, action.as_deref(), values)
            })?
        }).await?.map_err(AppError::Internal)
    }

    pub async fn get_book_info(
        &self,
        user_ns: &str,
        source: &BookSource,
        book_url: &str,
    ) -> Result<Book, AppError> {
        let res = self
            .fetch_source_url(user_ns, source, book_url, &source.book_source_url, "")
            .await?;
        self.parse_info_page(user_ns, source, &res, book_url).await
    }

    pub async fn get_chapter_list(
        &self,
        user_ns: &str,
        source: &BookSource,
        toc_url: &str,
    ) -> Result<Vec<BookChapter>, AppError> {
        self.get_chapter_list_with_cache(user_ns, source, toc_url, false)
            .await
    }

    pub async fn get_chapter_list_with_cache(
        &self,
        user_ns: &str,
        source: &BookSource,
        toc_url: &str,
        force_refresh: bool,
    ) -> Result<Vec<BookChapter>, AppError> {
        // Check cache first (unless force refresh)
        if !force_refresh {
            if let Ok(Some(cached)) = self.load_chapter_list_cache(user_ns, toc_url).await {
                if !cached.is_empty() {
                    return Ok(cached);
                }
            }
        }
        let (chapters, _) = self
            .get_chapter_list_with_pagination(user_ns, source, toc_url)
            .await?;
        // Save to cache
        let _ = self
            .save_chapter_list_cache(user_ns, toc_url, &chapters)
            .await;
        Ok(chapters)
    }

    /// Get first page of chapters and pagination info for background fetching
    pub async fn get_chapter_list_first_page(
        &self,
        user_ns: &str,
        source: &BookSource,
        toc_url: &str,
    ) -> Result<(Vec<BookChapter>, ChapterPagination), AppError> {
        let res = self
            .fetch_source_url(user_ns, source, toc_url, &source.book_source_url, "")
            .await?;
        let (chapters, next_urls) =
            self.parse_toc_page(user_ns, source, &res, toc_url).await?;
        tracing::info!(
            "get_chapter_list_first_page: toc_url={} final_url={} body_len={} chapters={} next_urls={}",
            toc_url,
            res.url,
            res.body.len(),
            chapters.len(),
            next_urls.len()
        );

        let mut chapter_index = 0i32;
        let mut result = Vec::new();
        for mut ch in chapters {
            ch.index = chapter_index;
            chapter_index += 1;
            result.push(ch);
        }

        // The actual URL we fetched (after redirects) should be considered visited
        let actual_visited_url = res.url.clone();

        // Get chapter URLs from first page for deduplication
        let first_page_chapter_urls: std::collections::HashSet<String> =
            result.iter().map(|c| c.url.clone()).collect();

        // Filter out already visited URLs and the current page URL from pending_urls
        // Also filter out URLs that point to the same page (same path but different domain)
        let pending_urls: Vec<String> = next_urls
            .into_iter()
            .filter(|u| {
                // Filter out exact matches
                if u == &actual_visited_url || u == toc_url {
                    return false;
                }
                // Filter out URLs with the same path but different domain
                // This handles cases like m.22biqu.com vs m.22biqu.net
                if let (Ok(parsed_u), Ok(parsed_visited)) =
                    (url::Url::parse(u), url::Url::parse(&actual_visited_url))
                {
                    if parsed_u.path() == parsed_visited.path() {
                        return false;
                    }
                }
                true
            })
            .collect();

        let pagination = ChapterPagination {
            user_ns: user_ns.to_string(),
            source: source.clone(),
            toc_url: toc_url.to_string(),
            visited_urls: vec![toc_url.to_string(), actual_visited_url],
            pending_urls,
            seen_chapter_urls: first_page_chapter_urls.iter().cloned().collect(),
            next_index: chapter_index,
        };

        Ok((result, pagination))
    }

    /// Continue fetching remaining chapters from pagination state
    pub async fn fetch_remaining_chapters(
        &self,
        pagination: ChapterPagination,
    ) -> Result<Vec<BookChapter>, AppError> {
        let mut all_chapters = Vec::new();
        let mut visited_page_urls: std::collections::HashSet<String> =
            pagination.visited_urls.iter().cloned().collect();
        let mut seen_chapter_urls: std::collections::HashSet<String> =
            pagination.seen_chapter_urls.iter().cloned().collect();
        let mut chapter_index = pagination.next_index;

        let pending_urls: Vec<String> = pagination
            .pending_urls
            .into_iter()
            .filter(|u| !visited_page_urls.contains(u))
            .collect();

        if pending_urls.len() > 1 {
            // Multiple URLs from option dropdown - fetch all pages
            for url in pending_urls {
                if visited_page_urls.contains(&url) {
                    continue;
                }
                visited_page_urls.insert(url.clone());

                let res = self
                    .fetch_source_url(
                        &pagination.user_ns,
                        &pagination.source,
                        &url,
                        &pagination.source.book_source_url,
                        "",
                    )
                    .await?;
                let (chapters, _) = self.parse_toc_page(&pagination.user_ns, &pagination.source, &res, &pagination.toc_url).await?;

                // Check if this page is a duplicate (all chapters already seen)
                // This handles cases where the first page URL differs from toc_url (e.g., different domain)
                let all_seen = chapters
                    .iter()
                    .all(|ch| seen_chapter_urls.contains(&ch.url));
                if all_seen && !chapters.is_empty() {
                    tracing::debug!("Skipping duplicate page: {}", url);
                    continue;
                }

                for ch in chapters {
                    if seen_chapter_urls.contains(&ch.url) {
                        continue;
                    }
                    seen_chapter_urls.insert(ch.url.clone());

                    all_chapters.push(BookChapter {
                        title: ch.title,
                        url: ch.url,
                        index: chapter_index,
                        ..Default::default()
                    });
                    chapter_index += 1;
                }
            }
        } else if pending_urls.len() == 1 {
            // Single next page link - follow sequentially
            let mut current_url = pending_urls[0].clone();
            loop {
                if visited_page_urls.contains(&current_url) {
                    break;
                }
                visited_page_urls.insert(current_url.clone());

                let res = self
                    .fetch_source_url(
                        &pagination.user_ns,
                        &pagination.source,
                        &current_url,
                        &pagination.source.book_source_url,
                        "",
                    )
                    .await?;
                let (chapters, next_urls) = self.parse_toc_page(&pagination.user_ns, &pagination.source, &res, &pagination.toc_url).await?;

                // Check if this page is a duplicate
                let all_seen = chapters
                    .iter()
                    .all(|ch| seen_chapter_urls.contains(&ch.url));
                if all_seen && !chapters.is_empty() {
                    tracing::debug!("Skipping duplicate page: {}", current_url);
                    break; // Stop following pagination if we hit a duplicate page
                }

                for ch in chapters {
                    if seen_chapter_urls.contains(&ch.url) {
                        continue;
                    }
                    seen_chapter_urls.insert(ch.url.clone());

                    all_chapters.push(BookChapter {
                        title: ch.title,
                        url: ch.url,
                        index: chapter_index,
                        ..Default::default()
                    });
                    chapter_index += 1;
                }

                // Get next page
                let next = next_urls
                    .into_iter()
                    .find(|u| !visited_page_urls.contains(u));
                match next {
                    Some(url) if !url.is_empty() => current_url = url,
                    _ => break,
                }
            }
        }

        Ok(all_chapters)
    }

    async fn get_chapter_list_with_pagination(
        &self,
        user_ns: &str,
        source: &BookSource,
        toc_url: &str,
    ) -> Result<(Vec<BookChapter>, Vec<String>), AppError> {
        let mut all_chapters = Vec::new();
        let mut visited_page_urls = std::collections::HashSet::new();
        let mut seen_chapter_urls = std::collections::HashSet::new();
        let mut chapter_index = 0i32;

        // Fetch first page
        let res = self
            .fetch_source_url(user_ns, source, toc_url, &source.book_source_url, "")
            .await?;
        let (chapters, next_urls) =
            self.parse_toc_page(user_ns, source, &res, toc_url).await?;

        visited_page_urls.insert(toc_url.to_string());

        // Add first page chapters with deduplication
        for ch in chapters {
            if seen_chapter_urls.contains(&ch.url) {
                continue;
            }
            seen_chapter_urls.insert(ch.url.clone());
            all_chapters.push(BookChapter {
                title: ch.title,
                url: ch.url,
                index: chapter_index,
                ..Default::default()
            });
            chapter_index += 1;
        }

        // Determine how to handle pagination
        // Filter out already visited URLs
        let pending_urls: Vec<String> = next_urls
            .into_iter()
            .filter(|u| !visited_page_urls.contains(u))
            .collect();

        if pending_urls.len() > 1 {
            // Multiple URLs from option dropdown - fetch all pages
            for url in pending_urls {
                if visited_page_urls.contains(&url) {
                    continue;
                }
                visited_page_urls.insert(url.clone());

                let res = self
                    .fetch_source_url(user_ns, source, &url, &source.book_source_url, "")
                    .await?;
                let (chapters, _) = self.parse_toc_page(user_ns, source, &res, toc_url).await?;

                for ch in chapters {
                    if seen_chapter_urls.contains(&ch.url) {
                        continue;
                    }
                    seen_chapter_urls.insert(ch.url.clone());
                    all_chapters.push(BookChapter {
                        title: ch.title,
                        url: ch.url,
                        index: chapter_index,
                        ..Default::default()
                    });
                    chapter_index += 1;
                }
            }
        } else if pending_urls.len() == 1 {
            // Single next page link - follow sequentially
            let mut current_url = pending_urls[0].clone();
            loop {
                if visited_page_urls.contains(&current_url) {
                    break;
                }
                visited_page_urls.insert(current_url.clone());

                let res = self
                    .fetch_source_url(user_ns, source, &current_url, &source.book_source_url, "")
                    .await?;
                let (chapters, next_urls) = self.parse_toc_page(user_ns, source, &res, toc_url).await?;

                for ch in chapters {
                    if seen_chapter_urls.contains(&ch.url) {
                        continue;
                    }
                    seen_chapter_urls.insert(ch.url.clone());
                    all_chapters.push(BookChapter {
                        title: ch.title,
                        url: ch.url,
                        index: chapter_index,
                        ..Default::default()
                    });
                    chapter_index += 1;
                }

                // Get next page
                let next = next_urls
                    .into_iter()
                    .find(|u| !visited_page_urls.contains(u));
                match next {
                    Some(url) if !url.is_empty() => current_url = url,
                    _ => break,
                }
            }
        }

        Ok((all_chapters, visited_page_urls.into_iter().collect()))
    }

    /// 正文 JS 的 `book` / `chapter` 上下文: 取书架书籍与缓存目录中的对应章节。
    async fn js_book_context(
        &self,
        user_ns: &str,
        book_url: &str,
        chapter_url: &str,
    ) -> Option<JsBookContext> {
        let book = self.read_bookshelf(user_ns).await.ok().and_then(|books| {
            books.into_iter().find(|book| {
                book.book_url == book_url || book.toc_url.as_deref() == Some(book_url)
            })
        });
        let toc_url = book
            .as_ref()
            .and_then(|b| b.toc_url.clone())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| book_url.to_string());
        let chapter = if chapter_url.is_empty() {
            None
        } else {
            self.load_chapter_list_cache(user_ns, &toc_url)
                .await
                .ok()
                .flatten()
                .and_then(|list| list.into_iter().find(|ch| ch.url == chapter_url))
        };
        let book_json = match book {
            Some(book) => serde_json::to_value(book).unwrap_or_else(|_| json!({ "bookUrl": book_url })),
            None => json!({ "bookUrl": book_url }),
        };
        let chapter_json = match chapter {
            Some(ch) => serde_json::to_value(ch).unwrap_or_else(|_| json!({})),
            None if chapter_url.is_empty() => json!({}),
            None => json!({ "url": chapter_url }),
        };
        Some(JsBookContext {
            book: book_json,
            chapter: chapter_json,
        })
    }

    /// 正文图片 `click` 脚本(legado `ReadBookActivity.clickImg`): 在书源 jsLib 作用域里执行,
    /// 因此插件定义的 `showCommentPanel(...)` 等函数可用。
    /// 返回 (脚本返回值, 本次产生的 JS 日志, 面板信息)。
    pub async fn eval_source_click(
        &self,
        user_ns: &str,
        book_url: &str,
        source: &BookSource,
        chapter_url: &str,
        script: &str,
        result: &str,
    ) -> Result<(String, Vec<String>, Option<ShowBrowserRequest>), AppError> {
        let context = self.http.source_context(user_ns, &source.book_source_url, None).map_err(AppError::Internal)?;
        let book_ctx = self.js_book_context(user_ns, book_url, chapter_url).await;
        // 面板登记需要原始 JSON(面板脚本 `writeConfig(chapter, ...)` 依赖这两个绑定)
        let (book_json, chapter_json) = book_ctx
            .as_ref()
            .map(|ctx| (ctx.book.to_string(), ctx.chapter.to_string()))
            .unwrap_or_else(|| ("{}".to_string(), "{}".to_string()));
        let source = source.clone();
        let script = script.to_string();
        let result_text = result.to_string();
        let source_key = book_url.to_string();
        let chapter_url = chapter_url.to_string();

        tokio::task::spawn_blocking(move || {
            take_js_logs();
            clear_show_browser_request();
            let base_url = chapter_url.clone();
            let output = crate::parser::js::with_http_context(context, || with_book_context(book_ctx, || {
                with_source(&source, || {
                    with_source_key(Some(&source_key), || {
                        crate::parser::js::eval_js(&script, &result_text, &base_url)
                    })
                })
            }));
            let logs = take_js_logs();
            // 面板 HTML 由后端托管: 前端按 panelId 加载, 面板内的桥才能同源回调宿主
            let browser = take_show_browser_request().map(|req| {
                crate::service::source_browser::register_browser_request(req, SourcePanelEntry {
                    book_source_url: source.book_source_url.clone(),
                    book_url: source_key.clone(),
                    chapter_url: base_url.clone(),
                    book_json: book_json.clone(),
                    chapter_json: chapter_json.clone(),
                    http: crate::parser::js::source_http_context(&source.book_source_url).ok(),
                    ..Default::default()
                })
            }).transpose().map_err(AppError::Internal)?;
            output
                .map(|value| (value, logs, browser))
                .map_err(|e| AppError::Internal(anyhow::anyhow!("执行 click 脚本失败: {e}")))
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("click 脚本任务失败: {e}")))?
    }

    /// 正文规则可能在 JS 里发起同步网络请求, 放到阻塞线程池执行, 不占用异步 worker。
    /// 返回 (正文, 本次解析产生的 JS 日志)。
    async fn parse_content_blocking(
        &self,
        user_ns: &str,
        source: &BookSource,
        res: &FetchResponse,
        base_url: &str,
        source_key: &str,
        book_ctx: Option<JsBookContext>,
    ) -> Result<(String, Vec<String>), AppError> {
        let context = self.http.source_context(user_ns, &source.book_source_url, None).map_err(AppError::Internal)?;
        let parser = self.parser.clone();
        let source = source.clone();
        let body = res.body.clone();
        // legado 里 `baseUrl` 是**章节的绝对 URL**(含 `#item_id` 这类片段), 请求
        // 之后的真实地址是 `redirectUrl`。把 `res.url` 当 baseUrl 会丢掉片段,
        // 纯 JS 正文规则(如用 `baseUrl.match(/#(\d+)$/)` 取章节标识)因此拿不到
        // 章节标识。这里优先用调用方给的章节 URL。
        let url = if base_url.trim().is_empty() { res.url.clone() } else { base_url.to_string() };
        let source_key = source_key.to_string();
        tokio::task::spawn_blocking(move || {
            take_js_logs();
            let content = crate::parser::js::with_http_context(context, || with_book_context(book_ctx, || {
                parser.content(&source, &body, &url, Some(&source_key))
            }));
            let logs = take_js_logs();
            if content.is_empty() {
                // 优先展示书源脚本自己的提示(如"本书源为VIP专属内容，开通VIP后可阅读"),
                // 它比"正文解析为空"更能告诉用户发生了什么。
                if let Some(message) = first_js_message(&logs) {
                    return Err(AppError::BadRequest(message));
                }
                if let Some(error) = first_js_error(&logs) {
                    return Err(AppError::BadRequest(format!("正文解析失败: {error}")));
                }
                return Err(AppError::BadRequest("书源正文解析为空，请检查正文规则".to_string()));
            }
            Ok((content, logs))
        })
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("正文解析任务失败: {e}")))?
    }

    pub async fn get_content(
        &self,
        user_ns: &str,
        book_url: &str,
        source: &BookSource,
        chapter_url: &str,
    ) -> Result<String, AppError> {
        self.get_content_with_stage(user_ns, book_url, source, chapter_url, None, false)
            .await
    }

    /// 与 `get_content` 相同, 但把加载阶段回调出去, 供前端显示"卡在哪一步"。
    ///
    /// `skip_js_lib` 为 true 时跳过选择器规则的可选脚本；纯 JS 正文仍保留必需的库。
    /// 正文规则里的插件调用
    /// (如 `addComment(result)`) 会因函数不存在而失败, 从而回退到选择器提取结果
    /// —— 也就是纯正文。离线缓存只需要正文, 没必要为每章付出脚本解析与段评请求
    /// 的代价。
    pub async fn get_content_with_stage(
        &self,
        user_ns: &str,
        book_url: &str,
        source: &BookSource,
        chapter_url: &str,
        on_stage: Option<&(dyn Fn(ContentLoadStage) + Send + Sync)>,
        skip_js_lib: bool,
    ) -> Result<String, AppError> {
        let notify = |stage: ContentLoadStage| {
            if let Some(callback) = on_stage {
                callback(stage);
            }
        };
        let book_key = md5_hex(book_url);
        tracing::debug!(
            "get_content called, chapter_url={}, book_key={}",
            chapter_url,
            book_key
        );
        let cached = self
            .cache
            .get(user_ns, &book_key, chapter_url)
            .await
            .ok()
            .flatten();
        // 离线缓存路径命中即用; 阅读路径则**优先在线解析**(保证段评是最新的),
        // 缓存只作为请求失败时的兜底 —— 离线缓存里只有纯正文, 直接用它就看不到段评。
        if skip_js_lib {
            if let Some(hit) = cached.clone() {
                tracing::debug!("get_content returning cached content, len={}", hit.len());
                return Ok(hit);
            }
        }
        tracing::debug!("get_content cache miss, fetching from network");

        let mut all_content = String::new();
        let mut visited_urls = std::collections::HashSet::new();
        let mut current_url = chapter_url.to_string();
        let book_ctx = self.js_book_context(user_ns, book_url, chapter_url).await;
        // Pure JS rules require their library to obtain/decode the content.
        // Selector-based rules can still skip optional presentation helpers.
        let content_rule = source.rule_content.as_ref().and_then(|rule| rule.content.as_deref()).unwrap_or("").trim_start();
        let requires_library = ["@js:", "js:", "<js>"].iter().any(|prefix| content_rule.starts_with(prefix));
        let stripped_source: Option<BookSource> = (skip_js_lib && !requires_library)
            .then(|| BookSource { js_lib: None, ..source.clone() });
        let parse_source = stripped_source.as_ref().unwrap_or(source);

        // Follow pagination to get all content pages
        loop {
            if visited_urls.contains(&current_url) {
                tracing::debug!("get_content detected loop, breaking");
                break;
            }
            visited_urls.insert(current_url.clone());

            notify(ContentLoadStage::Fetching);
            tracing::debug!("get_content fetching: {}", current_url);
            let res = match self
                .fetch_source_url(user_ns, source, &current_url, &source.book_source_url, "")
                .await
            {
                Ok(res) => res,
                Err(e) => {
                    // 网络失败(离线或书源故障)时回退到已缓存正文, 保证离线仍可阅读
                    if let Some(hit) = cached.clone() {
                        tracing::debug!("get_content fetch failed, falling back to cache: {e}");
                        return Ok(hit);
                    }
                    return Err(e);
                }
            };
            tracing::debug!("get_content fetch done, body len={}", res.body.len());
            // 正文规则里往往会触发书源脚本与段评请求, 这是最耗时的一步
            notify(ContentLoadStage::Parsing);
            let (content, _logs) = match self
                .parse_content_blocking(user_ns, parse_source, &res, &current_url, book_url, book_ctx.clone())
                .await
            {
                Ok(parsed) => parsed,
                // 解析失败(如书源提示 VIP 专属、正文规则为空)时:
                // 有缓存就用缓存保证可读, 否则把书源的真实提示抛给用户。
                Err(err) => {
                    match cached.clone() {
                        Some(hit) if !hit.trim().is_empty() => {
                            tracing::debug!("get_content parse failed, falling back to cache: {err:?}");
                            return Ok(hit);
                        }
                        _ => return Err(err),
                    }
                }
            };
            tracing::debug!("get_content parsed content len={}", content.len());

            if !content.is_empty() {
                if !all_content.is_empty() {
                    all_content.push('\n');
                }
                all_content.push_str(&content);
            }

            // Check for next page
            if let Some(next_url) = self.source_scope(user_ns, source, || self.parser.next_content_url(source, &res.body, &res.url))? {
                tracing::debug!("get_content found next_url: {}", next_url);
                if should_follow_content_page(chapter_url, &current_url, &next_url) {
                    current_url = next_url;
                } else {
                    tracing::debug!("get_content next_url appears to be next chapter, stopping");
                    break;
                }
            } else {
                tracing::debug!("get_content no more pages");
                break;
            }
        }

        tracing::debug!("get_content final content len={}", all_content.len());
        if !all_content.is_empty() {
            notify(ContentLoadStage::Caching);
            let _ = self
                .cache
                .put(user_ns, &book_key, chapter_url, &all_content)
                .await;
        }
        Ok(all_content)
    }

    /// Delete all chapter content cache for a book
    pub async fn delete_book_cache(&self, user_ns: &str, book_url: &str) -> Result<bool, AppError> {
        let book_key = md5_hex(book_url);
        self.cache
            .remove_book(user_ns, &book_key)
            .await
            .map_err(|e| AppError::Internal(e.into()))
    }

    /// Check if a specific chapter is cached
    pub async fn is_chapter_cached(
        &self,
        user_ns: &str,
        book_url: &str,
        chapter_url: &str,
    ) -> bool {
        let book_key = md5_hex(book_url);
        self.cache.exists(user_ns, &book_key, chapter_url).await
    }

    pub async fn chapter_list_cache_exists(&self, user_ns: &str, toc_url: &str) -> bool {
        let path = self.chapter_list_cache_path(user_ns, toc_url);
        path.exists()
    }

    pub async fn get_bookshelf(&self, user_ns: &str) -> Result<Vec<Book>, AppError> {
        self.read_bookshelf(user_ns).await
    }

    pub async fn get_shelf_book(
        &self,
        user_ns: &str,
        book_url: &str,
    ) -> Result<Option<Book>, AppError> {
        let list = self.read_bookshelf(user_ns).await?;
        Ok(list.into_iter().find(|b| b.book_url == book_url))
    }

    /// Find book by chapter URL (chapter URL typically shares domain with book URL)
    pub async fn get_shelf_book_by_chapter(
        &self,
        user_ns: &str,
        chapter_url: &str,
    ) -> Result<Option<Book>, AppError> {
        let list = self.read_bookshelf(user_ns).await?;

        // Extract domain from chapter_url
        let chapter_domain = url::Url::parse(chapter_url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()));

        for book in list {
            // Check if chapter URL starts with book URL (common pattern)
            if chapter_url.starts_with(&book.book_url) {
                return Ok(Some(book));
            }

            // Check if they share the same domain
            if let (Some(ref ch_domain), Ok(book_url_parsed)) =
                (&chapter_domain, url::Url::parse(&book.book_url))
            {
                if let Some(book_domain) = book_url_parsed.host_str() {
                    if ch_domain == book_domain {
                        // Check if chapter URL path contains book URL path prefix
                        if let (Ok(ch_parsed), Ok(b_parsed)) = (
                            url::Url::parse(chapter_url),
                            url::Url::parse(&book.book_url),
                        ) {
                            let ch_path = ch_parsed.path();
                            let b_path = b_parsed.path();
                            // Check if paths share a common prefix (e.g., /biqu104/)
                            if ch_path.starts_with(b_path.trim_end_matches('/'))
                                || b_path
                                    .trim_end_matches('/')
                                    .starts_with(ch_path.trim_end_matches('/'))
                            {
                                return Ok(Some(book));
                            }
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    /// Find book by name and author (for cases where book_url might differ)
    pub async fn find_shelf_book_by_name_author(
        &self,
        user_ns: &str,
        name: &str,
        author: &str,
    ) -> Result<Option<Book>, AppError> {
        let list = self.read_bookshelf(user_ns).await?;
        Ok(list.into_iter().find(|b| {
            same_remote_book_identity(&b.name, &b.author, name, author)
        }))
    }

    pub async fn save_book(&self, user_ns: &str, mut book: Book) -> Result<Book, AppError> {
        sanitize_book_urls(&mut book);
        if book.origin.trim().is_empty() {
            return Err(AppError::BadRequest("missing origin".to_string()));
        }
        if book.book_url.trim().is_empty() {
            return Err(AppError::BadRequest("bookUrl required".to_string()));
        }
        // 空简介归一化回 None。
        //
        // `intro` 的 `None` 在这套数据里有个具体含义: 「还没有简介」。
        // `merge_book` 正是用 `target.intro.is_none()` 判断「书源带回了简介就填进去」,
        // 所以 `None` 与 `Some("")` **不能**混为一谈。
        //
        // 前端「编辑书籍信息 → 还原」会把原始值(可能是空)整份提交回来, 于是原本
        // `None` 的简介会被写成 `Some("")` —— 那条回填判据从此永久失效, 换源/刷新
        // 再也补不进新书源的简介。在这里统一收口, 任何调用方都逃不掉。
        //
        // 注意只动 `intro`, 不碰 `original_intro`: 后者的 `''` 表示「记录过且原始为空」,
        // 是「还原」功能的依据, 归一化它会让还原目标丢失。
        if book.intro.as_deref() == Some("") {
            book.intro = None;
        }

        apply_book_read_config_override(&mut book);
        let _write_guard = self.bookshelf_write_lock.lock().await;
        let mut list = self.read_bookshelf(user_ns).await?;
        let mut exist_idx: Option<usize> = None;
        for (i, b) in list.iter().enumerate() {
            if books_match_for_save(b, &book) {
                exist_idx = Some(i);
                break;
            }
        }

        if let Some(i) = exist_idx {
            let exist = list[i].clone();
            merge_book_read_config(&exist, &mut book);
            merge_book_source_candidates(&exist, &mut book);
            if book.dur_chapter_index.is_none() {
                book.dur_chapter_index = exist.dur_chapter_index;
            }
            if book.dur_chapter_title.is_none() {
                book.dur_chapter_title = exist.dur_chapter_title;
            }
            if book.dur_chapter_time.is_none() {
                book.dur_chapter_time = exist.dur_chapter_time;
            }
            if book.dur_chapter_pos.is_none() {
                book.dur_chapter_pos = exist.dur_chapter_pos;
            }
            if book.total_chapter_num.is_none() {
                book.total_chapter_num = exist.total_chapter_num;
            }
            if book.last_check_time.is_none() {
                book.last_check_time = exist.last_check_time;
            }
            if book.group.is_none() {
                book.group = exist.group;
            }
            // 原始值一旦记下就不再被覆盖: 它代表「首次自定义之前的样子」。
            // 前端会整份回传, 这里兜底是为了任何只传部分字段的调用方(以及
            // 旧版本客户端)都不会把已记录的原始值冲掉。
            if book.original_name.is_none() {
                book.original_name = exist.original_name;
            }
            if book.original_author.is_none() {
                book.original_author = exist.original_author;
            }
            if book.original_intro.is_none() {
                book.original_intro = exist.original_intro;
            }
            list[i] = book.clone();
        } else {
            ensure_current_source_candidate(&mut book);
            list.push(book.clone());
        }

        self.write_bookshelf(user_ns, &list).await?;
        Ok(book)
    }

    pub async fn save_books(&self, user_ns: &str, books: Vec<Book>) -> Result<Vec<Book>, AppError> {
        let _write_guard = self.bookshelf_write_lock.lock().await;
        let mut normalized = Vec::with_capacity(books.len());
        for mut book in books {
            sanitize_book_urls(&mut book);
            if book.origin.trim().is_empty() {
                return Err(AppError::BadRequest("missing origin".to_string()));
            }
            if book.book_url.trim().is_empty() {
                return Err(AppError::BadRequest("bookUrl required".to_string()));
            }
            apply_book_read_config_override(&mut book);
            normalized.push(book);
        }
        self.write_bookshelf(user_ns, &normalized).await?;
        Ok(normalized)
    }

    pub async fn remove_source_candidates(
        &self,
        user_ns: &str,
        removed_source_urls: &HashSet<String>,
    ) -> Result<(), AppError> {
        if removed_source_urls.is_empty() {
            return Ok(());
        }
        let removed = removed_source_urls
            .iter()
            .map(|url| normalize_source_url(url))
            .collect::<HashSet<_>>();

        // 廉价短路: 先看原始 JSON 里有没有出现这些源地址, 没有就直接返回。
        //
        // 删失效书源时这步几乎总是命中 —— 失效源通常从没被任何书引用过。
        // 完整走一遍是「反序列化整本书架 + 遍历所有候选 + 整表重写」, 书架有几百
        // 上千本时是几百毫秒级; 一次子串扫描则几乎无成本。
        //
        // 只做**保守**判断: 未命中则一定没有引用, 可以安全跳过; 命中可能只是
        // 恰好出现同名字段, 仍走完整流程(那样也不会误删任何东西)。
        //
        // 转义形式要一并比较: JSON 里 `"`/`\` 等会被写成 `\"`/`\\`, 只比对原始
        // URL 会在含这类字符时误判成"无引用"而错误跳过。
        let candidates = removed
            .iter()
            .flat_map(|url| {
                let escaped = serde_json::to_string(url).unwrap_or_else(|_| format!("\"{url}\""));
                // to_string 会带外层引号, 去掉它才是字段值内部的样子。
                let inner = escaped
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .unwrap_or(url.as_str())
                    .to_string();
                [url.clone(), inner]
            })
            .collect::<Vec<_>>();
        let path = self.bookshelf_path(user_ns);
        if !path.exists() {
            return Ok(());
        }
        // 短路探测用锁外读: 它只做一次子串扫描, 不参与写入决策。
        // 真正的读-改-写必须在锁内重新读, 见下方 `_write_guard` 之后 —— 用这里的
        // `probe` 去反序列化会让写锁形同虚设 (与并发 save_books/delete_book 交错时
        // 用陈旧内容整表覆盖, 丢失并发写入)。
        let probe = fs::read_to_string(&path)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
        if !candidates.iter().any(|url| probe.contains(url.as_str())) {
            return Ok(());
        }
        drop(probe);

        let _write_guard = self.bookshelf_write_lock.lock().await;
        // 锁内重新读: 上面的探测与这里之间可能有并发写入, 必须基于最新内容修改。
        let raw = fs::read_to_string(&path)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
        let mut list: Vec<Book> = match serde_json::from_str(&raw) {
            Ok(list) => list,
            Err(primary_err) => {
                let recovered = recover_bookshelf_entries(&raw)
                    .ok_or_else(|| AppError::BadRequest(primary_err.to_string()))?;
                tracing::warn!(
                    "recovered malformed bookshelf for user_ns={}, path={}, entries={}",
                    user_ns,
                    path.display(),
                    recovered.len()
                );
                self.write_bookshelf(user_ns, &recovered).await?;
                recovered
            }
        };
        for book in &mut list {
            sanitize_book_urls(book);
        }
        let mut changed = false;
        for book in &mut list {
            if let Some(candidates) = book.source_candidates.as_mut() {
                let before = candidates.len();
                candidates.retain(|candidate| {
                    !removed.contains(&normalize_source_url(&candidate.origin))
                });
                changed |= before != candidates.len();
                if candidates.is_empty() {
                    book.source_candidates = None;
                }
            }
        }
        if changed {
            self.write_bookshelf(user_ns, &list).await?;
        }
        Ok(())
    }

    pub async fn delete_book(&self, user_ns: &str, book: &Book) -> Result<bool, AppError> {
        let _write_guard = self.bookshelf_write_lock.lock().await;
        let mut list = self.read_bookshelf(user_ns).await?;
        let orig_len = list.len();
        let removed: Vec<Book> = list
            .iter()
            .filter(|b| books_match_for_delete(b, book))
            .cloned()
            .collect();
        list.retain(|b| !books_match_for_delete(b, book));
        let deleted = list.len() != orig_len;
        if deleted {
            self.write_bookshelf(user_ns, &list).await?;
            for removed_book in &removed {
                let _ = self.clear_book_related_cache(user_ns, removed_book).await;
            }
        }
        Ok(deleted)
    }

    pub async fn delete_books(&self, user_ns: &str, books: Vec<Book>) -> Result<usize, AppError> {
        let _write_guard = self.bookshelf_write_lock.lock().await;
        let mut list = self.read_bookshelf(user_ns).await?;
        let mut deleted = 0usize;
        let mut removed_books: Vec<Book> = Vec::new();
        for book in books {
            let matched: Vec<Book> = list
                .iter()
                .filter(|b| books_match_for_delete(b, &book))
                .cloned()
                .collect();
            removed_books.extend(matched);
            let before = list.len();
            list.retain(|b| !books_match_for_delete(b, &book));
            if list.len() != before {
                deleted += 1;
            }
        }
        if deleted > 0 {
            self.write_bookshelf(user_ns, &list).await?;
            for removed_book in &removed_books {
                let _ = self.clear_book_related_cache(user_ns, removed_book).await;
            }
        }
        Ok(deleted)
    }

    pub async fn cached_chapter_count(
        &self,
        user_ns: &str,
        book_url: &str,
        chapter_urls: &[String],
    ) -> Result<usize, AppError> {
        let book_key = md5_hex(book_url);
        // 旧实现对每个章节 URL 单独 path.exists(), 一本书几千章就几千次系统调用,
        // 且在 async 上下文里做同步阻塞 IO。改为一次 read_dir 取回所有缓存文件名,
        // 用 HashSet 匹配, O(1) per chapter。
        let cached_files = self.cache.list_chapter_files(user_ns, &book_key).await?;
        if cached_files.is_empty() {
            return Ok(0);
        }
        let cached_set: std::collections::HashSet<&str> =
            cached_files.iter().map(|s| s.as_str()).collect();
        let count = chapter_urls
            .iter()
            .filter(|url| {
                let name = md5_hex(url);
                cached_set.contains(name.as_str())
            })
            .count();
        Ok(count)
    }

    pub async fn cached_chapter_urls(
        &self,
        user_ns: &str,
        book_url: &str,
        chapter_urls: &[String],
    ) -> Result<Vec<String>, AppError> {
        let book_key = md5_hex(book_url);
        let cached_files = self.cache.list_chapter_files(user_ns, &book_key).await?;
        if cached_files.is_empty() {
            return Ok(Vec::new());
        }
        let cached_set: std::collections::HashSet<&str> =
            cached_files.iter().map(|s| s.as_str()).collect();
        Ok(chapter_urls
            .iter()
            .filter(|url| cached_set.contains(md5_hex(url).as_str()))
            .cloned()
            .collect())
    }

    pub async fn cache_chapter(
        &self,
        user_ns: &str,
        book_url: &str,
        source: &BookSource,
        chapter_url: &str,
        refresh: bool,
    ) -> Result<(), AppError> {
        let book_key = md5_hex(book_url);
        if refresh {
            let _ = self.cache.remove(user_ns, &book_key, chapter_url).await;
        }
        let content = self
            .get_content_with_stage(user_ns, book_url, source, chapter_url, None, true)
            .await?;
        // 正文规则若是**纯 JS**（没有选择器部分，如 `@js:clean(result)`），跳过书源
        // 脚本会让整条规则失败并得到空内容。这类书源回退到完整解析（含 jsLib）再试。
        if content.trim().is_empty() {
            tracing::debug!("cache_chapter: 跳过书源脚本后正文为空, 回退完整解析");
            if let Err(e) = self
                .get_content_with_stage(user_ns, book_url, source, chapter_url, None, false)
                .await
            {
                tracing::warn!("cache_chapter 回退完整解析失败: {e}");
            }
        }
        // 以「缓存是否真的落盘」为准：空正文不会被写入缓存（见 get_content_with_stage
        // 里的 is_empty 判断）。若仍未落盘就报错计入失败数，否则进度会把没缓存的
        // 章节算作成功，且下次缓存时这些章又被判为未缓存。
        if !self.is_chapter_cached(user_ns, book_url, chapter_url).await {
            return Err(AppError::Internal(anyhow::anyhow!(
                "正文解析为空, 未写入缓存"
            )));
        }
        Ok(())
    }

    pub async fn get_cover(&self, user_ns: &str, url: &str) -> Result<(Vec<u8>, String), AppError> {
        let ext = file_ext_from_url(url).unwrap_or_else(|| "png".to_string());
        let name = md5_hex(url);
        let path = self
            .storage_dir
            .join("cache")
            .join(user_ns)
            .join("cover")
            .join(format!("{}.{}", name, ext));
        if path.exists() {
            let data = fs::read(&path)
                .await
                .map_err(|e| AppError::Internal(e.into()))?;
            let content_type = content_type_from_ext(&ext);
            return Ok((data, content_type));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::Internal(e.into()))?;
        }

        // Extract referer from URL for anti-hotlinking bypass
        let referer = url::Url::parse(url).ok().and_then(|u| {
            let scheme = u.scheme();
            let host = u.host_str()?;
            Some(format!("{}://{}", scheme, host))
        });

        let mut req = self.http.client().get(url);

        // Add necessary headers to bypass anti-hotlinking
        req = req
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .header("Accept", "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8");

        if let Some(ref referer) = referer {
            req = req.header("Referer", referer);
        }

        let res = req.send().await.map_err(|e| AppError::Internal(e.into()))?;
        if !res.status().is_success() {
            return Err(AppError::NotFound("cover not found".to_string()));
        }
        let content_type = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| content_type_from_ext(&ext));
        let bytes = res
            .bytes()
            .await
            .map_err(|e| AppError::Internal(e.into()))?
            .to_vec();
        let _ = fs::write(&path, &bytes).await;
        Ok((bytes, content_type))
    }

    pub async fn load_book_sources_cache(
        &self,
        user_ns: &str,
        book_url: &str,
    ) -> Result<Option<Vec<SearchBook>>, AppError> {
        let path = self.book_source_cache_path(user_ns, book_url);
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(&path)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
        let list: Vec<SearchBook> =
            serde_json::from_str(&data).map_err(|e| AppError::BadRequest(e.to_string()))?;
        Ok(Some(list))
    }

    pub async fn save_book_sources_cache(
        &self,
        user_ns: &str,
        book_url: &str,
        list: &Vec<SearchBook>,
    ) -> Result<(), AppError> {
        let path = self.book_source_cache_path(user_ns, book_url);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::Internal(e.into()))?;
        }
        let data = serde_json::to_string(list).map_err(|e| AppError::BadRequest(e.to_string()))?;
        fs::write(&path, data)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
        Ok(())
    }

    pub async fn delete_book_sources_cache(
        &self,
        user_ns: &str,
        book_url: &str,
    ) -> Result<(), AppError> {
        let path = self.book_source_cache_path(user_ns, book_url);
        if path.exists() {
            fs::remove_file(&path)
                .await
                .map_err(|e| AppError::Internal(e.into()))?;
        }
        Ok(())
    }

    fn book_source_cache_path(&self, user_ns: &str, book_url: &str) -> PathBuf {
        let name = md5_hex(book_url);
        self.storage_dir
            .join("data")
            .join(user_ns)
            .join("book_sources")
            // Earlier caches merged aggregate stations by origin and could omit
            // completed results. Rebuild once under the complete-list format.
            .join(format!("{}.v2.json", name))
    }

    fn bookshelf_path(&self, user_ns: &str) -> PathBuf {
        self.storage_dir
            .join("data")
            .join(user_ns)
            .join("bookshelf.json")
    }

    async fn read_bookshelf(&self, user_ns: &str) -> Result<Vec<Book>, AppError> {
        let path = self.bookshelf_path(user_ns);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let data = fs::read_to_string(&path)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
        let mut list: Vec<Book> = match serde_json::from_str(&data) {
            Ok(list) => list,
            Err(primary_err) => {
                let recovered = recover_bookshelf_entries(&data)
                    .ok_or_else(|| AppError::BadRequest(primary_err.to_string()))?;
                tracing::warn!(
                    "recovered malformed bookshelf for user_ns={}, path={}, entries={}",
                    user_ns,
                    path.display(),
                    recovered.len()
                );
                self.write_bookshelf(user_ns, &recovered).await?;
                recovered
            }
        };
        for book in &mut list {
            sanitize_book_urls(book);
            apply_book_read_config_override(book);
        }
        Ok(list)
    }

    async fn write_bookshelf(&self, user_ns: &str, list: &Vec<Book>) -> Result<(), AppError> {
        let path = self.bookshelf_path(user_ns);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::Internal(e.into()))?;
        }
        let data = serde_json::to_string(list).map_err(|e| AppError::BadRequest(e.to_string()))?;
        fs::write(&path, data)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
        Ok(())
    }

    // Chapter list cache methods
    fn chapter_list_cache_path(&self, user_ns: &str, toc_url: &str) -> PathBuf {
        let name = md5_hex(toc_url);
        // The aggregate catalog embeds chapter payloads and review scripts in
        // data URLs. Older java.put/get and getCookie implementations could
        // cache incomplete URLs, so rebuild only this protocol's catalogs once.
        // Content files stay available as offline fallbacks.
        let (url, options) = split_url_options(toc_url);
        let aggregate_catalog = url.starts_with("data:")
            && options
                .and_then(|options| serde_json::from_str::<serde_json::Value>(options).ok())
                .is_some_and(|options| {
                    options.get("type").and_then(|value| value.as_str()) == Some("gycatalog")
                });
        let revision = if aggregate_catalog { "v3" } else { "v2" };
        self.storage_dir
            .join("data")
            .join(user_ns)
            .join("chapters")
            .join(format!("{name}.{revision}.json"))
    }

    pub async fn load_chapter_list_cache(
        &self,
        user_ns: &str,
        toc_url: &str,
    ) -> Result<Option<Vec<BookChapter>>, AppError> {
        let path = self.chapter_list_cache_path(user_ns, toc_url);
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(&path)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
        let list: Vec<BookChapter> =
            serde_json::from_str(&data).map_err(|e| AppError::BadRequest(e.to_string()))?;
        Ok(Some(list))
    }

    pub async fn save_chapter_list_cache(
        &self,
        user_ns: &str,
        toc_url: &str,
        chapters: &Vec<BookChapter>,
    ) -> Result<(), AppError> {
        let path = self.chapter_list_cache_path(user_ns, toc_url);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::Internal(e.into()))?;
        }
        let data =
            serde_json::to_string(chapters).map_err(|e| AppError::BadRequest(e.to_string()))?;
        fs::write(&path, data)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
        Ok(())
    }

    pub async fn append_chapter_list_cache(
        &self,
        user_ns: &str,
        toc_url: &str,
        new_chapters: &Vec<BookChapter>,
    ) -> Result<Vec<BookChapter>, AppError> {
        let mut existing = self
            .load_chapter_list_cache(user_ns, toc_url)
            .await?
            .unwrap_or_default();
        let start_index = existing.len() as i32;
        for (i, ch) in new_chapters.iter().enumerate() {
            let mut ch = ch.clone();
            ch.index = start_index + i as i32;
            existing.push(ch);
        }
        self.save_chapter_list_cache(user_ns, toc_url, &existing)
            .await?;
        Ok(existing)
    }

    pub async fn delete_chapter_list_cache(
        &self,
        user_ns: &str,
        toc_url: &str,
    ) -> Result<(), AppError> {
        let path = self.chapter_list_cache_path(user_ns, toc_url);
        if path.exists() {
            fs::remove_file(&path)
                .await
                .map_err(|e| AppError::Internal(e.into()))?;
        }
        Ok(())
    }

    async fn clear_book_related_cache(&self, user_ns: &str, book: &Book) -> Result<(), AppError> {
        if !book.book_url.is_empty() {
            let _ = self.delete_book_cache(user_ns, &book.book_url).await;
            let _ = self
                .delete_book_sources_cache(user_ns, &book.book_url)
                .await;
            let _ = self
                .delete_chapter_list_cache(user_ns, &book.book_url)
                .await;
        }
        if let Some(toc_url) = &book.toc_url {
            if !toc_url.is_empty() {
                let _ = self.delete_chapter_list_cache(user_ns, toc_url).await;
            }
        }
        Ok(())
    }
}

/// 取日志里第一条可用于诊断的脚本错误。
///
/// `[正文 JS]` / `[目录 JS]` 这类**规则级**前缀必须包含在内: 书源脚本抛出的
/// 业务提示(如"本书源为VIP专属内容")正是通过它们传出来的, 漏掉会让正文
/// 解析失败退化成静默的空串。
fn first_js_error(logs: &[String]) -> Option<&str> {
    logs.iter()
        .find(|line| {
            ["[脚本]", "[jsLib]", "[http]", "[正文 JS]", "[目录 JS]", "[详情 JS]"]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        })
        .map(String::as_str)
}

/// 从日志里提取脚本抛出的**用户可读提示**（去掉 `[xxx JS] JS 异常:` 一类的
/// 内部前缀），供界面直接展示。
fn first_js_message(logs: &[String]) -> Option<String> {
    let raw = first_js_error(logs)?;
    let text = raw
        .split_once("JS 异常:")
        .map(|(_, rest)| rest)
        .unwrap_or(raw)
        .trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// 执行 `loginCheckJs`。语义与 legado 一致: 脚本的返回值即"处理后的响应"
/// (返回 `result` 表示放行, 返回新响应对象表示替换)。脚本里的
/// `source.login()` 会被记录, 由调用方用已存登录信息重跑登录。
/// 返回 `(响应, 是否请求了重新登录)`。
fn apply_login_check_js(source: &BookSource, res: FetchResponse) -> Result<(FetchResponse, bool), AppError> {
    let Some(script) = source
        .login_check_js
        .as_deref()
        .filter(|script| !script.trim().is_empty())
    else {
        return Ok((res, false));
    };

    with_source(source, || {
        crate::parser::js::take_login_effects();
        let str_response = StrResponse::from(res.clone());
        let response_json = serde_json::to_value(&str_response).unwrap_or_else(|_| json!({}));
        // legado: loginCheckJs 里 `java.getStrResponse()` 返回当前响应。
        crate::parser::js::set_last_response(&response_json.to_string());
        let mut bindings = HashMap::new();
        // legado 的 `result` 是 StrResponse 对象: 脚本按方法调用
        // (`result.code()`/`result.body()`), 必须用 Resp() 包装。
        bindings.insert("result".to_string(), crate::parser::js::resp_binding(&response_json));
        let login_requested = |()| crate::parser::js::take_login_effects().login;
        match eval_js_with_bindings(script, &res.body, &res.url, &bindings) {
            Ok(output) if !output.trim().is_empty() => {
                // 识别"替换响应"：带宿主信封标记，或含可辨识的响应字段
                // (code/url/headers/isSuccessful)。纯书源数据(如详情 JSON)
                // 不能被当信封 —— 否则会把正常正文替换成空响应(HTTP 0)，
                // 表现为"目录没有了但也不报错"。
                if let Some(next) = parse_resp_envelope(&output) {
                    Ok((FetchResponse::from(next), login_requested(())))
                } else {
                    Ok((
                        FetchResponse {
                            body: output,
                            ..res
                        },
                        login_requested(()),
                    ))
                }
            }
            Ok(_) => Ok((res, login_requested(()))),
            Err(err) => {
                // 校验脚本本身出错时不能把整个搜索/详情链路打断:
                // 绝大多数源在没有会话时只是想让脚本原样放行。
                tracing::warn!(
                    "loginCheckJs failed for {}: {:?}",
                    source.book_source_name,
                    err
                );
                Ok((res, false))
            }
        }
    })
}

/// 识别 loginCheckJs 返回的"替换响应"信封。
///
/// 判定条件(满足其一):
/// - 宿主 `Resp` 对象序列化出的信封(带 `__respEnvelope` 标记)
/// - 显式含 `code` 且是有效 HTTP 状态码(>=100)，同时带 `body`/`url` 之一
///
/// 纯书源数据(如详情 JSON `{name,author,bookUrl,...}`)两条都不满足，
/// 因此不会被误当信封把正常正文替换成空响应。
fn parse_resp_envelope(output: &str) -> Option<StrResponse> {
    let value: serde_json::Value = serde_json::from_str(output).ok()?;
    let object = value.as_object()?;
    let marked = object.get("__respEnvelope").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let has_code = object
        .get("code")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|code| code >= 100);
    if !marked && !(has_code && (object.contains_key("body") || object.contains_key("url"))) {
        return None;
    }
    serde_json::from_value(value).ok()
}

fn parse_explore_kinds(
    source: &BookSource,
    info_map: &HashMap<String, String>,
) -> Result<Vec<ExploreKind>, AppError> {
    let Some(raw) = source
        .explore_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(Vec::new());
    };

    let text = if let Some(script) = raw.strip_prefix("@js:") {
        crate::parser::js::eval_explore_kinds(script, &source.book_source_url, info_map)
            .map_err(|err| AppError::BadRequest(format!("发现规则执行失败: {err}")))?
    } else if let Some(script) = raw
        .strip_prefix("<js>")
        .and_then(|value| value.strip_suffix("</js>"))
    {
        crate::parser::js::eval_explore_kinds(script, &source.book_source_url, info_map)
            .map_err(|err| AppError::BadRequest(format!("发现规则执行失败: {err}")))?
    } else {
        raw.to_string()
    };

    for json_text in [&text, &normalize_relaxed_explore_json(&text)] {
        if let Ok(kinds) = serde_json::from_str::<Vec<ExploreKind>>(json_text) {
            let mut kinds: Vec<ExploreKind> = kinds
                .into_iter()
                .filter(|kind| !kind.title.trim().is_empty())
                .collect();
            resolve_explore_view_names(source, &mut kinds, info_map);
            return Ok(kinds);
        }
    }

    // 无前缀的裸 JS 对象字面量（这一类写法）：
    //   `[{'title':'我的书架','url':'...','style':{layout_flexGrow: 1}}, ...]`
    // 单引号 + 无引号键名，不是合法 JSON，`serde_json` 直接拒收。
    //
    // legado 用 GSON 解析，而 GSON 的 JsonReader 默认宽松（接受单引号与无引号
    // 键名），所以手机端能显示。这里改为交给 rquickjs 求值 —— JS 引擎天生就懂
    // JS 字面量，比拼字符串规则稳。
    //
    // 只在严格解析失败后尝试：正常书源走不到这里。求值失败也不报错，继续往下
    // 走纯文本切分，保持原有兜底行为。
    if looks_like_js_array_literal(&text) {
        if let Ok(escaped) = crate::parser::js::eval_explore_kinds(
            &text,
            &source.book_source_url,
            info_map,
        ) {
            if let Ok(kinds) = serde_json::from_str::<Vec<ExploreKind>>(escaped.trim()) {
                let mut kinds: Vec<ExploreKind> = kinds
                    .into_iter()
                    .filter(|kind| !kind.title.trim().is_empty())
                    .collect();
                if !kinds.is_empty() {
                    resolve_explore_view_names(source, &mut kinds, info_map);
                    return Ok(kinds);
                }
            }
        }
    }

    let splitter = regex::Regex::new(r"(&&|\n)+").unwrap();
    Ok(splitter
        .split(&text)
        .filter_map(|item| {
            let item = item.trim();
            if item.is_empty() {
                return None;
            }
            let mut parts = item.splitn(2, "::");
            let title = parts.next().unwrap_or_default().trim();
            if title.is_empty() {
                return None;
            }
            let url = parts
                .next()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            Some(ExploreKind {
                title: title.to_string(),
                url,
                ..ExploreKind::default()
            })
        })
        .collect())
}

/// 填充交互控件的显示名（legado `ExploreKind.viewName`）。
///
/// 只处理非 url 型控件：url 型就是普通分类按钮，标题即文字。求值失败时
/// 保持 `view_name` 为 None，前端回退用 `title` —— 显示名不该成为整页失败的
/// 原因。
fn resolve_explore_view_names(
    source: &BookSource,
    kinds: &mut [ExploreKind],
    info_map: &HashMap<String, String>,
) {
    for kind in kinds.iter_mut() {
        if kind.kind_type() == crate::model::book_source::ExploreKindType::Url {
            continue;
        }
        let Some(raw) = kind.view_name.as_deref().filter(|v| !v.trim().is_empty()) else {
            continue;
        };
        kind.view_name = crate::parser::js::eval_explore_view_name(
            raw,
            &source.book_source_url,
            info_map,
        );
    }
}

fn normalize_relaxed_explore_json(text: &str) -> String {    let mut normalized = String::with_capacity(text.len());
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;

    for ch in text.chars() {
        if in_string {
            normalized.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = true;
                quote = ch;
                normalized.push(ch);
            }
            '<' => normalized.push('{'),
            '>' => normalized.push('}'),
            _ => normalized.push(ch),
        }
    }

    normalized
}

/// 判断文本是否"看起来像 JS 数组里的对象字面量"。
///
/// 用来决定要不要动用 JS 引擎去宽松解析。判据刻意保守，只认这一种形态：
/// 整体是 `[...]`，且开头就出现 `{` 与一个带引号的键 —— 即
/// `[{'title':...}]`。普通文本分类规则（`标题::url&&标题2::url2`）不会命中。
///
/// 不命中时保持原有行为（严格 JSON → 宽松 JSON → 纯文本切分），所以这里漏判
/// 只是少了个优化，不会引入回归。
fn looks_like_js_array_literal(text: &str) -> bool {
    let trimmed = text.trim();
    if !(trimmed.starts_with('[') && trimmed.ends_with(']')) {
        return false;
    }
    let head = &trimmed[1..trimmed.len().min(64)];
    let Some(open) = head.find('{') else {
        return false;
    };
    // `{` 之后应当紧跟（可有空白）一个被引号包起来的键名。
    let after = head[open + 1..].trim_start();
    let Some(quote) = after.chars().next() else {
        return false;
    };
    if quote != '\'' && quote != '"' {
        return false;
    }
    after[quote.len_utf8()..].contains(quote)
}

fn parse_window_rate(rate: &str) -> Option<(usize, u64)> {
    let (limit, window) = rate.split_once('/')?;
    let limit = limit.trim().parse().ok()?;
    let window = window.trim().parse().ok()?;
    Some((limit, window))
}

fn should_follow_content_page(chapter_url: &str, current_url: &str, next_url: &str) -> bool {
    let next_url = strip_fragment(next_url);
    let current_url = strip_fragment(current_url);
    let chapter_url = strip_fragment(chapter_url);

    if next_url == current_url || next_url == chapter_url {
        return false;
    }

    match (
        url::Url::parse(chapter_url),
        url::Url::parse(current_url),
        url::Url::parse(next_url),
    ) {
        (Ok(chapter), Ok(current), Ok(next)) => {
            if chapter.scheme() != next.scheme()
                || chapter.host_str() != next.host_str()
                || chapter.port_or_known_default() != next.port_or_known_default()
            {
                return false;
            }

            let chapter_exact = content_path_exact_base(chapter.path());
            let current_exact = content_path_exact_base(current.path());
            let next_exact = content_path_exact_base(next.path());
            let next_page_base = content_path_page_base(next.path());

            next_exact == chapter_exact
                || next_exact == current_exact
                || next_page_base == chapter_exact
                || next_page_base == current_exact
        }
        _ => {
            let chapter_exact = content_path_exact_base(chapter_url);
            let current_exact = content_path_exact_base(current_url);
            let next_exact = content_path_exact_base(next_url);
            let next_page_base = content_path_page_base(next_url);

            next_exact == chapter_exact
                || next_exact == current_exact
                || next_page_base == chapter_exact
                || next_page_base == current_exact
        }
    }
}

fn strip_fragment(url: &str) -> &str {
    url.split_once('#').map(|(head, _)| head).unwrap_or(url)
}

fn content_path_exact_base(path: &str) -> String {
    content_path_base(path, false)
}

fn content_path_page_base(path: &str) -> String {
    content_path_base(path, true)
}

fn content_path_base(path: &str, strip_page_suffix: bool) -> String {
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    let (stem, _ext) = file.rsplit_once('.').unwrap_or((file, ""));
    let stem = if strip_page_suffix {
        strip_page_suffix_from_stem(stem)
    } else {
        stem
    };
    if dir.is_empty() {
        stem.to_string()
    } else {
        format!("{dir}/{stem}")
    }
}

fn strip_page_suffix_from_stem(stem: &str) -> &str {
    for sep in ['-', '_'] {
        if let Some(idx) = stem.rfind(sep) {
            let suffix = &stem[idx + sep.len_utf8()..];
            if !suffix.is_empty()
                && suffix.chars().all(|ch| ch.is_ascii_digit())
                && suffix
                    .parse::<usize>()
                    .map(|page| page >= 2)
                    .unwrap_or(false)
            {
                return &stem[..idx];
            }
        }
    }
    stem
}

fn is_local_book(book: &Book) -> bool {
    matches!(book.origin.trim(), "local-txt" | "local-epub" | "local-pdf")
        || book.book_url.trim().starts_with("local-txt:")
        || book.book_url.trim().starts_with("local-epub:")
        || is_local_pdf_origin(&book.origin)
        || is_local_pdf_url(&book.book_url)
}

fn books_match_for_save(existing: &Book, incoming: &Book) -> bool {
    if existing.book_url == incoming.book_url {
        return true;
    }
    if is_local_book(existing) || is_local_book(incoming) {
        return false;
    }
    !existing.name.is_empty()
        && same_remote_book_identity(
            &existing.name,
            &existing.author,
            &incoming.name,
            &incoming.author,
        )
}

fn same_remote_book_identity(
    left_name: &str,
    left_author: &str,
    right_name: &str,
    right_author: &str,
) -> bool {
    normalize_book_name(left_name) == normalize_book_name(right_name)
        && normalize_book_author(left_author) == normalize_book_author(right_author)
}

fn normalize_book_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalize_book_author(value: &str) -> String {
    let compact = normalize_book_name(value);
    compact
        .strip_prefix("作者：")
        .or_else(|| compact.strip_prefix("作者:"))
        .or_else(|| compact.strip_prefix("作者"))
        .unwrap_or(&compact)
        .trim_start_matches(['：', ':'])
        .to_string()
}

fn ensure_current_source_candidate(book: &mut Book) {
    if is_local_book(book) || book.origin.trim().is_empty() || book.book_url.trim().is_empty() {
        return;
    }
    let current = search_candidate_from_book(book);
    let candidates = book.source_candidates.get_or_insert_with(Vec::new);
    merge_source_candidate(candidates, current);
}

fn merge_book_source_candidates(existing: &Book, incoming: &mut Book) {
    if is_local_book(existing) || is_local_book(incoming) {
        return;
    }

    let mut candidates = Vec::new();
    if !existing.origin.trim().is_empty() && !existing.book_url.trim().is_empty() {
        merge_source_candidate(&mut candidates, search_candidate_from_book(existing));
    }
    for candidate in existing.source_candidates.clone().unwrap_or_default() {
        merge_source_candidate(&mut candidates, candidate);
    }
    for candidate in incoming.source_candidates.clone().unwrap_or_default() {
        merge_source_candidate(&mut candidates, candidate);
    }
    if !incoming.origin.trim().is_empty() && !incoming.book_url.trim().is_empty() {
        merge_source_candidate(&mut candidates, search_candidate_from_book(incoming));
    }
    incoming.source_candidates = (!candidates.is_empty()).then_some(candidates);
}

fn merge_source_candidate(candidates: &mut Vec<SearchBook>, candidate: SearchBook) {
    if candidate.origin.trim().is_empty() || candidate.book_url.trim().is_empty() {
        return;
    }
    let origin = normalize_source_url(&candidate.origin);
    if let Some(index) = candidates
        .iter()
        .position(|item| normalize_source_url(&item.origin) == origin && item.book_url == candidate.book_url)
    {
        candidates[index] = candidate;
    } else {
        candidates.push(candidate);
    }
}

fn search_candidate_from_book(book: &Book) -> SearchBook {
    SearchBook {
        name: book.name.clone(),
        author: book.author.clone(),
        book_url: book.book_url.clone(),
        origin: book.origin.clone(),
        cover_url: book.cover_url.clone(),
        intro: book.intro.clone(),
        kind: book.kind.clone(),
        last_chapter: book.latest_chapter_title.clone(),
        update_time: book.update_time.clone(),
        word_count: book.word_count.clone(),
        book_source_urls: None,
    }
}

fn books_match_for_delete(existing: &Book, target: &Book) -> bool {
    if !target.book_url.is_empty() && existing.book_url == target.book_url {
        return true;
    }
    if is_local_book(existing) || is_local_book(target) {
        return false;
    }
    !target.name.is_empty()
        && !target.author.is_empty()
        && existing.name == target.name
        && existing.author == target.author
}

fn apply_book_read_config_override(book: &mut Book) {
    if let Some(use_replace_rule) = crate::parser::js::book_read_config_override(&book.origin, &book.book_url)
        .and_then(|config| config.get("useReplaceRule").and_then(serde_json::Value::as_bool))
    {
        book.read_config.get_or_insert_with(Default::default).use_replace_rule = Some(use_replace_rule);
    }
}

fn merge_book_read_config(existing: &Book, incoming: &mut Book) {
    let Some(existing_config) = &existing.read_config else { return; };
    let config = incoming.read_config.get_or_insert_with(Default::default);
    if config.use_replace_rule.is_none() {
        config.use_replace_rule = existing_config.use_replace_rule;
    }
    for (key, value) in &existing_config.extra {
        config.extra.entry(key.clone()).or_insert_with(|| value.clone());
    }
}

fn sanitize_book_urls(book: &mut Book) {
    book.book_url = repair_encoded_url(&book.book_url);
    book.origin = normalize_source_url(&book.origin);
    if let Some(toc_url) = &book.toc_url {
        book.toc_url = Some(repair_encoded_url(toc_url));
    }
    if let Some(cover_url) = &book.cover_url {
        book.cover_url = Some(repair_encoded_url(cover_url));
    }
}

fn recover_bookshelf_entries(data: &str) -> Option<Vec<Book>> {
    let mut recovered = Vec::new();
    let mut seen = HashSet::new();
    let stream = serde_json::Deserializer::from_str(data).into_iter::<serde_json::Value>();

    for item in stream {
        let value = match item {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!("bookshelf recovery stream stopped: {}", err);
                break;
            }
        };
        match value {
            serde_json::Value::Array(items) => {
                for entry in items {
                    if let Ok(book) = serde_json::from_value::<Book>(entry) {
                        push_recovered_book(&mut recovered, &mut seen, book);
                    }
                }
            }
            serde_json::Value::Object(_) => {
                if let Ok(book) = serde_json::from_value::<Book>(value) {
                    push_recovered_book(&mut recovered, &mut seen, book);
                }
            }
            _ => {}
        }
    }

    if recovered.is_empty() {
        None
    } else {
        Some(recovered)
    }
}

fn push_recovered_book(recovered: &mut Vec<Book>, seen: &mut HashSet<String>, mut book: Book) {
    sanitize_book_urls(&mut book);
    let key = format!("{}::{}", book.book_url, book.origin);
    if seen.insert(key) {
        recovered.push(book);
    }
}

fn file_ext_from_url(url: &str) -> Option<String> {
    let url = url.split('?').next().unwrap_or(url);
    let url = url.split('#').next().unwrap_or(url);
    let pos = url.rfind('.')?;
    let ext = &url[pos + 1..];
    if ext.len() > 0 && ext.len() <= 8 {
        Some(ext.to_ascii_lowercase())
    } else {
        None
    }
}

fn content_type_from_ext(ext: &str) -> String {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 阶段枚举的序列化结果就是前端 `loadStage` 收到的字符串,
    /// 改动这里会直接破坏阅读页的阶段文案, 所以固定住。
    #[test]
    fn content_load_stage_serializes_as_expected_strings() {
        let to_json = |stage: ContentLoadStage| serde_json::to_string(&stage).unwrap();
        assert_eq!(to_json(ContentLoadStage::Fetching), "\"fetching\"");
        assert_eq!(to_json(ContentLoadStage::Parsing), "\"parsing\"");
        assert_eq!(to_json(ContentLoadStage::Caching), "\"caching\"");
    }

    #[test]
    fn detect_anti_crawler_flags_pc_verification_page() {
        // 某小说站 PC 站反爬: 202 + var buid
        let warnings = detect_anti_crawler(202, "var buid = xxx", &[]);
        assert!(
            warnings.iter().any(|w| w.contains("反爬")),
            "应提示反爬, 实际: {warnings:?}"
        );
    }

    #[test]
    fn detect_anti_crawler_flags_degraded_405_response() {
        let warnings = detect_anti_crawler(403, "<html></html>", &[]);
        assert!(warnings.iter().any(|w| w.contains("403")));
    }

    #[test]
    fn detect_anti_crawler_ignores_normal_html_200() {
        let warnings = detect_anti_crawler(
            200,
            "<html><body>正常页面内容足够长</body></html>",
            &[("Content-Type".to_string(), "text/html".to_string())],
        );
        assert!(warnings.is_empty(), "正常响应不应有告警: {warnings:?}");
    }

    #[test]
    fn source_cookie_keys_do_not_share_same_domain_sources() {
        let http = HttpClient::new(5, None).unwrap();
        let first = http.source_context("default", "https://a.example.com", None).unwrap();
        let second = http.source_context("default", "https://b.example.com", None).unwrap();
        assert!(!Arc::ptr_eq(&first.session.cookies, &second.session.cookies));
    }

    #[test]
    fn local_pdf_books_are_treated_as_local_books() {
        let book = Book {
            origin: "local-pdf".to_string(),
            book_url: "local-pdf:0123456789abcdef0123456789abcdef".to_string(),
            ..Book::default()
        };
        assert!(is_local_book(&book));
    }

    /// `remove_source_candidates` 的廉价短路不能因 JSON 转义而漏判。
    ///
    /// JSON 里 `"` / `\` 会被写成 `\"` / `\\`, 若只用原始 URL 做子串比较, 含这类
    /// 字符的源地址会被误判成"书架里没有引用"而**错误跳过清理**。
    #[tokio::test]
    async fn escaped_source_url_is_detected_by_removal_shortcut() {
        let storage_dir = std::env::temp_dir().join(format!(
            "reader-rust-remove-candidates-{}",
            uuid::Uuid::new_v4()
        ));
        let service = BookService::new(
            HttpClient::new(5, None).unwrap(),
            RuleEngine::new().unwrap(),
            FileCache::new(storage_dir.join("cache")),
            storage_dir.to_str().unwrap(),
        );

        // 含引号的源地址: 写进 JSON 后会出现 `\"` 转义。
        let awkward_origin = "https://a.example.com/\"quoted\"";
        let book = Book {
            book_url: "https://a.example.com/book/1".to_string(),
            name: "书".to_string(),
            author: "作者".to_string(),
            source_candidates: Some(vec![crate::model::search::SearchBook {
                origin: awkward_origin.to_string(),
                book_url: "https://a.example.com/book/1".to_string(),
                name: "书".to_string(),
                author: "作者".to_string(),
                ..Default::default()
            }]),
            ..Book::default()
        };
        service
            .write_bookshelf("default", &vec![book])
            .await
            .unwrap();

        service
            .remove_source_candidates(
                "default",
                &std::collections::HashSet::from([awkward_origin.to_string()]),
            )
            .await
            .unwrap();

        let list = service.read_bookshelf("default").await.unwrap();
        assert!(
            list[0].source_candidates.is_none(),
            "含转义字符的源地址也必须被清理, 不能被短路跳过"
        );
        let _ = tokio::fs::remove_dir_all(&storage_dir).await;
    }

    /// 书架里完全没有引用时, 短路应跳过清理(并且不破坏书架内容)。
    #[tokio::test]
    async fn removal_shortcut_keeps_bookshelf_intact_when_unreferenced() {
        let storage_dir = std::env::temp_dir().join(format!(
            "reader-rust-remove-shortcut-{}",
            uuid::Uuid::new_v4()
        ));
        let service = BookService::new(
            HttpClient::new(5, None).unwrap(),
            RuleEngine::new().unwrap(),
            FileCache::new(storage_dir.join("cache")),
            storage_dir.to_str().unwrap(),
        );

        let book = Book {
            book_url: "https://b.example.com/book/1".to_string(),
            name: "书".to_string(),
            author: "作者".to_string(),
            source_candidates: Some(vec![crate::model::search::SearchBook {
                origin: "https://b.example.com".to_string(),
                book_url: "https://b.example.com/book/1".to_string(),
                name: "书".to_string(),
                author: "作者".to_string(),
                ..Default::default()
            }]),
            ..Book::default()
        };
        service
            .write_bookshelf("default", &vec![book])
            .await
            .unwrap();

        service
            .remove_source_candidates(
                "default",
                &std::collections::HashSet::from(["https://unrelated.test".to_string()]),
            )
            .await
            .unwrap();

        let list = service.read_bookshelf("default").await.unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            list[0].source_candidates.is_some(),
            "无关源不应被清理"
        );
        let _ = tokio::fs::remove_dir_all(&storage_dir).await;
    }

    /// 造一个只用于简介归一化测试的 service。
    fn cover_test_service(tag: &str) -> (BookService, PathBuf) {
        let storage_dir = std::env::temp_dir().join(format!(
            "reader-rust-intro-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        let service = BookService::new(
            HttpClient::new(5, None).unwrap(),
            RuleEngine::new().unwrap(),
            FileCache::new(storage_dir.join("cache")),
            storage_dir.to_str().unwrap(),
        );
        (service, storage_dir)
    }

    #[tokio::test]
    async fn empty_intro_is_normalized_back_to_none_on_save() {
        // 回归测试: 前端「编辑信息 → 还原」会把原始简介整份提交回来, 原始为空时
        // 提交的是 Some("")。若原样落库, `merge_book` 的 `intro.is_none()` 判据
        // 就永久失效 —— 换源后再也补不进新书源的简介。save_book 必须把它收回 None。
        let (service, storage_dir) = cover_test_service("normalize");
        let book = Book {
            book_url: "https://i.example.com/book/1".to_string(),
            name: "书".to_string(),
            author: "作者".to_string(),
            origin: "https://i.example.com".to_string(),
            intro: None,
            ..Book::default()
        };
        service.save_book("default", book.clone()).await.unwrap();

        // 带空简介保存(模拟还原)
        let mut with_empty = book.clone();
        with_empty.intro = Some(String::new());
        service.save_book("default", with_empty).await.unwrap();

        let list = service.read_bookshelf("default").await.unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            list[0].intro.is_none(),
            "空简介应被归一化回 None, 实际: {:?}",
            list[0].intro
        );

        // 非空的简介当然要保留
        let mut with_text = book.clone();
        with_text.intro = Some("真实简介".to_string());
        service.save_book("default", with_text).await.unwrap();
        let list = service.read_bookshelf("default").await.unwrap();
        assert_eq!(list[0].intro.as_deref(), Some("真实简介"));

        let _ = tokio::fs::remove_dir_all(&storage_dir).await;
    }

    #[tokio::test]
    async fn normalized_intro_still_allows_source_backfill() {
        // 归一化的意义所在: 简介保持 None 才允许「书源带回了简介就填进去」。
        // `merge_book`(在 api/commands/book.rs) 的判据就是 `target.intro.is_none()`,
        // 这里不复刻那个函数, 只守住它依赖的不变量 —— 简介落库后必须是 None。
        let (service, storage_dir) = cover_test_service("backfill");
        let book = Book {
            book_url: "https://j.example.com/book/1".to_string(),
            name: "书".to_string(),
            author: "作者".to_string(),
            origin: "https://j.example.com".to_string(),
            intro: Some(String::new()),
            ..Book::default()
        };
        service.save_book("default", book).await.unwrap();

        let saved = service.read_bookshelf("default").await.unwrap();
        assert_eq!(saved.len(), 1);
        // 这条断言就是 merge_book 能回填的前提: 空简介入库后仍是 None。
        // 若哪天归一化被删掉, 这里会变成 Some(""), 回填能力静默失效。
        assert!(
            saved[0].intro.is_none(),
            "空简介入库后必须是 None, 否则「书源简介回填」永久失效; 实际: {:?}",
            saved[0].intro
        );

        let _ = tokio::fs::remove_dir_all(&storage_dir).await;
    }

    #[tokio::test]
    async fn login_sessions_are_random_and_bound_to_one_source() {
        let storage_dir = std::env::temp_dir().join(format!(
            "reader-rust-login-session-{}",
            std::process::id()
        ));
        let service = BookService::new(
            HttpClient::new(5, None).unwrap(),
            RuleEngine::new().unwrap(),
            FileCache::new(storage_dir.join("cache")),
            storage_dir.to_str().unwrap(),
        );
        let first = service
            .create_source_login_session("https://a.example.com")
            .await;
        let second = service
            .create_source_login_session("https://b.example.com")
            .await;

        assert_ne!(first, second);
        assert_eq!(
            service.source_for_login_session(&first).await.as_deref(),
            Some("https://a.example.com")
        );
        assert_eq!(
            service.source_for_login_session(&second).await.as_deref(),
            Some("https://b.example.com")
        );
        assert!(service.source_for_login_session("forged").await.is_none());
    }

    #[test]
    fn same_remote_book_matches_across_sources() {
        let existing = Book {
            // 书名带空格 + 作者带「作者：」前缀: 双重验证归一化匹配
            name: "测 试书目".to_string(),
            author: "作者：测试作者".to_string(),
            book_url: "https://source-a.test/book/1".to_string(),
            origin: "https://source-a.test".to_string(),
            ..Book::default()
        };
        let incoming = Book {
            name: "测试书目".to_string(),
            author: "测试作者".to_string(),
            book_url: "https://source-b.test/book/2".to_string(),
            origin: "https://source-b.test".to_string(),
            ..Book::default()
        };

        assert!(books_match_for_save(&existing, &incoming));
    }

    #[test]
    fn merging_same_book_retains_each_source_candidate() {
        let existing = Book {
            name: "测试书目".to_string(),
            author: "测试作者".to_string(),
            book_url: "https://source-a.test/book/1".to_string(),
            origin: "https://source-a.test".to_string(),
            ..Book::default()
        };
        let mut incoming = Book {
            name: "测试书目".to_string(),
            author: "测试作者".to_string(),
            book_url: "https://source-b.test/book/2".to_string(),
            origin: "https://source-b.test".to_string(),
            ..Book::default()
        };

        merge_book_source_candidates(&existing, &mut incoming);

        let candidates = incoming.source_candidates.expect("source candidates");
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|item| item.origin == "https://source-a.test"));
        assert!(candidates.iter().any(|item| item.origin == "https://source-b.test"));
    }

    #[test]
    fn merging_books_preserves_multiple_addresses_with_the_same_origin() {
        let existing = Book {
            name: "测试书目".to_string(), author: "测试作者".to_string(),
            book_url: "data:;base64,station-a".to_string(), origin: "聚合测试".to_string(),
            ..Book::default()
        };
        let mut incoming = Book { book_url: "data:;base64,station-b".to_string(), ..existing.clone() };
        merge_book_source_candidates(&existing, &mut incoming);
        ensure_current_source_candidate(&mut incoming);
        let candidates = incoming.source_candidates.unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].book_url, existing.book_url);
        assert_eq!(candidates[1].book_url, incoming.book_url);
    }

    #[tokio::test]
    async fn concurrent_source_saves_keep_one_book_and_both_candidates() {
        let storage_dir = std::env::temp_dir().join(format!(
            "reader-rust-bookshelf-source-merge-{}",
            std::process::id()
        ));
        let service = BookService::new(
            HttpClient::new(5, None).unwrap(),
            RuleEngine::new().unwrap(),
            FileCache::new(storage_dir.join("cache")),
            storage_dir.to_str().unwrap(),
        );
        let first_service = service.clone();
        let second_service = service.clone();
        let first = Book {
            name: "测试书目".to_string(),
            author: "测试作者".to_string(),
            book_url: "https://source-a.test/book/1".to_string(),
            origin: "https://source-a.test".to_string(),
            ..Book::default()
        };
        let second = Book {
            name: "测试书目".to_string(),
            author: "测试作者".to_string(),
            book_url: "https://source-b.test/book/2".to_string(),
            origin: "https://source-b.test".to_string(),
            ..Book::default()
        };

        let (first_result, second_result) = tokio::join!(
            first_service.save_book("default", first),
            second_service.save_book("default", second),
        );
        first_result.unwrap();
        second_result.unwrap();

        let books = service.get_bookshelf("default").await.unwrap();
        assert_eq!(books.len(), 1);
        let candidates = books[0]
            .source_candidates
            .as_ref()
            .expect("source candidates");
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|item| item.origin == "https://source-a.test"));
        assert!(candidates.iter().any(|item| item.origin == "https://source-b.test"));

        let _ = tokio::fs::remove_dir_all(&storage_dir).await;
    }

    #[tokio::test]
    async fn window_rate_waits_when_existing_starts_reach_limit() {
        let storage_dir =
            std::env::temp_dir().join(format!("reader-rust-window-rate-{}", std::process::id()));
        let service = BookService::new(
            HttpClient::new(5, None).unwrap(),
            RuleEngine::new().unwrap(),
            FileCache::new(storage_dir.join("cache")),
            storage_dir.to_str().unwrap(),
        );
        let now = Instant::now();
        service.rate_states.write().await.insert(
            "source".to_string(),
            RateState {
                window_starts: vec![now, now],
                ..Default::default()
            },
        );

        let result = tokio::time::timeout(
            Duration::from_millis(20),
            service.wait_for_window_rate("source", 2, 200),
        )
        .await;

        let _ = tokio::fs::remove_dir_all(&storage_dir).await;
        assert!(result.is_err());
    }
}
