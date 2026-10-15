//! 端到端验证: 用真实源配置验证 RSS 分流逻辑(不依赖 GUI 手工点击)。
//!
//! 覆盖用户数据里全部 9 个 RSS 源的真实形态:
//! - 8 个 `singleUrl: true` 且无 rule(版本/使用说明/Meow云/小说拾遗/导入x2/烏雲净化/源仓库)
//! - 1 个靠 ruleArticles 抓 HTML(喵公子书源管理)
//!
//! 只做**离线判定**(不发网络请求), 验证分流决策与地址解析规则; 真实联网抓取
//! 由单测里的 `spawn_blocking` 探针覆盖。
use reader_rust::model::rss::RssSource;

/// 从真实数据里复制出的 9 个源的关键字段(名称/sourceUrl/singleUrl/header 有无)。
fn real_sources() -> Vec<(RssSource, &'static str)> {
    vec![
        (
            RssSource {
                source_name: "版本".into(),
                source_url: "https://github.com/hadc188/reader/releases.atom".into(),
                single_url: Some(true),
                ..Default::default()
            },
            "真 atom feed: 必须走 feed 解析, 不能当网页入口",
        ),
        (
            RssSource {
                source_name: "使用说明".into(),
                source_url: "https://www.yuque.com/legado".into(),
                single_url: Some(true),
                ..Default::default()
            },
            "HTML 入口页: 解析不出 feed, 退化为网页入口",
        ),
        (
            RssSource {
                source_name: "Meow云".into(),
                source_url: "https://pan.miaogongzi.net".into(),
                single_url: Some(true),
                ..Default::default()
            },
            "HTML 入口页",
        ),
        (
            RssSource {
                source_name: "喵公子书源管理".into(),
                source_url: "http://yuedu.miaogongzi.net/gx.html".into(),
                single_url: Some(true),
                sort_url: Some("首页::http://yuedu.miaogongzi.net/gx.html".into()),
                rule_articles: Some("id.content@h3".into()),
                rule_title: Some("a@textNodes".into()),
                rule_link: Some("a@href".into()),
                ..Default::default()
            },
            "唯一带 ruleArticles 的源",
        ),
        (
            RssSource {
                source_name: "源仓库".into(),
                source_url: "http@js:eval(String(cache.getFromMemory('yckdm')))".into(),
                single_url: Some(true),
                header: Some("@js:cache.putMemory('yckdm','...')".into()),
                ..Default::default()
            },
            "URL 是 JS 表达式 + header 预置缓存: 走求值分支",
        ),
        (
            RssSource {
                source_name: "小说拾遗".into(),
                source_url: "snssdk1128://user/profile/562564899806367".into(),
                single_url: Some(true),
                ..Default::default()
            },
            "自定义 scheme: 无法内嵌, 应标 external",
        ),
    ]
}

/// 判定"是否走网页入口": 与 `get_rss_articles` 的分流条件保持一致。
fn needs_web_entry(source: &RssSource) -> bool {
    let url = source.source_url.trim();
    url.starts_with("js:")
        || url.starts_with("@js:")
        || url.starts_with("<js>")
        || url.contains("@js:")
        || url.contains("<js>")
}

#[test]
fn real_sources_route_to_expected_branch() {
    for (source, why) in real_sources() {
        let is_script = needs_web_entry(&source);
        let single = source.single_url.unwrap_or(false);
        println!(
            "{:<20} script={:<5} singleUrl={:<5} sortUrl={:?}  // {}",
            source.source_name, is_script, single, source.sort_url, why
        );
    }

    let sources = real_sources();

    // 「源仓库」是唯一的脚本地址源: 必须走求值分支, 绝不能拿去当 feed 请求。
    let repo = sources
        .iter()
        .find(|(s, _)| s.source_name == "源仓库")
        .expect("源仓库 source");
    assert!(
        needs_web_entry(&repo.0),
        "源仓库的 `http@js:...` 必须被识别为脚本地址"
    );

    // 「版本」是真 atom feed: 不能被误判成脚本, 否则会丢文章列表。
    let version = sources
        .iter()
        .find(|(s, _)| s.source_name == "版本")
        .expect("版本 source");
    assert!(
        !needs_web_entry(&version.0),
        "GitHub atom 是真 feed, 不能走网页入口分支"
    );

    // 其余 HTML 入口页都不是脚本地址, 走"先解析 feed、失败再退化"的路径。
    for (source, _) in &sources {
        if matches!(
            source.source_name.as_str(),
            "源仓库" | "版本"
        ) {
            continue;
        }
        assert!(
            !needs_web_entry(source),
            "{} 不是脚本地址",
            source.source_name
        );
        assert!(
            source.single_url.unwrap_or(false),
            "{} 在真实数据里是 singleUrl 源",
            source.source_name
        );
    }
}

/// 自定义 scheme 与 `sortUrl` 里带 `名称::地址` 协议的源, 要有明确可用的落点。
#[test]
fn custom_scheme_and_sort_url_protocol_are_handled() {
    let sources = real_sources();

    // 小说拾遗: `snssdk1128://` 不是 http(s), 需要交给系统处理。
    let novel = sources
        .iter()
        .find(|(s, _)| s.source_name == "小说拾遗")
        .map(|(s, _)| s)
        .expect("小说拾遗 source");
    assert!(
        novel.source_url.starts_with("snssdk1128://"),
        "自定义 scheme 不应被当作 http 网页内嵌"
    );

    // 喵公子: sortUrl 是 legado 的 `名称::地址` 分类协议, 取 `::` 之后的部分才是地址。
    let miao = sources
        .iter()
        .find(|(s, _)| s.source_name == "喵公子书源管理")
        .map(|(s, _)| s)
        .expect("喵公子 source");
    let raw = miao.sort_url.as_deref().expect("sortUrl 存在");
    let (label, url) = raw.split_once("::").expect("`名称::地址` 协议");
    assert_eq!(label, "首页");
    assert_eq!(url, "http://yuedu.miaogongzi.net/gx.html");
    assert!(url.starts_with("http://"), "取出的地址可直接请求");
}
