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

    // setContent: 部分源依赖它把上一步片段交给下一步规则解析
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
/// 聚合类源的 header 脚本正是 `Collections.reverse(rr)` 之后
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

/// 部分书源 jsLib 的设备指纹生成依赖 `Packages.java.util.UUID.randomUUID`;
/// 缺失时脚本抛 "cannot read property 'randomUUID' of undefined" 并中断整个
/// 设备池初始化，目录与正文都取不到。
#[test]
fn java_util_uuid_random_uuid_returns_distinct_uuids() {
    let result = eval_js(
        r#"
        var a = String(Packages.java.util.UUID.randomUUID());
        var b = String(Packages.java.util.UUID.randomUUID());
        JSON.stringify([
            /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(a),
            a !== b
        ]);
        "#,
        "",
        "https://uuid.test",
    )
    .unwrap();
    assert_eq!(result, r#"[true,true]"#);
}

/// `new BigInteger(numBits, SecureRandom)` 是书源 DH 通道的私钥生成入口;
/// 缺了随机大数构造与 `modPow` 会在正文解密前就失败。
#[test]
fn java_big_integer_supports_random_constructor_and_mod_pow() {
    let result = eval_js(
        r#"
        var J = Packages.java;
        var pk = new J.math.BigInteger(256, new J.security.SecureRandom());
        var pub = J.math.BigInteger.valueOf(2).modPow(pk, new J.math.BigInteger('17'));
        var roundTrip = J.math.BigInteger.valueOf(2).modPow(J.math.BigInteger.valueOf(5), new J.math.BigInteger('13'));
        JSON.stringify([
            pk.toString(16).length > 0 && pk.signum() > 0,
            pub.compareTo(new J.math.BigInteger('0')) >= 0 && pub.compareTo(new J.math.BigInteger('17')) < 0,
            roundTrip.toString()
        ]);
        "#,
        "",
        "https://biginteger.test",
    )
    .unwrap();
    assert_eq!(result, r#"[true,true,"6"]"#);
}

/// `Packages.java.nio.charset.Charset.forName` 与 `java.net.URL` 是书源把二进制
/// 字节与 JS 字符串互转、构造连接的入口；缺任一项 jsLib 顶层就不可用。
#[test]
fn java_nio_charset_and_net_url_surface_is_available() {
    let result = eval_js(
        r#"
        var J = Packages.java;
        var cs = J.nio.charset.Charset.forName('ISO-8859-1');
        var url = new J.net.URL('https://example.test/a?b=1');
        var connection = url.openConnection();
        connection.setRequestMethod('POST');
        JSON.stringify([
            String(cs),
            String(url.getHost()),
            String(url.getProtocol()),
            typeof connection.getOutputStream,
            typeof connection.getInputStream
        ]);
        "#,
        "",
        "https://nio.test",
    )
    .unwrap();
    assert_eq!(result, r#"["ISO-8859-1","example.test","https","function","function"]"#);
}

/// 源用 `new java.lang.String(bytes, Charset.forName('ISO-8859-1'))` 把字节还原成
/// 字符串；Charset 实例必须能被 JavaString 的 charset 参数识别。
#[test]
fn java_string_accepts_charset_instance_for_byte_conversion() {
    let result = eval_js(
        r#"
        var J = Packages.java;
        var latin1 = J.nio.charset.Charset.forName('ISO-8859-1');
        var text = new J.lang.String([104, 105, 45, 200], latin1);
        String(text) + '|' + text.getBytes(latin1).length;
        "#,
        "",
        "https://charset.test",
    )
    .unwrap();
    assert_eq!(result, "hi-\u{c8}|4");
}

/// `GZIPOutputStream` / `BufferedReader` 是设备注册包与响应读取的必要组件。
#[test]
fn java_zip_output_stream_and_buffered_reader_round_trip() {
    let result = eval_js(
        r#"
        var J = Packages.java;
        var sink = new J.io.ByteArrayOutputStream();
        var gz = new J.util.zip.GZIPOutputStream(sink);
        gz.write(new J.lang.String('hello').getBytes('UTF-8'));
        gz.close();
        var reader = new J.io.BufferedReader(new J.io.InputStreamReader(
            new J.util.zip.GZIPInputStream(new J.io.ByteArrayInputStream(sink.toByteArray())), 'UTF-8'
        ));
        var line = reader.readLine();
        JSON.stringify([line, sink.toByteArray().length > 10]);
        "#,
        "",
        "https://zip.test",
    )
    .unwrap();
    assert_eq!(result, r#"["hello",true]"#);
}

/// 字节数组形态的 key/iv/明文必须按**原始字节**参与加解密。
///
/// 回归: 早先 `enc` 分支用 `data.as_bytes()`（UTF-8），含 ≥0x80 的字节会在加密前
/// 被重新编码成两字节, 密文与 legado 不一致。期望值由 Node `crypto` 生成:
/// `key = 0xC0..0xCF`(16 字节数组)、`iv = 0x00..0x0F`、明文 `[0x41,0xC3,0xA9]`。
#[test]
fn symmetric_crypto_uses_raw_bytes_for_byte_array_inputs() {
    let result = eval_js(
        r#"
        var J = Packages.java;
        var key = [], iv = [];
        for (var i = 0; i < 16; i++) { key.push(0xC0 + i); iv.push(i); }
        var encrypted = J.createSymmetricCrypto('AES/CBC/PKCS5Padding', key, iv)
            .encryptBase64([0x41, 0xC3, 0xA9]);
        encrypted;
        "#,
        "",
        "https://sym-bytes.test",
    )
    .unwrap();
    // 若明文按 UTF-8 重新编码（0xC3 0xA9 -> 0xC3 0x83 0xC2 0xA9），这里是另一个值。
    assert_eq!(result, "YwNNYXl3QAsfsI8drtX6CQ==");
}

/// 文本 key 中含 U+0080..U+00FF 的字符时按 UTF-8 参与运算 ——
/// 不能被当成 latin1 字节序列（否则同一 key 的密文与 legado 不一致）。
/// 期望值由 Node `crypto` 生成: key = `'abcdefghijklmn' + 'é'`（UTF-8 恰好 16 字节）。
#[test]
fn symmetric_crypto_keeps_text_key_as_utf8() {
    let result = eval_js(
        r#"
        var J = Packages.java;
        J.createSymmetricCrypto('AES/ECB/PKCS5Padding', 'abcdefghijklmn\u00e9', '')
            .encryptHex('abc');
        "#,
        "",
        "https://sym-text.test",
    )
    .unwrap();
    assert_eq!(result, "2eba350dfc5b48107c691e21bff41cb6");
}
