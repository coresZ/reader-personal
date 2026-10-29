//! TLS 校验策略：与 legado 一致，书源请求不校验证书与主机名。
//!
//! legado `HttpHelper.kt` 用的是 `SSLHelper.unsafeSSLSocketFactory` +
//! `unsafeTrustManager` + `unsafeHostnameVerifier`，即全量跳过校验。书源站点里
//! 自签名证书、过期证书、证书链不全的情况很常见 —— 严格校验会把源直接判死，
//! 表现成「手机端能读、桌面端报错」。
//!
//! 这里起一个**带自签名证书的本地 TLS 服务**，断言 `HttpClient` 能连上；
//! 同时断言「用标准校验的 reqwest 客户端连不上」，确保测试真的在测这件事，
//! 而不是碰巧因为别的原因连通了。
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 生成一张自签名证书（DER）与私钥（PKCS#8 DER）。
///
/// 用 `rcgen` 太重，这里直接内嵌一张固定测试证书 —— 只要求「自签名、不被任何
/// 根信任库收录」，内容是什么无所谓。证书的有效期给到 2099 年，避免测试
/// 因时间推移而失败。
const SELF_SIGNED_CERT_PEM: &str = include_str!("fixtures/self_signed_cert.pem");

/// 自签名证书连接必须成功（因为豁免了校验）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_client_accepts_self_signed_certificate() {
    // 证书缺失时跳过（fixtures 未生成），避免把 CI 卡在证书生成上。
    if SELF_SIGNED_CERT_PEM.trim().is_empty() {
        eprintln!("跳过：缺少自签名证书 fixture");
        return;
    }
    let (url, _hits) = spawn_tls_server().await;

    let client = reader_rust::crawler::http_client::HttpClient::new(10, None)
        .expect("http client")
        .source_context("test", "https://source.example.test", None)
        .expect("context")
        .client;

    let response = client
        .get(&url)
        .send()
        .await
        .expect("书源 client 应接受自签名证书（与 legado 的 unsafeTrustManager 对齐）");
    assert_eq!(response.status(), 200);
}

/// 起一个自签名 TLS 服务，返回（URL，命中计数）。
async fn spawn_tls_server() -> (String, Arc<Mutex<usize>>) {
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use tokio_rustls::rustls::ServerConfig;

    // `rustls-pemfile 1.x`：`certs` 返回 `Result<Vec<Vec<u8>>>`，不是迭代器。
    let certs: Vec<CertificateDer<'static>> = {
        let mut reader = std::io::BufReader::new(SELF_SIGNED_CERT_PEM.as_bytes());
        rustls_pemfile::certs(&mut reader)
            .expect("解析证书")
            .into_iter()
            .map(CertificateDer::from)
            .collect()
    };
    // fixture 是 PKCS#8（"PRIVATE KEY"）。`rustls-pemfile 1.x` 的
    // `pkcs8_private_keys` 同样返回 `Result<Vec<Vec<u8>>>`。
    let key: PrivateKeyDer<'static> = {
        let mut reader = std::io::BufReader::new(SELF_SIGNED_CERT_PEM.as_bytes());
        let mut keys = rustls_pemfile::pkcs8_private_keys(&mut reader).expect("解析私钥");
        let der = keys.remove(0);
        PrivateKeyDer::Pkcs8(tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(der))
    };

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("TLS 配置");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits: Arc<Mutex<usize>> = Arc::default();
    let counter = hits.clone();

    tokio::spawn(async move {
        // 只服务一次就够。
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let Ok(mut tls) = acceptor.accept(stream).await else {
            return;
        };
        let mut buf = vec![0u8; 8192];
        let _ = tls.read(&mut buf).await;
        *counter.lock().unwrap() += 1;
        let _ = tls
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await;
        let _ = tls.flush().await;
    });

    (format!("https://127.0.0.1:{}/probe", addr.port()), hits)
}


/// 对照组：**严格校验**的 reqwest 客户端连同一个服务必须失败。
///
/// 没有这条，上面的测试可能因为「本地回环被特别放行」而假通过 ——
/// 那样就完全没验证到豁免是否生效。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn strict_client_rejects_the_same_self_signed_server() {
    if SELF_SIGNED_CERT_PEM.trim().is_empty() {
        eprintln!("跳过：缺少自签名证书 fixture");
        return;
    }
    let (url, _hits) = spawn_tls_server().await;

    let strict = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .no_proxy()
        .build()
        .expect("strict client");

    let result = strict.get(&url).send().await;
    assert!(
        result.is_err(),
        "严格校验的客户端不应接受自签名证书（否则本测试无意义）"
    );
}
