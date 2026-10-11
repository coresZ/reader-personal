use reader_rust::{
    api::protocol::build_source_panel_page,
    crawler::http_client::HttpClient,
    model::book_source::{book_source_from_value, BookSource},
    parser::{js::eval_js, rule_engine::RuleEngine},
    service::{
        book_service::{get_source_panel, remove_source_panel, BookService, ShowBrowserRequest},
        source_browser::{cancel_browser, complete_browser, BrowserHandler, BrowserPage},
    },
    storage::cache::file_cache::FileCache,
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};

fn service() -> (Arc<BookService>, PathBuf) {
    let root = std::env::temp_dir().join(format!("reader-browser-{}", uuid::Uuid::new_v4()));
    let service = BookService::new(HttpClient::new(5, None).unwrap(), RuleEngine::new().unwrap(), FileCache::new(root.join("cache")), root.to_str().unwrap());
    (Arc::new(service), root)
}

fn source() -> BookSource {
    book_source_from_value(json!({
        "bookSourceUrl":format!("聚合测试{}", uuid::Uuid::new_v4()), "bookSourceName":"面板测试",
        "loginUi":[{"name":"输入","type":"text"},{"name":"设置","type":"button","action":"configure()"}],
        "loginUrl":r#"
            function configure() {
                source.put('executions', String(Number(source.get('executions') || 0) + 1));
                var before = result['输入'];
                var body = java.startBrowserAwait('data:text/html,%3Cspan%20id="choice"%3Einitial%3C/span%3E', '设置线路', false).body();
                source.setVariable(before + '|' + body.match(/id="choice">([^<]*)</)[1]);
                java.upLoginData({'输入': '已保存'});
            }
        "#
    })).unwrap()
}

fn read(service: &BookService, source: &BookSource, code: &str) -> String {
    service.source_scope("default", source, || eval_js(code, "", "")).unwrap().unwrap()
}

fn browser_channel() -> (BrowserHandler, tokio::sync::mpsc::UnboundedReceiver<ShowBrowserRequest>) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    (Arc::new(move |request| sender.send(request).map_err(|e| e.to_string())), receiver)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn await_keeps_the_original_stack_until_the_document_is_confirmed() {
    let (service, root) = service();
    let source = source();
    read(&service, &source, "source.setVariable('original')");
    let (handler, mut events) = browser_channel();
    let task = {
        let service = service.clone(); let source = source.clone();
        tokio::spawn(async move { service.run_source_login_with_browser(&source, Some("button:1".into()), Some(serde_json::Map::from_iter([("输入".into(), json!("user-input"))])), Some(handler)).await })
    };
    let request = tokio::time::timeout(Duration::from_secs(10), events.recv()).await.unwrap().unwrap();
    let id = request.panel_id.unwrap();
    assert!(request.await_result);
    assert_eq!(request.title.as_deref(), Some("设置线路"));
    assert!(get_source_panel(&id).unwrap().html.contains("initial"));
    assert!(!task.is_finished());
    assert_eq!(read(&service, &source, "source.getVariable()"), "original");
    assert!(complete_browser(&id, BrowserPage { url: String::new(), body: String::new() }).is_err());
    assert!(!task.is_finished());
    complete_browser(&id, BrowserPage { url: "data:text/html,fixture".into(), body: "<span id=\"choice\">selected</span>".into() }).unwrap();
    let view = tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap().unwrap();
    assert_eq!(view["values"]["输入"], "已保存");
    assert_eq!(read(&service, &source, "source.getVariable()"), "user-input|selected");
    assert_eq!(read(&service, &source, "source.get('executions')"), "1");
    assert!(get_source_panel(&id).is_none());
    drop(service);
    let restored = BookService::new(HttpClient::new(5, None).unwrap(), RuleEngine::new().unwrap(), FileCache::new(root.join("cache")), root.to_str().unwrap());
    assert_eq!(read(&restored, &source, "source.getVariable()"), "user-input|selected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_page_does_not_apply_empty_defaults_and_can_be_reopened() {
    let (service, _) = service();
    let mut source = source();
    source.login_url = Some(format!("{}\nfunction configureSafely() {{ try {{ configure(); }} catch(e) {{ java.toast('cancelled'); }} }}", source.login_url.unwrap()));
    source.login_ui = Some(r#"[{"name":"输入"},{"name":"设置","type":"button","action":"configureSafely()"}]"#.into());
    read(&service, &source, "source.setVariable('keep-me')");
    for action in ["configure()", "configureSafely()"] {
        source.login_ui = Some(json!([{"name":"输入"},{"name":"设置","type":"button","action":action}]).to_string());
        let (handler, mut events) = browser_channel();
        let task = {
            let service = service.clone(); let source = source.clone();
            tokio::spawn(async move { service.run_source_login_with_browser(&source, Some("button:1".into()), None, Some(handler)).await })
        };
        let request = tokio::time::timeout(Duration::from_secs(10), events.recv()).await.unwrap().unwrap();
        let id = request.panel_id.unwrap();
        cancel_browser(&id);
        let view = tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap().unwrap();
        assert_eq!(view["cancelled"], true);
        assert_eq!(view["actionCompleted"], false);
        assert_eq!(read(&service, &source, "source.getVariable()"), "keep-me");
        assert!(get_source_panel(&id).is_none());
    }
}

#[tokio::test]
async fn show_browser_keeps_options_and_preloads_before_the_page_script() {
    let (service, _) = service();
    let mut source = source();
    let html = r#"<!DOCTYPE html><html><head><title>线路设置</title><script>window.pageSawPreload=window.fixturePreload;</script></head><body><label for="line">线路</label><input id="line" value="初始线路"><label for="note">备注</label><textarea id="note">初始备注</textarea><label for="theme">样式</label><select id="theme"><option value="light">浅色</option><option value="dark">深色</option></select><span id="choice">initial</span><button onclick="java.upConfig(JSON.stringify({heightPercentage:0.6}))">调整高度</button><button onclick="java.showBrowser('https://fixture.test/nested','<title>子面板</title><button onclick=&quot;window.close()&quot;>完成</button>',null,{heightPercentage:0.5})">打开子面板</button></body></html>"#;
    source.login_url = Some(format!("function configure() {{ java.showBrowser('https://fixture.test/settings', {}, {}, JSON.stringify({{heightPercentage:0.8, isDraggable:false, expandedCornersRadius:20, dismissOnTouchOutside:false}})); }}", json!(html), json!("window.fixturePreload='ready'; window.scriptLiteral='</script>';")));
    let view = service.run_source_login(&source, Some("button:1".into()), None).await.unwrap();
    let request: ShowBrowserRequest = serde_json::from_value(view["showBrowser"].clone()).unwrap();
    assert_eq!(request.options.as_ref().unwrap()["heightPercentage"], 0.8);
    let id = request.panel_id.unwrap();
    let page = build_source_panel_page(&id).unwrap();
    assert!(page.find("window.java =").unwrap() < page.find("window.fixturePreload='ready'").unwrap());
    assert!(page.find("window.fixturePreload='ready'").unwrap() < page.find("window.pageSawPreload").unwrap());
    assert!(page.contains("\\u003c/script>"));
    // The browser suite serves this actual backend-generated bridge and page.
    let fixture_dir = PathBuf::from("target/source-panel-fixtures");
    std::fs::create_dir_all(&fixture_dir).unwrap();
    std::fs::write(fixture_dir.join("settings.html"), page.replace(&id, "fixture-settings")).unwrap();
    remove_source_panel(&id);
}

#[tokio::test]
async fn login_controls_keep_defaults_and_apply_resets_and_button_labels() {
    let (service, _) = service();
    let source = book_source_from_value(json!({
        "bookSourceUrl":format!("控件测试{}", uuid::Uuid::new_v4()), "bookSourceName":"控件",
        "loginUi":[{"name":"选项","type":"select","chars":[null,"甲","乙"]},{"name":"密码","type":"password","default":"默认"},{"name":"操作","type":"button","action":"update()"},{"name":"说明","type":"button"}],
        "loginUrl":"function update(){java.upLoginData(null);java.upLoginData({'操作':'已更新','选项':'乙'});java.reLoginView();}"
    })).unwrap();
    let view = service.login_book_source(&source).await.unwrap();
    assert_eq!(view["values"]["选项"], "甲");
    let view = service.run_source_login(&source, Some("button:2".into()), Some(serde_json::from_value::<serde_json::Map<String, Value>>(json!({"选项":"甲","密码":"输入"})).unwrap())).await.unwrap();
    assert_eq!(view["values"]["密码"], "默认");
    assert_eq!(view["values"]["选项"], "乙");
    assert_eq!(view["fields"][2]["viewName"], "已更新");
    service.run_source_login(&source, Some("button:3".into()), None).await.unwrap();
}
