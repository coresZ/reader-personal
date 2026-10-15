use reader_rust::parser::js::eval_js;

#[test]
fn java_aes_base64_decode_to_string_decrypts_legado_paths() {
    let encrypted = "UhQTfQq/qXGCKPd5D+cjxB7Y0AzwiFMYBmcN5nIm2PboUavKiWEIVaAPIhDXbkox";
    let result = eval_js(
        r#"java.aesBase64DecodeToString(result, "f041c49714d39908", "AES/CBC/PKCS5Padding", "0123456789abcdef")"#,
        encrypted,
        "http://api.jmlldsc.com",
    )
    .unwrap();

    assert_eq!(result, "http://api.lemiyigou.com/655/655791/70398.json");
}

#[test]
fn java_importer_supports_rhino_with_scope() {
    // 部分书源 jsLib 顶层使用 Rhino 的 JavaImporter + with 语法
    let result = eval_js(
        r#"
        var importer = new JavaImporter(Packages.java.util, Packages.java.lang);
        var viaImport, viaFull = new java.util.ArrayList();
        with (importer) {
            var list = new ArrayList();
            list.add('a');
            viaImport = list.size() + ':' + String(new java.lang.String('ok')).length;
        }
        importer.importClass(Packages.java.lang.System);
        JSON.stringify([viaImport, viaFull.size(), importer.System !== undefined]);
        "#,
        "",
        "https://importer.test",
    )
    .unwrap();
    assert_eq!(result, r#"["1:2",0,true]"#);

    // 空参构造（safeImport 模式）不应报错
    let result = eval_js(
        "var i = new JavaImporter(); i.importPackage(Packages.java.util); typeof i.HashMap",
        "",
        "https://importer.test",
    )
    .unwrap();
    assert_eq!(result, "function");
}

#[test]
fn browser_family_aliases_and_ajax_all_match_legado_surface() {
    // startBrowserDp / showReadingBrowser 是 startBrowser 的别名;
    // ajaxAll 返回 StrResponse 数组。
    let result = eval_js(
        r#"
        var out = [];
        out.push(java.startBrowserDp === java.startBrowser);
        out.push(typeof java.showReadingBrowser);
        out.push(typeof java.ajaxAll);
        out.push(java.ajaxAll([]).length);
        JSON.stringify(out);
        "#,
        "",
        "https://browser-family.test",
    )
    .unwrap();
    assert_eq!(result, r#"[true,"function","function",0]"#);
}

/// 兼容层补齐的纯工具方法：`java.t2s`/`s2t`（繁简）、`java.setContent`
/// （替换后续 `getString` 的解析对象）、`java.encodeURI`、`java.toURL`。
#[test]
fn java_text_and_rule_helpers_match_legado_semantics() {
    // 繁转简: 源用 java.t2s 把繁体站点的章节名/正文转成简体
    let t2s = eval_js(
        r#"JSON.stringify([java.t2s('第一章 萬古神帝'), java.t2s('這個書國學時體'), java.t2s('简体字不应变')])"#,
        "",
        "https://text-helpers.test",
    )
    .unwrap();
    assert_eq!(
        t2s,
        r#"["第一章 万古神帝","这个书国学时体","简体字不应变"]"#,
        "繁体转简体, 简体保持不变"
    );

    let s2t = eval_js(r#"java.s2t('万古神帝')"#, "", "https://text-helpers.test").unwrap();
    assert_eq!(s2t, "萬古神帝", "简体转繁体");

    // setContent: 晋江等源依赖它把上一步片段交给下一步规则解析
    let set_content = eval_js(
        r#"(function(){
            java.setContent('{"title":"新内容"}');
            var a = java.getString('$.title');
            java.setContent('{"title":"再次替换"}');
            return a + '|' + java.getString('$.title');
        })()"#,
        "",
        "https://text-helpers.test",
    )
    .unwrap();
    assert_eq!(set_content, "新内容|再次替换", "setContent 应替换解析对象");

    let encoded = eval_js(r#"java.encodeURI('第一章 & 测试')"#, "", "https://text-helpers.test").unwrap();
    assert!(encoded.contains("%26") && !encoded.contains(' '), "应做 URL 编码: {encoded}");

    // toURL: 源取 .origin/.host（QuickJS 无 Web URL 构造器, 由宿主解析）
    let origin = eval_js(
        r#"java.toURL('/a/b?x=1').pathname + '|' + java.toURL('https://www.example.com/x').origin"#,
        "",
        "https://base.example.test",
    )
    .unwrap();
    assert_eq!(origin, "/a/b|https://www.example.com", "相对地址应基于当前页面解析");
}

/// `java.qread()` / `java.deviceID()` / `java.getAppVariant()` 是**环境判据**，
/// 源用 `try{ java.qread() }catch{}` 判断运行在哪个 App 上。它们必须保持
/// 不可用，否则源会误判环境并走错分支（例如把正文当"短剧播放器"渲染）。
#[test]
fn environment_probe_apis_stay_unavailable() {
    for probe in ["java.qread()", "java.deviceID()", "java.getAppVariant()"] {
        // 注意花括号转义: `format!` 里 `{{`/`}}` 各产出一个字面花括号,
        // 而 `(function(){{ ... }})()` 需要 **两个** 闭合花括号
        // (一个闭合 function 体, 一个闭合外层 `(function(){...})` 的圆括号前缀之外)。
        //
        // 这里直接拼字符串而不是用 format! 模板, 避免转义计数出错导致
        // "unexpected token ')'" 这类与被测行为无关的语法失败。
        let script = format!(
            "(function(){{ try {{ {probe}; return 'available'; }} catch (e) {{ return 'unavailable'; }} }})()"
        );
        let result = eval_js(&script, "", "https://probe.test").unwrap();
        assert_eq!(result, "unavailable", "{probe} 必须保持不可用, 否则源会误判运行环境");
    }
}

/// `Packages.java.util.Collections.reverse` 必须**就地按索引交换**，不能经
/// `array()` 重建列表 —— 入参常是 jsoup 包装的 Elements，元素带 `attr()` 方法，
/// 重建后会退化成纯数据，`char.attr('href')` 随即报 "not a function"。
///
/// 「源仓库」订阅源的 header 脚本正是 `Collections.reverse(rr)` 之后
/// `for (char of rr) char.attr('href')`，这条用例守住该行为。
#[test]
fn collections_reverse_keeps_element_methods() {
    let html = r#"<div class="address-list"><a href="https://m1.example/x">m1</a><a href="https://m2.example/y">m2</a></div>"#;
    let script = format!(
        r#"
        var rr = org.jsoup.Jsoup.parse({html:?}).select(".address-list a");
        var before = String(rr[0].attr('href'));
        Packages.java.util.Collections.reverse(rr);
        var after = String(rr[0].attr('href'));
        JSON.stringify([before, after, rr.length]);
        "#,
        html = html
    );
    let result = eval_js(&script, "", "https://collections.test").unwrap();
    assert_eq!(
        result, r#"["https://m1.example/x","https://m2.example/y",2]"#,
        "reverse 后应按索引反转且元素仍可调用 attr(): {result}"
    );
}

/// `Packages.java.util.Collections` 缺失时源脚本会抛
/// "cannot read property 'reverse' of undefined" 并中断整段脚本。
#[test]
fn java_util_collections_is_available() {
    let result = eval_js(
        "typeof Packages.java.util.Collections.reverse + '|' + typeof java.util.Collections.sort",
        "",
        "https://collections.test",
    )
    .unwrap();
    assert_eq!(result, "function|function");
}

/// `org.jsoup.Jsoup.connect(url)` 链式客户端（legado 源用它做表单提交与连通性
/// 探测）。这里只验证链式 API 表面可构建，不发真实请求。
#[test]
fn jsoup_connect_chain_is_constructible() {
    let result = eval_js(
        r#"
        var c = org.jsoup.Jsoup.connect('https://connect.test/x')
            .timeout(5000).ignoreContentType(true).followRedirects(false)
            .method(Packages.org.jsoup.Connection.Method.HEAD);
        JSON.stringify([
            typeof c.execute,
            Packages.org.jsoup.Connection.Method.HEAD,
            typeof Packages.org.jsoup.Jsoup.connect
        ]);
        "#,
        "",
        "https://connect.test",
    )
    .unwrap();
    assert_eq!(result, r#"["function","HEAD","function"]"#);
}

/// `JavaImporter(Packages.org.jsoup.Jsoup)` 要按**类名**注册 importer 属性。
/// 早期实现把对象成员平铺(变成 `connect`/`parse`), `with (aly) { Jsoup.connect }`
/// 因此找不到 `Jsoup` 而抛 "not a function"。
#[test]
fn java_importer_registers_class_names_for_objects() {
    let result = eval_js(
        r#"
        var aly = new JavaImporter(Packages.org.jsoup.Jsoup, Packages.org.jsoup.Connection);
        JSON.stringify([typeof aly.Jsoup, typeof aly.Connection, typeof aly.connect]);
        "#,
        "",
        "https://importer.test",
    )
    .unwrap();
    // 第三个断言: 不再把 Jsoup 的成员平铺到 importer 上。
    assert_eq!(result, r#"["object","object","undefined"]"#);
}

