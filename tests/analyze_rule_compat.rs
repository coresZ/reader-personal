//! `Packages.io.legado.app.model.analyzeRule.AnalyzeRule` 的兼容性回归。
//!
//! 一类书源的 jsLib 用它把规则求值作用在**另一份 HTML** 上(当前页面是详情页,
//! 要解析的是目录页):
//!
//! ```js
//! function evalOn(x, r) {
//!   return r == undefined ? this.java.getElement(x)
//!     : Packages.io.legado.app.model.analyzeRule.AnalyzeRule().setContent(r).getElement(x);
//! }
//! ```
//!
//! 缺这个类时目录脚本在 `evalOn(".book-title a", page)` 处直接抛
//! "cannot read property 'analyzeRule' of undefined", 目录一章都出不来。

const TOC_HTML: &str = r#"<html><body><div class="index" id="book-list">
<dd><a class="db" href="https://example.test/read/1/11.html" title="2024-09-17 15:31更新，共8461字">第1章 (09-17)</a></dd>
<dd><a class="db" href="https://example.test/read/1/12.html" title="2024-09-17 15:31更新，共7925字">第2章 (09-17)</a></dd>
<dd><a class="db" href="https://example.test/read/1/13.html" title="2024-09-17 15:31更新，共7902字">第3章 (09-17)</a></dd>
</div></body></html>"#;

#[test]
fn analyze_rule_evaluates_on_given_content() {
    use reader_rust::parser::js::eval_js;

    // 通过 java 桥验证 setContent 链式语义与元素抽取。
    let code = r#"
var ar = new Packages.io.legado.app.model.analyzeRule.AnalyzeRule();
ar.setContent(String(src));
var els = ar.getElement('.index dd');
var out = [];
for (var i = 0; i < els.size(); i++) {
  var a = els.get(i).select('a').first();
  out.push(a.ownText());
}
out.join('|')
"#;
    // eval_js 的 input 作为 src, 这里把它当作"另给的 HTML"。
    let out = eval_js(code, TOC_HTML, "https://example.test/book/1.html").expect("AnalyzeRule 应可构造");
    assert_eq!(out, "第1章 (09-17)|第2章 (09-17)|第3章 (09-17)", "在另给内容上应解析出全部章节");
}

/// `setContent(...)` 必须返回自身, 才能写成 `.setContent(r).getElement(x)`。
#[test]
fn analyze_rule_set_content_is_chainable() {
    use reader_rust::parser::js::eval_js;

    let out = eval_js(
        r#"
var ar = new Packages.io.legado.app.model.analyzeRule.AnalyzeRule();
var same = ar.setContent(String(src));
(same === ar) + '|' + ar.getElement('.index dd').size()
"#,
        TOC_HTML,
        "https://example.test/book/1.html",
    )
    .expect("链式调用应可用");
    assert_eq!(out, "true|3");
}

/// `getString(rule)` 也作用在 setContent 的内容上。
#[test]
fn analyze_rule_get_string_uses_given_content() {
    use reader_rust::parser::js::eval_js;

    let out = eval_js(
        r#"
var ar = new Packages.io.legado.app.model.analyzeRule.AnalyzeRule();
ar.setContent(String(src));
ar.getString('#book-list dd.0@text')
"#,
        TOC_HTML,
        "https://example.test/book/1.html",
    )
    .expect("getString 应可用");
    assert!(
        out.contains("第1章"),
        "getString 应作用于 setContent 的内容: {out}"
    );
}




