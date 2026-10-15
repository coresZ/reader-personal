use axum::{http::HeaderMap, response::Json, routing::get, Router};
use reader_rust::crawler::http_client::HttpClient;
use reader_rust::model::{book_source::BookSource, rule::TocRule};
use reader_rust::parser::rule_engine::RuleEngine;
use reader_rust::service::book_service::BookService;
use reader_rust::storage::cache::file_cache::FileCache;
use serde_json::{json, Value};
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_and_panel_requests_evaluate_headers_with_current_source_state() {
    use reader_rust::parser::js::{eval_js, run_panel_fetch, with_http_context, with_source};
    let app = Router::new().route("/headers", get(|headers: HeaderMap| async move {
        Json(json!({
            "token": headers.get("authorization").and_then(|value| value.to_str().ok()).unwrap_or_default(),
            "ua": headers.get("user-agent").and_then(|value| value.to_str().ok()).unwrap_or_default(),
        }))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let source = BookSource {
        book_source_url: format!("custom://rapid/{}", Uuid::new_v4()),
        header: Some("@js:var v=JSON.parse(source.getVariable()||'{}');JSON.stringify(v.token?{'Authorization':'Bearer '+v.token,'User-Agent':'Legado/3.0'}:{'User-Agent':'Legado/3.0'})".into()),
        ..Default::default()
    };
    let http = HttpClient::new(3, None).unwrap();
    let context = http.source_context("dynamic-headers", &source.book_source_url, None).unwrap();
    let results = tokio::task::spawn_blocking(move || with_http_context(context, || with_source(&source, || {
        let url = json!(format!("{base}/headers"));
        let script = format!(r#"
            var out=[];
            out.push(JSON.parse(java.ajax({url})));
            source.setVariable(JSON.stringify({{token:'first'}}));
            out.push(JSON.parse(java.connect({url}).body()));
            source.setVariable(JSON.stringify({{token:'second'}}));
            out.push(JSON.parse(java.get({url},{{authorization:'explicit'}}).body()));
            JSON.stringify(out)
        "#);
        let script_result = eval_js(&script, "", &base).unwrap();
        let panel_result = run_panel_fetch(&source, &base, "fetch", &json!({"url":"/headers","method":"GET"})).unwrap();
        (serde_json::from_str::<Value>(&script_result).unwrap(), panel_result)
    }))).await.unwrap();
    assert_eq!(results.0, json!([
        {"token":"","ua":"Legado/3.0"},
        {"token":"Bearer first","ua":"Legado/3.0"},
        {"token":"explicit","ua":"Legado/3.0"},
    ]));
    assert_eq!(results.1["code"], 200);
    assert_eq!(serde_json::from_str::<Value>(results.1["body"].as_str().unwrap()).unwrap(), json!({"token":"Bearer second","ua":"Legado/3.0"}));
    server.abort();
}

async fn protected_toc(headers: HeaderMap) -> Json<Value> {
    let has_source_token = headers
        .get("x-source-token")
        .and_then(|value| value.to_str().ok())
        == Some("secret");

    if has_source_token {
        Json(json!({
            "data": {
                "list": [
                    {
                        "chapterName": "第一章",
                        "path": "https://chapters.example/1"
                    }
                ]
            }
        }))
    } else {
        Json(json!({
            "code": 4005,
            "msg": "认证失败",
            "data": {
                "list": []
            }
        }))
    }
}

#[tokio::test]
async fn chapter_list_requests_include_legacy_source_headers() {
    let app = Router::new().route("/toc", get(protected_toc));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let storage_dir =
        std::env::temp_dir().join(format!("reader-rust-header-test-{}", Uuid::new_v4()));
    let cache = FileCache::new(storage_dir.join("cache"));
    let service = BookService::new(
        HttpClient::new(5, None).unwrap(),
        RuleEngine::new().unwrap(),
        cache,
        storage_dir.to_str().unwrap(),
    );

    let source = BookSource {
        book_source_name: "Header protected source".to_string(),
        book_source_url: format!("http://{}", addr),
        header: Some("{'X-Source-Token':'secret'}".to_string()),
        rule_toc: Some(TocRule {
            chapter_list: Some("$.data.list[*]".to_string()),
            chapter_name: Some("$.chapterName".to_string()),
            chapter_url: Some("$.path".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };

    let chapters = service
        .get_chapter_list_with_cache("default", &source, &format!("http://{}/toc", addr), true)
        .await
        .unwrap();

    server.abort();
    let _ = tokio::fs::remove_dir_all(&storage_dir).await;

    assert_eq!(chapters.len(), 1);
    assert_eq!(chapters[0].title, "第一章");
    assert_eq!(chapters[0].url, "https://chapters.example/1");
}
