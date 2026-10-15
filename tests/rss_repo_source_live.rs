//! 端到端验证: 用用户数据库里「源仓库」源的**真实配置**跑通完整求值链路。
//!
//! 这是本次改造的核心目标: 从源仓库导入的 RSS 源(源地址是 JS 表达式)能真正
//! 解析出可用地址。脚本会真实访问 yckceo.vip 发布页并测速选镜像, 因此该用例
//! 标记 `#[ignore]`, 用 `--ignored` 显式运行:
//!
//! ```text
//! cargo test -j 4 --test rss_repo_source_live -- --ignored --nocapture
//! ```
use reader_rust::parser::js::eval_js;

/// 从真实源配置取出的 sourceUrl(JS 表达式)。
const SOURCE_URL: &str = "http@js:eval(String(cache.getFromMemory('yckdm')))";

/// 用户库里「源仓库」的 header 脚本(真实内容, 见 reader.db 的 rssSources.json)。
const HEADER_BODY: &str = r#"
cache.putMemory('yckdm',String.raw`
java.toast('地址发布页 yckceo.vip');
var surl='https://yckceo.vip', mint=9999;
var rr=java.ajax(surl);
rr = org.jsoup.Jsoup.parse(rr);
rr = rr.select(".address-list a:not([href='#'])");
rr.length<2&&java.longToast('发布页访问异常');
Packages.java.util.Collections.reverse(rr);
let aly = new JavaImporter(Packages.org.jsoup.Jsoup, Packages.org.jsoup.Connection);
for (char of rr) {
	let url = char.attr("href");
	url = 'https://' + String(url).replace(/^.*\//, '');
	try {
	let st = Date.now();
		with (aly) {
			Jsoup.connect(url)
				.timeout(5000)
				.ignoreContentType(true)
				.followRedirects(false)
				.method(Connection.Method.HEAD)
				.execute()}
	let et = Date.now();
	url+=(cache.get('yckwb')||'');
	let tm = et-st;
	if (tm<900) {surl=url; break;}
	if (tm<mint) {mint=tm;surl=url;}
	} catch(err) {}
}
try {
cookie.setCookie(surl,source.getLoginHeaderMap().Cookie);
} catch(e) {cookie.removeCookie(surl);}
surl
`);
JSON.stringify({"User-Agent":java.getWebViewUA().replaceAll("; wv| Version\/4.0",'')});
"#;

/// 源仓库的 `header` 只负责"把探测脚本写进内存缓存"; 真正的探测发生在
/// `sourceUrl` 被 `eval()` 求值的那一刻。这个用例验证两步串起来能产出可用地址。
#[test]
#[ignore = "真实访问 yckceo.vip 并做镜像测速, 需要网络"]
fn repo_source_resolves_to_live_url() {
    // 第 1 步: 跑 header, 把探测脚本写入内存缓存。
    let header = HEADER_BODY.trim();
    let header_out = eval_js(header, "", SOURCE_URL).expect("header 脚本应执行成功");
    println!("[1] header 输出: {header_out}");
    assert!(
        header_out.contains("User-Agent"),
        "header 规则最终要产出请求头 JSON, 拿到: {header_out}"
    );

    // 断言缓存里确实有探测脚本(前置条件, 否则第 2 步必然求值失败)。
    let cached = eval_js(
        "var v=cache.getFromMemory('yckdm'); typeof v==='string' ? String(v.length) : 'MISSING'",
        "",
        "",
    )
    .expect("读内存缓存");
    println!("[2] yckdm 缓存长度: {cached}");
    assert_ne!(cached, "MISSING", "header 必须把探测脚本写进内存缓存");

    // 第 2 步: 求值 sourceUrl —— 这一步会真实访问发布页并测速。
    let resolved = eval_js("eval(String(cache.getFromMemory('yckdm')))", "", SOURCE_URL)
        .expect("sourceUrl 求值应成功");
    println!("[3] 解析出的地址: {resolved}");

    assert!(
        resolved.starts_with("https://") || resolved.starts_with("http://"),
        "应解析出真实可用的镜像地址, 拿到: {resolved}"
    );
    assert!(
        resolved.contains("yck"),
        "应是 yckceo 系镜像, 拿到: {resolved}"
    );
}
