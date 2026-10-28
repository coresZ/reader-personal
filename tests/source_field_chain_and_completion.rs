//! 两条「手机端正常、桌面端空白」的字段/脚本求值回归。
//!
//! 1. **详情页字段规则里「脚本段 + 后续选择器段」要串联求值**。legado 的
//!    `AnalyzeRule.splitSourceRule` 把 `<js>…</js>` 单独切出，其余文本前后各成
//!    一段，逐段串起来（上一段的输出是下一段的输入）。一类书源的字段规则正是
//!    「先 `<js>` 另抓一页，再在其上选元素」——早先只取 `</js>` 之前的部分，
//!    脚本输出被原样当成字段值，症状是**简介显示成一整页 HTML 源码**。
//! 2. **脚本末尾的空完成语句不能把前面的结果冲掉**。Rhino（legado 手机端）
//!    返回「最后一条有值的语句」，ECMAScript 返回「完成值」。书源常见的
//!    `result = decode(...); if (result.length < 500) { … }` 在条件为假时，
//!    QuickJS 的完成值是 `undefined`，前面算好的正文整个丢掉 —— 症状是
//!    **正文恒空、日志里没有任何异常**。

use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js::eval_js;
use reader_rust::parser::rule_engine::RuleEngine;
use serde_json::json;

/// 字段规则：`<js>…</js>` 之后的 CSS 段必须在脚本输出上继续求值。
///
/// 脚本用 `java.getString` 之外的方式另造一份 HTML（真实写法里是 `java.ajax`
/// 抓另一页），这里用纯字符串拼装，避免测试依赖网络。
#[test]
fn book_info_field_rule_chains_script_then_selector() {
    let source = book_source_from_value(json!({
        "bookSourceUrl": "https://chain-field.example.test",
        "bookSourceName": "字段链式样例源",
        "ruleBookInfo": {
            "name": "h1@text",
            // 真实书写形态：脚本给出新内容（等价于另抓一页），再在其上选元素。
            "intro": "<js>'<div><p>导航</p><p>第一段简介。</p><p>第二段简介。</p></div>'</js>\ntag.p.1:2@text"
        }
    }))
    .expect("book source");

    let body = r#"<html><body><h1>示例书名</h1>
        <span class="meta">图书简介：<a href="detail.html">摘要…</a></span>
    </body></html>"#;

    let engine = RuleEngine::new().expect("rule engine");
    let info = engine.book_info(
        &source,
        body,
        "https://chain-field.example.test/books/1/index.html",
        "https://chain-field.example.test/books/1/index.html",
    );
    let intro = info.intro.unwrap_or_default();
    assert!(
        intro.contains("第一段简介。") && intro.contains("第二段简介。"),
        "`</js>` 之后的选择器段必须生效，实际：{intro:?}"
    );
    assert!(
        !intro.contains("导航"),
        "索引 `!1:2` 应跳过第 1 个 <p>，实际：{intro:?}"
    );
}

/// 脚本末尾的空完成语句不能清掉脚本结果。
#[test]
fn trailing_empty_statement_keeps_script_result() {
    // 条件为假 → 最后一条语句是空完成；结果仍应是赋过值的 result。
    // （正文超过 500 字，`result.length < 500` 为假，正是真实书源的形态。）
    let long_text = "正".repeat(600);
    let out = eval_js(
        &format!(
            r#"var raw = "{long_text}";
result = "解码后的" + raw;
if (result.length < 500) {{ result = result + "（短）"; }}"#
        ),
        "",
        "https://empty-tail.example.test/",
    )
    .expect("脚本不应报错");
    assert_eq!(out.chars().count(), "解码后的".chars().count() + 600);
    assert!(!out.contains("（短）"), "条件为假时不该追加提示");

    // 条件为真时同样取赋值结果（此时完成值本身就有值，两条路径要一致）。
    let out = eval_js(
        r#"var raw = "正文";
result = "解码后的" + raw;
if (result.length < 500) { result = result + "（短）"; }"#,
        "",
        "https://empty-tail.example.test/",
    )
    .expect("脚本不应报错");
    assert_eq!(out, "解码后的正文（短）");

    // 脚本没写 `result =` 时必须保持空，不能把输入原文当输出。
    let out = eval_js(
        "var parsed = JSON.parse(result); String(parsed.a); if (false) {}",
        r#"{"a":1}"#,
        "https://empty-tail.example.test/",
    )
    .expect("脚本不应报错");
    assert_eq!(out, "");
}
