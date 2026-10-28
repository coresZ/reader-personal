//! 三条「手机端正常、桌面端漏解析」的书源兼容回归。
//!
//! 1. **列表规则里夹 `<js>…</js>`**。legado 的 `AnalyzeRule.splitSourceRule` 把
//!    JS 段单独切出来，剩下的文本前后各成一段，逐段串联求值（上一段的输出喂给
//!    下一段）。桌面端早先只认「规则以 JS 开头」，中间夹 JS 时整条当成一个 CSS
//!    选择器 —— 选择器里含 `<js>` 必然选不到，症状是**搜索恒为 0 条**
//!    （发现页不走这段脚本，所以正常）。
//! 2. **列表规则 JS 段的 `result` 是 JSoup `Elements`**（可 `toArray()` /
//!    `size()` / 索引），不是单个元素、也不是裸 JSON 数组。一类书源的 `bookList`
//!    第二行就是 `list = result.toArray()`。
//! 3. **正文规则里的 `##匹配##替换` 要先生成主体再替换**。早先把整条规则
//!    （含 `##` 后的替换正则）交给提取器，`…p@text##广告行.*` 里的
//!    `text##…` 不是合法提取器，取不出任何文本 —— 症状是「正文解析为空」。

use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::rule_engine::RuleEngine;
use serde_json::json;

/// 列表规则：`CSS 选择器 → <js> 过滤 → CSS 选择器`，且脚本用 `result.toArray()`。
///
/// 脚本里的关键词固定写成字面量：真实书源靠 `searchUrl` 的
/// `{{java.put("key",key)}}` 把它存进书源变量，这里只验证值链本身。
#[test]
fn search_list_rule_with_embedded_js_runs_the_whole_chain() {
    let source = book_source_from_value(json!({
        "bookSourceUrl": "https://chain.example.test",
        "bookSourceName": "列表规则夹 JS 样例源",
        "ruleSearch": {
            "bookList": "class.book_cell\n<js>\nkey = \"我的\";\nlist = result.toArray();\nre = new RegExp(key);\nhtml = \"\";\nfor (i in list) { re.test(list[i]) ? html += list[i] : \"\" }\nresult = String(html)\n</js>\na",
            "name": "text",
            "bookUrl": "href"
        }
    }))
    .expect("book source");

    let body = r#"<html><body><table>
        <tr><td class="book_cell"><a href="/book-keep">我的书</a></td></tr>
        <tr><td class="book_cell"><a href="/book-drop">别人的书</a></td></tr>
        <tr><td class="book_cell"><a href="/book-keep-2">我的第二本</a></td></tr>
    </table></body></html>"#;

    let engine = RuleEngine::new().expect("rule engine");
    let books = engine.search_books(&source, body, "https://chain.example.test/search");
    let names: Vec<&str> = books.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(
        names,
        ["我的书", "我的第二本"],
        "夹在列表规则里的 JS 段必须真正执行，且 `result.toArray()` 可用"
    );
    assert_eq!(books[0].book_url, "https://chain.example.test/book-keep");
}

/// 正文规则：`@css:…@text##替换` 必须先生成正文、再套替换。
#[test]
fn content_rule_applies_trailing_replace_regex() {
    let source = book_source_from_value(json!({
        "bookSourceUrl": "https://replace.example.test",
        "bookSourceName": "正文替换样例源",
        "ruleContent": {
            "content": "@css:div.read-content p@text##示例书城.*|请收藏.*|本站域名.*"
        }
    }))
    .expect("book source");

    let body = r#"<html><body><div class="read-content">
        <p>第一段正文。</p>
        <p>第二段正文。</p>
        <p>请收藏本站，后续章节更精彩</p>
        <p>示例书城 版权所有</p>
    </div></body></html>"#;

    let engine = RuleEngine::new().expect("rule engine");
    let content = engine.content(&source, body, "https://replace.example.test/1.html", None);
    assert!(
        content.contains("第一段正文。") && content.contains("第二段正文。"),
        "`##` 之前的提取器必须照常生效，实际：{content:?}"
    );
    assert!(
        !content.contains("请收藏") && !content.contains("示例书城"),
        "`##` 之后的替换正则必须把广告行清掉，实际：{content:?}"
    );
}
