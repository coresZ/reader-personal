// 本地下载的某个聚合书源兼容验证（样本不入库）。
// 其设置按钮用 startBrowserAwait 等待用户确认内嵌 HTML，再读取 DOM 保存设置。
//
// 若对应样本 JSON 不存在则跳过（该文件由本地脚本生成，不入库）。

use reader_rust::model::book_source::BookSource;
use reader_rust::parser::js::{eval_js, take_js_logs, with_source};
use serde_json::Value;
use std::sync::Arc;

fn load_source() -> Option<BookSource> {
    let path = std::path::Path::new("tmp_probe/yck7249-login.json");
    if !path.exists() {
        eprintln!("跳过: 未找到 {}", path.display());
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    serde_json::from_value(value).ok()
}

#[test]
fn aggregated_source_login_page_survives_js_lib_and_form_parsing() {
    let Some(source) = load_source() else { return };
    eprintln!("书源: {}", source.book_source_name);

    // 层 1: 146KB 的 jsLib 能否在 rquickjs 下加载（依赖 Packages.java.* 桥）
    with_source(&source, || {
        take_js_logs();
        let out = eval_js(
            "JSON.stringify(['getToken','ste','getServerSettings','getHtmlSettings',\
             'renderVersionPage','deleteVariable','request','setAllCookies','checkEnv']\
             .map(function(n){return typeof globalThis[n]}))",
            "",
            "",
        )
        .expect("jsLib 应能加载");
        eprintln!("层1 jsLib 加载: {out}");
        assert!(!out.contains("undefined"), "所有函数都应有定义: {out}");
    });

    // 层 2: loginUi 是合法 JSON（我们的 fields() 要求 serde_json 能解析）
    let ui_raw = source.login_ui.as_deref().unwrap_or("");
    let rows: Vec<Value> = serde_json::from_str(ui_raw).expect("loginUi 应是合法 JSON");
    let buttons = rows
        .iter()
        .filter(|r| r.get("type").and_then(Value::as_str) == Some("button"))
        .count();
    eprintln!("层2 loginUi 解析: {} 个控件, 其中 {buttons} 个按钮", rows.len());
    assert_eq!(rows.len(), 34);
}

/// showBrowser 的弹窗配置需要完整传给桌面面板。
#[test]
fn show_browser_keeps_fourth_options_argument() {
    let Some(source) = load_source() else { return };
    with_source(&source, || {
        take_js_logs();
        eval_js(
            r#"
            java.showBrowser(
              'https://panel.example/x',
              '<p>hi</p>',
              'window.java=java;',
              JSON.stringify({heightPercentage: 0.8, isDraggable: true, expandedCornersRadius: 20})
            );
            'called'
            "#,
            "",
            "",
        )
        .expect("四参数调用不应报错");

        let request = reader_rust::service::book_service::take_show_browser_request()
            .expect("showBrowser 应捕获面板请求");
        assert_eq!(request.url, "https://panel.example/x");
        assert!(request.html.as_deref().unwrap_or("").contains("<p>hi</p>"));

        assert_eq!(request.options.as_ref().unwrap()["heightPercentage"], 0.8);
        assert!(
            request.options.is_some(),
            "第 4 个参数(options)必须被保留：书源靠它决定面板高度/可拖拽等行为",
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_settings_buttons_save_the_confirmed_page() {
    use reader_rust::{
        crawler::http_client::HttpClient,
        parser::rule_engine::RuleEngine,
        service::{book_service::{BookService, get_source_panel, remove_source_panel}, source_browser::{BrowserHandler, BrowserPage, complete_browser}},
        storage::cache::file_cache::FileCache,
    };
    use std::sync::Arc;
    let Some(mut source) = load_source() else { return };
    source.book_source_url = format!("聚合源测试{}", uuid::Uuid::new_v4());
    let root = std::env::temp_dir().join(format!("reader-aggregate-panel-{}", uuid::Uuid::new_v4()));
    let service = BookService::new(HttpClient::new(5, None).unwrap(), RuleEngine::new().unwrap(), FileCache::new(root.join("cache")), root.to_str().unwrap());
    assert_eq!(service.login_book_source(&source).await.unwrap()["fields"].as_array().unwrap().len(), 34);
    let fixtures = [
        ("button:0", "线路", "serverValue", "https://chosen.example"),
        ("button:1", "更多设置", "searchModeValue", "漫画"),
        ("button:2", "段评设置", "duanpingStyleValue", "3"),
    ];
    for (action, variable, span, selected) in fixtures {
        let document = format!("<span id=\"{span}\">{selected}</span>");
        let handler: BrowserHandler = Arc::new(move |request| {
            if !request.await_result { return Err("设置页应等待用户确认".into()); }
            let id = request.panel_id.as_deref().ok_or("未登记面板")?;
            let panel = get_source_panel(id).ok_or("未找到面板")?;
            if !panel.html.contains(span) { return Err(format!("设置页缺少 {span}")); }
            complete_browser(id, BrowserPage { url: panel.url, body: document.clone() })
        });
        let view = service.run_source_login_with_browser(&source, Some(action.into()), None, Some(handler)).await.unwrap();
        assert_eq!(view["actionCompleted"], true, "{action}: {view}");
        let saved = service.source_scope("default", &source, || eval_js("source.getVariable()", "", "")).unwrap().unwrap();
        let saved: Value = serde_json::from_str(&saved).unwrap();
        match variable {
            "线路" => assert_eq!(saved[variable], selected),
            "更多设置" => assert_eq!(saved[variable]["搜索模式"], selected),
            _ => assert_eq!(saved[variable]["段评样式"], selected),
        }
    }
    let version = service.run_source_login(&source, Some("button:14".into()), None).await.unwrap();
    let id = version["showBrowser"]["panelId"].as_str().unwrap();
    assert!(get_source_panel(id).unwrap().html.contains("书源更新"));
    remove_source_panel(id);
}

/// 某第三方平台源：登录拿 token 存**源变量**（不是 cookie），
/// 请求靠 `header` 规则动态拼 `Authorization: Bearer <token>`，个人中心则是
/// `java.startBrowser(host + '/user/token-login?token=' + token)` 用 URL 换会话。
/// 验证这条链路不依赖 cookie jar 也能把登录态带进面板。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_url_login_sources_carry_session_into_the_panel() {
    use axum::{http::HeaderMap, routing::{get, post}, Router};
    use reader_rust::{
        crawler::http_client::HttpClient,
        parser::rule_engine::RuleEngine,
        service::book_service::{BookService, take_show_browser_request},
        storage::cache::file_cache::FileCache,
    };

    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let record = |headers: &HeaderMap, path: &str| {
        format!("{path} cookie=[{}] auth=[{}]",
            headers.get("cookie").and_then(|v| v.to_str().ok()).unwrap_or(""),
            headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or(""))
    };
    let s2 = seen.clone();
    let s3 = seen.clone();
    let app = Router::new()
        .route("/legado/login", post(|| async {
            serde_json::json!({"code": 200, "data": {"token": "mock-token-abc123", "user": {"nickname": "测试用户号"}}}).to_string()
        }))
        .route("/user/token-login", get(move |headers: HeaderMap| {
            let s = s2.clone();
            async move {
                s.lock().unwrap().push(format!("token-login {}", record(&headers, "")));
                // token 换会话: 302 → /user + Set-Cookie
                axum::response::Response::builder()
                    .status(302).header("location", "/user")
                    .header("set-cookie", "mysession=panel-session-42; Path=/")
                    .body(axum::body::Body::empty()).unwrap()
            }
        }))
        .route("/user", get(move |headers: HeaderMap| {
            let s = s3.clone();
            async move {
                let line = record(&headers, "/user");
                let logged = line.contains("mysession=panel-session-42");
                s.lock().unwrap().push(line);
                if logged { "<html><head><title>书源四个人中心</title></head><body>已登录</body></html>" }
                else { "<html><head><title>书源四个人中心</title></head><body>未登录</body></html>" }
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let source: BookSource = serde_json::from_value(serde_json::json!({
        "bookSourceName": "书源四(测试)", "bookSourceUrl": "custom://rapid",
        "jsLib": "var host = 'https://iota.example.com';",
        "header": "@js:var v=JSON.parse(source.getVariable()||'{}');JSON.stringify(v.token?{'Authorization':'Bearer '+v.token}:{})",
        "loginUrl": "function login(flag){\n\
            try{ result=JSON.parse(source.getLoginInfo()||'{}'); }catch(e){ result={}; }\n\
            var hh=String(result['线路']||'');\n\
            var opt=JSON.stringify({method:'POST',body:JSON.stringify({email:String(result['邮箱']),password:String(result['密码'])}),headers:{'Content-Type':'application/json'}});\n\
            var j=JSON.parse(java.ajax(hh+'/legado/login,'+opt));\n\
            if(j && j.code==200 && j.data && j.data.token){var vv={};try{vv=JSON.parse(source.getVariable()||'{}');}catch(e){}vv.token=j.data.token;vv.host=hh;source.setVariable(JSON.stringify(vv));}\n\
            else{ throw (j&&j.msg)||'登录失败'; }\n\
        }",
        "loginUi": serde_json::json!([
            {"name": "邮箱", "type": "text", "default": ""},
            {"name": "密码", "type": "password", "default": ""},
            {"name": "登录书源四", "type": "button", "action": "login(true)"},
            {"name": "个人中心", "type": "button", "action":
                "(function(){var v={};try{v=JSON.parse(source.getVariable()||'{}');}catch(e){}var t=String(v.token||'');if(!t){java.toast('请先登录书源账号');return;}var bh=String(v.host||'')||host;java.startBrowser(bh+'/user/token-login?token='+encodeURIComponent(t),'书源四·个人中心');})()"}
        ]).to_string(),
    })).unwrap();

    let root = std::env::temp_dir().join(format!("reader-custom-{}", uuid::Uuid::new_v4()));
    let service = BookService::new(HttpClient::new(5, None).unwrap(), RuleEngine::new().unwrap(), FileCache::new(root.join("cache")), root.to_str().unwrap());

    // 层1: 走生产登录管线；线路指向本地模拟站（源本身支持 loginInfo['线路']）
    let mut values = serde_json::Map::new();
    values.insert("邮箱".into(), "reader@example.test".into());
    values.insert("密码".into(), "secret123".into());
    values.insert("线路".into(), base.as_str().into());
    service.run_source_login(&source, Some("login".into()), Some(values)).await.unwrap();

    // 层2: token 存入源变量
    let token = service.source_scope("default", &source, || eval_js(r#"JSON.parse(source.getVariable()||'{}').token||''"#, "", ""))
        .unwrap().unwrap();
    assert_eq!(token, "mock-token-abc123", "登录 token 应存入源变量");

    // 层3: header 规则从变量动态取 token
    let spec = service.source_scope("default", &source, || reader_rust::crawler::url_analyzer::source_header_spec(&source)).unwrap().unwrap();
    let auth = spec.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization")).map(|(_, v)| v.clone());
    assert_eq!(auth.as_deref(), Some("Bearer mock-token-abc123"), "Authorization 应从源变量动态求值");

    // 层4: 个人中心按钮产出 token-login 面板请求（startBrowser 非等待面板）
    let ui: Value = serde_json::from_str(source.login_ui.as_deref().unwrap()).unwrap();
    let action = ui.as_array().unwrap().iter()
        .find(|row| row.get("name").and_then(|n| n.as_str()) == Some("个人中心"))
        .and_then(|row| row.get("action")).and_then(|a| a.as_str()).unwrap().to_string();
    let _ = service.source_scope("default", &source, || eval_js(&action, "", ""));
    let request = take_show_browser_request().expect("个人中心应产生浏览器请求");
    assert!(!request.await_result, "startBrowser 应是非等待面板");
    assert!(request.url.ends_with(&format!("/user/token-login?token={token}")), "URL 应携带 token: {}", request.url);

    // 层5: 面板加载 token-login → 302 /user，会话 cookie 必须随行
    let client = service.source_http_client("default", &source.book_source_url, None).await.unwrap();
    let body = client.get(&request.url).send().await.unwrap().text().await.unwrap();
    assert!(body.contains("已登录"), "个人中心应识别登录态: {body}");
    let log = seen.lock().unwrap().join("\n");
    assert!(log.contains("/user cookie=[mysession=panel-session-42]"), "token-login 换取的会话应带到 /user:\n{log}");
    server.abort();
}

/// `loginCheckJs` 的 legado 契约：`result` 是 **StrResponse 对象**（方法调用
/// `result.code()` / `result.body()`），`java.getStrResponse()` 取最近响应，
/// `source.login()` 请求用已存信息重新登录。脚本本身出错不能打断搜索。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_check_js_sees_str_response_and_reauthenticates_on_401() {
    use axum::{http::HeaderMap, routing::{get, post}, Router};
    use reader_rust::{
        crawler::{fetcher::{FetchResponse, StrResponse}, http_client::HttpClient},
        parser::js::{eval_js_with_bindings, resp_binding, set_last_response, take_login_effects},
        parser::rule_engine::RuleEngine,
        service::book_service::BookService,
        storage::cache::file_cache::FileCache,
    };
    use std::collections::HashMap;
    use serde_json::json;

    let authed = Arc::new(std::sync::Mutex::new(false));
    let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (a1, a2) = (authed.clone(), authed.clone());
    let (l1, l2) = (log.clone(), log.clone());
    let app = Router::new()
        .route("/legado/login", post(move || {
            let a = a1.clone();
            let l = l1.clone();
            async move {
                l.lock().unwrap().push("login".into());
                *a.lock().unwrap() = true;
                serde_json::json!({"code": 200, "data": {"token": "fresh-token-xyz"}}).to_string()
            }
        }))
        .route("/legado/search", get(move |headers: HeaderMap| {
            let a = a2.clone();
            let l = l2.clone();
            async move {
                let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                l.lock().unwrap().push(format!("search auth=[{auth}]"));
                let signed_in = *a.lock().unwrap() && auth.contains("fresh-token-xyz");
                let body = if signed_in {
                    serde_json::json!([{"name": "斗破苍穹", "author": "天蚕土豆", "bookUrl": "/book/1"}]).to_string()
                } else {
                    serde_json::json!({"code": 401, "msg": "40101 未登录"}).to_string()
                };
                ([("content-type", "application/json")], body)
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let source: BookSource = serde_json::from_value(serde_json::json!({
        "bookSourceName": "书源四登录校验(测试)",
        "bookSourceUrl": format!("custom://checkjs-{}", uuid::Uuid::new_v4()),
        "jsLib": format!("var host = '{base}';"),
        "header": "@js:var v=JSON.parse(source.getVariable()||'{}');JSON.stringify(v.token?{'Authorization':'Bearer '+v.token}:{})",
        "loginUrl": format!(
            "function login(flag){{\n\
             try{{ result=JSON.parse(source.getLoginInfo()||'{{}}'); }}catch(e){{ result={{}}; }}\n\
             var hh=String(result['线路']||'{base}');\n\
             var opt=JSON.stringify({{method:'POST',body:'{{}}',headers:{{'Content-Type':'application/json'}}}});\n\
             var j=JSON.parse(java.ajax(hh+'/legado/login,'+opt));\n\
             if(j && j.code==200 && j.data && j.data.token){{var vv={{}};try{{vv=JSON.parse(source.getVariable()||'{{}}');}}catch(e){{}}vv.token=j.data.token;vv.host=hh;source.setVariable(JSON.stringify(vv));}}\n\
             else{{ throw (j&&j.msg)||'登录失败'; }}\n\
            }}"
        ),
        "loginUi": serde_json::json!([
            {"name": "邮箱", "type": "text", "default": ""},
            {"name": "密码", "type": "password", "default": ""},
            {"name": "登录", "type": "button", "action": "login(true)"}
        ]).to_string(),
        // 该类书源的真实写法: 401 时 source.login() + java.getStrResponse()
        "loginCheckJs": "if(result.code()==401||String(result.body()).indexOf('40101')>=0){source.login();java.getStrResponse();}else{result;}",
        "searchUrl": "<js>host + '/legado/search?key=' + encodeURIComponent(key) + '&page=' + page;</js>",
        "ruleSearch": {
            "bookList": "$", "name": "$.name", "author": "$.author",
            "bookUrl": "<js>host + J(result).bookUrl</js>"
        },
    })).unwrap();

    let root = std::env::temp_dir().join(format!("reader-checkjs-{}", uuid::Uuid::new_v4()));
    let service = BookService::new(HttpClient::new(5, None).unwrap(), RuleEngine::new().unwrap(), FileCache::new(root.join("cache")), root.to_str().unwrap());

    // 层1: `result` 必须是带方法的 StrResponse —— 纯 JSON 对象会让
    // `result.code()` 抛 "not a function"（原故障现场）。
    let res = FetchResponse {
        body: r#"{"code":401,"msg":"40101 未登录"}"#.into(),
        url: format!("{base}/legado/search?key=x"), status: 200,
        raw_body: Vec::new(),
        headers: vec![], content_type: Some("application/json".into()), is_successful: true,
    };
    let script = source.login_check_js.clone().unwrap();
    let str_response = StrResponse::from(res.clone());
    let (outcome, login_requested) = service.source_scope("default", &source, || {
        take_login_effects();
        let response_json = serde_json::to_value(&str_response).unwrap();
        set_last_response(&response_json.to_string());
        let mut bindings = HashMap::new();
        bindings.insert("result".to_string(), resp_binding(&response_json));
        let out = eval_js_with_bindings(&script, &res.body, &res.url, &bindings);
        (out, take_login_effects().login)
    }).unwrap();
    assert!(outcome.is_ok(), "result.code()/body() 应可调用: {outcome:?}");
    assert!(login_requested, "401 响应应触发 source.login()");

    // 层2: java.getStrResponse() 返回最近响应对象
    let probe = service.source_scope("default", &source, || {
        let response_json = serde_json::to_value(&str_response).unwrap();
        set_last_response(&response_json.to_string());
        eval_js("JSON.stringify({code: java.getStrResponse().code(), hasBody: String(java.getStrResponse().body()).length > 0})", "", "")
    }).unwrap().unwrap();
    assert_eq!(probe, r#"{"code":200,"hasBody":true}"#, "getStrResponse 应返回 StrResponse 对象");

    // 层3: 脚本出错时不得中断搜索(无会话的源很常见)
    let broken = BookSource { login_check_js: Some("undefinedFunction()".into()), ..source.clone() };
    let (broken_out, broken_login) = service.source_scope("default", &broken, || {
        take_login_effects();
        let response_json = serde_json::to_value(&str_response).unwrap();
        set_last_response(&response_json.to_string());
        let mut bindings = HashMap::new();
        bindings.insert("result".to_string(), resp_binding(&response_json));
        let out = eval_js_with_bindings(&broken.login_check_js.clone().unwrap(), &res.body, &res.url, &bindings);
        (out, take_login_effects().login)
    }).unwrap();
    assert!(broken_out.is_err(), "脚本错误本身仍应被记录");
    assert!(!broken_login);

    // 层4: 完整搜索链路 —— 首次 401 → 自动重登录 → 重取拿到结果
    let mut values = serde_json::Map::new();
    values.insert("邮箱".into(), json!("reader@example.test"));
    values.insert("密码".into(), json!("secret123"));
    values.insert("线路".into(), json!(base));
    service.run_source_login(&source, Some("login".into()), Some(values)).await.unwrap();
    let books = service.search_book("default", &source, "斗破苍穹", 1).await;
    let names: Vec<String> = books.as_ref().map(|list| list.iter().map(|b| b.name.clone()).collect()).unwrap_or_default();
    assert_eq!(names, vec!["斗破苍穹"], "登录后搜索应拿到结果: {books:?}");
    let log = log.lock().unwrap().join("\n");
    assert!(log.contains("search auth=[Bearer fresh-token-xyz]"), "搜索请求应带登录后的 token:\n{log}");
    server.abort();
}
