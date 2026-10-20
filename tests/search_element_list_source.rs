//! 搜索解析回归: "元素列表 + 元素上的 CSS/JS 字段规则" 这一类书源。
//!
//! 书源形态(脱敏):
//! ```text
//! ruleSearch.bookList  = <js>result = java.getElement('#list>div'); result</js>
//! ruleSearch.name      = .title@text||h4@text <js>…book.bookUrl=…
//! ruleSearch.author    = <js>result.select('p.meta').first().html()…</js>
//! ruleSearch.bookUrl   = @js:book.bookUrl
//! ruleSearch.coverUrl  = img.0@src <js>result=withImageHeaders(result)…</js>
//! ```
//!
//! 四个环节此前各有一处偏差, 合起来导致搜索结果 0 条:
//! 1. `java.getElement` 只返回第一个元素 → 列表变单条;
//! 2. 元素快照被当纯 JSON 注入 `result` → `result.select(...)` 不存在;
//! 3. 元素上的字段规则走 JSON 字段取值 → `h4@text` 恒为空;
//! 4. 封面上拼接的 URL option 未剥离 → 封面地址带 `,{"headers":…}` 尾巴。
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::rule_engine::RuleEngine;
use serde_json::json;

const SEARCH_HTML: &str = r#"<html><body>
<div id="content">
  <div class="list-body" id="book-list">
    <div class="c_row cf">
      <a class="db cf" href="https://example.test/book/1001.html">
        <div class="row_cover"><img class="cover_m" src="https://img.test/1001s.jpg"></div>
        <div class="row_text">
          <h4>第一本书</h4>
          <p class="meta">作者甲 | #标签一
          <br>这是第一本的简介。</p>
        </div>
      </a>
    </div>
    <div class="c_row cf">
      <a class="db cf" href="https://example.test/book/1002.html">
        <div class="row_cover"><img class="cover_m" src="https://img.test/1002s.jpg"></div>
        <div class="row_text">
          <h4>第二本书</h4>
          <p class="meta">作者乙 | #标签二
          <br>这是第二本的简介。</p>
        </div>
      </a>
    </div>
  </div>
</div>
</body></html>"#;

/// 与真实书源同构的规则(站点/类名已脱敏, 结构一模一样)。
fn element_list_source() -> reader_rust::model::book_source::BookSource {
    book_source_from_value(json!({
        "bookSourceUrl": "https://example.test",
        "bookSourceName": "元素列表样例源",
        "jsLib": r#"
function withImageHeaders(url) {
  var u = String(url == null ? '' : url);
  if (!u || u == 'undefined' || u == 'null') return '';
  if (u.indexOf(',{') >= 0) return u;
  return u + ',' + JSON.stringify({ headers: { Cookie: 'verify_ok=1', Referer: 'https://example.test' } });
}
"#,
        "searchUrl": "https://example.test/search?key={{key}}&page={{page}}",
        "ruleSearch": {
            "bookList": "<js>\nlet r1 = java.getString('#content .list-body@text');\nif (r1.includes(\"错误原因\")) {result=[];}\nelse {\n\tresult = java.getElement('#book-list>div');\n}\nresult\n</js>",
            "name": ".book-name@text||h4@text\n<js>\nlet rs=java.getString('a.0@href');\nlet id=(rs.match(/book\\/(\\d+)/)||['',''])[1];\nbook.bookUrl=`data:;base64,${java.base64Encode(rs)},{\"type\":\"\"}`;\nresult\n</js>",
            "author": "<js>\nlet r='';\ntry {\n  let p = result.select('p.meta').first();\n  if (p != null) {\n    let h = String(p.html());\n    let seg = h.split(/<br[ ]*[/]?>/i);\n    r = String(seg[0]||'').replace(/<[^>]+>/g,'').trim().split('|')[0].trim();\n  }\n} catch(e) {}\nr\n</js>",
            "intro": "<js>\nlet r='';\ntry {\n  let p = result.select('p.meta').first();\n  if (p != null) {\n    let h = String(p.html());\n    let seg = h.split(/<br[ ]*[/]?>/i);\n    if (seg.length > 1) r = String(seg.slice(1).join(' ')).replace(/<[^>]+>/g,'').replace(/\\s+/g,' ').trim();\n  }\n} catch(e) {}\nr\n</js>",
            "bookUrl": "@js:book.bookUrl",
            "coverUrl": "img.0@src\n<js>\nresult=withImageHeaders(result);\nresult\n</js>"
        }
    }))
    .expect("book source")
}

#[test]
fn element_list_search_parses_every_row() {
    let engine = RuleEngine::new().expect("rule engine");
    let source = element_list_source();
    let books = engine.search_books(&source, SEARCH_HTML, "https://example.test/search?key=x&page=1");

    assert_eq!(books.len(), 2, "两行结果都要解析出来, 实际: {}", books.len());
    assert_eq!(books[0].name, "第一本书");
    assert_eq!(books[1].name, "第二本书");
    assert_eq!(books[0].author, "作者甲");
    assert_eq!(books[1].author, "作者乙");
    assert!(
        books[0].intro.as_deref().unwrap_or_default().contains("第一本的简介"),
        "简介应取自 p.meta 的 br 之后: {:?}",
        books[0].intro
    );
}

/// 每本书的详情地址必须来自 `name` 里写入的 `book.bookUrl`, 而不是回退成搜索页。
///
/// 该源把详情地址包成 `data:;base64,<base64>,{"type":""}` 交给后续的 URL
/// 分析器解析(见 `analyze_url`), 因此这里校验 base64 内容而不是字面地址。
#[test]
fn element_list_search_shares_book_url_across_fields() {
    use base64::Engine;

    let engine = RuleEngine::new().expect("rule engine");
    let source = element_list_source();
    let books = engine.search_books(&source, SEARCH_HTML, "https://example.test/search?key=x&page=1");

    assert_eq!(books.len(), 2);
    assert_ne!(
        books[0].book_url, books[1].book_url,
        "每本书的详情地址不能相同: {:?}",
        books[0].book_url
    );
    for (book, id) in books.iter().zip(["1001", "1002"]) {
        let payload = book
            .book_url
            .strip_prefix("data:;base64,")
            .and_then(|rest| rest.split_once(',').map(|(b64, _)| b64))
            .unwrap_or_else(|| panic!("详情地址应是 data:;base64 信封: {}", book.book_url));
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .expect("base64");
        let decoded = String::from_utf8(decoded).expect("utf8");
        assert!(
            decoded.contains(id),
            "详情地址应包含书籍 id {id}: {decoded}"
        );
        assert!(
            !decoded.contains("/search"),
            "详情地址不应回退成搜索页: {decoded}"
        );
    }
}

/// 封面地址必须剥掉书源拼接的 URL option(`,{"headers":…}`)。
#[test]
fn element_list_search_strips_cover_url_options() {
    let engine = RuleEngine::new().expect("rule engine");
    let source = element_list_source();
    let books = engine.search_books(&source, SEARCH_HTML, "https://example.test/search?key=x&page=1");

    assert_eq!(books.len(), 2);
    assert_eq!(
        books[0].cover_url.as_deref(),
        Some("https://img.test/1001s.jpg"),
        "封面地址不能带 URL option 尾巴"
    );
    assert_eq!(books[1].cover_url.as_deref(), Some("https://img.test/1002s.jpg"));
}


