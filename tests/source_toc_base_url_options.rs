//! 目录页解析时 `baseUrl` 必须保留 legado 的 URL 选项。
//!
//! 一类书源把书籍编号放在**目录接口的 POST body** 里，目录脚本再从 `baseUrl`
//! 反查它：
//!
//! ```js
//! var catalogBookId = "";
//! try { catalogBookId = String(bookId || ""); } catch (e) {}
//! if (!catalogBookId) {
//!   var m = String(baseUrl || "").match(/bookId=(\d+)/);   // ← 依赖 body 里的 bookId
//!   if (m) catalogBookId = m[1];
//! }
//! java.put("chapterMeta", …);   // 存进章节映射，正文规则再取
//! ```
//!
//! `AnalyzeUrl` 在 legado 里一直保留规则原文（`地址,{\"body\":…,\"method\":\"POST\"}`），
//! 脚本看到的 `baseUrl` 就是它。桌面端早先把**实际请求地址**（只有路径、没有
//! body）当 `baseUrl` 传进去，正则匹配不到 → 编号为空 → 写进映射后正文规则取不到，
//! 报「获取正文失败：缺少书籍或章节编号，请刷新目录」。
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js::{eval_js, with_source};

fn demo_source() -> reader_rust::model::book_source::BookSource {
    book_source_from_value(serde_json::json!({
        "bookSourceName": "示例源",
        "bookSourceUrl": "https://example.test",
    }))
    .expect("source")
}

/// 目录规则从 `baseUrl` 里取编号：带 URL 选项时能取到，被剥掉时取不到。
#[test]
fn toc_base_url_option_carries_book_id() {
    let source = demo_source();
    let rule = r#"
var id = "";
try { id = String(bookId || ""); } catch (e) {}
if (!id) {
  var m = String(baseUrl || "").match(/bookId=(\d+)/);
  if (m) id = m[1];
}
id
"#;

    // 规则原文（legado 里 baseUrl 的样子）：body 里带 bookId。
    let with_option = r#"https://example.test/api/chapterlist,{"body":"bookId=8875162&sign=x","method":"POST"}"#;
    let got = with_source(&source, || eval_js(rule, "", with_option)).expect("求值应成功");
    assert_eq!(got, "8875162", "带 URL 选项的 baseUrl 应能取到书籍编号");

    // 实际请求地址（选项被剥掉）：取不到 —— 这正是旧行为，会导致正文报错。
    let without_option = "https://example.test/api/chapterlist";
    let got = with_source(&source, || eval_js(rule, "", without_option)).expect("求值应成功");
    assert!(got.is_empty(), "纯路径应取不到编号（旧行为的成因）：{got}");
}

/// 正文规则在编号缺失时给出的提示，必须是可诊断的那一句。
///
/// 这条钉住「用户看到的是书源自己的文案，而不是我们的通用错误」，
/// 方便以后判断问题出在编号链路还是别处。
#[test]
fn missing_chapter_meta_reports_actionable_message() {
    let source = demo_source();
    let rule = r#"
function meta(url) {
  var m = String(url || "").match(/[#&?]bookRef=([^&#]+)/);
  return m ? { bookId: m[1] } : null;
}
var got = meta(baseUrl);
got ? "ok" : "获取正文失败：缺少书籍或章节编号，请刷新目录"
"#;
    // 没有 fragment → 提示「刷新目录」。
    let plain = "https://example.test/content?bookId=1";
    let msg = with_source(&source, || eval_js(rule, "", plain)).expect("求值应成功");
    assert!(msg.contains("缺少书籍或章节编号"), "实际：{msg}");

    // 有 fragment → 正常。
    let tagged = "https://example.test/content?bookId=1#bookRef=8875162&chapterRef=9";
    let ok = with_source(&source, || eval_js(rule, "", tagged)).expect("求值应成功");
    assert_eq!(ok, "ok");
}
