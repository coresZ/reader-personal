use base64::Engine;
use reader_rust::crawler::http_client::HttpClient;
use reader_rust::model::book::{Book, BookReadConfig};
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js::set_js_storage_dir;
use reader_rust::parser::rule_engine::RuleEngine;
use reader_rust::service::book_service::BookService;
use reader_rust::storage::cache::file_cache::FileCache;
use serde_json::{json, Value};
use std::path::Path;

fn data_url(content: &str) -> String {
    format!("data:;base64,{}", base64::engine::general_purpose::STANDARD.encode(content))
}

fn book_service(storage: &Path) -> BookService {
    BookService::new(
        HttpClient::new(5, None).unwrap(),
        RuleEngine::new().unwrap(),
        FileCache::new(storage.join("cache")),
        storage.to_str().unwrap(),
    )
}

#[test]
fn absent_book_read_config_keeps_the_default_unspecified() {
    for value in [json!({}), json!({"readConfig": null}), json!({"readConfig": {}}), json!({"readConfig": {"useReplaceRule": null}})] {
        let book: Book = serde_json::from_value(value).unwrap();
        assert_eq!(book.read_config.and_then(|config| config.use_replace_rule), None);
    }
}

#[test]
fn book_read_config_round_trips_flags_and_other_legado_settings() {
    for enabled in [false, true] {
        let config = json!({"useReplaceRule": enabled, "imageStyle": "fit", "fontSize": 18, "futureSetting": {"enabled": true}});
        let book: Book = serde_json::from_value(json!({"name": "配置书", "readConfig": config})).unwrap();
        assert_eq!(book.read_config.as_ref().unwrap().use_replace_rule, Some(enabled));
        assert_eq!(serde_json::to_value(book).unwrap()["readConfig"], config);
    }
}

#[tokio::test]
async fn source_read_config_reaches_details_shelf_catalog_and_content() {
    let storage = std::env::temp_dir().join(format!("reader-book-read-config-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&storage).unwrap();
    set_js_storage_dir(&storage);
    let service = book_service(&storage);
    let chapter_url = data_url("<p>正文与段评</p>");
    let toc_url = data_url(&json!({"chapters": [{"title": "第一章", "url": chapter_url}]}).to_string());
    let book_url = data_url(&json!({"name": "配置书", "author": "测试作者", "tocUrl": toc_url}).to_string());
    // A trailing slash exercises the same source URL normalization as saved books.
    let origin = format!("https://read-config-{}.fixture.test/", uuid::Uuid::new_v4());
    let source = book_source_from_value(json!({
        "bookSourceUrl": origin,
        "bookSourceName": "阅读配置测试",
        "ruleBookInfo": {
            "name": "$.name",
            "author": "$.author",
            "intro": "@js:book.setUseReplaceRule(false); '书籍简介'",
            "kind": "@js:book.getUseReplaceRule() ? 'unexpected' : 'compatible'",
            "tocUrl": "$.tocUrl"
        },
        "ruleToc": {
            "chapterList": "$.chapters",
            "chapterName": "@js:result.title + ':' + book.getUseReplaceRule()",
            "chapterUrl": "$.url"
        },
        "ruleContent": {
            "content": "@js:JSON.stringify({enabled:book.getUseReplaceRule(),stored:book.readConfig.useReplaceRule,fontSize:book.readConfig.fontSize,chapter:chapter.title})"
        }
    })).unwrap();
    service.save_book("default", Book {
        name: "配置书".into(),
        author: "测试作者".into(),
        book_url: book_url.clone(),
        origin: origin.clone(),
        read_config: Some(serde_json::from_value::<BookReadConfig>(json!({"fontSize": 18, "imageStyle": "fit"})).unwrap()),
        ..Default::default()
    }).await.unwrap();

    let detail = service.get_book_info("default", &source, &book_url).await.unwrap();
    assert_eq!(detail.name, "配置书");
    assert_eq!(detail.kind.as_deref(), Some("compatible"));
    assert_eq!(serde_json::to_value(&detail).unwrap()["readConfig"], json!({"useReplaceRule": false, "fontSize": 18, "imageStyle": "fit"}));
    service.save_book("default", detail).await.unwrap();

    let shelf_file = storage.join("data/default/bookshelf.json");
    let persisted: Value = serde_json::from_str(&std::fs::read_to_string(&shelf_file).unwrap()).unwrap();
    assert_eq!(persisted[0]["readConfig"]["useReplaceRule"], false);
    let reloaded = book_service(&storage);
    let shelf_book = reloaded.get_shelf_book("default", &book_url).await.unwrap().unwrap();
    assert_eq!(shelf_book.read_config.unwrap().use_replace_rule, Some(false));

    let chapters = reloaded.get_chapter_list("default", &source, &toc_url).await.unwrap();
    assert_eq!(chapters.len(), 1);
    assert_eq!(chapters[0].title, "第一章:false");
    let content = reloaded.get_content("default", &book_url, &source, &chapter_url).await.unwrap();
    assert_eq!(serde_json::from_str::<Value>(&content).unwrap(), json!({"enabled": false, "stored": false, "fontSize": 18, "chapter": "第一章:false"}));

    let (enabled, _, _) = reloaded.eval_source_click(
        "default", &book_url, &source, &chapter_url,
        "book.setUseReplaceRule(true); String(book.getUseReplaceRule())", "",
    ).await.unwrap();
    assert_eq!(enabled, "true");
    let current = reloaded.get_shelf_book("default", &book_url).await.unwrap().unwrap();
    assert_eq!(current.read_config.as_ref().unwrap().use_replace_rule, Some(true));
    let shelf = reloaded.get_bookshelf("default").await.unwrap();
    assert_eq!(shelf[0].read_config.as_ref().unwrap().use_replace_rule, Some(true));
    // 本章正文已缓存, 读取会直接命中本地而不再重跑正文规则; 要让配置变更反映到正文
    // 解析结果里, 必须先让这一章的缓存失效 —— 与阅读页"刷新本章"走同一条路径。
    reloaded.delete_chapter_cache("default", &book_url, &chapter_url).await.unwrap();
    let content = reloaded.get_content("default", &book_url, &source, &chapter_url).await.unwrap();
    assert_eq!(serde_json::from_str::<Value>(&content).unwrap()["enabled"], true);

    // Metadata updates from clients without this setting must retain it and all
    // other per-book settings when writing the ordinary bookshelf file.
    let updated = reloaded.save_book("default", Book { read_config: None, ..current }).await.unwrap();
    assert_eq!(serde_json::to_value(&updated).unwrap()["readConfig"], json!({"useReplaceRule": true, "fontSize": 18, "imageStyle": "fit"}));
    let persisted: Value = serde_json::from_str(&std::fs::read_to_string(&shelf_file).unwrap()).unwrap();
    assert_eq!(persisted[0]["readConfig"]["useReplaceRule"], true);
    let saved = reloaded.save_books("default", vec![updated]).await.unwrap();
    assert_eq!(saved[0].read_config.as_ref().unwrap().use_replace_rule, Some(true));
    tokio::fs::remove_dir_all(&storage).await.unwrap();
}
