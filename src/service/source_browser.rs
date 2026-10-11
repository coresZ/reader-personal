use crate::{
    crawler::{fetcher::{data_uri_response, RequestSpec}, source_session::SourceHttpContext},
    model::book_source::BookSource,
    parser::js::source_http_context,
    service::book_service::{register_source_panel, ShowBrowserRequest, SourcePanelEntry},
};
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, collections::HashMap, sync::{mpsc, Arc, LazyLock, Mutex}, time::Duration};

pub type BrowserHandler = Arc<dyn Fn(ShowBrowserRequest) -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
struct BrowserContext {
    source_url: String,
    http: SourceHttpContext,
    handler: BrowserHandler,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BrowserPage {
    pub url: String,
    pub body: String,
}

thread_local! {
    static BROWSER_CONTEXT: RefCell<Option<BrowserContext>> = const { RefCell::new(None) };
    static BROWSER_CANCELLED: RefCell<bool> = const { RefCell::new(false) };
}

type BrowserReply = Result<BrowserPage, String>;
static PENDING: LazyLock<Mutex<HashMap<String, mpsc::SyncSender<BrowserReply>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn with_browser_handler<T>(
    source: &BookSource,
    handler: Option<BrowserHandler>,
    run: impl FnOnce() -> T,
) -> anyhow::Result<T> {
    let context = handler.map(|handler| {
        Ok::<_, anyhow::Error>(BrowserContext {
            source_url: source.book_source_url.clone(),
            http: source_http_context(&source.book_source_url)?,
            handler,
        })
    }).transpose()?;
    struct Restore(Option<BrowserContext>);
    impl Drop for Restore {
        fn drop(&mut self) {
            BROWSER_CONTEXT.with(|cell| cell.replace(self.0.take()));
        }
    }
    let _restore = Restore(BROWSER_CONTEXT.with(|cell| cell.replace(context)));
    Ok(run())
}

pub fn browser_was_cancelled() -> bool {
    BROWSER_CANCELLED.with(|cell| *cell.borrow())
}

pub fn take_browser_cancelled() -> bool {
    BROWSER_CANCELLED.with(|cell| cell.replace(false))
}

/// Host data: pages on the reader origin so they can use the source bridge.
/// Keep the source's HTTP context even when its identifier is not a URL.
pub fn register_browser_request(
    mut request: ShowBrowserRequest,
    mut entry: SourcePanelEntry,
) -> anyhow::Result<ShowBrowserRequest> {
    entry.url = request.url.clone();
    entry.html = match request.html.as_ref() {
        Some(html) => html.clone(),
        None => data_uri_response(&RequestSpec { url: request.url.clone(), ..Default::default() })?
            .map(|response| response.body).unwrap_or_default(),
    };
    entry.preload_js = request.preload_js.clone();
    request.panel_id = Some(register_source_panel(entry));
    // HTML and preload code are served through /sourcePanel, not duplicated in IPC.
    request.html = None;
    request.preload_js = None;
    if request.url.starts_with("data:") { request.url.clear(); }
    Ok(request)
}

pub fn source_panel_entry(source_url: &str) -> anyhow::Result<SourcePanelEntry> {
    Ok(SourcePanelEntry {
        book_source_url: source_url.to_string(),
        http: Some(source_http_context(source_url)?),
        ..Default::default()
    })
}

/// Suspend only the blocking JS worker. The desktop receives the panel over an
/// IPC channel and confirms/cancels it through a separate request; the original
/// QuickJS stack then continues once, with the user's actual document.
pub fn await_browser(mut request: ShowBrowserRequest) -> BrowserReply {
    let context = BROWSER_CONTEXT.with(|cell| cell.borrow().clone())
        .ok_or_else(|| "此操作需要打开书源登录窗口后确认网页结果".to_string())?;
    request.await_result = true;
    let request = register_browser_request(request, SourcePanelEntry {
        book_source_url: context.source_url,
        http: Some(context.http),
        ..Default::default()
    }).map_err(|err| err.to_string())?;
    let panel_id = request.panel_id.clone().expect("registered panel");
    let (sender, receiver) = mpsc::sync_channel(1);
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).insert(panel_id.clone(), sender);
    struct PendingPanel(String);
    impl Drop for PendingPanel {
        fn drop(&mut self) {
            PENDING.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
            crate::service::book_service::remove_source_panel(&self.0);
        }
    }
    let _pending = PendingPanel(panel_id);
    let outcome = (context.handler)(request).and_then(|_| {
        receiver.recv_timeout(Duration::from_secs(30 * 60))
            .map_err(|_| "网页操作已结束或等待超时".to_string())?
    });
    if outcome.is_err() {
        BROWSER_CANCELLED.with(|cell| cell.replace(true));
    }
    outcome
}

pub fn complete_browser(panel_id: &str, page: BrowserPage) -> Result<(), String> {
    if page.body.trim().is_empty() {
        return Err("网页尚未加载完成，请稍后确认".to_string());
    }
    let sender = PENDING.lock().unwrap_or_else(|e| e.into_inner()).remove(panel_id)
        .ok_or_else(|| "网页操作已结束，请重新打开".to_string())?;
    sender.send(Ok(page)).map_err(|_| "网页操作已结束".to_string())
}

pub fn cancel_browser(panel_id: &str) {
    if let Some(sender) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).remove(panel_id) {
        let _ = sender.send(Err("已取消网页操作".to_string()));
    }
}
