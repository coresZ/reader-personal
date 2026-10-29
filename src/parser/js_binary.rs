use base64::Engine;
use rand::RngCore;
use serde_json::{json, Value};
use sha2::Digest;
use std::io::Read;

fn bytes(value: &Value) -> anyhow::Result<Vec<u8>> {
    value.as_array().ok_or_else(|| anyhow::anyhow!("需要字节数组"))?.iter()
        .map(|v| v.as_i64().map(|v| v as u8).ok_or_else(|| anyhow::anyhow!("无效字节"))).collect()
}

fn output(bytes: Vec<u8>) -> Value { json!(bytes.into_iter().map(|b| b as i8).collect::<Vec<_>>()) }

pub fn run(operation: &str, payload: &Value) -> anyhow::Result<Value> {
    let text = |key: &str| payload[key].as_str().unwrap_or("");
    match operation {
        "encode" => {
            let charset = if text("charset").is_empty() { "UTF-8" } else { text("charset") };
            let encoding = encoding_rs::Encoding::for_label(charset.as_bytes()).ok_or_else(|| anyhow::anyhow!("不支持字符编码 {charset}"))?;
            Ok(output(encoding.encode(text("text")).0.into_owned()))
        }
        "decode" => {
            let charset = if text("charset").is_empty() { "UTF-8" } else { text("charset") };
            let encoding = encoding_rs::Encoding::for_label(charset.as_bytes()).ok_or_else(|| anyhow::anyhow!("不支持字符编码 {charset}"))?;
            Ok(json!(encoding.decode(&bytes(&payload["data"])?).0.into_owned()))
        }
        "base64Encode" => Ok(json!(base64::engine::general_purpose::STANDARD.encode(bytes(&payload["data"])?))),
        // 与 java.base64Decode 一样按 Android/hutool 语义容忍缺省补位。
        "base64Decode" => Ok(output(super::js::decode_base64_lenient(text("text"))?)),
        "digest" => {
            let data = bytes(&payload["data"])?;
            let algorithm = text("algorithm").replace('-', "").to_ascii_uppercase();
            let digest = match algorithm.as_str() {
                "MD5" => md5::Md5::digest(&data).to_vec(),
                "SHA1" => sha1::Sha1::digest(&data).to_vec(),
                "SHA224" => sha2::Sha224::digest(&data).to_vec(),
                "SHA256" => sha2::Sha256::digest(&data).to_vec(),
                "SHA384" => sha2::Sha384::digest(&data).to_vec(),
                "SHA512" => sha2::Sha512::digest(&data).to_vec(),
                _ => anyhow::bail!("不支持摘要算法 {algorithm}"),
            };
            Ok(output(digest))
        }
        // `Packages.javax.crypto.Mac.getInstance(alg).doFinal(bytes)`：返回原始
        // 字节而非十六进制。一类书源用它做签名（`HmacSHA256` / `HmacSHA1`），
        // 再把结果 hex/base64 化拼进请求头。此前 `javax.crypto` 下没有 `Mac`，
        // 脚本第一行就是 `cannot read property 'getInstance' of undefined`，
        // 整个搜索 URL 拼不出来 —— 表现为「搜索 0 条」。
        // `Packages.javax.crypto.Mac.getInstance(alg).doFinal(bytes)`：返回原始
        // 字节而非十六进制。一类书源用它做签名（`HmacSHA256` 等），再把结果
        // hex/base64 化拼进请求头。此前 `javax.crypto` 下没有 `Mac`，
        // 脚本第一行就是 `cannot read property 'getInstance' of undefined`，
        // 整个搜索 URL 拼不出来 —— 表现为「搜索 0 条」。
        "mac" => {
            let data = bytes(&payload["data"])?;
            let key = bytes(&payload["key"])?;
            // 复用 `java.HMacHex` 的算法分发表（返回十六进制），再解回字节：
            // 这样只需维护一处算法映射，也避开 hmac/digest crate 的多版本
            // trait bound 纠缠（依赖树里同时存在 digest 0.10 与 0.11）。
            let hex_digest = super::js::java_hmac_hex_bytes(&key, &text("algorithm"), &data)?;
            Ok(output(hex::decode(hex_digest)?))
        }
        "random" => {
            let length = payload["length"].as_u64().filter(|n| *n <= 16 * 1024 * 1024).ok_or_else(|| anyhow::anyhow!("无效数组长度"))? as usize;
            let mut data = vec![0; length];
            rand::rngs::OsRng.fill_bytes(&mut data);
            Ok(output(data))
        }
        "cipher" => Ok(output(super::js::symmetric_crypt(
            payload["encrypt"].as_bool().unwrap_or(false), text("transformation"),
            &bytes(&payload["key"])? , &bytes(&payload["iv"])? , &bytes(&payload["data"])?
        )?)),
        "gunzip" => {
            let data = bytes(&payload["data"])?;
            let mut decoder = flate2::read::GzDecoder::new(data.as_slice()).take(64 * 1024 * 1024 + 1);
            let mut result = Vec::new();
            decoder.read_to_end(&mut result)?;
            anyhow::ensure!(result.len() <= 64 * 1024 * 1024, "解压内容过大");
            Ok(output(result))
        }
        // `Packages.java.util.zip.Inflater(true)`（nowrap）：书源把自定义请求体的
        // 压缩段用 raw deflate 存起来，再在脚本里逐块 inflate 取回 JSON。
        // nowrap=true 表示**不带 zlib 头**，必须用 `DeflateDecoder` 而不是 `ZlibDecoder`；
        // 用错会直接报 "invalid zlib header"，书源表现为「解码出 null、发现页空白」。
        "inflate" => {
            let data = bytes(&payload["data"])?;
            let nowrap = payload["nowrap"].as_bool().unwrap_or(true);
            let mut result = Vec::new();
            if nowrap {
                let mut decoder =
                    flate2::read::DeflateDecoder::new(data.as_slice()).take(64 * 1024 * 1024 + 1);
                decoder.read_to_end(&mut result)?;
            } else {
                let mut decoder =
                    flate2::read::ZlibDecoder::new(data.as_slice()).take(64 * 1024 * 1024 + 1);
                decoder.read_to_end(&mut result)?;
            }
            anyhow::ensure!(result.len() <= 64 * 1024 * 1024, "解压内容过大");
            Ok(output(result))
        }
        // `Packages.java.util.zip.GZIPOutputStream`: 源脚本用它把设备注册包
        // gzip 压缩后再按自定义头部字节发送。
        "gzip" => {
            use std::io::Write;
            let data = bytes(&payload["data"])?;
            let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&data)?;
            Ok(output(encoder.finish()?))
        }
        _ => anyhow::bail!("不支持字节操作 {operation}"),
    }
}
