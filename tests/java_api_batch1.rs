//! 第一批补齐的 `java.*` / Java 类型扩展 / 对象方法回归。
//!
//! 这些 API 在 legado 里由 `JsExtensions.kt`、`CacheManager`、`BaseBook`
//! 提供。缺任何一个, 依赖它的书源脚本都会在对应位置抛 "not a function",
//! 整条字段规则失败。
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js::{eval_js, with_book_context, with_source, JsBookContext};
use serde_json::json;

fn plain_source() -> reader_rust::model::book_source::BookSource {
    book_source_from_value(json!({
        "bookSourceUrl": "https://example.test",
        "bookSourceName": "API 样例源",
        "ruleContent": { "content": "@css:.content@html" }
    }))
    .expect("book source")
}

fn with_book(script: &str) -> String {
    let source = plain_source();
    let ctx = JsBookContext {
        book: json!({ "bookUrl": "https://example.test/book/1.html", "name": "样例书" }),
        chapter: json!({ "url": "https://example.test/read/1.html", "title": "第1章", "index": 0 }),
    };
    with_source(&source, || {
        with_book_context(Some(ctx), || eval_js(script, "", "https://example.test/"))
    })
    .expect("脚本应执行成功")
}

/// `java.strToBytes` / `java.bytesToStr`: UTF-8 往返与字节值。
#[test]
fn str_bytes_roundtrip_utf8() {
    let out = eval_js(
        "var b = java.strToBytes('中文');\n\
         b.length + '|' + b.join(',') + '|' + java.bytesToStr(b)",
        "",
        "",
    )
    .unwrap();
    assert_eq!(out, "6|228,184,173,230,150,135|中文");
}

/// 非 UTF-8 字符集要按真实字节编码(GBK 下"中文"是 4 字节)。
#[test]
fn str_bytes_honours_charset() {
    let out = eval_js(
        "var b = java.strToBytes('中文', 'GBK');\n\
         b.length + '|' + b.join(',') + '|' + java.bytesToStr(b, 'GBK')",
        "",
        "",
    )
    .unwrap();
    assert_eq!(out, "4|214,208,206,196|中文", "GBK 编码/解码应与 Java 一致");
}

/// `java.randomUUID()` 形如 8-4-4-4-12, 且版本位为 4。
#[test]
fn random_uuid_shape() {
    let out = eval_js(
        "var u = java.randomUUID();\n\
         (/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/).test(u) + '|' + u.length",
        "",
        "",
    )
    .unwrap();
    assert_eq!(out, "true|36");
}

/// Java `String.contains` / `equalsIgnoreCase` / `hashCode`。
#[test]
fn java_string_extensions() {
    let out = eval_js(
        "'abcde'.contains('bcd') + '|' + 'abcde'.contains('zz') + '|' \
         + 'ABC'.equalsIgnoreCase('abc') + '|' + ''.hashCode() + '|' \
         + 'hello'.hashCode() + '|' + 'abc'.hashCode()",
        "",
        "",
    )
    .unwrap();
    // Java: "".hashCode()=0, "hello".hashCode()=99162322, "abc".hashCode()=96354
    assert_eq!(out, "true|false|true|0|99162322|96354");
}

/// `new java.lang.StringBuilder()` 的 append/toString。
#[test]
fn string_builder_append() {
    let out = eval_js(
        "var sb = new java.lang.StringBuilder('甲');\n\
         sb.append('乙').append(3);\n\
         sb.toString() + '|' + sb.length()",
        "",
        "",
    )
    .unwrap();
    assert_eq!(out, "甲乙3|3");
}

/// `java.lang.Long.parseLong` 与 `java.util.Date.getTime`。
#[test]
fn long_and_date() {
    let out = eval_js(
        "java.lang.Long.parseLong('1234567890') + '|' + new java.util.Date(1700000000000).getTime()",
        "",
        "",
    )
    .unwrap();
    assert_eq!(out, "1234567890|1700000000000");
}

/// `java.net.URLEncoder` / `URLDecoder` 按 Java 语义(`%20` 记为 `+`)。
#[test]
fn url_encoder_decoder() {
    let out = eval_js(
        "var e = java.net.URLEncoder.encode('a b&c');\n\
         e + '|' + java.net.URLDecoder.decode(e)",
        "",
        "",
    )
    .expect("URLEncoder/URLDecoder 应可调用");
    assert_eq!(out, "a+b%26c|a b&c");
}

/// `cache.getInt/getLong/getDouble/getFloat/getByteArray` 的取值与空值语义。
#[test]
fn cache_typed_readers() {
    let out = eval_js(
        "cache.put('__t_int', '42');\n\
         cache.put('__t_float', '3.5');\n\
         cache.put('__t_bytes', '\\u0000\\u00ff');\n\
         cache.getInt('__t_int') + '|' + cache.getLong('__t_int') + '|' \
         + cache.getDouble('__t_float') + '|' + cache.getFloat('__t_float') + '|' \
         + cache.getByteArray('__t_bytes').join(',') + '|' \
         + String(cache.getInt('__t_missing'))",
        "",
        "",
    )
    .unwrap();
    assert_eq!(out, "42|42|3.5|3.5|0,255|null", "缺键应返回 null 而不是 0");
}

/// `book.getCustomVariable()` / `putCustomVariable()` 与 `getVariable('custom')` 互通。
#[test]
fn book_custom_variable() {
    let out = with_book(
        "book.putCustomVariable('已加锁');\n\
         book.getCustomVariable() + '|' + book.getVariable('custom')",
    );
    assert_eq!(out, "已加锁|已加锁", "两个入口应访问同一份存储");
}

/// `java.openUrl` 不应抛错(桌面端只登记外部打开意图)。
#[test]
fn open_url_does_not_throw() {
    let out = eval_js(
        "try { java.openUrl('https://example.test/x'); 'ok'; } catch (e) { 'err:' + e; }",
        "",
        "",
    )
    .unwrap();
    assert_eq!(out, "ok");
}