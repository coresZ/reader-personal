//! Android 专有 Java 类与 raw-deflate 解压的兼容回归。
//!
//! 一类书源把首页数据压进 `APP.Index('<base64>')`，再用脚本自己解：
//!
//! ```js
//! var raw = Packages.android.util.Base64.decode(b64, 2);   // Android 专有类
//! var inf = new Packages.java.util.zip.Inflater(true);      // nowrap = raw deflate
//! ```
//!
//! 手机端是 Android 运行时，这些类天然存在；桌面端不补就是
//! `cannot read property 'util' of undefined`，脚本 catch 后返回 null ——
//! 症状是**发现页空白、分类有名字但一本书都没有**。
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js::{eval_js, with_source};

fn source_with_lib(lib: &str) -> reader_rust::model::book_source::BookSource {
    let value = serde_json::json!({
        "bookSourceName": "示例源",
        "bookSourceUrl": "https://example.test",
        "jsLib": lib,
    });
    book_source_from_value(value).expect("source")
}

/// `Packages.android.util.Base64` 必须存在，且 `decode(text, URL_SAFE)` 能解出字节。
#[test]
fn android_base64_namespace_exists() {
    let source = source_with_lib("");
    let out = with_source(&source, || {
        eval_js(
            "typeof Packages.android + '|' + typeof Packages.android.util.Base64.decode",
            "",
            "",
        )
    });
    let out = out.expect("求值应成功");
    assert_eq!(out, "object|function", "Packages.android 链缺失：{out}");
}

/// `Packages.java.util.zip.Inflater(true)` 解 raw-deflate（不带 zlib 头）。
///
/// `nowrap=true` 必须走 `DeflateDecoder`；用 `ZlibDecoder` 会报
/// "invalid zlib header"，书源的 try/catch 随即吞掉异常、返回 null。
#[test]
fn inflater_blob_round_trip() {
    // 一段真实的 raw deflate 数据（"hello reader" 压出来）。
    let payload = b"hello reader";
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    use std::io::Write;
    encoder.write_all(payload).unwrap();
    let compressed = encoder.finish().unwrap();
    let bytes: Vec<i32> = compressed.iter().map(|b| *b as i32).collect();

    let source = source_with_lib("");
    let script = format!(
        r#"
var inf = new Packages.java.util.zip.Inflater(true);
var buf = Packages.java.lang.reflect.Array.newInstance(Packages.java.lang.Byte.TYPE, 8192);
inf.setInput({bytes:?}, 0, {len});
var out = new Packages.java.io.ByteArrayOutputStream();
while (!inf.finished()) {{
  var n = inf.inflate(buf);
  if (n <= 0) break;
  out.write(buf, 0, n);
}}
inf.end();
var data = out.toByteArray();
var text = '';
for (var i = 0; i < data.length; i++) text += String.fromCharCode(data[i] & 255);
text
"#,
        bytes = bytes,
        len = bytes.len()
    );
    let out = with_source(&source, || eval_js(&script, "", "")).expect("解压应成功");
    assert_eq!(out, "hello reader");
}
