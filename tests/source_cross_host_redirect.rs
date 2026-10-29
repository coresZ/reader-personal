//! 换域跳转不得把「显式 `Host`」带进下一跳。
//!
//! 书源请求为了让 Cloudflare 的托管挑战放行，会显式把 `Host` 放在首位首部
//! （见 `source_request_headers_wire.rs`）。但 reqwest 跟随 3xx 时会**原样沿用**
//! 这个显式 `Host`：跳到新域后发出去的仍是旧域的名字，若对端按 `Host` 又回 301，
//! 就成了死循环，最终 `too many redirects`。
//!
//! 真实表现（一类站点把 `m.xxx.net` 301 到 `m.xxx.com`）：界面显示
//! `internal error: ... too many redirects`，而浏览器/curl 打开同一地址正常。
//!
//! 修复策略是**保留原请求不动，只在撞上这个错误时去掉显式 `Host` 重发一次**
//! （见 `fetcher::fetch_inner`）。因此这里用本地服务模拟那个死循环：
//! 只要请求里带 `Host: 127.0.0.1:port` 就回 301，不带才回 200。
use reader_rust::crawler::fetcher::{fetch_with_client, RequestSpec};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 起一个「按 Host 首部决定跳不跳」的服务，复刻换域站点的行为。
///
/// - 带 `Host: 127.0.0.1:port` → 301 指向 `localhost:port`（换域）
/// - 其他（含不带 Host）→ 200
///
/// 返回（起点 URL，每个请求的 `Host` 首部）。
async fn spawn_host_sensitive_redirect() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let port = addr.port();
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = seen.clone();

    tokio::spawn(async move {
        // 上限设大一些：修复前客户端会在这里转到 reqwest 的跳数上限。
        for _ in 0..40 {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 8192];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            let host = text
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("host:"))
                .map(|line| line.trim()[5..].trim().to_string())
                .unwrap_or_default();
            let ip_literal = format!("127.0.0.1:{port}");
            let loops = host == ip_literal;
            sink.lock().unwrap().push(host);

            let response = if loops {
                format!(
                    "HTTP/1.1 301 Moved Permanently\r\nLocation: http://localhost:{port}/final\r\nContent-Length: 0\r\n\r\n"
                )
            } else {
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()
            };
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;
        }
    });

    (format!("http://127.0.0.1:{port}/start"), seen)
}

/// 撞上换域死循环后，去掉显式 `Host` 重发必须成功。
///
/// 修复前：`fetch_with_client` 直接返回 `too many redirects`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_host_redirect_loop_recovers_without_explicit_host() {
    let (url, seen) = spawn_host_sensitive_redirect().await;
    let client = reader_rust::crawler::http_client::HttpClient::new(10, None)
        .expect("http client")
        .source_context("test", "https://source.example.test", None)
        .expect("context")
        .client;

    let spec = RequestSpec {
        url: url.clone(),
        ..Default::default()
    };
    let response = fetch_with_client(&client, spec)
        .await
        .expect("撞上换域死循环时应回退重发，而不是报 too many redirects");
    assert_eq!(response.status, 200, "回退后应拿到 200");

    // 前若干次都带着显式 Host（因此被 301 打回），最后一次成功的那跳 Host 变成了
    // `localhost` —— 这正是「不再沿用旧域 Host」的证据。
    let hosts = seen.lock().unwrap().clone();
    assert!(
        hosts.iter().any(|h| h.starts_with("127.0.0.1:")),
        "应当先尝试带显式 Host 的请求，实际 {hosts:?}"
    );
    assert!(
        hosts.last().map(|h| h.starts_with("localhost:")).unwrap_or(false),
        "回退后最后一跳的 Host 应跟着换域，实际 {hosts:?}"
    );
}
