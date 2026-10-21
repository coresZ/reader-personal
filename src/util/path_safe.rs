//! 把外部输入的路径归一化成"只含普通段的相对路径"。
//!
//! 两处调用方都要读磁盘上用户可写的目录:
//!
//! - `server::assets` —— Web 端托管的前端产物;
//! - `api::protocol::files_route` —— `files?path=default/ai-maps/x.png` 这类资源。
//!
//! **两者必须用同一份校验**。曾经 `files_route` 自己写了一套"按 `/` 分段、
//! 拒绝 `..` 段"的判断, 在 Windows 上被 `..\..\Windows\win.ini` 绕过:
//! 反斜杠在 `split('/')` 眼里只是一个普通字符, 整串作为一个段通过检查,
//! 交给 `PathBuf::join` 之后才被系统当成上级目录。而 `/files` 同时挂在
//! 未鉴权的 Web 服务上, 等于把本机任意文件的读权限交给了同网段。
//!
//! 修复方式不是补一个 `\` 判断(还有 `%5c`、UNC、盘符、`\\?\` 等一长串变体),
//! 而是**只允许普通文件名段** —— 由标准库的 `Component` 决定什么是"一段",
//! 加上两条显式规则(反斜杠、盘符冒号)覆盖非 Windows 主机上的同一份输入。
//!
//! 校验发生在 `join` 之前, 因此不依赖目标文件是否存在, 也不用 `canonicalize`
//! (它在 Windows 上会展开 8.3 短名, 与请求路径对不上, 且目标可能尚未创建)。

use std::path::{Component, Path, PathBuf};

/// 归一化一个**已解码**的相对路径。
///
/// 空段与 `.` 被跳过(容忍 `/assets/a.js` 这样的前导斜杠, 历史数据里有这种写法);
/// `..`、盘符前缀、UNC 前缀、含反斜杠的段一律返回 `None`。
///
/// 注意: 本函数**不做**百分号解码。调用方拿到的是 `uri.path()`(未解码)时,
/// 必须先过 [`percent_decode`]; 拿到的是 query 参数(`serde_urlencoded` 已解码)
/// 时则不要重复解码 —— 二次解码会把 `%252e%252e` 变成 `..` 而绕过校验。
pub fn safe_relative_path(input: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in input.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        // `..` 显式拦一次: 在 Unix 上它本来就是 `ParentDir`, 但判定标准要是
        // 路径语义而不是平台实现。
        if part == ".." || part.contains('\\') || part.contains(':') {
            return None;
        }
        // 只接受普通段。根 `/`(RootDir)、`..`(ParentDir) 在这里也会被挡下。
        let candidate = Path::new(part);
        if candidate
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return None;
        }
        out.push(part);
    }
    Some(out)
}

/// 极简 percent-decode: 只处理 `%XX`。
///
/// 不用 `urlencoding` crate 是为了避免把 `+` 解成空格 —— 文件名里的 `+`
/// 是合法字符, 且新版浏览器不会在路径里编码空格为 `+`。
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_traversal() {
        assert!(safe_relative_path("/../etc/passwd").is_none());
        assert!(safe_relative_path("/a/../../b").is_none());
        assert!(safe_relative_path("..").is_none());
    }

    /// 回归: Windows 反斜杠曾绕过 `split('/')` 形式的校验。
    ///
    /// 这条是本次修复的核心 —— 之前 `files_route` 用 `split('/')` 判断,
    /// `..\..\Windows\win.ini` 会作为**一个**段通过, 随后的 `join` 把它
    /// 还原成上级目录, 于是一个未鉴权的 HTTP 服务能读本机任意文件。
    #[test]
    fn rejects_backslash_traversal() {
        assert!(safe_relative_path(r"..\..\Windows\win.ini").is_none());
        assert!(safe_relative_path(r"default\..\..\secret").is_none());
        assert!(safe_relative_path("a/..\\..\\b").is_none());
    }

    #[test]
    fn rejects_prefixed_segments() {
        assert!(safe_relative_path("C:/Windows/win.ini").is_none());
        assert!(safe_relative_path(r"\\server\share\x").is_none());
        // NTFS 备用数据流 / 盘符相对路径都靠冒号这一段拦住。
        assert!(safe_relative_path("a/secret.txt:hidden").is_none());
    }

    #[test]
    fn accepts_normal_assets() {
        assert_eq!(
            safe_relative_path("/assets/index-abc.js").unwrap(),
            PathBuf::from("assets/index-abc.js")
        );
        assert_eq!(
            safe_relative_path("default/backgrounds/a b.png").unwrap(),
            PathBuf::from("default/backgrounds/a b.png")
        );
        assert_eq!(
            safe_relative_path("default/covers/deadbeef.webp").unwrap(),
            PathBuf::from("default/covers/deadbeef.webp")
        );
        assert_eq!(safe_relative_path("/").unwrap(), PathBuf::new());
    }

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(percent_decode("a%20b.js"), "a b.js");
        assert_eq!(percent_decode("%E4%B8%AD.txt"), "中.txt");
        // 编码过的 `..` 解出来之后仍然会被上层拒绝。
        assert!(safe_relative_path(&percent_decode("%2e%2e%2fetc%2fpasswd")).is_none());
        assert!(safe_relative_path(&percent_decode("%2e%2e%5cWindows%5cwin.ini")).is_none());
    }

    #[test]
    fn keeps_plus_sign() {
        assert_eq!(percent_decode("a+b.js"), "a+b.js");
    }
}
