use reader_rust::crawler::http_client::HttpClient;
use reader_rust::crawler::url_analyzer::analyze_url;
use reader_rust::model::book_source::{book_source_from_value, BookSource};
use reader_rust::parser::js::{book_read_config_override, eval_js, take_js_logs, with_book_context, with_http_context, with_source, JsBookContext};
use reader_rust::parser::rule_engine::RuleEngine;
use serde_json::{json, Value};

#[test]
fn book_replace_rule_methods_update_read_config() {
    let context = JsBookContext {
        book: json!({"bookUrl":"https://fixture.test/book/comments","readConfig":{"fontSize":18}}),
        chapter: json!({}),
    };
    let result = with_book_context(Some(context), || eval_js(
        "book.setUseReplaceRule(false); JSON.stringify({enabled:book.getUseReplaceRule(),stored:book.readConfig.useReplaceRule,fontSize:book.readConfig.fontSize})",
        "", "https://fixture.test/book/comments",
    )).expect("Legado book reading configuration methods must be callable");
    let result: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(result, json!({"enabled":false,"stored":false,"fontSize":18}));
}

#[test]
fn java_get_cookie_accepts_domain_tags_and_named_values() {
    let http = HttpClient::new(2, None).unwrap();
    let source = BookSource { book_source_url: format!("https://comments-{}.test", uuid::Uuid::new_v4()), ..Default::default() };
    let context = http.source_context("comment-fixture", &source.book_source_url, None).unwrap();
    context.session.cookies.set(&url::Url::parse("https://theta.example.com/").unwrap(), "sessionid=fixture==; other=kept").unwrap();
    let result = with_http_context(context, || with_source(&source, || eval_js(
        "JSON.stringify({named:java.getCookie('theta.example.com','sessionid'),missing:java.getCookie('theta.example.com','missing'),full:java.getCookie('https://theta.example.com/'),nullKey:java.getCookie('theta.example.com',null)})",
        "", "https://fixture.test/book/comments",
    ))).expect("The cookie helper used by the original aggregate chapter rule must be callable");
    let result: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(result["named"], "fixture==");
    assert_eq!(result["missing"], "");
    assert!(result["full"].as_str().unwrap().contains("sessionid=fixture=="));
    assert!(result["full"].as_str().unwrap().contains("other=kept"));
    assert_eq!(result["full"], result["nullKey"]);
    let other = BookSource { book_source_url: format!("https://other-comments-{}.test", uuid::Uuid::new_v4()), ..Default::default() };
    let context = http.source_context("comment-fixture", &other.book_source_url, None).unwrap();
    let cookie = with_http_context(context, || with_source(&other, || eval_js("java.getCookie('theta.example.com','sessionid')", "", ""))).unwrap();
    assert!(cookie.is_empty());
}

#[test]
fn aggregate_catalog_generates_review_capable_chapter_urls() {
    // Same rule as the supplied aggregate source; all identifiers and cookies
    // are local fixtures, and no upstream request is made.
    let chapter_url_rule = r#"<js>
let { tab, source: sources, title, item_id } = result;
let book_id = java.get('book_id');
let base_url = BaseUrl();
let url = result.toc_url || "";

let gycontent = { book_id, item_id, title, sources, tab, url };
let encodedGycontent = java.base64Encode(JSON.stringify(gycontent));

let content_url;

if (sources == '卷') {
    content_url = item_id;
} else if (['源甲', '源乙', '源丁'].includes(sources) && tab == "小说") {
    const reviewSession = java.getCookie('theta.example.com', 'sessionid');
    const sourcess = sources.replace('svip_', '');
    content_url = `data:;base64,${encodedGycontent},${JSON.stringify({
        type: "gycontent",
        js: `book ? result : '${base_url}/get_review?book_id=${book_id}&item_id=${item_id}&ssionid=${reviewSession}&source=${sourcess}'`
    })}`;
} else {
    content_url = `data:;base64,${encodedGycontent},${JSON.stringify({ type: "gycontent" })}`;
}
</js>"#;
    let source = book_source_from_value(json!({
        "bookSourceUrl":format!("https://catalog-comments-{}.test",uuid::Uuid::new_v4()),
        "bookSourceName":"Comment URL fixture",
        "jsLib":"function BaseUrl(){return 'https://reviews.fixture.test';}",
        "ruleToc":{"chapterList":"$.data","chapterName":"$.title","chapterUrl":chapter_url_rule}
    })).unwrap();
    let http = HttpClient::new(2, None).unwrap();
    let context = http.source_context("comment-fixture", &source.book_source_url, None).unwrap();
    context.session.cookies.set(&url::Url::parse("https://theta.example.com/").unwrap(), "sessionid=fixture-session").unwrap();
    with_http_context(context, || with_source(&source, || {
        eval_js("java.put('book_id','fixture-book')", "", "https://fixture.test/toc").unwrap();
        take_js_logs();
        let body = json!({"data":[{"title":"测试章节","item_id":"fixture-chapter","source":"源甲","tab":"小说"}]}).to_string();
        let (chapters, _) = RuleEngine::new().unwrap().chapter_list(&source, &body, "https://fixture.test/toc", None);
        assert_eq!(chapters.len(), 1);
        let chapter_url = &chapters[0].url;
        assert!(chapter_url.starts_with("data:;base64,"), "chapter URL must retain review options; logs: {:?}", take_js_logs());
        assert!(chapter_url.contains("/get_review?book_id=fixture-book&item_id=fixture-chapter&ssionid=fixture-session&source=源甲"));
        let spec = analyze_url(chapter_url, "", 1, &source.book_source_url, &source).unwrap();
        assert_eq!(spec.response_type.as_deref(), Some("gycontent"));
    }));
}


#[test]
fn rule_variables_are_scoped_and_do_not_issue_http_requests() {
    let source = BookSource { book_source_url: format!("https://variables-{}.test",uuid::Uuid::new_v4()), ..Default::default() };
    let context = JsBookContext {
        book: json!({"bookUrl":"https://fixture.test/book/variables","name":"书籍变量测试"}),
        chapter: json!({}),
    };
    with_source(&source, || with_book_context(Some(context.clone()), || {
        let value = eval_js(
            "globalThis.__http=function(){throw new Error('variable operation attempted HTTP')}; var saved=java.put('book_id','fixture-book'); JSON.stringify({saved:saved,value:java.get('book_id'),bookName:java.get('bookName'),missing:java.get('absent')})",
            "", "https://fixture.test/book/variables",
        ).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&value).unwrap(),json!({"saved":"fixture-book","value":"fixture-book","bookName":"书籍变量测试","missing":""}));
        let chapter_context = JsBookContext { chapter:json!({"url":"https://fixture.test/chapter/variables","title":"章节变量测试"}), ..context.clone() };
        with_book_context(Some(chapter_context), || {
            let value = eval_js("globalThis.__http=function(){throw new Error('variable operation attempted HTTP')}; var inherited=java.get('book_id'); java.put('book_id','fixture-chapter'); JSON.stringify([inherited,java.get('book_id'),java.get('title')])","","").unwrap();
            assert_eq!(serde_json::from_str::<Value>(&value).unwrap(),json!(["fixture-book","fixture-chapter","章节变量测试"]));
        });
        assert_eq!(eval_js("java.get('book_id')","","").unwrap(),"fixture-book");
    }));
}

#[test]
fn explicit_http_get_and_put_overloads_are_preserved() {
    let result=eval_js(r#"
        var calls=[];
        globalThis.__http=function(method,url,body,headers){calls.push([method,url,body,JSON.parse(headers)]);return JSON.stringify({body:'fixture response',code:200,url:url,headers:{}})};
        var a=java.get('https://fixture.test/a', {'X-Fixture':'1'}).body();
        var b=java.get('https://fixture.test/b').body();
        var c=java.put('https://fixture.test/c','fixture body',{}).body();
        JSON.stringify({bodies:[a,b,c],calls:calls})
    "#,"","").unwrap();
    let result:Value=serde_json::from_str(&result).unwrap();
    assert_eq!(result["bodies"],json!(["fixture response","fixture response","fixture response"]));
    assert_eq!(result["calls"][0][0],"GET");
    assert_eq!(result["calls"][0][3]["X-Fixture"],"1");
    assert_eq!(result["calls"][1][0],"GET");
    assert_eq!(result["calls"][2][0],"PUT");
    assert_eq!(result["calls"][2][2],"fixture body");
}

#[test]
fn book_read_config_survives_subsequent_field_evaluation() {
    let book_url=format!("https://fixture.test/book/read-config/{}",uuid::Uuid::new_v4());
    let context=JsBookContext {book:json!({"bookUrl":book_url,"readConfig":{"fontSize":18}}),chapter:json!({})};
    with_book_context(Some(context), || {
        eval_js("book.setUseReplaceRule(false)","","").unwrap();
        let result=eval_js("JSON.stringify({enabled:book.getUseReplaceRule(),stored:book.readConfig.useReplaceRule,fontSize:book.readConfig.fontSize})","","").unwrap();
        assert_eq!(serde_json::from_str::<Value>(&result).unwrap(),json!({"enabled":false,"stored":false,"fontSize":18}));
    });
    assert_eq!(book_read_config_override("", &book_url), Some(json!({"useReplaceRule":false})));
    let other=JsBookContext {book:json!({"bookUrl":format!("https://fixture.test/book/other/{}",uuid::Uuid::new_v4())}),chapter:json!({})};
    with_book_context(Some(other), || assert_eq!(eval_js("book.getUseReplaceRule()","","").unwrap(),"true"));
}

#[test]
fn aggregate_platform_probe_uses_android_paragraph_comments() {
    let value=eval_js(
        "var deviceType='安卓'; try {java.deviceID();deviceType='苹果'}catch(e){}; JSON.stringify({deviceType:deviceType,androidIdType:typeof java.androidId(),sameAlias:java.androidId()===java.getAndroidId(),webViewUAType:typeof java.getWebViewUA()})",
        "", "",
    ).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&value).unwrap(),json!({"deviceType":"安卓","androidIdType":"string","sameAlias":true,"webViewUAType":"string"}));
}


#[test]
fn concurrent_rule_variables_do_not_leak_between_books_of_one_source() {
    let source = BookSource { book_source_url: format!("https://book-scope-{}.test",uuid::Uuid::new_v4()), ..Default::default() };
    let barrier=std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles:Vec<_>=["fixture-book-a","fixture-book-b"].into_iter().map(|id| {
        let source=source.clone();
        let barrier=barrier.clone();
        std::thread::spawn(move || {
            let context=JsBookContext {book:json!({"bookUrl":format!("https://fixture.test/books/{id}/{}",uuid::Uuid::new_v4())}),chapter:json!({})};
            with_source(&source, || with_book_context(Some(context), || {
                let written = eval_js(&format!("globalThis.__http=function(){{throw new Error('variable operation attempted HTTP')}};java.put('book_id',{})",serde_json::to_string(id).unwrap()),"","");
                barrier.wait();
                written.unwrap();
                assert_eq!(eval_js("java.get('book_id')","","").unwrap(),id);
            }));
        })
    }).collect();
    for handle in handles { handle.join().unwrap(); }
}


#[test]
fn read_config_overlay_uses_the_same_source_key_after_shelf_normalization() {
    let origin=format!("https://read-config-{}.fixture.test",uuid::Uuid::new_v4());
    let source=BookSource {book_source_url:format!("{origin}/"),..Default::default()};
    let book_url=format!("https://fixture.test/book/config/{}",uuid::Uuid::new_v4());
    let context=JsBookContext {book:json!({"bookUrl":book_url}),chapter:json!({})};
    with_source(&source,|| with_book_context(Some(context),|| {eval_js("book.setUseReplaceRule(false)","","").unwrap();}));
    assert_eq!(book_read_config_override(&origin,&book_url),Some(json!({"useReplaceRule":false})));
    assert_eq!(book_read_config_override(&format!("{origin}/"),&book_url),Some(json!({"useReplaceRule":false})));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn comment_panel_fetch_preserves_query_cookies_and_json_response() {
    use axum::{extract::Query, http::HeaderMap, routing::get, Json, Router};
    use std::collections::HashMap;

    let router = Router::new().route("/api/comments", get(|Query(query): Query<HashMap<String, String>>, headers: HeaderMap| async move {
        assert_eq!(query.get("book_id").map(String::as_str), Some("book+1"));
        assert_eq!(query.get("item_id").map(String::as_str), Some("chapter-2"));
        assert_eq!(headers.get("x-source").unwrap(), "comment-fixture");
        assert_eq!(headers.get("x-panel").unwrap(), "request");
        assert!(headers.get("cookie").unwrap().to_str().unwrap().contains("sessionid=fixture-session"));
        Json(json!({"code":0,"data":[{"content":"所选章节的段评"}]}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let source = BookSource {
        book_source_url: base.clone(),
        header: Some(r#"{"X-Source":"comment-fixture"}"#.into()),
        ..Default::default()
    };
    let http = HttpClient::new(3, None).unwrap();
    let context = http.source_context("comment-fixture", &base, None).unwrap();
    context.session.cookies.set(&url::Url::parse(&base).unwrap(), "sessionid=fixture-session").unwrap();
    let response = tokio::task::spawn_blocking(move || with_http_context(context, || {
        reader_rust::parser::js::run_panel_fetch(&source, &format!("{base}/get_review"), "fetch", &json!({
            "method":"GET", "url":"/api/comments?book_id=book%2B1&item_id=chapter-2",
            "headers":{"X-Panel":"request"}, "body":""
        }))
    })).await.unwrap().unwrap();
    assert_eq!(response["code"], 200);
    assert_eq!(serde_json::from_str::<Value>(response["body"].as_str().unwrap()).unwrap(), json!({"code":0,"data":[{"content":"所选章节的段评"}]}));
    server.abort();
}
