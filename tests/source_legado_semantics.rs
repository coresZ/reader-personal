//! legado（jayway JsonPath / Rhino / Android 运行时）与桌面端实现的语义差异回归。
//!
//! 每一条都对应一个真实症状：发现页空白、搜索 0 条、每本书地址都一样。
use reader_rust::parser::jsonpath::jsonpath_query;
use serde_json::json;

/// jayway 里 `.[?(…)]` 的 `.` 是**递归下降**起点，等价于 `$..[?(…)]`。
///
/// 书源常写 `.[?(@.bookName)]||.[?(@.title)]`（不写 `$`、不写 `..`）做「全文档找书」。
/// 早先的实现把前导点剥掉、拼成 `$.?(@.x)` —— 非法路径，恒定 0 项，
/// 症状是**搜索永远 0 条、发现页空白**。
#[test]
fn leading_dot_filter_is_recursive_like_jayway() {
    // 书列表藏在两层对象里，只有递归下降才找得到。
    let doc = json!({
        "status": 1,
        "data": [
            {"title": "甲", "bid": 1},
            {"title": "乙", "bid": 2}
        ],
        "aladdin": {"title": "丙", "bid": 3}
    });

    let found = jsonpath_query(&doc, ".[?(@.title)]");
    assert_eq!(found.len(), 3, "前导点过滤应按递归下降找出 3 项");

    // 与显式递归写法结果一致。
    let explicit = jsonpath_query(&doc, "$..[?(@.title)]");
    assert_eq!(explicit.len(), 3);
}

/// 普通前导点路径（`.data.books`）**不能**变成递归下降，否则语义反转。
#[test]
fn leading_dot_plain_path_stays_shallow() {
    let doc = json!({
        "data": {"books": [1, 2]},
        "other": {"data": {"books": [9]}}
    });
    let found = jsonpath_query(&doc, ".data.books");
    assert_eq!(found.len(), 2, "普通路径应只取根下的 data.books");
}

/// `{{$.a||$.b}}` 里的 `||` 是 legado 的回退链（第一个非空），不是 JSONPath 语法。
#[test]
fn json_field_fallback_chain_picks_first_non_empty() {
    use reader_rust::parser::rule_engine::pick_json_field_for_test;

    let node = json!({"bid": 8875162, "title": "某书"});
    // 新字段名不存在时回退到旧字段名。
    assert_eq!(
        pick_json_field_for_test(&node, "$.bookId||$.bid").as_deref(),
        Some("8875162")
    );
    // 第一个有值时不再看后面。
    assert_eq!(
        pick_json_field_for_test(&node, "$.title||$.bid").as_deref(),
        Some("某书")
    );
    // 全都取不到时为空（不 panic）。
    assert_eq!(pick_json_field_for_test(&node, "$.nope||$.nada"), None);
}

/// `&&` 是拼接语义（全部求值后按行合并），与 `||` 不同。
#[test]
fn json_field_and_joins_all_values() {
    use reader_rust::parser::rule_engine::pick_json_field_for_test;
    let node = json!({"a": "1", "b": "2"});
    assert_eq!(
        pick_json_field_for_test(&node, "$.a&&$.b").as_deref(),
        Some("1\n2")
    );
}

/// URL 选项 `,{"body":…,"method":"POST"}` 必须跟着地址走。
///
/// 书源的 `bookUrl` / `tocUrl` 规则常返回这种复合串（`GetUrl()` 拼出来的），
/// legado 的 `getAbsoluteURL` 对绝对地址原样返回，选项由后续 `AnalyzeUrl` 解析。
/// 早先剥掉选项 → 请求变成无 body 的 GET → 服务端拿不到 bookId，
/// 症状是**搜索有结果但每本书地址都一样、点进去详情/目录全空**。
#[test]
fn url_options_survive_resolution() {
    use reader_rust::parser::rule_engine::resolve_url_for_test;

    let raw = r#"https://example.test/api/book/info,{"body":"bookId=1","method":"POST"}"#;
    let resolved = resolve_url_for_test("https://example.test", raw);
    assert!(resolved.contains("bookId=1"), "body 不能丢，实际：{resolved}");
    assert!(resolved.contains("POST"), "method 不能丢，实际：{resolved}");
    // 选项只能出现一次（曾经被拼两遍）。
    assert_eq!(
        resolved.matches("\"method\"").count(),
        1,
        "选项不应重复：{resolved}"
    );
    // 地址部分要干净：选项之前不能出现第二个 `{`。
    let url_part = resolved.split(",{").next().unwrap_or("");
    assert_eq!(url_part, "https://example.test/api/book/info");
}

/// 相对地址拼接时保留选项，且拼在正确的基址上。
#[test]
fn relative_url_keeps_options_after_joining() {
    use reader_rust::parser::rule_engine::resolve_url_for_test;
    let resolved = resolve_url_for_test(
        "https://example.test/dir/page.html",
        r#"/api/info,{"method":"POST"}"#,
    );
    assert!(resolved.starts_with("https://example.test/api/info"), "实际：{resolved}");
    assert!(resolved.contains(r#""method":"POST""#), "实际：{resolved}");
}
