//! 静态资源托管。
//!
//! 两种来源, 由 `StaticSource` 区分:
//!
//! - **嵌入资源**: release 构建里前端产物被 `tauri::generate_context!` 编译进
//!   exe(`frontendDist`), 运行时通过 `app.asset_resolver()` 读取。这时磁盘上
//!   **没有** `dist/` 目录, 只能走这条路。
//! - **文件系统**: 开发期把 `frontend/dist` 指过去, 改完 `npm run build` 刷新
//!   即生效, 不必重编 Rust。
//!
//! SPA 回退: 找不到实体文件时返回 `index.html`, 让前端路由接管。
//! `frontend/src/router/index.ts` 用 `createWebHashHistory()`, 正常导航不会打到
//! 这里; 但用户手输带路径的地址(如 `/reader`)时仍需要它。
//!
//! 注意本模块**只有一处**碰 `tauri::AppHandle`(`StaticSource::read`)。这不是
//! 洁癖: 一旦测试代码构造出 `StaticSource`, 链接器就会把 tauri 的
//! `TaskDialogIndirect`(comctl32 v6)拉进导入表, 而 cargo 生成的测试 exe 没有
//! 应用 manifest, 只加载得到 comctl32 v5 —— 进程在进 `main` 之前就以
//! `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)` 退出, 报错文本里连一句 Rust
//! panic 都没有。所以可测的判定逻辑全部抽在这个枚举**之外**。

use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::path::{Path, PathBuf};

/// 入口页候选。`index.html` 是唯一入口(桌面与 Web 共用同一份 `main.ts`)。
const INDEX: &str = "index.html";

/// 静态资源来源。
#[derive(Clone)]
pub enum StaticSource {
    /// Tauri 打包进二进制的资源。
    Embedded(tauri::AppHandle),
    /// 磁盘目录(开发期)。
    Directory(PathBuf),
}

impl StaticSource {
    /// 读取一个相对路径。返回 `(字节, MIME)`。
    ///
    /// 这是全模块唯一接触 `tauri::AppHandle` 的地方 —— 保持它不被测试调用,
    /// 见模块头的说明。
    fn read(&self, relative: &Path) -> Option<(Vec<u8>, String)> {
        match self {
            StaticSource::Directory(root) => {
                let path = root.join(relative);
                let bytes = std::fs::read(&path).ok()?;
                Some((bytes, content_type_for(&path).to_string()))
            }
            StaticSource::Embedded(app) => {
                let resolver = app.asset_resolver();
                // Tauri 的资源 key 用 `/` 分隔且带前导斜杠。
                let key = format!("/{}", relative.to_string_lossy().replace('\\', "/"));
                let asset = resolver.get(key)?;
                Some((asset.bytes, asset.mime_type))
            }
        }
    }
}

/// 一次请求要读哪个文件。
///
/// 抽成纯函数是为了能测 —— 它不碰 `StaticSource`, 也就不会把 tauri 拖进
/// 测试二进制(见模块头)。
#[derive(Debug, PartialEq, Eq)]
enum Lookup {
    /// 读这个相对路径(带扩展名的静态资源)。
    File(PathBuf),
    /// 回退到入口页(SPA 路由)。
    Index,
}

/// 决定这次请求该读实体文件还是回退入口页。
///
/// 判据只有一条: **有没有扩展名**。带扩展名的一律当静态资源, 找不到就 404;
/// 不带扩展名的当 SPA 路由(`/stats`、`/rss/article`), 回退 `index.html`。
fn resolve_lookup(relative: &Path) -> Lookup {
    // 带扩展名的资源找不到就是真找不到, **不能**回退成 index.html。
    //
    // 回退会让 `/assets/app-oldhash.js` 返回 200 + `text/html`, 浏览器把它当
    // JS 解析后在控制台报 "Unexpected token '<'" —— 一个和真实原因(文件名变
    // 了/构建产物不全)毫无关系的错误。这类问题只在浏览器里出现, 单测和 curl
    // 都看不出来。
    if relative.extension().is_some() {
        return Lookup::File(relative.to_path_buf());
    }
    Lookup::Index
}

pub fn router(source: StaticSource) -> Router {
    Router::new().fallback(get(move |uri: Uri| serve(source.clone(), uri)))
}

async fn serve(source: StaticSource, uri: Uri) -> Response {
    // `uri.path()` 还是 percent-encoded 的, 先解码再校验。校验本身走
    // `util::path_safe` 的唯一实现 —— 与 `/files` 共用同一套判定, 避免
    // 两个入口各自演进后其中一个被绕过。
    let decoded = crate::util::path_safe::percent_decode(uri.path());
    let Some(relative) = crate::util::path_safe::safe_relative_path(&decoded) else {
        return (StatusCode::BAD_REQUEST, "非法路径").into_response();
    };

    match resolve_lookup(&relative) {
        Lookup::File(path) => match source.read(&path) {
            Some((bytes, mime)) => respond(bytes, &mime, &path),
            None => (StatusCode::NOT_FOUND, "资源不存在").into_response(),
        },
        Lookup::Index => {
            let index = Path::new(INDEX);
            match source.read(index) {
                Some((bytes, mime)) => respond(bytes, &mime, index),
                None => (
                    StatusCode::NOT_FOUND,
                    "Web 端资源未找到: 开发期请先执行 `npm run build`",
                )
                    .into_response(),
            }
        }
    }
}

fn respond(bytes: Vec<u8>, mime: &str, path: &Path) -> Response {
    (
        [(header::CONTENT_TYPE, mime.to_string())],
        [(header::CACHE_CONTROL, cache_control_for(path).to_string())],
        Body::from(bytes),
    )
        .into_response()
}

fn content_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("ico") => "image/x-icon",
        Some("wasm") => "application/wasm",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// 带内容哈希的构建产物可以长期缓存; 入口 HTML 必须每次校验,
/// 否则用户升级后仍会加载旧页面引用的旧 chunk。
fn cache_control_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "no-cache",
        _ => "public, max-age=31536000, immutable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 路径校验本身在 `util::path_safe` 的单测里覆盖(含反斜杠穿越的回归)。
    /// 这里只锁这一层特有的行为: 入口页回退、404 与缓存头。
    #[test]
    fn html_is_not_cached() {
        assert_eq!(cache_control_for(Path::new("index.html")), "no-cache");
        assert!(cache_control_for(Path::new("assets/a.js")).contains("immutable"));
    }

    /// SPA 回退: 没有扩展名的路径(前端 router 的 `/stats`、`/rss` 等)回 index.html。
    #[test]
    fn extensionless_paths_fall_back_to_index() {
        assert_eq!(resolve_lookup(Path::new("stats")), Lookup::Index);
        assert_eq!(resolve_lookup(Path::new("rss/article")), Lookup::Index);
        assert_eq!(resolve_lookup(Path::new("")), Lookup::Index);
    }

    /// 回归: 缺失的**静态资源**必须 404, 不能回退成 index.html。
    ///
    /// 之前无差别回退, 于是 `GET /assets/app-gone.js` 返回 200 + `text/html`,
    /// 浏览器按 JS 解析后报 `Unexpected token '<'` —— 错误信息与真实原因
    /// (构建产物不全 / 版本不匹配)毫无关系, 且只在真实浏览器里出现。
    #[test]
    fn asset_paths_are_files_not_index() {
        assert_eq!(
            resolve_lookup(Path::new("assets/app-gone.js")),
            Lookup::File(PathBuf::from("assets/app-gone.js"))
        );
        assert_eq!(
            resolve_lookup(Path::new("favicon.ico")),
            Lookup::File(PathBuf::from("favicon.ico"))
        );
    }
}
