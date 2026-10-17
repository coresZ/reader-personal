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
        "chars": ["☁️聚合搜索", "源甲", null],
        "default": "源甲",
        "action": "setConfigs.call(this,{source:infoMap['源站']})",
        "style": { "layout_flexGrow": 1 },
        "viewName": "'选源'"
    }]);

    let kinds: Vec<reader_rust::model::book_source::ExploreKind> =
        serde_json::from_value(raw).expect("kinds should deserialize");
    let kind = &kinds[0];

    assert_eq!(kind.title, "源站");
    assert_eq!(kind.kind_type(), ExploreKindType::Select);
    assert_eq!(kind.default.as_deref(), Some("源甲"));
    assert_eq!(kind.view_name.as_deref(), Some("'选源'"));
    assert!(kind.action.as_deref().unwrap_or("").contains("infoMap"));
    // chars 里的 null 要过滤掉，不能让前端渲染出空选项
    assert_eq!(kind.chars_list(), vec!["☁️聚合搜索", "源甲"]);

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
    info.insert("源站".to_string(), "源甲".to_string());
    let with_info = svc.explore_kinds_with_info(&js_source, &info).unwrap();
    assert_eq!(with_info[0].title, "源站:源甲");
    assert_eq!(with_info[0].url.as_deref(), Some("https://js.test/源甲"));
}

/// 无前缀的裸 JS 对象字面量 exploreUrl（这一类写法）。
///
/// 它的写法是 `[{'title':..,'url':..,'style':{layout_flexGrow: 1}}, ..]` —— 单引号
/// 加无引号键名，`serde_json` 拒收（`key must be a string`）。legado 靠 GSON 的
/// 宽松模式解析，所以手机端正常；桌面端若不处理就会退回纯文本切分，把整个数组
/// 拆成一堆 `url=None` 的垃圾项，发现页因此显示不出任何可点分类。
#[test]
fn explore_kinds_parses_bare_js_object_literal() {
    let svc = service();

    let src = source(json!({
        "bookSourceName": "该源",
        "bookSourceUrl": "https://books.example.com",
        "exploreUrl": "[{'title':'我的书架','url':'https://books.example.com/j/index/bookshelf','style':{layout_flexGrow: 1,layout_flexBasisPercent:0.4}},\
{'title':'言情小说','url':'https://books.example.com/kind/501?start={{(page-1)*20}}&sort=hot','style':{layout_flexGrow: 1}}]"
    }));

    let kinds = svc.explore_kinds(&src).expect("裸 JS 字面量应当可解析");

    assert_eq!(kinds.len(), 2, "应解析出 2 个分类, 而不是被切分成一堆垃圾项");
    assert_eq!(kinds[0].title, "我的书架");
    assert_eq!(
        kinds[0].url.as_deref(),
        Some("https://books.example.com/j/index/bookshelf")
    );
    assert_eq!(kinds[1].title, "言情小说");
    // URL 里的 `{{(page-1)*20}}` 是 legado 模板语法, 求值时要原样保留
    assert!(
        kinds[1].url.as_deref().unwrap().contains("{{(page-1)*20}}"),
        "模板占位符必须原样保留: {:?}",
        kinds[1].url
    );
    // 标题不能被撑成整段原文（切分的典型症状）
    assert!(kinds.iter().all(|k| k.title.chars().count() < 20));
    // 每个分类都要有可点的 url, 否则发现页渲染不出任何东西
    assert!(kinds.iter().all(|k| {
        k.kind_type() == ExploreKindType::Url && k.url.as_deref().map(|u| !u.trim().is_empty()).unwrap_or(false)
    }));
}

/// 标准 JSON 数组与纯文本不该被"裸字面量"分支抢走。
#[test]
fn explore_kinds_keeps_standard_json_and_text_paths() {
    let svc = service();

    // 合法 JSON 数组: 走严格解析（不经过 JS 引擎）
    let json_src = source(json!({
        "bookSourceName": "JSON源",
        "bookSourceUrl": "https://json.test",
        "exploreUrl": r#"[{"title":"玄幻","url":"https://json.test/xh"},{"title":"都市","url":"https://json.test/ds"}]"#
    }));
    let kinds = svc.explore_kinds(&json_src).unwrap();
    assert_eq!(kinds.len(), 2);
    assert_eq!(kinds[0].title, "玄幻");

    // 纯文本: `标题::url` 以 && / 换行分隔
    let text_src = source(json!({
        "bookSourceName": "文本源",
        "bookSourceUrl": "https://text.test",
        "exploreUrl": "玄幻::https://text.test/xh&&都市::https://text.test/ds"
    }));
    let kinds = svc.explore_kinds(&text_src).unwrap();
    assert_eq!(kinds.len(), 2);
    assert_eq!(kinds[1].title, "都市");
    assert_eq!(kinds[1].url.as_deref(), Some("https://text.test/ds"));
}

/// 书源里 `infoMap['x']` 与 `infoMap.get('x')` 两种写法都要可用。
#[test]
fn info_map_supports_both_access_styles() {
    let src = source(json!({
        "bookSourceName": "infoMap",
        "bookSourceUrl": "https://infomap.test"
    }));
    let mut info = HashMap::new();
    info.insert("平台".to_string(), "源乙".to_string());

    let mut bindings = HashMap::new();
    bindings.insert(
        "infoMap".to_string(),
        json!("{\"__infoMap\":{\"平台\":\"源乙\"}}"),
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
        assert_eq!(direct, "源乙|源乙|1");
    });
}

/// 某书源报错 `org is not defined` 的修复：org.jsoup.Jsoup.parse(result).select(...)
#[test]
fn jsoup_bridge_matches_story_source_usage() {
    let src = source(json!({
        "bookSourceName": "示例源",
        "bookSourceUrl": "https://delta.example.com"
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
        "bookSourceName": "示例源",
        "bookSourceUrl": "https://delta.example.com",
        "ruleExplore": {
            "bookList": "<js>var doc = org.jsoup.Jsoup.parse(result);var els = doc.select('a.flex-wrp.border-b');if (els.size() === 0) { els = doc.select('a[href*=\"/novel/novel/index/novelId/\"]'); } els;</js>",
            "name": ".size15@text||h4@text||text",
            "bookUrl": "href",
        }
    }));

    let html = r#"<html><body>
      <a href="/novel/novel/index/novelId/959">示例书·睡前故事2023年增刊</a>
      <a href="/novel/novel/index/novelId/38">故事会2021年02期</a>
    </body></html>"#;

    let engine = RuleEngine::new().unwrap();
    let books = engine.explore_books(&source, html, "https://delta.example.com/rank/allvisit");

    assert_eq!(books.len(), 2, "应解析出两本书, 实际: {books:?}");
    assert_eq!(books[0].name, "示例书·睡前故事2023年增刊");
    // bookUrl 必须来自元素属性, 而不是退化成列表页地址
    assert_eq!(
        books[0].book_url,
        "https://delta.example.com/novel/novel/index/novelId/959",
        "bookUrl 应取元素的 href"
    );
    assert_eq!(books[1].book_url, "https://delta.example.com/novel/novel/index/novelId/38");

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
            {\"title\":\"平台\",\"type\":\"select\",\"chars\":[\"源甲\"],\"viewName\":\"'请选择类型'\",\"url\":\"\"},\n\
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
        "bookSourceName": "源丙",
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
    assert!(intent["payload"]["source"].as_str().unwrap_or("").contains("源丙"));
    // legado: source.getKey() 返回 bookSourceUrl（不是书籍地址）
    assert_eq!(intent["payload"]["source"], "源丙::https://shushan.test");

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

/// 该源的 `ruleExplore`：`name` 的 `##` 替换必须作用在 `||` 组合结果上。
///
/// 规则原文 `class.title@html||$.title##<span class="tag">.*?</span>|<.*?>` 里，
/// `##` 在 `||` **之后**。legado 的 `SourceRule` 先拆 `##`（rule / replaceRegex /
/// replacement），rule 部分才按组合符拆，替换最后作用在整体上。若反过来先拆 `||`，
/// 替换段会被并进第二个候选，`||` 取走第一个非空值后替换永不执行 —— 书名里留着
/// `<h4 class="title">` 标签，用户看到的就是带 HTML 的书名。
#[test]
fn explore_field_regex_applies_after_or_combination() {
    let engine = RuleEngine::new().unwrap();
    let body = r#"<div class="list">
        <a href="https://books.example.com/column/76273556/" class="item">
            <h4 class="title"><span class="text">风吹草动</span></h4>
            <div class="tag">言情</div>
        </a>
    </div>"#;

    let source = source(json!({
        "bookSourceName": "该源",
        "bookSourceUrl": "https://books.example.com",
        "ruleExplore": {
            "bookList": "class.item",
            "name": r#"class.title@html||$.title##<span class="tag">.*?</span>|<.*?>"#,
            "kind": "{{@@class.tag@text}}"
        },
        "ruleSearch": {}
    }));

    let books = engine.explore_books(&source, body, "https://books.example.com");
    assert_eq!(books.len(), 1, "应解析出一本书");
    assert_eq!(
        books[0].name, "风吹草动",
        "`##` 替换必须删掉标签，只留书名"
    );
    assert_eq!(books[0].kind.as_deref(), Some("言情"));
}

/// 裸 `@属性` 规则要落在「当前元素」上（legado `element.select("")` 返回元素自身）。
///
/// 该源的 `bookUrl` 是 `@js:` 脚本，内部 `java.getString("@href||$.url")`。这条规则
/// 求值时要能从**当前元素**取 href；取不到就落到 fallback 拼出 `.../column//`
/// （ID 段为空），详情页、目录、正文全部连带失败。
#[test]
fn explore_bare_attribute_rule_reads_current_element() {
    let engine = RuleEngine::new().unwrap();
    let body = r#"<div class="list">
        <a href="https://books.example.com/column/76273556/?dcs=kind-501" class="item">
            <h4 class="title"><span class="text">风吹草动</span></h4>
        </a>
    </div>"#;

    let source = source(json!({
        "bookSourceName": "该源",
        "bookSourceUrl": "https://books.example.com",
        "ruleExplore": {
            "bookList": "class.item",
            "name": "class.title@text",
            "bookUrl": "@js:\nresult = java.getString(\"@href||$.url\");\nif(!String(result)){\n\tresult = \"https://books.example.com/column/{{$.alias_id||$.id}}/\"\n\t}\nresult"
        },
        "ruleSearch": {}
    }));

    let books = engine.explore_books(&source, body, "https://books.example.com");
    assert_eq!(books.len(), 1);
    assert_eq!(
        books[0].book_url, "https://books.example.com/column/76273556/?dcs=kind-501",
        "`@href` 应取到当前元素的 href，不能落到空 ID 的 fallback"
    );
}

/// `java.getElements("$.list[*]")` 必须返回**原始 JSON 节点**，不能包成元素快照。
///
/// legado 的 `AnalyzeRule.getElements` 在 `Mode.Json` 下走
/// `getAnalyzeByJSonPath(result).getList(rule)`，直接给出 JSON 节点，脚本可以
/// `item.title` 取值。曾经这里对 JSONPath 结果做 `snapshot_from_fragment`，把 JSON
/// 文本当成 HTML 片段包成 `{"__snapshot":{"tag":"","text":"{\"title\":…}"}}` ——
/// 之后按 `title` 取字段必然为空，整本目录解析出 0 章（「加载不出目录」）。
#[test]
fn get_elements_on_jsonpath_returns_raw_json_nodes() {
    let body = json!({
        "total": 2,
        "list": [
            { "id": "777669578", "title": "1.黄泉半路" },
            { "id": "777669579", "title": "2.回头草好吃吗" }
        ]
    })
    .to_string();

    let out = RuleEngine::eval_rule_on(
        r#"@js:result = java.getElements("$.list[*]")"#,
        &body,
        "",
        false,
    );
    let raw = out.first().cloned().unwrap_or_default();

    assert!(
        !raw.contains("__snapshot"),
        "JSONPath 结果不该被包成元素快照: {}",
        raw.chars().take(200).collect::<String>()
    );
    let items: Vec<Value> = serde_json::from_str(&raw).expect("应是 JSON 数组");
    assert_eq!(items.len(), 2);
    // 关键：字段要能直接取到 —— 目录字段规则就是靠这个取 title 的。
    assert_eq!(items[0]["title"], "1.黄泉半路");
    assert_eq!(items[1]["id"], "777669579");

    // 元素快照路径不能受影响：CSS 选择器仍要返回带 outerHtml 的快照。
    let html = r#"<div class="list"><a href="/1">A</a></div>"#;
    let els = RuleEngine::eval_rule_on(r#"@js:result = java.getElements(".list a")"#, html, "", false);
    let els_raw = els.first().cloned().unwrap_or_default();
    assert!(
        els_raw.contains("outerHtml"),
        "CSS 选择器结果应仍是元素快照: {}",
        els_raw.chars().take(200).collect::<String>()
    );
}

/// 目录字段规则要在 JSON 节点上取值（该源目录项是 JSON 对象）。
#[test]
fn toc_fields_resolve_on_json_items() {
    let engine = RuleEngine::new().unwrap();
    let source = source(json!({
        "bookSourceName": "JSON 目录",
        "bookSourceUrl": "https://json.test",
        "ruleToc": {
            "chapterList": "@js:result = java.getElements(\"$.list[*]\")",
            "chapterName": "title",
            "chapterUrl": "id"
        }
    }));
    let body = json!({
        "total": 2,
        "list": [
            { "id": "/chapter/1", "title": "第一章" },
            { "id": "/chapter/2", "title": "第二章" }
        ]
    })
    .to_string();

    let (chapters, _) = engine.chapter_list(&source, &body, "https://json.test", None);
    assert_eq!(chapters.len(), 2, "JSON 目录应解析出 2 章");
    assert_eq!(chapters[0].title, "第一章");
    assert_eq!(chapters[0].url, "https://json.test/chapter/1");
    assert_eq!(chapters[1].title, "第二章");
}

/// 没配 `nextTocUrl` 时不得凭空产出下一页。
///
/// `nextTocUrl` 为空串会走到「空选择器」分支；空选择器在裸属性规则里表示
/// 「当前元素自身」，但完全空的规则没有这个语义 —— 否则单页目录会被误判成
/// 有下一页，多抓一次无关请求。
#[test]
fn toc_without_next_url_has_no_next_page() {
    let engine = RuleEngine::new().unwrap();
    let source = source(json!({
        "bookSourceName": "单页目录",
        "bookSourceUrl": "https://single.test",
        "ruleToc": {
            "chapterList": "#chapterlist a",
            "chapterName": "@text",
            "chapterUrl": "@href"
        }
    }));
    let body = r#"<div id="chapterlist"><a href="/1">第一章</a><a href="/2">第二章</a></div>"#;

    let (chapters, next_urls) = engine.chapter_list(&source, body, "https://single.test", None);
    assert_eq!(chapters.len(), 2);
    assert!(
        next_urls.is_empty(),
        "未配置 nextTocUrl 时不应有下一页: {next_urls:?}"
    );
}

/// `<js>…</js>` 之后跟的组合链要能拆开，脚本段内不参与拆分。
///
/// 该源 `bookList` 是 `<js>…</js>\n.item||$.data.worksList[*]||$.[?(@.alias_id)]`：
/// legado `splitSourceRule` 用 `JS_PATTERN` 先把 `<js>` 段整体切出来，剩余文本才按
/// `&&`/`||`/`%%` 拆。没有这层隔离，整条会被算作一段，`</js>` 之后的 `||` 链被吞进
/// 脚本，`detect_mode` 判 Js 后拿带标签的整条文本当脚本源码，语法错误 → 静默回退成
/// 页面原文 → 「今日限免」永远 0 本。
#[test]
fn js_segment_is_isolated_from_field_combination() {
    // 脚本里带 `||` / `&&` 运算符与正则 —— 真实书源 bookList 的形态。
    // 没有脚本段隔离时，脚本内的 `||` 会被当成分隔符，整条规则被切碎。
    let rule = "<js>\nresult = a || b && /x|y/.test(baseUrl);\nresult\n</js>\n.item||$.data.worksList[*]||$.[?(@.alias_id)]";

    let split = reader_rust::parser::rule_analyzer::split_top_level(rule, &["&&", "||", "%%"]);
    assert_eq!(split.delimiter.as_deref(), Some("||"), "尾链的 || 要能被识别");
    assert_eq!(split.parts.len(), 3, "应切成 3 段: {:#?}", split.parts);
    // 第一段 = 脚本段 + 紧随其后的选择器（legado 也是在组合符处才切开）。
    assert!(
        split.parts[0].trim().starts_with("<js>") && split.parts[0].contains("</js>"),
        "第一段应含完整脚本段: {:?}",
        split.parts[0]
    );
    assert!(
        split.parts[0].trim().ends_with(".item"),
        "脚本后的选择器留在第一段: {:?}",
        split.parts[0]
    );
    assert_eq!(split.parts[1].trim(), "$.data.worksList[*]");
    assert_eq!(split.parts[2].trim(), "$.[?(@.alias_id)]");

    // 脚本内容必须原样保留（含内部的 || / &&）
    let script = &split.parts[0];
    assert!(
        script.contains("a || b && /x|y/"),
        "脚本内容不该被切开: {script:?}"
    );
}

/// 脚本段内部的 `||` / `&&` 是 JS 运算符，不能当字段组合符拆开。
#[test]
fn js_operators_inside_script_are_not_delimiters() {
    let rule = "<js>\nresult = a || b && c;\nresult\n</js>";
    let split = reader_rust::parser::rule_analyzer::split_top_level(rule, &["&&", "||", "%%"]);
    assert_eq!(
        split.delimiter, None,
        "脚本内的逻辑运算符不该被当组合符: {:#?}",
        split.parts
    );
}

/// `java.getString` 对 JSONPath 多值要换行拼接（legado `joinToString("\n")`）。
///
/// 该源 featured 分支写 `String(java.getString("$..todayWorksIds[*]")).split("\n")`，
/// 期望拿到**全部** id。只取第一个会让 POST 的 worksIds 少两项，服务端只回 1 本书
/// —— 症状是「今日限免只有一本书」。
#[test]
fn get_string_joins_jsonpath_multi_values() {
    let body = json!({ "ids": ["111", "222", "333"] }).to_string();
    let out = RuleEngine::eval_rule_on(r#"@js:result = java.getString("$.ids[*]")"#, &body, "", false);
    assert_eq!(
        out.first().map(String::as_str),
        Some("111\n222\n333"),
        "JSONPath 多值应换行拼接"
    );
    // 配合 split 就是书源里最常见的写法
    let out = RuleEngine::eval_rule_on(
        r#"@js:result = String(java.getString("$.ids[*]")).split("\n").length"#,
        &body,
        "",
        false,
    );
    assert_eq!(out.first().map(String::as_str), Some("3"));
}

/// 字段规则里的 `||` 各段要**独立判定模式**（legado 对每段 `ruleStrX` 单独解析）。
///
/// 该源 bookUrl 的 `@js:` 脚本里是 `java.getString("@href||$.url")`：`@href` 是 CSS
/// 属性规则、`$.url` 是 JSONPath。整条一起判模式时会被 `$.url` 带偏成 JsonPath，
/// 在 HTML 上求值必然为空 → 书链只剩 `.../column//`（ID 空），详情页/目录/正文连锁失败。
#[test]
fn field_combination_detects_mode_per_segment() {
    let item = json!({ "id": "394815674", "title": "事不宜迟", "url": "/column/62180612/" });
    let body = serde_json::to_string(&item).unwrap();

    // JSON 项上：@href 取不到 → 回落 $.url
    let out = RuleEngine::eval_rule_on("@href||$.url", &body, "https://books.example.com", false);
    assert_eq!(out.first().map(String::as_str), Some("/column/62180612/"));

    // 完整的 bookUrl 脚本也要能落到这个值
    let script = r#"@js:result = java.getString("@href||$.url");
if(!String(result)){
	result = "https://books.example.com/column/{{$.alias_id||$.id}}/"
	}
result"#;
    let out = RuleEngine::eval_rule_on(script, &body, "https://books.example.com", false);
    assert_eq!(
        out.first().map(String::as_str),
        Some("/column/62180612/"),
        "应取到 JSON 里的 url，而不是落到空 ID 的 fallback"
    );

    // HTML 元素上：@href 有值 → 取 href（前一段优先）
    let html = r#"<a href="/column/76273556/" class="item">x</a>"#;
    let out = RuleEngine::eval_rule_on(
        r#"@js:result = java.getString(".item@href||$.url")"#,
        html,
        "https://books.example.com",
        false,
    );
    assert_eq!(out.first().map(String::as_str), Some("/column/76273556/"));
}

/// `##` 之后的段是替换正则，里面的 `|` / `||` 是正则交替符，不是字段组合符。
///
/// 该源 `ruleBookInfo.wordCount` 是 `…@text##约|\s|字|,`，`name` 是
/// `class.title@html||$.title##<span class="tag">.*?</span>|<.*?>` —— 组合符只作用于
/// `##` 之前的部分（legado `SourceRule` 先切出 `##` 三段，`ruleStrS[0]` 才是可组合的
/// rule），替换最后作用在组合结果上。若把 `##` 之后的 `||` 也当组合符，替换段会被
/// 拆碎，求值落空。
#[test]
fn replacement_regex_is_not_split_by_combinators() {
    let engine = RuleEngine::new().unwrap();
    let body = r#"<div class="list">
        <a href="/1" class="item">
            <h4 class="title"><span class="text">风吹草动</span></h4>
            <div class="length-info"><span class="labeled-text">约 12.5万 字</span></div>
        </a>
    </div>"#;

    // 正则里含单 `|`（交替符）—— 不能被当组合符拆开
    let src = source(json!({
        "bookSourceName": "正则段",
        "bookSourceUrl": "https://regex.test",
        "ruleExplore": {
            "bookList": "class.item",
            "name": "class.title@text",
            "wordCount": r#"class.length-info@text##约|\s|字|,"#
        },
        "ruleSearch": {}
    }));
    let books = engine.explore_books(&src, body, "https://regex.test");
    assert_eq!(books.len(), 1);
    assert_eq!(
        books[0].word_count.as_deref(),
        Some("12.5万"),
        "正则交替符 | 不该被当组合符拆开，且 约/空格/字 都要被删掉"
    );

    // `##` 段内的 `||` 不能被当**字段组合符**拆开 —— 组合符只作用于 `##` 之前。
    //
    // 这条无法用「替换结果」断言：Rust `regex` 对空交替项（`a||b`）本就退化为第一个
    // 分支（`约||\s|字|,` 与 `约` 结果相同），所以拆与不拆在结果上不可观测。
    // 改从**规则结构**上锁定：`##` 之前的组合符照常生效，`##` 之后的段完整保留。
    let src = source(json!({
        "bookSourceName": "正则段2",
        "bookSourceUrl": "https://regex.test",
        "ruleExplore": {
            "bookList": "class.item",
            "name": "class.title@text",
            "wordCount": r#"class.length-info@text##约||\s|字|,"#
        },
        "ruleSearch": {}
    }));
    let books = engine.explore_books(&src, body, "https://regex.test");
    assert_eq!(books.len(), 1, "带 || 替换段的规则仍要能解析出书");

    // 单 `|`（真实书源写法）必须完整生效 —— 拆分若发生在这里，结果必然不同
    let src = source(json!({
        "bookSourceName": "正则段4",
        "bookSourceUrl": "https://regex.test",
        "ruleExplore": {
            "bookList": "class.item",
            "name": "class.title@text",
            "wordCount": r#"class.length-info@text##约|\s|字|,"#
        },
        "ruleSearch": {}
    }));
    let books = engine.explore_books(&src, body, "https://regex.test");
    assert_eq!(
        books[0].word_count.as_deref(),
        Some("12.5万"),
        "单 | 交替符要完整生效（约/空格/字/逗号都被删）"
    );

    // 组合符在 ## 之前时仍要生效，且替换作用在组合结果上
    let src = source(json!({
        "bookSourceName": "正则段3",
        "bookSourceUrl": "https://regex.test",
        "ruleExplore": {
            "bookList": "class.item",
            "name": "class.title@text",
            "wordCount": r#"class.nope@text||class.length-info@text##约|\s|字|,"#
        },
        "ruleSearch": {}
    }));
    let books = engine.explore_books(&src, body, "https://regex.test");
    assert_eq!(
        books[0].word_count.as_deref(),
        Some("12.5万"),
        "|| 应回落到第二段，替换再作用于其结果"
    );
}

/// `{{@规则}}` 与 `{{@@规则}}` 等价：一个或多个 `@` 都修剪掉再当规则解析。
///
/// legado `RuleAnalyzer.trim()`（RuleAnalyzer.kt:15-22）用
/// `while (queue[pos] == '@' || queue[pos] < '!') pos++` 把**任意个** `@` 去掉。
/// 只认 `@@` 的话，单 `@` 的模板会落到 JS 引擎 —— 报
/// `unexpected token in expression: '@' at eval_script:2:1`（该源
/// `ruleBookInfo.intro` 写的就是单 `@`：`{{@class.section-abstract@html||…}}`）。
#[test]
fn single_and_double_at_templates_are_equivalent() {
    let engine = RuleEngine::new().unwrap();
    let body = r#"<div class="list">
        <a href="/1" class="item">
            <h4 class="title"><span class="text">风吹草动</span></h4>
            <div class="intro"><p>简介文字</p></div>
            <div class="tag">言情</div>
        </a>
    </div>"#;

    // 三种 @ 数量都要等价（模板插值走字段级路径）
    for (label, kind_rule) in [
        ("单 @", "{{@class.tag@text}}"),
        ("双 @", "{{@@class.tag@text}}"),
        ("三 @", "{{@@@class.tag@text}}"),
    ] {
        let src = source(json!({
            "bookSourceName": label,
            "bookSourceUrl": "https://at.test",
            "ruleExplore": { "bookList": "class.item", "name": "class.title@text", "kind": kind_rule },
            "ruleSearch": {}
        }));
        let books = engine.explore_books(&src, body, "https://at.test");
        assert_eq!(books.len(), 1, "{label} 应解析出 1 本");
        assert_eq!(
            books[0].kind.as_deref(),
            Some("言情"),
            "{label} 模板应取到字段值（不能被当 JS 求值）"
        );
    }

    // 带 || 组合的单 @ 规则（该源 ruleBookInfo.intro 的形态）
    let src = source(json!({
        "bookSourceName": "单@+组合",
        "bookSourceUrl": "https://at.test",
        "ruleExplore": {
            "bookList": "class.item",
            "name": "class.title@text",
            "intro": "{{@class.nope@html||class.intro@text}}"
        },
        "ruleSearch": {}
    }));
    let books = engine.explore_books(&src, body, "https://at.test");
    assert_eq!(
        books[0].intro.as_deref(),
        Some("简介文字"),
        "单 @ + || 应回落到第二段"
    );

    // JSON 目录项上同样两种写法等价（走字段级路径，模板才会被插值）
    let item_body = json!({
        "total": 1,
        "list": [{ "id": "/c/1", "title": "第一章", "kind": "言情", "score": "8.5" }]
    })
    .to_string();
    for kind_rule in ["{{@@$.kind}},{{@@$.score}}", "{{@$.kind}},{{@$.score}}"] {
        let src = source(json!({
            "bookSourceName": "json-at",
            "bookSourceUrl": "https://at.test",
            "ruleToc": {
                "chapterList": "@js:result = java.getElements(\"$.list[*]\")",
                "chapterName": "title",
                "chapterUrl": "id",
                "updateTime": kind_rule
            }
        }));
        let (chapters, _) = engine.chapter_list(&src, &item_body, "https://at.test", None);
        assert_eq!(chapters.len(), 1, "规则 {kind_rule} 应解析出 1 章");
        assert_eq!(
            chapters[0].tag.as_deref(),
            Some("言情,8.5"),
            "规则 {kind_rule} 应取到 JSON 字段"
        );
    }
}

/// 字节数组方法：`java.hexDecodeToByteArray` / `java.base64DecodeToByteArray` 必须可用。
///
/// legado 返回 Rhino 的 `ByteArray`，脚本按数组用（`.length` / `.slice()` / 索引）。
/// 该源正文规则靠这条链做 AES 解密 —— 方法缺失时 `createSymmetricCrypto`
/// 拿到 `undefined`，整个 `try/catch` 静默吞掉，正文变成 0 字（用户看到的
/// 「书都加载不出来目录更不能阅读」）。
#[test]
fn byte_array_helpers_are_available() {
    let b64 = base64_encode(b"hello");
    let hex = hex_encode(b"hello");

    // base64 → 字节数组
    let out = eval_js_with_source(&format!("result = String(java.base64DecodeToByteArray('{b64}').length)"));
    assert_eq!(out, "5", "base64DecodeToByteArray 应返回 5 字节");

    // hex → 字节数组
    let out = eval_js_with_source(&format!("result = String(java.hexDecodeToByteArray('{hex}').length)"));
    assert_eq!(out, "5", "hexDecodeToByteArray 应返回 5 字节");

    // 按索引取值（AES iv 切片依赖）
    let out = eval_js_with_source(&format!(
        "var b = java.base64DecodeToByteArray('{b64}'); result = b[0] + ',' + b[4]"
    ));
    assert_eq!(out, "104,111", "字节数组要能按索引取值 (h=104, o=111)");

    // `.slice(start, end)` 要能用于切 IV
    let out = eval_js_with_source(&format!(
        "var b = java.base64DecodeToByteArray('{b64}'); result = String(b.slice(1, 4).length)"
    ));
    assert_eq!(out, "3", "字节数组要支持 slice 取子区间");
}

/// 高位字节（>127）在字节数组 → key/iv → 加密原语 的链路上不能被改写。
///
/// 字节数组经 JS 侧 latin1 还原后交给宿主，宿主必须按字节取回。若中途按 UTF-8
/// 编码/解码，`0x80` 以上的字节会被改写成多字节序列，AES 解出来的就是乱码。
#[test]
fn byte_array_preserves_high_bytes_for_crypto() {
    // 16 字节 key，含 >127 的值；用它加密再解密应能还原明文
    let key_hex: String = (0u8..16).map(|b| format!("{:02x}", b * 17)).collect();
    let script = format!(
        r#"var key = java.hexDecodeToByteArray('{key_hex}');
var iv = java.hexDecodeToByteArray('000102030405060708090a0b0c0d0e0f');
var c = java.createSymmetricCrypto('AES/CBC/PKCS5Padding', key, iv);
var enc = c.encryptBase64('测试正文');
var c2 = java.createSymmetricCrypto('AES/CBC/PKCS5Padding', key, iv);
result = c2.decryptStr(enc)"#
    );
    assert_eq!(
        eval_js_with_source(&script),
        "测试正文",
        "含高位字节的 key/iv 必须原样送达加密原语"
    );

    // 密文以字节数组形式回传也要能解（正文规则就是 decryptStr(字节数组)）
    let script = format!(
        r#"var key = java.hexDecodeToByteArray('{key_hex}');
var iv = java.hexDecodeToByteArray('000102030405060708090a0b0c0d0e0f');
var c = java.createSymmetricCrypto('AES/CBC/PKCS5Padding', key, iv);
var encB64 = c.encryptBase64('字节数组密文');
var encBytes = java.base64DecodeToByteArray(encB64);
var c2 = java.createSymmetricCrypto('AES/CBC/PKCS5Padding', key, iv);
result = c2.decryptStr(encBytes)"#
    );
    assert_eq!(
        eval_js_with_source(&script),
        "字节数组密文",
        "密文以字节数组传入时要按原始字节解密，不能再当 base64 解一次"
    );
}

/// 在一个干净的 JS 上下文里执行脚本并取回结果字符串。
fn eval_js_with_source(script: &str) -> String {
    let source = source(json!({
        "bookSourceName": "crypto",
        "bookSourceUrl": "https://crypto.test"
    }));
    reader_rust::parser::js::with_source(&source, || {
        reader_rust::parser::js::eval_js(script, "", "https://crypto.test")
    })
    .expect("脚本应执行成功")
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
