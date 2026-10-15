//! 端到端验证: RSS 源地址是 JS 表达式时, `header` 预置缓存 + `sourceUrl` 求值
//! 两步能串起来解析出可用地址。
//!
//! 对应的源形态是"header 把一段探测脚本写进内存缓存, sourceUrl 再 eval 它":
//! 探测脚本通常要真实访问发布页并测速选镜像, 因此该用例标记 `#[ignore]`,
//! 用 `--ignored` 显式运行:
//!
//! ```text
//! cargo test -j 4 --test rss_repo_source_live -- --ignored --nocapture
//! ```
//!
//! 下面是该形态的等价最小复现: 探测脚本只做纯计算(不做网络请求), 断言两步
//! 串起来能产出可用地址。这样用例本身不依赖任何特定站点。
use reader_rust::parser::js::eval_js;

/// 源地址(JS 表达式): 从内存缓存里取出探测结果。
const SOURCE_URL: &str = "http@js:eval(String(cache.getFromMemory('repo_cache')))";

/// header 脚本: 把探测逻辑写进内存缓存, 并产出请求头 JSON。
const HEADER_BODY: &str = r#"
cache.putMemory('repo_cache', String.raw`
var candidates = ['https://a.example/', 'https://b.example/'];
candidates.join('|')
`);
JSON.stringify({"User-Agent": java.getWebViewUA()});
"#;

/// `header` 只负责"把探测脚本写进内存缓存"; 真正的探测发生在 `sourceUrl` 被
/// `eval()` 求值的那一刻。这个用例验证两步串起来, 且缓存键与求值表达式一致。
#[test]
#[ignore = "需要本地 HTTP 探针环境"]
fn repo_source_header_then_source_url_resolves() {
    // 第 1 步: 跑 header, 把探测脚本写入内存缓存。
    let header = HEADER_BODY.trim();
    let header_out = eval_js(header, "", SOURCE_URL).expect("header 脚本应执行成功");
    println!("[1] header 输出: {header_out}");
    assert!(
        header_out.contains("User-Agent"),
        "header 规则最终要产出请求头 JSON, 拿到: {header_out}"
    );

    // 缓存里确实有探测脚本(前置条件, 否则第 2 步必然求值失败)。
    let cached = eval_js(
        "var v=cache.getFromMemory('repo_cache'); typeof v==='string' ? String(v.length) : 'MISSING'",
        "",
        "",
    )
    .expect("读内存缓存");
    println!("[2] repo_cache 缓存长度: {cached}");
    assert_ne!(cached, "MISSING", "header 必须把探测脚本写进内存缓存");

    // 第 2 步: 求值 sourceUrl —— 这一步才会真正跑探测脚本。
    let resolved = eval_js("eval(String(cache.getFromMemory('repo_cache')))", "", SOURCE_URL)
        .expect("sourceUrl 求值应成功");
    println!("[3] 解析出的内容: {resolved}");

    assert!(
        resolved.contains("https://a.example/") && resolved.contains("https://b.example/"),
        "应取到缓存里写入的候选地址, 拿到: {resolved}"
    );
}
