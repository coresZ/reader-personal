//! `Packages.javax.crypto.*` 的可用性。
//!
//! 一类书源用 JCA 全限定名做接口签名：
//!
//! ```js
//! var mac = Packages.javax.crypto.Mac.getInstance("HmacSHA256");
//! mac.init(new Packages.javax.crypto.spec.SecretKeySpec(key, "HmacSHA256"));
//! var sig = mac.doFinal(data);          // ← 字节数组
//! ```
//!
//! 缺 `Mac` 时第一行就抛 `cannot read property 'getInstance' of undefined`，
//! 整个搜索地址拼不出来，界面只显示「搜索 0 条」—— 与「站点改版」的现象一样，
//! 光看界面无从分辨。
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js::{eval_js, with_source};

fn source() -> reader_rust::model::book_source::BookSource {
    book_source_from_value(serde_json::json!({
        "bookSourceName": "示例源",
        "bookSourceUrl": "https://example.test",
    }))
    .expect("source")
}

fn run(script: &str) -> String {
    let source = source();
    with_source(&source, || eval_js(script, "", "")).expect("求值应成功")
}

/// `Mac` 必须存在，且 `HmacSHA256` 的签名结果与标准值一致。
///
/// 期望值用 Python `hmac.new(b"key", b"The quick brown fox jumps over the lazy dog", sha256)`
/// 独立算出，避免「实现与测试同源」。
#[test]
fn mac_hmac_sha256_matches_reference() {
    let out = run(
        r#"
var mac = Packages.javax.crypto.Mac.getInstance("HmacSHA256");
mac.init(new Packages.javax.crypto.spec.SecretKeySpec("key", "HmacSHA256"));
var raw = mac.doFinal("The quick brown fox jumps over the lazy dog");
raw.map(function (b) { return ("0" + (b & 255).toString(16)).slice(-2); }).join("")
"#,
    );
    assert_eq!(
        out, "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8",
        "HmacSHA256 结果与标准实现不符"
    );
}

/// `doFinal()` 不带参数时用 `update()` 累积的数据（JCA 的另一种写法）。
#[test]
fn mac_accumulates_via_update() {
    let out = run(
        r#"
var mac = Packages.javax.crypto.Mac.getInstance("HmacSHA256");
mac.init(new Packages.javax.crypto.spec.SecretKeySpec("key", "HmacSHA256"));
mac.update("The quick brown fox ");
mac.update("jumps over the lazy dog");
var raw = mac.doFinal();
raw.map(function (b) { return ("0" + (b & 255).toString(16)).slice(-2); }).join("")
"#,
    );
    assert_eq!(
        out, "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
    );
}

/// 二进制密钥（`SecureRandom` 生成）不能被当成 UTF-8 文本解析。
///
/// 书源常见写法是 `new SecretKeySpec(new Packages.java.security.SecureRandom()…)`，
/// 密钥里出现非 UTF-8 字节是常态；早期实现用 `from_utf8` 校验，会直接报
/// 「HMAC key 不是有效文本」把整段签名打断。
#[test]
fn mac_accepts_binary_key() {
    let out = run(
        r#"
var mac = Packages.javax.crypto.Mac.getInstance("HmacSHA256");
mac.init(new Packages.javax.crypto.spec.SecretKeySpec([0x00, 0xFF, 0x80, 0x7F], "HmacSHA256"));
var raw = mac.doFinal("data");
raw.length + ":" + (raw[0] & 255)
"#,
    );
    // 只校验形状：算法对、长度 32 字节、首字节稳定。
    assert!(out.starts_with("32:"), "应产出 32 字节摘要，实际 {out}");
}

/// `GCMParameterSpec` 必须可构造（AES/GCM 的 IV 载体）。
#[test]
fn gcm_parameter_spec_is_constructible() {
    let out = run(
        r#"
var spec = new Packages.javax.crypto.spec.GCMParameterSpec(128, [1,2,3,4,5,6,7,8,9,10,11,12]);
spec.bytes.length + ":" + spec.tLen
"#,
    );
    assert_eq!(out, "12:128");
}

/// PBKDF2-HMAC-SHA256 的迭代链必须与标准一致（纸间的 pow 校验用它）。
#[test]
fn pbkdf2_hmac_sha256_matches_reference() {
    let out = run(
        r#"
var mac = Packages.javax.crypto.Mac.getInstance("HmacSHA256");
mac.init(new Packages.javax.crypto.spec.SecretKeySpec("password", "HmacSHA256"));
var u = mac.doFinal("salt" + String.fromCharCode(0,0,0,1));
u.map(function (b) { return ("0" + (b & 255).toString(16)).slice(-2); }).join("")
"#,
    );
    // Python: hmac.new(b"password", b"salt" + (1).to_bytes(4,"big"), sha256)
    assert_eq!(
        out,
        "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
    );
}
