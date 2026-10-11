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
    // 大灰狼聚合等 jsLib 顶层使用 Rhino 的 JavaImporter + with 语法
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
        let result = eval_js(
            &format!(r#"(function(){{ try {{ {probe}; return 'available'; }} catch (e) {{ return 'unavailable'; }} }})()"#),
            "",
            "https://probe.test",
        )
        .unwrap();
        assert_eq!(result, "unavailable", "{probe} 必须保持不可用, 否则源会误判运行环境");
    }
}
