// 发现页重构的后端验证：
//   1. ExploreKind 的 type/action/chars/default/viewName 不再被丢弃
//   2. infoMap 能注入 JS（书源两种访问写法都要可用）
//   3. org.jsoup 桥（某书源报错 org is not defined 的修复）
//   4. java.refreshExplore() 语义（不再等同于重登录）
//   5. java.open / java.searchBook 意图回传
use reader_rust::crawler::http_client::HttpClient;
use reader_rust::model::book_source::{book_source_from_value, BookSource, ExploreKindType};
use reader_rust::parser::js::{eval_js_with_bindings, with_source};
use reader_rust::parser::rule_engine::RuleEngine;
use reader_rust::service::book_service::BookService;
use reader_rust::storage::cache::file_cache::FileCache;
use serde_json::{json, Value};
use std::collections::HashMap;

fn source(value: Value) -> BookSource {
    book_source_from_value(value).expect("source should deserialize")
}

fn service() -> BookService {
    let root = std::env::temp_dir().join(format!("reader-explore-refactor-{}", uuid::Uuid::new_v4()));
    BookService::new(
        HttpClient::new(20, None).unwrap(),
        RuleEngine::new().unwrap(),
        FileCache::new(root.join("cache")),
        root.to_str().unwrap(),
    )
}

/// 交互控件字段必须完整穿过反序列化 —— 这是"桌面端实现不了发现页"的根因。
#[test]
fn explore_kind_keeps_interactive_fields() {
    let raw = json!([{
        "title": "源站",
        "type": "select",
        "chars": ["☁️聚合搜索", "番茄", null],
        "default": "番茄",
        "action": "setConfigs.call(this,{source:infoMap['源站']})",
        "style": { "layout_flexGrow": 1 },
        "viewName": "'选源'"
    }]);

    let kinds: Vec<reader_rust::model::book_source::ExploreKind> =
        serde_json::from_value(raw).expect("kinds should deserialize");
    let kind = &kinds[0];

    assert_eq!(kind.title, "源站");
    assert_eq!(kind.kind_type(), ExploreKindType::Select);
    assert_eq!(kind.default.as_deref(), Some("番茄"));
    assert_eq!(kind.view_name.as_deref(), Some("'选源'"));
    assert!(kind.action.as_deref().unwrap_or("").contains("infoMap"));
    // chars 里的 null 要过滤掉，不能让前端渲染出空选项
    assert_eq!(kind.chars_list(), vec!["☁️聚合搜索", "番茄"]);

    // 缺省 type 按 legado 语义回退为 url
    let plain: Vec<reader_rust::model::book_source::ExploreKind> =
        serde_json::from_value(json!([{ "title": "排行", "url": "/rank" }])).unwrap();
    assert_eq!(plain[0].kind_type(), ExploreKindType::Url);
}

/// 序列化回来时键名必须是 `type`（书源与前端都按 `type` 读）。
#[test]
fn explore_kind_serializes_type_key() {
    let kind = reader_rust::model::book_source::ExploreKind {
        title: "模式".to_string(),
        kind_type: Some("toggle".to_string()),
        ..Default::default()
    };
    let value = serde_json::to_value(&kind).unwrap();
    assert_eq!(value["type"], "toggle");
    assert!(value.get("kindType").is_none());
}

/// 纯文本型 exploreUrl 仍能解析；`<js>` 型走沙盒求值并注入 infoMap。
#[test]
fn explore_kinds_evaluates_js_rule_with_info_map() {
    let svc = service();

    let plain = source(json!({
        "bookSourceName": "文本源",
        "bookSourceUrl": "https://plain.test",
        "exploreUrl": "玄幻::https://plain.test/xh/{{page}}/\n都市::https://plain.test/ds/{{page}}/"
    }));
    let kinds = svc.explore_kinds(&plain).unwrap();
    assert_eq!(kinds.len(), 2);
    assert_eq!(kinds[0].title, "玄幻");
    assert_eq!(kinds[0].url.as_deref(), Some("https://plain.test/xh/{{page}}/"));

    // JS 型：读 infoMap 决定输出，并回写默认值（该类书源的真实写法）
    let js_source = source(json!({
        "bookSourceName": "JS源",
        "bookSourceUrl": "https://js.test",
        "exploreUrl": "<js>\nvar src = (typeof infoMap !== 'undefined' && infoMap['源站']) || '默认源';\nJSON.stringify([{title:'源站:'+src, url:'https://js.test/'+src}]);\n</js>"
    }));

    let empty = svc.explore_kinds(&js_source).unwrap();
    assert_eq!(empty[0].title, "源站:默认源");

    let mut info = HashMap::new();
    info.insert("源站".to_string(), "番茄".to_string());
    let with_info = svc.explore_kinds_with_info(&js_source, &info).unwrap();
    assert_eq!(with_info[0].title, "源站:番茄");
    assert_eq!(with_info[0].url.as_deref(), Some("https://js.test/番茄"));
}

/// 书源里 `infoMap['x']` 与 `infoMap.get('x')` 两种写法都要可用。
#[test]
fn info_map_supports_both_access_styles() {
    let src = source(json!({
        "bookSourceName": "infoMap",
        "bookSourceUrl": "https://infomap.test"
    }));
    let mut info = HashMap::new();
    info.insert("平台".to_string(), "七猫".to_string());

    let mut bindings = HashMap::new();
    bindings.insert(
        "infoMap".to_string(),
        json!("{\"__infoMap\":{\"平台\":\"七猫\"}}"),
    );

    with_source(&src, || {
        // 下标写法
        let direct = eval_js_with_bindings(
            "infoMap['平台'] + '|' + (infoMap.get ? infoMap.get('平台') : 'NOGET') + '|' + infoMap.size()",
            "",
            "",
            &bindings,
        )
        .expect("infoMap 访问不应报错");
        assert_eq!(direct, "七猫|七猫|1");
    });
}

/// 某书源报错 `org is not defined` 的修复：org.jsoup.Jsoup.parse(result).select(...)
#[test]
fn jsoup_bridge_matches_story_source_usage() {
    let src = source(json!({
        "bookSourceName": "故事族",
        "bookSourceUrl": "https://www.gushizu.com"
    }));

    // 取自该书源 ruleExplore.bookList 的真实写法
    let script = r#"
var doc = org.jsoup.Jsoup.parse(result);
var els = doc.select('a.flex-wrp.border-b');
if (els.size() === 0) { els = doc.select('a[href*="/novel/"]'); }
els.size() + '|' + els.get(0).attr('href') + '|' + els.get(0).text();
"#;
    let html = r#"<html><body>
      <a class="flex-wrp border-b" href="/novel/novel/index/novelId/41">科幻故事</a>
      <a class="flex-wrp border-b" href="/novel/novel/index/novelId/42">推理故事</a>
    </body></html>"#;

    with_source(&src, || {
        let out = reader_rust::parser::js::eval_js_with_bindings(script, html, "", &HashMap::new())
            .expect("org.jsoup 桥应当可用");
        assert_eq!(out, "2|/novel/novel/index/novelId/41|科幻故事");
    });
}

/// JSoup 桥在元素上继续 select（Elements.select / Element.select）也要可用。
#[test]
fn jsoup_bridge_supports_nested_select() {
    let src = source(json!({
        "bookSourceName": "jsoup-nested",
        "bookSourceUrl": "https://jsoup.test"
    }));
    let html = r#"<div class="list"><span class="n">一</span><span class="n">二</span></div>"#;
    with_source(&src, || {
        let out = reader_rust::parser::js::eval_js_with_bindings(
            "var doc = org.jsoup.Jsoup.parse(result); doc.select('div.list').select('span.n').size() + ':' + doc.select('span.n').first().text()",
            html,
            "",
            &HashMap::new(),
        )
        .expect("嵌套 select 应当可用");
        assert_eq!(out, "2:一");
    });
}

/// 端到端回归：该类书源式的 `<js>` 列表规则返回 JSoup Elements 时，parse_search_list
/// 必须能把它当成书籍列表解析出来（书名 / 书址都要正确）。
///
/// 这正是"解析 0 本、且不报错"的根因：Elements 被序列化成
/// `[object Object]` 后丢失了元素的 tag/attrs，字段规则再取 `href` 就一无所获。
/// 修复分两处：Elements 的 `toJSON` 交出元素快照；`pick_json_field` 在快照上
/// 回退查 `attrs`。
#[test]
fn js_elements_rule_parses_into_books() {
    use reader_rust::parser::rule_engine::RuleEngine;
    use reader_rust::model::rule::SearchRule;

    let source = source(json!({
        "bookSourceName": "故事族",
        "bookSourceUrl": "https://www.gushizu.com",
        "ruleExplore": {
            "bookList": "<js>var doc = org.jsoup.Jsoup.parse(result);var els = doc.select('a.flex-wrp.border-b');if (els.size() === 0) { els = doc.select('a[href*=\"/novel/novel/index/novelId/\"]'); } els;</js>",
            "name": ".size15@text||h4@text||text",
            "bookUrl": "href",
        }
    }));

    let html = r#"<html><body>
      <a href="/novel/novel/index/novelId/959">故事族·睡前故事2023年增刊</a>
      <a href="/novel/novel/index/novelId/38">故事会2021年02期</a>
    </body></html>"#;

    let engine = RuleEngine::new().unwrap();
    let books = engine.explore_books(&source, html, "https://www.gushizu.com/rank/allvisit");

    assert_eq!(books.len(), 2, "应解析出两本书, 实际: {books:?}");
    assert_eq!(books[0].name, "故事族·睡前故事2023年增刊");
    // bookUrl 必须来自元素属性, 而不是退化成列表页地址
    assert_eq!(
        books[0].book_url,
        "https://www.gushizu.com/novel/novel/index/novelId/959",
        "bookUrl 应取元素的 href"
    );
    assert_eq!(books[1].book_url, "https://www.gushizu.com/novel/novel/index/novelId/38");

    // 顺带确认字段规则与源定义一致时也走同一条路径
    let _ = SearchRule::default();
}

/// viewName 的三种形态：`'字面量'` 取引号内文字、JS 表达式求值、缺省回退 title。
/// 某聚合源的筛选控件全都写作 `"'请选择类型'"`，不解析就会把 `title`（如"平台"）
/// 当显示名，与手机端不一致。
#[test]
fn explore_view_name_resolves_literal_and_js() {
    let svc = service();
    let src = source(json!({
        "bookSourceName": "viewName",
        "bookSourceUrl": "https://viewname.test",
        "exploreUrl": "<js>JSON.stringify([\n\
            {\"title\":\"平台\",\"type\":\"select\",\"chars\":[\"番茄\"],\"viewName\":\"'请选择类型'\",\"url\":\"\"},\n\
            {\"title\":\"动态\",\"type\":\"button\",\"viewName\":\"'选'+'源'\",\"url\":\"\"},\n\
            {\"title\":\"缺省\",\"type\":\"button\",\"url\":\"\"}\n\
            ])</js>"
    }));

    let kinds = svc.explore_kinds(&src).unwrap();
    let by_title = |t: &str| kinds.iter().find(|k| k.title == t).unwrap();

    // `'请选择类型'` → 去掉引号的字面量
    assert_eq!(by_title("平台").view_name.as_deref(), Some("请选择类型"));
    // 非字面量形态按 JS 求值
    assert_eq!(by_title("动态").view_name.as_deref(), Some("选源"));
    // 没写 viewName → 保持 None，前端回退 title
    assert_eq!(by_title("缺省").view_name, None);
}

/// viewName 的 JS 报错不该让整页发现页失败。
#[test]
fn explore_view_name_failure_does_not_break_kinds() {
    let svc = service();
    let src = source(json!({
        "bookSourceName": "viewName-bad",
        "bookSourceUrl": "https://viewname-bad.test",
        "exploreUrl": "<js>JSON.stringify([\n\
            {\"title\":\"坏控件\",\"type\":\"button\",\"viewName\":\"throw new Error('nope')\",\"url\":\"\"},\n\
            {\"title\":\"总榜\",\"type\":\"url\",\"url\":\"/rank\"}\n\
            ])</js>"
    }));

    let kinds = svc.explore_kinds(&src).expect("viewName 报错不应让整页失败");
    assert_eq!(kinds.len(), 2);
    let bad = kinds.iter().find(|k| k.title == "坏控件").unwrap();
    assert_eq!(bad.view_name, None, "求值失败应回退为 None(前端用 title)");
    assert_eq!(kinds.iter().find(|k| k.title == "总榜").unwrap().url.as_deref(), Some("/rank"));
}

/// exploreUrl 求值也会触发 `java.refreshExplore()` 之类的副作用调用，
/// 这些残留必须被清掉，否则会被下一次无辜的 action 当成自己的意图。
#[test]
fn explore_kinds_does_not_leak_effects_into_next_action() {
    let svc = service();

    // 该类书源的形态：exploreUrl 顶层就调用 java.refreshExplore() / java.open / java.searchBook
    let noisy = source(json!({
        "bookSourceName": "脏源",
        "bookSourceUrl": "https://noisy.test",
        "exploreUrl": "<js>java.refreshExplore(); java.open('login','http',null); java.searchBook('x','y'); JSON.stringify([{title:'总榜',type:'url',url:'/rank'}]);</js>"
    }));
    let kinds = svc.explore_kinds(&noisy).expect("求值应当成功");
    assert_eq!(kinds.len(), 1);

    // 另一个源的纯数据处理 action：不该继承上面残留的意图
    let innocent = source(json!({
        "bookSourceName": "无辜源",
        "bookSourceUrl": "https://innocent.test"
    }));
    let outcome = svc
        .explore_action(&innocent, "'no-side-effect'", &HashMap::new())
        .expect("action 应当成功");
    assert!(!outcome.explore_refresh, "不得继承上一个源残留的刷新意图");
    assert!(outcome.intents.is_empty(), "不得继承残留的 open/search 意图: {:?}", outcome.intents);
}

/// `pick_json_field` 的 attrs 回退只在**元素快照**上生效：
/// 普通 JSON 源即便恰好带 `attrs` 字段，也不能被回退逻辑取错值。
#[test]
fn json_source_with_attrs_field_is_not_treated_as_element_snapshot() {
    use reader_rust::model::rule::SearchRule;
    use reader_rust::parser::rule_engine::RuleEngine;

    let source = source(json!({
        "bookSourceName": "普通JSON源",
        "bookSourceUrl": "https://json.test",
        "ruleSearch": {
            "bookList": "$.data",
            "name": "$.title",
            // 规则是裸属性名 href，而顶层没有 href，只有 attrs.href
            "bookUrl": "href"
        }
    }));

    // 注意 tag/outerHtml 都不存在 —— 这不是元素快照，不该走 attrs 回退
    let body = r#"{"data":[{"title":"书名C","attrs":{"href":"https://WRONG.example/x"}}]}"#;
    let engine = RuleEngine::new().unwrap();
    let books = engine.search_books(&source, body, "https://json.test");

    assert_eq!(books.len(), 1);
    assert_eq!(books[0].name, "书名C");
    assert_ne!(
        books[0].book_url, "https://WRONG.example/x",
        "普通 JSON 源的 attrs 字段不得被当成元素快照属性"
    );
    let _ = SearchRule::default();
}

/// 真正的元素快照（含 attrs + tag + outerHtml 三键）仍要能取到属性。
#[test]
fn element_snapshot_still_resolves_bare_attribute_rules() {
    use reader_rust::model::rule::SearchRule;
    use reader_rust::parser::rule_engine::RuleEngine;

    let source = source(json!({
        "bookSourceName": "快照源",
        "bookSourceUrl": "https://snap.test",
        "ruleSearch": {
            "bookList": "<js>JSON.stringify([{tag:'a',text:'书名D',innerHtml:'书名D',outerHtml:'<a href=\"/d\">书名D</a>',ownText:'书名D',attrs:{href:'/d'}}])</js>",
            "name": "text",
            "bookUrl": "href"
        }
    }));

    let engine = RuleEngine::new().unwrap();
    let books = engine.search_books(&source, "<html></html>", "https://snap.test");
    assert_eq!(books.len(), 1, "快照应能解析出书: {books:?}");
    assert_eq!(books[0].name, "书名D");
    assert_eq!(books[0].book_url, "https://snap.test/d");
    let _ = SearchRule::default();
}

/// `doc.select` 必须覆盖整个文档（含 head/title/meta），且**不能重复计数**。
///
/// 两个坑都踩过：只取 `body, body *` 会选不到 meta；而把 `html`/`body` 都当作
/// 可选根再逐个 select，会让同一批元素被累加（书列表翻倍）。
#[test]
fn jsoup_document_select_covers_head_without_duplicates() {
    let src = source(json!({
        "bookSourceName": "jsoup-doc",
        "bookSourceUrl": "https://jsoup-doc.test"
    }));
    let html = r#"<html><head>
        <title>站点标题</title>
        <meta property="og:novel:book_name" content="元数据书名">
      </head><body>
        <a class="row" href="/b/1">第一本</a>
        <a class="row" href="/b/2">第二本</a>
      </body></html>"#;

    with_source(&src, || {
        // head 内的 meta / title 要能选到
        let head_hit = reader_rust::parser::js::eval_js_with_bindings(
            "var d = org.jsoup.Jsoup.parse(result); d.select('meta[property=\"og:novel:book_name\"]').attr('content')",
            html,
            "",
            &HashMap::new(),
        )
        .expect("meta 应当可选");
        assert_eq!(head_hit, "元数据书名", "doc.select 应覆盖 head");

        // 同一批元素不得被重复计数
        let count = reader_rust::parser::js::eval_js_with_bindings(
            "var d = org.jsoup.Jsoup.parse(result); d.select('a.row').size()",
            html,
            "",
            &HashMap::new(),
        )
        .expect("a.row 应当可选");
        assert_eq!(count, "2", "元素不得因多个根而重复");

        // title 便捷方法
        let title = reader_rust::parser::js::eval_js_with_bindings(
            "org.jsoup.Jsoup.parse(result).title()",
            html,
            "",
            &HashMap::new(),
        )
        .expect("title 应当可读");
        assert_eq!(title, "站点标题");
    });
}

/// refreshExplore 必须登记"刷新发现页"，且**不能**被当成重登录。
#[test]
fn refresh_explore_is_distinct_from_relogin() {    let svc = service();
    let src = source(json!({
        "bookSourceName": "刷新",
        "bookSourceUrl": "https://refresh.test"
    }));

    let outcome = svc
        .explore_action(&src, "java.refreshExplore();", &HashMap::new())
        .expect("refreshExplore 应当执行成功");
    assert!(outcome.explore_refresh, "应登记发现页刷新");
    assert!(!outcome.login_refresh, "不得被误当成重登录视图");

    // reLoginView 仍保持自己的语义
    let relogin = svc
        .explore_action(&src, "java.reLoginView();", &HashMap::new())
        .expect("reLoginView 应当执行成功");
    assert!(relogin.login_refresh);
    assert!(!relogin.explore_refresh);
}

/// 某聚合源的搜索/登录按钮：action 里的 java.searchBook / java.open 要回传意图。
#[test]
fn explore_action_reports_search_and_open_intents() {
    let svc = service();
    let src = source(json!({
        "bookSourceName": "书山",
        "bookSourceUrl": "https://shushan.test"
    }));

    let search = svc
        .explore_action(
            &src,
            "java.searchBook(infoMap['☃关键词💭：书名/作者'] || '', source.bookSourceName + '::' + source.getKey())",
            &HashMap::from([("☃关键词💭：书名/作者".to_string(), "剑来".to_string())]),
        )
        .expect("searchBook 应当执行成功");
    let intent = search.intents.first().expect("应回传搜索意图");
    assert_eq!(intent["kind"], "search");
    assert_eq!(intent["payload"]["key"], "剑来");
    assert!(intent["payload"]["source"].as_str().unwrap_or("").contains("书山"));

    let open = svc
        .explore_action(&src, "java.open('login','http',null)", &HashMap::new())
        .expect("open 应当执行成功");
    assert_eq!(open.intents.first().unwrap()["kind"], "open");
    assert_eq!(open.intents.first().unwrap()["payload"]["type"], "login");
}

/// action 抛错时要变成可读错误，而不是静默成功。
#[test]
fn explore_action_surfaces_script_errors() {
    let svc = service();
    let src = source(json!({
        "bookSourceName": "报错",
        "bookSourceUrl": "https://boom.test"
    }));
    let err = svc
        .explore_action(&src, "throw new Error('boom');", &HashMap::new())
        .expect_err("脚本异常必须上报");
    assert!(format!("{err:?}").contains("boom"), "错误信息应含原始原因: {err:?}");
}
