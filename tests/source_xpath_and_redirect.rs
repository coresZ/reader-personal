// 书源兼容性回归：XPath 规则对真实页面 + `java.get` 的重定向语义。
//
// 三个根因各自独立，都在真实书源上表现为「整块不可用」：
// 1. XPath 求值只接受良构 XML，真实页面直接解析失败 → 发现页/目录 0 条；
// 2. 字段级 XPath 用绝对路径从文档根求值 → 列表里每本书都是第一条的副本；
// 3. `java.get` 自动跟随 302 → 书源读不到 `Location`，两步搜索直接抛空引用。

use axum::{
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use reader_rust::crawler::http_client::HttpClient;
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::html::{html_to_xml, select_xpath};
use reader_rust::parser::rule_engine::RuleEngine;
use reader_rust::service::book_service::BookService;
use reader_rust::storage::cache::file_cache::FileCache;
use serde_json::json;

/// 真实页面形态：`<br>`/`<img>`/`<meta>` 未闭合、属性重复、`<script>` 里有 `&&`。
/// 任何一条都足以让严格 XML 解析器整份拒收。
const MESSY_PAGE: &str = r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>清单</title>
<script>if (a && b) { document.write("<div>"); }</script>
</head><body>
<div class="one-row">
  <div class="col-md-3 home-item"><div class="caption">
    <a href="/novel1/" title="首部"><h3 itemprop="name">首部</h3></a>
    <div class="chi-tiet chuyen-muc"><span class="icon"></span><a>分类甲</a></div>
  </div></div>
  <div class="col-md-3 home-item"><div class="caption">
    <a href="/novel2/" title="次部"><h3 itemprop="name">次部</h3></a>
    <div class="chi-tiet chuyen-muc"><span class="icon"></span><a>分类乙</a></div>
  </div></div>
</div>
<img src="/a.jpg" alt="x" alt="y">
</body></html>"#;

#[test]
fn xpath_rules_work_on_malformed_html() {
    // 直接喂原始 HTML: 严格 XML 解析器必然失败。
    assert!(
        sxd_document::parser::parse(MESSY_PAGE).is_err(),
        "样本必须是非法 XML, 否则这条用例失去意义"
    );
    // 经 HTML5 DOM 转成良构 XML 后能正常求值。
    let xml = html_to_xml(MESSY_PAGE);
    assert!(xml.starts_with("<?xml"));
    assert_eq!(
        select_xpath(MESSY_PAGE, "//div[@class='one-row']/div").len(),
        2
    );
    assert_eq!(
        select_xpath(MESSY_PAGE, "//div[@class='caption']/a/@href"),
        vec!["/novel1/", "/novel2/"]
    );
    // 重复属性只保留第一个, 且不能破坏整份文档。
    assert_eq!(
        select_xpath(MESSY_PAGE, "//img/@alt"),
        vec!["x".to_string()]
    );
}

#[test]
fn xpath_list_fields_are_scoped_to_each_item() {
    // 书源的原样写法: bookList 与所有字段都是 `//…` 绝对路径。
    let source = book_source_from_value(json!({
        "bookSourceUrl": "https://xpath-scope.test",
        "bookSourceName": "XPath 作用域",
        "ruleExplore": {
            "bookList": "//div[@class='one-row']/div[@class='col-md-3 home-item']",
            "name": "//div[@class='caption']/a/@title",
            "author": "//div[@class='caption']/div[@class='chi-tiet chuyen-muc']//text()",
            "bookUrl": "//div[@class='caption']/a/@href",
            "kind": "//div[@class='caption']/h3/@itemprop"
        }
    }))
    .unwrap();
    let engine = RuleEngine::new().unwrap();
    let books = engine.explore_books(&source, MESSY_PAGE, "https://xpath-scope.test/list.html");

    assert_eq!(books.len(), 2, "两条列表项必须是两本书");
    assert_eq!(books[0].name, "首部");
    assert_eq!(books[1].name, "次部");
    assert_eq!(books[0].book_url, "https://xpath-scope.test/novel1/");
    assert_eq!(books[1].book_url, "https://xpath-scope.test/novel2/");
    // `//text()` 命中的首个节点是排版空白, 必须跳过它取到真实文本。
    assert_eq!(books[0].author, "分类甲");
    assert_eq!(books[1].author, "分类乙");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn java_get_keeps_302_location_for_two_step_search() {
    // legado 的 `java.get` 是 `Jsoup.connect(...).followRedirects(false)`,
    // 书源靠 `Location` 里的 searchid 拼第二步地址。
    let router = Router::new()
        .route(
            "/search",
            get(|| async {
                (
                    StatusCode::FOUND,
                    [(header::LOCATION, "result/?searchid=4321")],
                    "",
                )
                    .into_response()
            }),
        )
        .route(
            "/result/",
            get(|| async {
                r#"<div class="item"><a href="/book/1/">重定向之后</a></div>"#.into_response()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let source = book_source_from_value(json!({
        "bookSourceUrl": base,
        "bookSourceName": "重定向",
        "searchUrl": "@js:\nlet res = java.get(baseUrl + '/search', {});\nlet loc = res.header('location');\nbaseUrl + '/' + loc;",
        "ruleSearch": { "bookList": ".item", "name": "a@text", "bookUrl": "a@href" }
    }))
    .unwrap();

    let root = std::env::temp_dir().join(format!("reader-xpath-redirect-{}", uuid::Uuid::new_v4()));
    let service = BookService::new(
        HttpClient::new(10, None).unwrap(),
        RuleEngine::new().unwrap(),
        FileCache::new(root.join("cache")),
        root.to_str().unwrap(),
    );
    let books = service.search_book("test", &source, "任意", 0).await.unwrap();
    assert_eq!(books.len(), 1, "302 的 Location 必须能读到, 否则搜索走不到第二步");
    assert_eq!(books[0].name, "重定向之后");
    server.abort();
}

#[test]
fn explore_json_with_raw_newlines_in_js_url_is_parsed() {
    // 书源把多行 JS 直接写进 JSON 字符串(`{{` 内换行), 严格 JSON 会报
    // "control character found while parsing a string"; GSON 宽松放行。
    let source = book_source_from_value(json!({
        "bookSourceUrl": "https://explore-newline.test",
        "bookSourceName": "多行发现规则",
        "exploreUrl": "[\n{\"title\":\"排行\",\"url\":\"/popular/{{\n\tvar p='index.html';\n\tif(page!=1) p='index_'+page+'.html';\n\tp}}\"},\n{\"title\":\"分类\",\"url\":\"/list/{{page-1}}.html\"}\n]"
    }))
    .unwrap();
    let root = std::env::temp_dir().join(format!("reader-explore-newline-{}", uuid::Uuid::new_v4()));
    let service = BookService::new(
        HttpClient::new(5, None).unwrap(),
        RuleEngine::new().unwrap(),
        FileCache::new(root.join("cache")),
        root.to_str().unwrap(),
    );
    let kinds = service.explore_kinds(&source).unwrap();
    assert_eq!(
        kinds.iter().map(|kind| kind.title.as_str()).collect::<Vec<_>>(),
        ["排行", "分类"]
    );
    let js_url = kinds[0].url.as_deref().unwrap();
    assert!(js_url.contains("var p='index.html'"), "多行 JS 必须原样保留: {js_url:?}");
}
