use crate::api::commands::{decode_header_file_name, decode_upload_body};
use crate::api::AppState;
use serde::Deserialize;
use serde_json::Value;

use crate::error::error::{ApiResponse, AppError};
use crate::model::rss::{RssArticle, RssSource};
use crate::util::time::now_ts;

#[derive(Debug, Deserialize)]
pub struct RemoteRssSourceParam {
    url: String,
}

#[derive(Debug, Deserialize)]
pub struct RssArticlesRequest {
    #[serde(rename = "sourceUrl")]
    pub source_url: Option<String>,
    #[serde(rename = "sortName")]
    pub sort_name: Option<String>,
    #[serde(rename = "sortUrl")]
    pub sort_url: Option<String>,
    pub page: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct RssContentRequest {
    #[serde(rename = "sourceUrl")]
    pub source_url: Option<String>,
    pub link: Option<String>,
    pub origin: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RssPageRequest {
    #[serde(rename = "sourceUrl")]
    pub source_url: Option<String>,
    /// 要打开的页面地址; 省略时用源自身解析出的地址。
    pub url: Option<String>,
}

/// 为 RSS 源登记一个网页面板, 返回前端 `SourceBrowserPanel` 可直接加载的请求。
///
/// `singleUrl` 源(legado 语义)没有文章列表, 源地址本身就是"一篇文章" —— 点开
/// 应当直接看那个网页。书源的 `java.showBrowser` 面板走 `/sourcePanel` 时要求
/// 面板归属一个**书源**(见 `fetch_source_panel_page`), RSS 源没有书源记录, 会
/// 在抓取阶段 404。
///
/// 因此这里由后端**先把页面抓下来**, 再以静态 HTML 的形式登记面板:
/// `render_source_panel_page` 是纯渲染路径, 不查书源表, 于是 RSS 也能复用同一套
/// 面板(含桥脚本、`<base>` 修复、相对资源改写)。
#[tauri::command]
pub async fn open_rss_source_panel(
    state: tauri::State<'_, AppState>,
    req: RssPageRequest,
) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    let source_url = req
        .source_url
        .ok_or_else(|| AppError::BadRequest("RSS源链接不能为空".to_string()))?;

    let list = read_list::<RssSource>(&state, &user_ns, "rssSources.json").await?;
    let source = list
        .into_iter()
        .find(|s| s.source_url == source_url)
        .ok_or_else(|| AppError::BadRequest("RSS源不存在".to_string()))?;

    // 目标地址: 显式传入优先(前端可能已算好), 否则按源语义解析。
    let target = match req.url.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        Some(url) => url.to_string(),
        None => resolve_single_url(&source).await.0,
    };
    if !looks_like_url(&target) {
        return Err(AppError::BadRequest(
            "该 RSS 源没有可打开的网页地址".to_string(),
        ));
    }

    // 只代理 http(s); 自定义 scheme(如 `app-scheme://`)交给前端另行处理。
    if !target.starts_with("http://") && !target.starts_with("https://") {
        return Ok(ApiResponse::ok(serde_json::json!({
            "url": target,
            "external": true,
        })));
    }

    // 面板会把抓到的页面托管到 `reader` origin 下(同源桥可触达宿主网络), 因此
    // 目标必须是**公网**地址: 拒绝环回/私网/链路本地/保留地址, 避免恶意 RSS 源
    // (导入的远程源 JSON 不受信任)把这里当成内网探测入口。
    reject_non_public_target(&target)?;

    // 抓页面: 用**源级** http 上下文(独立 cookie jar + 源级代理), 与书源规则一致。
    // 不能用全局 `http_client()`: 那会让不同 RSS 源共享 cookie, 且源级代理设置失效。
    let context = crate::parser::js::source_http_context(&source.source_url)
        .map_err(|e| AppError::Internal(e.into()))?;
    let client = context.client.clone();
    let mut request = client.get(&target);
    if let Some(headers) = source.header.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        // 源 header 可能是 JS(如聚合类源), 交给 JS 引擎求值成 JSON 头。
        let raw = headers
            .strip_prefix("@js:")
            .map(str::trim)
            .unwrap_or(headers)
            .to_string();
        let base = target.clone();
        let header_json = tokio::task::spawn_blocking(move || {
            crate::parser::js::eval_js(&raw, "", &base).unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        for (key, value) in crate::crawler::url_analyzer::parse_source_headers(&header_json) {
            request = request.header(key, value);
        }
    }
    let response = request
        .send()
        .await
        .map_err(|e| AppError::BadRequest(format!("打开网页失败: {e}")))?;
    let final_url = response.url().to_string();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::BadRequest(format!("读取网页失败: {e}")))?;
    let html = crate::crawler::fetcher::decode_body(&bytes, None, None);

    let entry = crate::service::book_service::SourcePanelEntry {
        url: target.clone(),
        html: html.clone(),
        // 必须带上源级 http 上下文: 面板会注入 `document.cookie` 劫持脚本, 而
        // `set_panel_cookie` 依赖 `entry.http` 里的 jar 落盘。留空会让它回落到
        // `source_http_context("")` —— 给空键新建会话, cookie 写进垃圾 jar,
        // 面板 origin 的登录态判断与后续请求都拿不到。
        http: Some(context),
        // 面板归属留空: RSS 源不需要书源的 jsLib / 登录桥能力。
        //
        // 二级链接交给系统浏览器: 首屏 HTML 由后端托管(在 reader origin 下渲染,
        // 无 X-Frame-Options 限制), 但页面里的链接指向上游站点, 在 iframe 内直接
        // 导航会被上游的 X-Frame-Options / CSP frame-ancestors 拒绝渲染 → 白屏。
        external_links: true,
        ..Default::default()
    };
    let panel_id = register_rss_panel(&source.source_url, entry);

    Ok(ApiResponse::ok(serde_json::json!({
        "url": final_url,
        "title": source.source_name,
        "panelId": panel_id,
        "external": false,
    })))
}

#[tauri::command]
pub async fn get_rss_sources(
    state: tauri::State<'_, AppState>,
) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    let list = read_list::<RssSource>(&state, &user_ns, "rssSources.json").await?;
    Ok(ApiResponse::ok(
        serde_json::to_value(list).unwrap_or_default(),
    ))
}

#[tauri::command]
pub async fn save_rss_source(
    state: tauri::State<'_, AppState>,
    req: RssSource,
) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    if req.source_url.is_empty() {
        return Err(AppError::BadRequest("RSS链接不能为空".to_string()));
    }
    if req.source_name.is_empty() {
        return Err(AppError::BadRequest("RSS名称不能为空".to_string()));
    }
    let mut list = read_list::<RssSource>(&state, &user_ns, "rssSources.json").await?;
    upsert_by_key(&mut list, req, |s| s.source_url.clone());
    write_list(&state, &user_ns, "rssSources.json", &list).await?;
    Ok(ApiResponse::ok(Value::String("".to_string())))
}

#[tauri::command]
pub async fn save_rss_sources(
    state: tauri::State<'_, AppState>,
    mut req: Vec<RssSource>,
) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    let mut list = read_list::<RssSource>(&state, &user_ns, "rssSources.json").await?;
    req.retain(|s| !s.source_url.is_empty() && !s.source_name.is_empty());
    for s in req {
        upsert_by_key(&mut list, s, |v| v.source_url.clone());
    }
    write_list(&state, &user_ns, "rssSources.json", &list).await?;
    Ok(ApiResponse::ok(Value::String("".to_string())))
}

#[tauri::command]
pub async fn delete_rss_source(
    state: tauri::State<'_, AppState>,
    req: RssSource,
) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    let mut list = read_list::<RssSource>(&state, &user_ns, "rssSources.json").await?;
    list.retain(|s| s.source_url != req.source_url);
    write_list(&state, &user_ns, "rssSources.json", &list).await?;
    Ok(ApiResponse::ok(Value::String("".to_string())))
}

#[tauri::command]
pub async fn delete_rss_sources(
    state: tauri::State<'_, AppState>,
    req: Vec<RssSource>,
) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    let mut list = read_list::<RssSource>(&state, &user_ns, "rssSources.json").await?;
    let deleted = remove_rss_sources_by_url(&mut list, &req);
    write_list(&state, &user_ns, "rssSources.json", &list).await?;
    Ok(ApiResponse::ok(
        serde_json::json!({ "deleted": deleted }),
    ))
}

#[tauri::command]
pub async fn read_remote_rss_source_file(
    state: tauri::State<'_, AppState>,
    req: RemoteRssSourceParam,
) -> Result<ApiResponse<Vec<String>>, AppError> {
    let client = state.book_service.http_client();

    let text = client
        .get(&req.url)
        .send()
        .await
        .map_err(|e| AppError::BadRequest(format!("网络请求失败: {}", e)))?
        .text()
        .await
        .map_err(|e| AppError::BadRequest(format!("读取响应失败: {}", e)))?;

    let sources: Vec<RssSource> = serde_json::from_str(&text).or_else(|_| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            extract_rss_sources(v)
        } else {
            Err(AppError::BadRequest(
                "invalid rss sources json format".to_string(),
            ))
        }
    })?;

    let json_str = serde_json::to_string(&sources)
        .map_err(|e| AppError::BadRequest(format!("序列化RSS源失败: {}", e)))?;

    Ok(ApiResponse::ok(vec![json_str]))
}

#[tauri::command]
pub async fn read_rss_source_file(
    request: tauri::ipc::Request<'_>,
) -> Result<ApiResponse<serde_json::Value>, AppError> {
    // 扩展名判断先于 body 解码：与原实现一致，非法扩展名一律返回
    // "No json file uploaded"，而不是先报 body 形式的错。
    let file_name = decode_header_file_name(&request)?;
    if file_name.ends_with(".json") || file_name.ends_with(".txt") {
        let file = decode_upload_body(request.body())?;
        let text = String::from_utf8_lossy(&file);
        let sources: Vec<RssSource> = serde_json::from_str(&text).or_else(|_| {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                extract_rss_sources(v)
            } else {
                Err(AppError::BadRequest(
                    "invalid rss sources json format".to_string(),
                ))
            }
        })?;
        return Ok(ApiResponse::ok(serde_json::to_value(sources).unwrap_or_default()));
    }
    Err(AppError::BadRequest("No json file uploaded".to_string()))
}

#[tauri::command]
pub async fn get_rss_articles(
    state: tauri::State<'_, AppState>,
    req: RssArticlesRequest,
) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    let source_url = req
        .source_url
        .ok_or_else(|| AppError::BadRequest("RSS源链接不能为空".to_string()))?;
    let sort_url = req.sort_url.unwrap_or_else(|| source_url.clone());
    let sort_name = req.sort_name.unwrap_or_default();
    let page = req.page.unwrap_or(1).max(1);

    let list = read_list::<RssSource>(&state, &user_ns, "rssSources.json").await?;
    let rss_source = list
        .into_iter()
        .find(|s| s.source_url == source_url)
        .ok_or_else(|| AppError::BadRequest("RSS源不存在".to_string()))?;

    // 源地址本身是 JS 表达式时(如
    // `http@js:eval(String(cache.getFromMemory('repo_cache')))`), 它不是 feed
    // 地址, 拿去做 HTTP 请求没有意义 —— 直接走「网页入口」分支。
    if source_url_is_script(&rss_source.source_url) {
        return single_url_entry(&rss_source, &source_url, &sort_name).await;
    }

    // `sortUrl` 可能带 legado 的分类协议前缀(`首页::http://...`)。**必须先剥掉**
    // 再请求: 带前缀的串在请求层就会失败, 那样连下面的"退化网页入口"都走不到,
    // 用户看到的是"列表空 + 报错"而不是可用的网页入口。
    let request_url = split_sort_url_protocol(&sort_url);

    let res = state
        .book_service
        .http_client()
        .get(&request_url)
        .send()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    let bytes = res
        .bytes()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    let feed = match feed_rs::parser::parse(&bytes[..]) {
        Ok(feed) => feed,
        // 解析不出 feed: 对 `singleUrl` 源(legado 语义: 源地址即"一篇文章",
        // 点开直接看网页, 如各类 HTML 入口页)
        // 退化成网页入口, 而不是报错让整个列表空掉。真 feed 源(如「版本」的
        // GitHub atom)解析成功, 不受影响。
        Err(err) => {
            if rss_source.single_url.unwrap_or(false) {
                return single_url_entry(&rss_source, &source_url, &sort_name).await;
            }
            return Err(AppError::BadRequest(err.to_string()));
        }
    };

    let mut items = Vec::new();
    for entry in feed.entries {
        let title = entry
            .title
            .as_ref()
            .map(|t| t.content.clone())
            .unwrap_or_default();
        let link = entry
            .links
            .first()
            .map(|l| l.href.clone())
            .unwrap_or_default();
        let description = entry.summary.as_ref().map(|s| s.content.clone());
        let content = entry.content.as_ref().and_then(|c| c.body.clone());
        let pub_date = entry.published.or(entry.updated).map(|d| d.to_rfc3339());
        let image = entry
            .media
            .first()
            .and_then(|m| m.thumbnails.first())
            .map(|t| t.image.uri.clone());
        let order = entry
            .published
            .or(entry.updated)
            .map(|d| d.timestamp())
            .unwrap_or(now_ts());
        items.push(RssArticle {
            origin: sort_url.clone(),
            sort: sort_name.clone(),
            title,
            order,
            link,
            pub_date,
            description,
            content,
            image,
            read: Some(false),
            variable: None,
        });
    }

    let page_size = 50usize;
    let start = ((page - 1) as usize) * page_size;
    let end = std::cmp::min(start + page_size, items.len());
    let page_items = if start < items.len() {
        items[start..end].to_vec()
    } else {
        Vec::new()
    };
    let data = serde_json::json!({"first": page_items, "second": null});
    Ok(ApiResponse::ok(data))
}

#[tauri::command]
pub async fn get_rss_content(
    state: tauri::State<'_, AppState>,
    req: RssContentRequest,
) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    let source_url = req
        .source_url
        .ok_or_else(|| AppError::BadRequest("RSS链接不能为空".to_string()))?;
    let link = req
        .link
        .ok_or_else(|| AppError::BadRequest("RSS文章链接不能为空".to_string()))?;
    let _origin = req
        .origin
        .ok_or_else(|| AppError::BadRequest("RSS文章来源不能为空".to_string()))?;

    let list = read_list::<RssSource>(&state, &user_ns, "rssSources.json").await?;
    let _rss_source = list
        .into_iter()
        .find(|s| s.source_url == source_url)
        .ok_or_else(|| AppError::BadRequest("RSS源不存在".to_string()))?;

    let res = state
        .book_service
        .http_client()
        .get(&link)
        .send()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    let body = res.text().await.map_err(|e| AppError::Internal(e.into()))?;
    Ok(ApiResponse::ok(Value::String(body)))
}

/// 源地址是否是 JS 表达式(`@js:` / `<js>` / `js:`)而非真实 URL。
///
/// 某类源的地址是 `http@js:eval(String(cache.getFromMemory('repo_cache')))`——
/// 前半截 `http` 只是残壳, 真域名由 `header` 脚本探测后写进内存缓存。
///
/// **先做 URL 短路**: 能被 `Url::parse` 解析且 scheme 是 http(s) 的, 一定是普通
/// 地址。否则 `https://example.com/f?x=@js:1` 这种"参数里恰好含 `@js:`"的真 feed
/// 地址会被误判成脚本, 整个文章列表被替换成一条合成条目。
fn source_url_is_script(source_url: &str) -> bool {
    let trimmed = source_url.trim();
    if is_plain_http_url(trimmed) {
        return false;
    }
    trimmed.starts_with("js:")
        || trimmed.starts_with("@js:")
        || trimmed.starts_with("<js>")
        || trimmed.contains("@js:")
        || trimmed.contains("<js>")
}

/// 是否是可直接请求的普通 http(s) URL。
fn is_plain_http_url(value: &str) -> bool {
    match url::Url::parse(value) {
        Ok(parsed) => matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some(),
        Err(_) => false,
    }
}

/// `singleUrl` 源(legado 语义)没有文章列表: 源地址本身就是"一篇文章"。
///
/// 返回一条合成条目, 让前端按 `link` 打开网页面板。`singleUrl` 标记与
/// `urlEvaluated`(JS 是否求值成功)一并回给前端, 供其决定提示文案。
async fn single_url_entry(
    source: &RssSource,
    source_url: &str,
    sort_name: &str,
) -> Result<ApiResponse<Value>, AppError> {
    let (link, evaluated) = resolve_single_url(source).await;
    let article = RssArticle {
        origin: source.source_name.clone(),
        sort: sort_name.to_string(),
        title: source.source_name.clone(),
        order: now_ts(),
        link,
        pub_date: None,
        description: source.source_comment.clone(),
        content: None,
        image: source.source_icon.clone(),
        read: Some(false),
        variable: Some(source_url.to_string()),
    };
    let data = serde_json::json!({
        "first": serde_json::to_value(vec![article]).unwrap_or_default(),
        "second": null,
        "singleUrl": true,
        "urlEvaluated": evaluated,
    });
    Ok(ApiResponse::ok(data))
}

/// 源地址求值结果的进程内缓存: `源URL -> (解析出的地址, 是否由 JS 求值, 时间戳)`。
///
/// 脚本地址源的求值要跑一遍镜像测速循环(实测 ~18s), 每次进 RSS 页都重跑会明显
/// 拖慢列表。缓存 10 分钟, 兼顾"域名会变"与"不反复测速"。
static SINGLE_URL_CACHE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, (String, bool, std::time::Instant)>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// 同键在途求值的合并槽: 并发请求只跑一次慢求值, 其余等结果。
///
/// 缓存只覆盖**已完成**的结果, 挡不住并发: 快速切源/连点刷新会同时发起多个
/// `get_rss_articles`, 各自跑一遍 18 秒镜像测速, 白占多个阻塞线程。
static SINGLE_URL_INFLIGHT: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, tokio::sync::watch::Receiver<Option<(String, bool)>>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

const SINGLE_URL_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// legado 的分类协议: `名称::地址`。取 `::` 之后的地址部分。
///
/// 多分类时 legado 允许换行或 `&&` 分隔, 这里取第一个非空段(单 URL 源只需要一个
/// 落点)。没有 `::` 时原样返回。
fn split_sort_url_protocol(raw: &str) -> String {
    let first = raw
        .split(['\n', '\r'])
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(raw)
        .split("&&")
        .map(str::trim)
        .find(|part| !part.is_empty())
        .unwrap_or(raw);
    first
        .rsplit_once("::")
        .map(|(_, url)| url.trim())
        .filter(|url| !url.is_empty())
        .unwrap_or(first)
        .to_string()
}

/// `singleUrl` 源要打开的落点。
///
/// legado 语义是**用 `sortUrl`**(分类协议取 `::` 后半段)当文章地址, 而不是
/// `sourceUrl`: 例如某源的 `sortUrl` 是 `首页::http://example.test/list.html`,
/// 取出的地址才是要打开的那一页。
/// `sortUrl` 为空时退回 `sourceUrl`。
fn single_url_target(source: &RssSource, raw: &str) -> String {
    source
        .sort_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(split_sort_url_protocol)
        .unwrap_or_else(|| raw.to_string())
}

/// 解析 `singleUrl` 源要打开的地址。
///
/// 源地址是 JS 表达式时(`@js:`)在真实 JS 引擎里求值 —— 这是 legado 的语义,
/// 例如聚合类源靠它读出 `header` 脚本探测到的可用镜像域名。求值失败或结果
/// 不像 URL 时退回 `sortUrl`, 再退回原串, 保证入口始终可点。
///
/// 求值可能很慢(镜像测速), 因此丢到阻塞线程执行, 不占用异步调度线程。
///
/// 返回 `(最终地址, 是否由 JS 求值得到)`。
async fn resolve_single_url(source: &RssSource) -> (String, bool) {
    let raw = source.source_url.trim().to_string();
    // 兜底: `sortUrl` 走了分类协议, 取出其中真正的地址。
    let fallback = || {
        let target = single_url_target(source, &raw);
        if target.is_empty() {
            raw.clone()
        } else {
            target
        }
    };

    if !source_url_is_script(&raw) {
        // 非脚本地址: legado 同样以 `sortUrl` 为准(它可能带 `名称::` 前缀)。
        let target = fallback();
        return (target, false);
    }

    if let Some(cached) = single_url_cache_get(&raw) {
        return cached;
    }

    let script = extract_js_script(&raw);
    if script.trim().is_empty() {
        return (fallback(), false);
    }

    // 同键已有在途求值时直接等它, 不重复跑镜像测速。
    if let Some(mut receiver) = single_url_inflight_join(&raw) {
        // 发送端被丢弃(watch 值未变)时按未命中处理, 下面会重新求值。
        if receiver.changed().await.is_ok() {
            if let Some(resolved) = receiver.borrow().clone() {
                return resolved;
            }
        }
    }
    let (sender, receiver) = tokio::sync::watch::channel::<Option<(String, bool)>>(None);
    single_url_inflight_register(&raw, receiver);

    let resolved = evaluate_single_url(source, &raw, &fallback, &script).await;

    // 广播结果并清除在途标记(放在广播之前, 让后来的请求走缓存而不是再等)。
    if resolved.1 {
        single_url_cache_put(&raw, resolved.clone());
    }
    let _ = sender.send(Some(resolved.clone()));
    single_url_inflight_clear(&raw);
    resolved
}

/// 同一键的求值主体: 先跑 `header`(它可能把待执行脚本写进内存缓存), 再求值。
async fn evaluate_single_url(
    source: &RssSource,
    raw: &str,
    fallback: &impl Fn() -> String,
    script: &str,
) -> (String, bool) {
    // 源级 header 脚本可能是"把待执行脚本写进内存缓存"的形式(某类源正是如此:
    // header 把一段 JS 源码 putMemory 成一个键, sourceUrl 再 eval 它)。
    // 所以求值前必须先跑一次 header, 否则内存缓存是空的。
    let header = source.header.clone().unwrap_or_default();
    let base_url = raw.to_string();
    let script = script.to_string();
    let evaluated = tokio::task::spawn_blocking(move || {
        if !header.trim().is_empty() {
            let header_script = header
                .trim()
                .strip_prefix("@js:")
                .map(str::trim)
                .unwrap_or(header.trim());
            // header 执行失败不致命(部分源只是设置请求头, 与地址无关), 但要留痕:
            // 否则现场只剩"求值失败"一条 warn, 分不清是 header 还是求值本身的问题。
            if let Err(err) = crate::parser::js::eval_js(header_script, "", &base_url) {
                tracing::warn!("RSS singleUrl header 脚本执行失败({base_url}): {err}");
            }
        }
        crate::parser::js::eval_js(&script, "", &base_url)
    })
    .await
    .unwrap_or_else(|err| Err(anyhow::anyhow!("{err}")));

    match evaluated {
        Ok(value) => {
            let value = value.trim().to_string();
            if looks_like_url(&value) {
                (value, true)
            } else {
                // 求值成功但结果不是 URL(内存缓存里还没有对应键时会是
                // `http` 这类残壳), 按求值失败处理。
                (fallback(), false)
            }
        }
        Err(err) => {
            tracing::warn!("RSS singleUrl 求值失败({raw}): {err}");
            (fallback(), false)
        }
    }
}

/// 加入同键在途求值; 没有在途请求时返回 `None`。
fn single_url_inflight_join(
    key: &str,
) -> Option<tokio::sync::watch::Receiver<Option<(String, bool)>>> {
    let inflight = SINGLE_URL_INFLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    inflight.get(key).cloned()
}

fn single_url_inflight_register(
    key: &str,
    receiver: tokio::sync::watch::Receiver<Option<(String, bool)>>,
) {
    let mut inflight = SINGLE_URL_INFLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    inflight.insert(key.to_string(), receiver);
}

fn single_url_inflight_clear(key: &str) {
    let mut inflight = SINGLE_URL_INFLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    inflight.remove(key);
}

fn single_url_cache_get(key: &str) -> Option<(String, bool)> {
    let cache = SINGLE_URL_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let (value, evaluated, at) = cache.get(key)?;
    if at.elapsed() < SINGLE_URL_CACHE_TTL {
        Some((value.clone(), *evaluated))
    } else {
        None
    }
}

fn single_url_cache_put(key: &str, value: (String, bool)) {
    let mut cache = SINGLE_URL_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.insert(key.to_string(), (value.0, value.1, std::time::Instant::now()));
}

/// 从源地址里取出待执行的 JS 片段。
fn extract_js_script(raw: &str) -> String {
    if let Some((_, rest)) = raw.split_once("<js>") {
        return rest.trim_end_matches("</js>").trim().to_string();
    }
    if let Some((_, rest)) = raw.split_once("@js:") {
        return rest.trim().to_string();
    }
    raw.strip_prefix("js:").unwrap_or(raw).trim().to_string()
}

/// 是否是可用于打开面板的地址(`http`/`https`, 或 legado 自定义 scheme)。
fn looks_like_url(value: &str) -> bool {
    if value.is_empty() || value.eq_ignore_ascii_case("http") || value.eq_ignore_ascii_case("https") {
        return false;
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        return true;
    }
    // 自定义 scheme(如 `app-scheme://`)交给系统/WebView 处理。
    value
        .split_once("://")
        .map(|(scheme, rest)| !scheme.is_empty() && !rest.is_empty())
        .unwrap_or(false)
}

/// RSS 网页面板自己的登记表: `(源地址, 面板id)`。
///
/// 书源面板与 RSS 面板共用 `register_source_panel` 的 8 个槽位, 而该函数在满了
/// 之后会淘汰最旧的**并 `cancel_browser`** —— 若 RSS 面板无限累积, 反复切换
/// 单 URL 源就会挤掉书源正在等待的登录面板(登录流程被"已取消网页操作"打断)。
///
/// 因此 RSS 侧只保留最近 `MAX_RSS_PANELS` 个, 且只移除**自己登记的**面板
/// (用 `remove_source_panel`, 不触发 cancel, 因为 RSS 面板从不需要 await)。
static RSS_PANELS: std::sync::LazyLock<
    std::sync::Mutex<Vec<(String, String)>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(Vec::new()));

/// RSS 面板槽位数: 当前正在看的网页最多 1 个, 留 1 个余量给切换瞬间。
const MAX_RSS_PANELS: usize = 2;

/// 登记 RSS 网页面板, 同源复用并限制总量。
fn register_rss_panel(
    source_url: &str,
    entry: crate::service::book_service::SourcePanelEntry,
) -> String {
    let mut panels = RSS_PANELS.lock().unwrap_or_else(|e| e.into_inner());
    // 同源复用: 先撤掉该源之前的旧面板, 避免同一源堆积。
    panels.retain(|(url, id)| {
        if url == source_url {
            crate::service::book_service::remove_source_panel(id);
            false
        } else {
            true
        }
    });
    // 超出上限时撤掉最旧的(只动自己登记的面板)。
    while panels.len() >= MAX_RSS_PANELS {
        let (_, id) = panels.remove(0);
        crate::service::book_service::remove_source_panel(&id);
    }
    let id = crate::service::book_service::register_source_panel(entry);
    panels.push((source_url.to_string(), id.clone()));
    id
}

/// 拒绝把面板目标指向内网/本机地址。
///
/// 面板 HTML 会被托管到 `reader` origin 下, 而该 origin 的桥脚本能触达宿主网络与
/// 脚本能力。RSS 源可从远程 JSON 导入(不受信任), 若不设限, 恶意源即可把这里当成
/// 内网探测器(如 `http://127.0.0.1:8080/`, `http://169.254.169.254/` 云元数据)。
///
/// 只做**字面量**判定: 主机名是 IP 时检查其网段。不做 DNS 解析(解析后再连会引入
/// TOCTOU, 且本地单机应用的用户本就对自己的网络有完全控制权 —— 这里的目标是拦住
/// "导入的源偷偷探测内网", 而非防御本机用户)。
pub(crate) fn reject_non_public_target(target: &str) -> Result<(), AppError> {
    let parsed = url::Url::parse(target)
        .map_err(|e| AppError::BadRequest(format!("网页地址无效: {e}")))?;
    let Some(host) = parsed.host_str() else {
        return Err(AppError::BadRequest("网页地址缺少主机名".to_string()));
    };
    // 去掉 IPv6 字面量的方括号。
    let host = host.trim_start_matches('[').trim_end_matches(']');

    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(AppError::BadRequest(
            "出于安全考虑, 不能打开本机地址".to_string(),
        ));
    }

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if is_non_public_ip(&ip) {
            return Err(AppError::BadRequest(
                "出于安全考虑, 不能打开内网地址".to_string(),
            ));
        }
    }
    Ok(())
}

/// 是否是环回/私网/链路本地/保留等**非公网**地址。
fn is_non_public_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()          // 169.254.0.0/16, 含云元数据 169.254.169.254
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                // 100.64.0.0/10(CGNAT)与 0.0.0.0/8 也不该作为面板目标。
                || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
                || v4.octets()[0] == 0
        }
        // IPv6: 环回、未指定、唯一本地(fc00::/7)、链路本地(fe80::/10)、组播。
        // 另外拒掉 IPv4 映射地址(::ffff:a.b.c.d), 否则可用它绕过上面的 IPv4 判定。
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6.to_ipv4_mapped().map(|v4| is_non_public_ip(&std::net::IpAddr::V4(v4))).unwrap_or(false)
        }
    }
}

async fn read_list<T: for<'de> serde::Deserialize<'de>>(
    state: &AppState,
    user_ns: &str,
    name: &str,
) -> Result<Vec<T>, AppError> {
    state.json_document_service.read_list(user_ns, name).await
}

async fn write_list<T: serde::Serialize>(
    state: &AppState,
    user_ns: &str,
    name: &str,
    list: &Vec<T>,
) -> Result<(), AppError> {
    state
        .json_document_service
        .write_list(user_ns, name, list)
        .await
}

fn extract_rss_sources(payload: serde_json::Value) -> Result<Vec<RssSource>, AppError> {
    if payload.is_array() {
        return serde_json::from_value::<Vec<RssSource>>(payload)
            .map_err(|e| AppError::BadRequest(e.to_string()));
    }
    if let Some(obj) = payload.as_object() {
        for key in ["rssSources", "rssSourceList", "data", "sources"] {
            if let Some(v) = obj.get(key) {
                if v.is_array() {
                    return serde_json::from_value::<Vec<RssSource>>(v.clone())
                        .map_err(|e| AppError::BadRequest(e.to_string()));
                }
            }
        }
    }
    Err(AppError::BadRequest(
        "invalid rss sources payload".to_string(),
    ))
}

fn upsert_by_key<T, F>(list: &mut Vec<T>, item: T, key_fn: F)
where
    F: Fn(&T) -> String,
{
    let key = key_fn(&item);
    if let Some(pos) = list.iter().position(|v| key_fn(v) == key) {
        list[pos] = item;
    } else {
        list.push(item);
    }
}

fn remove_rss_sources_by_url(list: &mut Vec<RssSource>, targets: &[RssSource]) -> usize {
    let before = list.len();
    let target_urls = targets
        .iter()
        .filter(|source| !source.source_url.is_empty())
        .map(|source| source.source_url.as_str())
        .collect::<std::collections::HashSet<_>>();
    list.retain(|source| !target_urls.contains(source.source_url.as_str()));
    before.saturating_sub(list.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rss_source(name: &str, url: &str) -> RssSource {
        RssSource {
            source_name: name.to_string(),
            source_url: url.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn remove_rss_sources_by_url_keeps_unmatched_sources() {
        let mut list = vec![
            rss_source("News", "https://news.example"),
            rss_source("Tech", "https://tech.example"),
            rss_source("Blog", "https://blog.example"),
        ];
        let targets = vec![
            rss_source("Ignored Name", "https://tech.example"),
            rss_source("Missing", "https://missing.example"),
        ];

        let deleted = remove_rss_sources_by_url(&mut list, &targets);

        assert_eq!(deleted, 1);
        assert_eq!(
            list.iter()
                .map(|source| source.source_url.as_str())
                .collect::<Vec<_>>(),
            vec!["https://news.example", "https://blog.example"]
        );
    }

    #[test]
    fn source_url_is_script_detects_js_expressions() {
        // 某类源的真实形态: 前面 `http` 是残壳, 真地址靠 JS 求值。
        assert!(source_url_is_script(
            "http@js:eval(String(cache.getFromMemory('repo_cache')))"
        ));
        assert!(source_url_is_script("@js:'https://a.example'"));
        assert!(source_url_is_script("<js>'https://a.example'</js>"));
        assert!(source_url_is_script("js:'https://a.example'"));

        // 真实 URL 不能被误判成脚本, 否则正常的 feed 源会被当成网页入口。
        assert!(!source_url_is_script("https://feeds.example.com/releases.atom"));
        assert!(!source_url_is_script("https://pan.example.net"));
        assert!(!source_url_is_script("http://entry.example.net/list.html"));
    }

    /// `@js:` 子串出现在**真 URL 的参数或路径**里时, 不能被判成脚本地址。
    ///
    /// 否则 `https://example.com/feed?x=@js:1` 这类地址会让整个文章列表被替换成
    /// 一条合成条目(单 URL 入口), 代价极低、后果明显。
    #[test]
    fn plain_http_url_short_circuits_script_detection() {
        assert!(!source_url_is_script("https://example.com/feed?x=@js:1"));
        assert!(!source_url_is_script("https://example.com/path/<js>/feed.xml"));
        assert!(is_plain_http_url("https://example.com/feed?x=@js:1"));

        // 该形态的地址解析不出 http(s) scheme, 仍必须判为脚本。
        assert!(source_url_is_script(
            "http@js:eval(String(cache.getFromMemory('repo_cache')))"
        ));
        assert!(!is_plain_http_url(
            "http@js:eval(String(cache.getFromMemory('repo_cache')))"
        ));
        // `js:xxx` 的 scheme 是 `js`, 不是 http(s), 也不能短路。
        assert!(source_url_is_script("js:'https://a.example'"));
        assert!(!is_plain_http_url("js:'https://a.example'"));
    }

    #[test]
    fn extract_js_script_drops_leading_shell() {
        assert_eq!(
            extract_js_script("http@js:eval(String(cache.getFromMemory('repo_cache')))"),
            "eval(String(cache.getFromMemory('repo_cache')))"
        );
        assert_eq!(extract_js_script("@js:  'https://a.example'"), "'https://a.example'");
        assert_eq!(
            extract_js_script("<js>'https://a.example'</js>"),
            "'https://a.example'"
        );
    }

    #[test]
    fn looks_like_url_rejects_bare_scheme_shell() {
        // 内存缓存里没有对应键时, 表达式会求值成裸 `http` —— 这不能当地址打开。
        assert!(!looks_like_url("http"));
        assert!(!looks_like_url("https"));
        assert!(!looks_like_url(""));
        assert!(looks_like_url("https://mirror.example.vip"));
        assert!(looks_like_url("http://entry.example.net/list.html"));
        // legado 自定义 scheme 也要放行。
        assert!(looks_like_url("app-scheme://user/profile/123456"));
    }

    #[test]
    fn resolve_single_url_keeps_plain_url_untouched() {
        // 真 feed 源(如 atom)不是脚本地址: 分流函数必须原样放行。
        let source = rss_source("版本", "https://feeds.example.com/releases.atom");
        assert!(
            !source_url_is_script(&source.source_url),
            "真 feed 源不能被判成脚本地址, 否则会被当成网页入口、丢失文章列表"
        );
        assert!(looks_like_url(&source.source_url));
    }

    /// `resolve_single_url` 是异步的(求值要走阻塞线程, 可能耗时十几秒), 单测只验证
    /// 其中**不涉及网络**的判定分支: 脚本提取、地址合法性与 `sortUrl` 协议。
    /// 真实求值链路由 `tests/rss_repo_source_live.rs` 端到端覆盖。
    #[test]
    fn single_url_fallback_rules() {
        // 求值结果是裸 scheme 残壳(内存缓存还没有对应键时的真实表现): 不能当地址。
        assert!(!looks_like_url("http"));
        // 正常镜像地址可用。
        assert!(looks_like_url("https://mirror.example.com"));

        // 脚本提取: 前缀残壳 `http` 必须被丢掉, 否则整串交给 JS 是语法错误。
        assert_eq!(
            extract_js_script("http@js:eval(String(cache.getFromMemory('repo_cache')))"),
            "eval(String(cache.getFromMemory('repo_cache')))"
        );
    }

    /// legado 的分类协议 `名称::地址`: 请求与打开网页前都必须剥掉 `名称::` 前缀。
    ///
    /// 某类源的 `sortUrl` 是 `首页::http://entry.example.net/list.html`,
    /// 带前缀直接发请求会在请求层失败, 导致连"退化成网页入口"都走不到。
    #[test]
    fn split_sort_url_protocol_strips_category_prefix() {
        assert_eq!(
            split_sort_url_protocol("首页::http://entry.example.net/list.html"),
            "http://entry.example.net/list.html"
        );
        // 无协议前缀时原样返回。
        assert_eq!(
            split_sort_url_protocol("https://example.test/feed.xml"),
            "https://example.test/feed.xml"
        );
        // 多分类(换行分隔)取第一个可用地址。
        assert_eq!(
            split_sort_url_protocol("首页::https://a.test/\n更新::https://b.test/"),
            "https://a.test/"
        );
        // `&&` 分隔同样支持。
        assert_eq!(
            split_sort_url_protocol("首页::https://a.test/&&更新::https://b.test/"),
            "https://a.test/"
        );
        // 前缀为空( `::url` )也要取到地址。
        assert_eq!(split_sort_url_protocol("::https://c.test/x"), "https://c.test/x");
    }

    /// `singleUrl` 源的落点以 `sortUrl` 为准(legado 语义), 为空时才退回 `sourceUrl`。
    #[test]
    fn single_url_target_prefers_sort_url() {
        let mut source = rss_source("入口源", "http://entry.example.net/list.html");
        source.sort_url = Some("首页::http://entry.example.net/list.html".to_string());
        assert_eq!(
            single_url_target(&source, &source.source_url),
            "http://entry.example.net/list.html"
        );

        // 没有 sortUrl 的源退回 sourceUrl。
        let repo = rss_source("脚本源", "http@js:eval(String(cache.getFromMemory('repo_cache')))");
        assert_eq!(
            single_url_target(&repo, &repo.source_url),
            "http@js:eval(String(cache.getFromMemory('repo_cache')))"
        );

        // sortUrl 是空串时同样退回。
        let mut blank = rss_source("空 sortUrl", "https://blank.test/");
        blank.sort_url = Some("   ".to_string());
        assert_eq!(single_url_target(&blank, &blank.source_url), "https://blank.test/");
    }

    /// 面板目标必须拒绝内网/本机地址: 面板 HTML 托管在 `reader` origin 下,
    /// 其桥脚本可触达宿主网络。RSS 源可从远程 JSON 导入(不受信任), 不设限就成了
    /// 内网探测入口。
    #[test]
    fn panel_target_rejects_non_public_addresses() {
        // 必须拒绝: 环回、私网、链路本地(含云元数据)、CGNAT、未指定。
        for denied in [
            "http://127.0.0.1:8080/",
            "http://127.1.2.3/",
            "https://localhost/admin",
            "https://foo.localhost/",
            "http://10.0.0.1/",
            "http://192.168.1.1/",
            "http://172.16.5.5/",
            "http://169.254.169.254/latest/meta-data/",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fe80::1]/",
            "http://[fd00::1]/",
            "http://[::ffff:127.0.0.1]/",
        ] {
            assert!(
                reject_non_public_target(denied).is_err(),
                "应拒绝内网/本机地址: {denied}"
            );
        }

        // 必须放行: 公网地址。
        for allowed in [
            "https://mirror.example.com/",
            "https://feeds.example.com/releases.atom",
            "http://entry.example.net/list.html",
            "https://8.8.8.8/dns",
        ] {
            assert!(
                reject_non_public_target(allowed).is_ok(),
                "应放行公网地址: {allowed}"
            );
        }

        // 非法地址要报错而不是放行。
        assert!(reject_non_public_target("not a url").is_err());
        assert!(reject_non_public_target("file:///etc/passwd").is_err());
    }
}
