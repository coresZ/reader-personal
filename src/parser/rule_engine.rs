use crate::model::rule::{BookInfoRule, SearchRule, TocRule};
use crate::model::{
    book::Book, book_chapter::BookChapter, book_source::BookSource, search::SearchBook,
};
use crate::parser::{
    html,
    js::{eval_js, eval_js_with_bindings, push_js_log, with_book_url, with_source},
    jsonpath,
    rule_analyzer::split_top_level,
};
use crate::util::text::{apply_regex_replace, normalize_source_url};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::OnceLock;
use sxd_xpath::{Context as XPathContext, Factory as XPathFactory, Value as XPathValue};

/// 固定 pattern 的正则提升为全局静态, 避免每次字段解析都重新编译。
/// 这些函数在解析搜索结果列表时对每个字段、每个元素调用, 属于极高频热路径。
fn inline_js_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\{\{([^}]+)\}\}").unwrap())
}

fn inline_js_lazy_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\{\{(.*?)\}\}").unwrap())
}

fn get_template_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"@get:\{([^}]+)\}").unwrap())
}

fn placeholder_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\$(\d{1,2})").unwrap())
}

#[derive(Clone, Default)]
pub struct RuleEngine;

#[derive(Debug, Clone, PartialEq)]
enum ParseMode {
    Css,      // CSS selector
    XPath,    // XPath expression
    JsonPath, // JSONPath expression
    Regex,    // Regex pattern
    Js,       // JavaScript
}

impl RuleEngine {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self)
    }

    /// Detect the parsing mode from the rule string
    fn detect_mode(&self, rule: &str, content: &str) -> ParseMode {
        let rule = rule.trim();

        // Explicit mode forcing
        if rule.starts_with("@css:") || rule.starts_with("@CSS:") {
            return ParseMode::Css;
        }
        if rule.starts_with("@xpath:") || rule.starts_with("@XPath:") || rule.starts_with("@XPATH:")
        {
            return ParseMode::XPath;
        }
        if rule.starts_with("@json:") || rule.starts_with("@Json:") || rule.starts_with("@JSON:") {
            return ParseMode::JsonPath;
        }
        if rule.starts_with("@regex:") || rule.starts_with("@Regex:") {
            return ParseMode::Regex;
        }
        if rule.starts_with("js:") || rule.starts_with("@js:") || rule.starts_with("<js>") {
            return ParseMode::Js;
        }

        // Auto-detect from rule prefix
        if rule.starts_with('/') || rule.starts_with("./") {
            return ParseMode::XPath;
        }
        if rule.starts_with("$.") || rule.starts_with("$[") {
            return ParseMode::JsonPath;
        }
        if rule.starts_with(':') {
            return ParseMode::Regex;
        }

        // Auto-detect from content
        let content_trimmed = content.trim();
        if content_trimmed.starts_with('{') || content_trimmed.starts_with('[') {
            // Likely JSON content
            if rule.starts_with("$.") || rule.starts_with("$[") {
                return ParseMode::JsonPath;
            }
            // Try to parse as JSON
            if serde_json::from_str::<Value>(content_trimmed).is_ok() {
                return ParseMode::JsonPath;
            }
        }

        // Default to CSS
        ParseMode::Css
    }

    /// Strip mode prefix from rule
    fn strip_mode_prefix<'a>(&self, rule: &'a str) -> &'a str {
        strip_mode_prefix(rule)
    }

    fn parse_search_list(
        &self,
        source: &BookSource,
        body: &str,
        base_url: &str,
        rule: &SearchRule,
        list_rule: &str,
    ) -> Vec<SearchBook> {
        // 先处理开头的 JS 段：legado `AnalyzeRule.splitSourceRule` 用 `JS_PATTERN` 把
        // `<js>…</js>` 单独切成一个 `Mode.Js` 段，**它的输出是后续段的内容**
        // （`getString` 循环里 result 逐段往后传），不是并列的候选。
        //
        // 该源 bookList 正是这个形态：
        //   `<js>…</js>\n.item||$.data.worksList[*]||$.[?(@.alias_id)]`
        // 先跑脚本拿到 graphql 响应，再在其上按 `||` 找列表。
        if matches!(self.detect_mode(list_rule, body), ParseMode::Js) {
            return self.search_books_js(source, body, base_url, rule, list_rule);
        }

        // 非 JS 前缀的列表规则支持 `&&` / `||` / `%%` 组合
        //（legado `RuleAnalyzer.splitRule`），各段独立在同一内容上求值。
        //
        // 必须在**分模式之前**拆：否则 `$.data.worksList[*]||$.[?(@.alias_id)]`
        // 会被当成一个 JSONPath 表达式交给 `jsonpath_query`，求值必然为空。
        let combo = split_top_level(list_rule, &["&&", "||", "%%"]);
        if let Some(operator) = combo.delimiter.as_deref() {
            let parts = combo.parts;
            match operator {
                // `||`：取第一个非空列表
                "||" => {
                    for part in &parts {
                        if part.trim().is_empty() {
                            continue;
                        }
                        let books = self.parse_search_list(source, body, base_url, rule, part);
                        if !books.is_empty() {
                            return books;
                        }
                    }
                    return Vec::new();
                }
                // `&&`：依次解析并合并
                "&&" => {
                    let mut out = Vec::new();
                    for part in &parts {
                        if part.trim().is_empty() {
                            continue;
                        }
                        out.extend(self.parse_search_list(source, body, base_url, rule, part));
                    }
                    return out;
                }
                // `%%`：按索引交错
                "%%" => {
                    let lists: Vec<Vec<SearchBook>> = parts
                        .iter()
                        .filter(|p| !p.trim().is_empty())
                        .map(|p| self.parse_search_list(source, body, base_url, rule, p))
                        .collect();
                    let max_len = lists.iter().map(Vec::len).max().unwrap_or(0);
                    let mut out = Vec::new();
                    for i in 0..max_len {
                        for list in &lists {
                            if let Some(book) = list.get(i) {
                                out.push(book.clone());
                            }
                        }
                    }
                    return out;
                }
                _ => {}
            }
        }

        match self.detect_mode(list_rule, body) {
            ParseMode::JsonPath => self.search_books_json(source, body, base_url, rule, list_rule),
            ParseMode::XPath => self.search_books_xpath(source, body, base_url, rule, list_rule),
            ParseMode::Js => self.search_books_js(source, body, base_url, rule, list_rule),
            ParseMode::Regex => self.search_books_regex(source, body, base_url, rule, list_rule),
            ParseMode::Css => self.search_books_html(source, body, base_url, rule, list_rule),
        }
    }

    pub fn search_books(&self, source: &BookSource, body: &str, base_url: &str) -> Vec<SearchBook> {
        with_source(source,|| {
            if let Some(pattern) = source.book_url_pattern.as_deref().filter(|p| !p.trim().is_empty()) {
                match regex::Regex::new(&format!("^(?:{pattern})$")) {
                    Ok(pattern) if pattern.is_match(base_url) => {
                        return self.search_detail_fallback(source, body, base_url).into_iter().collect();
                    }
                    Err(err) => push_js_log(format!("[书籍地址规则] {err}")),
                    _ => {}
                }
            }
            let rule = source.rule_search.clone().unwrap_or_default();
            let (list_rule, reverse) = normalize_list_rule(rule.book_list.as_deref().unwrap_or(""));
            let mut results = self.parse_search_list(source, body, base_url, &rule, list_rule);

            if results.is_empty()
                && source
                    .book_url_pattern
                    .as_deref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true)
            {
                if let Some(detail_book) = self.search_detail_fallback(source, body, base_url) {
                    results.push(detail_book);
                }
            }
            if reverse {
                results.reverse();
            }
            results
        })
    }

    pub fn explore_books(
        &self,
        source: &BookSource,
        body: &str,
        base_url: &str,
    ) -> Vec<SearchBook> {
        with_source(source,|| {
            let rule = source
                .rule_explore
                .clone()
                .filter(|rule| {
                    rule.book_list
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty())
                })
                .unwrap_or_else(|| source.rule_search.clone().unwrap_or_default());
            let (list_rule, reverse) = normalize_list_rule(rule.book_list.as_deref().unwrap_or(""));
            let mut results = self.parse_search_list(source, body, base_url, &rule, list_rule);
            if reverse {
                results.reverse();
            }
            results
        })
    }

    pub fn book_info(
        &self,
        source: &BookSource,
        body: &str,
        base_url: &str,
        book_url: &str,
    ) -> Book {
        with_source(source,|| {
            let rule = source.rule_book_info.clone().unwrap_or_default();
            let mut context = HashMap::new();

            // 详情页规则可能依赖 `java.put` 写下的变量。该源的
            // `ruleBookInfo.kind` 就会 `java.put("is_bundle", …)`，而 `tocUrl`
            // 用 `java.get("is_bundle")` 判断走不走专栏目录接口 —— 字段按
            // init → name → author → intro → kind → … → tocUrl 的顺序执行
            //（`parse_book_info_html`），`kind` 先跑，所以 `tocUrl` 读得到。
            //
            // 这里不再额外重跑书链规则：发现页解析完的 `java.put` 已由
            // `putVariable` 落到持久 KV（见 js.rs 的 `book.putVariable`），
            // 而详情页自己也会通过 `kind` 字段补齐。
            let prepared = rule.init.as_deref().filter(|s| !s.trim().is_empty()).map(|init| {
                self.prepare_rule_body(init, body, base_url).unwrap_or_default()
            });
            let body = prepared.as_deref().unwrap_or(body);

            let mode = self.detect_mode(rule.name.as_deref().unwrap_or(""), body);
            match mode {
                ParseMode::JsonPath => {
                    if let Ok(v) = serde_json::from_str::<Value>(body) {
                        return parse_book_info_json(
                            source,
                            &v,
                            base_url,
                            &rule,
                            book_url,
                            &mut context,
                        );
                    }
                }
                ParseMode::XPath => {
                    return parse_book_info_xpath(
                        source,
                        body,
                        base_url,
                        &rule,
                        book_url,
                        &mut context,
                    );
                }
                _ => {}
            }
            parse_book_info_html(source, body, base_url, &rule, book_url, &mut context)
        })
    }

    fn prepare_rule_body(&self, rule: &str, body: &str, base_url: &str) -> anyhow::Result<String> {
        match self.detect_mode(rule, body) {
            ParseMode::Js => {
                let (output, remaining_rule) = eval_leading_js_rule(rule, body, base_url)?;
                if remaining_rule.is_empty() {
                    Ok(output)
                } else {
                    self.prepare_rule_body(remaining_rule, &output, base_url)
                }
            }
            ParseMode::Css | ParseMode::XPath => Ok(Self::eval_rule_elements(rule, body).first()
                .and_then(|value| value["outerHtml"].as_str()).unwrap_or("").to_string()),
            _ => Ok(self.extract_content(rule, body)),
        }
    }

    fn parse_toc_list(
        &self,
        body: &str,
        base_url: &str,
        rule: &TocRule,
        list_rule: &str,
        context: &mut HashMap<String, String>,
    ) -> (Vec<BookChapter>, Vec<String>) {
        match self.detect_mode(list_rule, body) {
            ParseMode::JsonPath => parse_chapter_list_json(body, base_url, rule, self.strip_mode_prefix(list_rule), context),
            ParseMode::XPath => parse_chapter_list_xpath(body, base_url, rule, self.strip_mode_prefix(list_rule), context),
            ParseMode::Js => self.parse_chapter_list_js(body, base_url, rule, list_rule, context),
            ParseMode::Regex => self.parse_chapter_list_regex(body, base_url, rule, list_rule),
            ParseMode::Css => parse_chapter_list_html(body, base_url, rule, list_rule, context),
        }
    }

    pub fn chapter_list(
        &self,
        source: &BookSource,
        body: &str,
        base_url: &str,
        book_url: Option<&str>,
    ) -> (Vec<BookChapter>, Vec<String>) {
        with_source(source,|| {
            with_book_url(book_url, || {
                let rule = source.rule_toc.clone().unwrap_or_default();
            let mut context = HashMap::new();
            let (list_rule, reverse) =
                normalize_list_rule(rule.chapter_list.as_deref().unwrap_or(""));
            let prepared_body = prepare_toc_body(body, base_url, &rule);
            let (mut chapters, next_urls) = self.parse_toc_list(
                &prepared_body, base_url, &rule, list_rule, &mut context,
            );
                apply_toc_format_js(&mut chapters, rule.format_js.as_deref(), base_url);
                if reverse {
                    chapters.reverse();
                }
                for (index, chapter) in chapters.iter_mut().enumerate() {
                    chapter.index = index as i32;
                }
                (chapters, next_urls)
            })
        })
    }

    pub fn content(
        &self,
        source: &BookSource,
        body: &str,
        base_url: &str,
        book_url: Option<&str>,
    ) -> String {
        with_source(source,|| {
            with_book_url(book_url, || {
                let rule = source.rule_content.clone().unwrap_or_default();
                let mut content_body = body.to_string();

            if let Some(source_regex) = rule
                .source_regex
                .as_deref()
                .filter(|s| !s.trim().is_empty())
            {
                content_body = apply_legado_regex(&content_body, source_regex);
            }
            if let Some(web_js) = rule.web_js.as_deref().filter(|s| !s.trim().is_empty()) {
                match eval_js(self.strip_mode_prefix(web_js), &content_body, base_url) {
                    Ok(processed) if !processed.trim().is_empty() => content_body = processed,
                    Ok(_) => {}
                    Err(e) => push_js_log(format!("[webJs] {e}")),
                }
            }

            if let Some(mut content_rule) = rule.content.clone() {
                // Execute leading scripts before selecting their returned content,
                // including aggregate rules ending in </js>$.content.
                while matches!(self.detect_mode(&content_rule, &content_body), ParseMode::Js) {
                    match eval_leading_js_rule(&content_rule, &content_body, base_url) {
                        Ok((output, remaining_rule)) => {
                            content_body = output;
                            content_rule = remaining_rule.to_string();
                            if content_rule.is_empty() { break; }
                        }
                        Err(err) => {
                            push_js_log(format!("[正文 JS] {err}"));
                            tracing::warn!("content js failed for {}: {err}", source.book_source_name);
                            content_body.clear();
                            content_rule.clear();
                            break;
                        }
                    }
                }
                let (pure, js) = extract_js(&content_rule);
                let (selector_rule, script) = (pure.to_string(), js.map(str::to_string));

                let mut content = if selector_rule.trim().is_empty() {
                    content_body.clone()
                } else {
                    let selector_rule =
                        self.process_inline_js(&selector_rule, &content_body, base_url);
                    self.extract_content(&selector_rule, &content_body)
                };

                if let Some(script) = script.filter(|s| !s.trim().is_empty()) {
                    match eval_js(&script, &content, base_url) {
                        Ok(res) => content = res,
                        Err(e) => {
                            // JS 失败时保留选择器提取结果, 整条 JS 规则则返回空
                            push_js_log(format!("[正文 JS] {e}"));
                            tracing::warn!("content js failed for {}: {e}", source.book_source_name);
                            if selector_rule.trim().is_empty() {
                                content = String::new();
                            }
                        }
                    }
                }

                if let Some(replace) = rule.replace_regex.as_deref() {
                    if matches!(self.detect_mode(replace, &content), ParseMode::Js) {
                        match eval_js(self.strip_mode_prefix(replace), &content, base_url) {
                            Ok(result) => content = result,
                            Err(err) => push_js_log(format!("[正文替换] {err}")),
                        }
                    } else {
                        content = apply_legado_regex(&content, replace);
                    }
                }

                return content;
            }

            String::new()
            })
        })
    }

    /// 用一条规则解析给定内容, 供 JS 的 `java.getString` / `java.getStringList` 使用
    /// (对齐 legado `AnalyzeRule.getString` / `getStringList`)。
    ///
    /// `list` 为 true 时返回全部匹配值, 否则只取第一个。
    pub fn eval_rule_on(rule: &str, content: &str, base_url: &str, list: bool) -> Vec<String> {
        let engine = RuleEngine;
        let rule = rule.trim();
        if rule.is_empty() {
            return Vec::new();
        }
        // 与正文规则 `content()` 一致: 整条是 JS(`js:` / `@js:` / `<js>`)时直接以
        // 页面内容为 result; 否则按 `选择器@js:脚本` 先提取、再把结果交给 JS。
        // (`extract_js` 只认 `@js:` / `<js>`, 所以这里必须先判一次模式。)
        //
        // 脚本段用 `extract_js` 取，不用 `strip_mode_prefix`：后者要求 `</js>` 落在
        // **字符串末尾**，而 `<js>…</js>` 后面常跟字段组合链（该源 bookList 是
        // `<js>…</js>\n.item||$.data.worksList[*]`）。`strip_suffix` 取不到脚本时会把
        // 带标签的整条文本当 JS 源码，轻则语法错误、重则静默回退成页面原文。
        //
        // `extract_js` 认得 `<js>` / `@js:` / `js:`；都认不出时（裸 JS，如
        // `java.getString('…')` 那种写法）把整条规则当脚本。
        let (selector_rule, script) = extract_js(rule);
        let (selector_rule, script) = match script {
            Some(script) => (selector_rule.to_string(), Some(script.to_string())),
            None if matches!(engine.detect_mode(rule, content), ParseMode::Js) => {
                (String::new(), Some(rule.to_string()))
            }
            None => (selector_rule.to_string(), None),
        };
        let mut values = if selector_rule.trim().is_empty() {
            vec![content.to_string()]
        } else if list {
            engine.extract_content_list(&selector_rule, content)
        } else {
            vec![engine.extract_content(&selector_rule, content)]
        };
        if let Some(script) = script.filter(|s| !s.trim().is_empty()) {
            values = values
                .iter()
                .map(|value| eval_js(&script, value, base_url).unwrap_or_else(|_| value.clone()))
                .collect();
            // legado: 最终结果是 String 时按行拆成列表(仅 list 模式;
            // 元素选择器返回的是 List, 不触发拆分)。保留空项以对齐 split 语义。
            if list {
                values = values
                    .iter()
                    .flat_map(|value| value.split('\n').map(str::to_string))
                    .collect();
            }
        }
        values
    }

    /// 用一条规则取元素快照, 供 JS 的 `java.getElement` / `java.getElements` 使用。
    ///
    /// legado 返回可继续操作的 JSoup Element; 这里返回等价的只读快照
    /// (tag/text/ownText/innerHtml/outerHtml/attrs), 由 JS 侧包装成对象。
    pub fn eval_rule_elements(rule: &str, content: &str) -> Vec<Value> {
        let engine = RuleEngine;
        let rule = rule.trim();
        if rule.is_empty() {
            return Vec::new();
        }
        // 只有 CSS 选择器能直接拿到 ElementRef, 其余模式必须先按各自模式取值再包装,
        // 否则 `//p` 会被当成 CSS 选择器解析、结果恒为空。
        match engine.detect_mode(rule, content) {
            ParseMode::Css => {
                let doc = html::parse_document(content);
                let selector = engine.strip_mode_prefix(rule).trim();
                html::select_list(&doc, selector)
                    .into_iter()
                    .map(|element| element_snapshot(&element))
                    .collect()
            }
            ParseMode::JsonPath => {
                // `java.getElements("$.list[*]")` 在 legado 里返回的是**原始 JSON 节点**
                // （JSONPath 求值结果），元素上可以直接 `.title` / `.id` 访问。
                //
                // 不能走下面的 `snapshot_from_fragment` 兜底：那会把 JSON 文本当成
                // HTML 片段，包成 `{"__snapshot":{"tag":"","text":"{\"title\":…}"}}`。
                // 之后 `build_chapter_from_json` 在快照对象上求值 `title` 必然取不到，
                // 整本目录解析出 0 章 —— 「加载不出目录」正是这个原因。
                let Ok(root) = serde_json::from_str::<Value>(content) else {
                    return Vec::new();
                };
                let path = engine.strip_mode_prefix(rule);
                jsonpath::jsonpath_query(&root, path)
                    .into_iter()
                    .filter(|item| !item.is_null())
                    .collect()
            }
            ParseMode::XPath => {
                // 无 `::extractor` 时 XPath 取的是 string_value(纯文本), 快照会丢 tag/attrs;
                // 显式改用 `::outerHtml`, 让快照保留元素结构。
                let path = engine.strip_mode_prefix(rule).trim();
                let path = if path.contains("::") {
                    path.to_string()
                } else {
                    format!("{path}::outerHtml")
                };
                html::select_xpath(content, &path)
                    .into_iter()
                    .filter(|fragment| !fragment.trim().is_empty())
                    .map(|fragment| snapshot_from_fragment(&fragment))
                    .collect()
            }
            ParseMode::Js => {
                // 整条规则就是脚本, 执行结果按 HTML 片段包装(legado 直接返回 evalJS 的值)。
                let script = engine.strip_mode_prefix(rule).to_string();
                match eval_js(&script, content, "") {
                    Ok(fragment) if !fragment.trim().is_empty() => {
                        vec![snapshot_from_fragment(&fragment)]
                    }
                    Ok(_) => Vec::new(),
                    Err(e) => {
                        push_js_log(format!("[getElements] {e}"));
                        Vec::new()
                    }
                }
            }
            _ => engine
                .extract_content_list(rule, content)
                .into_iter()
                .filter(|fragment| !fragment.trim().is_empty())
                .map(|fragment| snapshot_from_fragment(&fragment))
                .collect(),
        }
    }

    fn extract_content(&self, content_rule: &str, content_body: &str) -> String {
        // 组合规则按段**各自判定模式**再串联，与 legado 一致：
        // `AnalyzeByJSoup.getStringList` 对每段 `ruleStrX` 独立调 `getResultList`
        //（CSS / XPath / JSONPath 各按自己的模式解析），而不是拿整条规则判一次模式。
        //
        // 该源 `bookUrl` 的 `@js:` 脚本里就是 `java.getString("@href||$.url")`：
        // `@href` 是 CSS 属性规则、`$.url` 是 JSONPath。整条一起判模式时会被 `$.url`
        // 带偏成 JsonPath，在 HTML 上求值必然为空 —— 书链于是只拼出
        // `https://books.example.com/column//`（ID 段缺失），详情页/目录/正文连锁失败。
        //
        // JS / Regex 里不拆：那里的 `||` 是 JS 逻辑或、正则里的 `|`，拆了会破坏脚本。
        let mode = self.detect_mode(content_rule, content_body);
        if matches!(mode, ParseMode::Js | ParseMode::Regex) {
            return self.extract_content_single(content_rule, content_body);
        }
        // 组合符只在 `##` **之前**生效：`##` 之后是替换正则，里面的 `|` / `||` 是
        // 正则交替符，不是字段组合符（legado 的 `SourceRule` 也是先切出 `##` 三段，
        // 组合符只作用于 `ruleStrS[0]`）。该源的 `name` / `wordCount` 都带这类正则段。
        let (combinable, replace_part) = split_legado_regex(content_rule);
        let combo = split_top_level(&combinable, &["&&", "||", "%%"]);
        let Some(operator) = combo.delimiter.as_deref() else {
            return self.extract_content_single(content_rule, content_body);
        };
        // 段本身可能带 `@css:` / `@json:` 前缀，各段独立判定。
        let values: Vec<String> = combo
            .parts
            .iter()
            .filter(|part| !part.trim().is_empty())
            .map(|part| self.extract_content_single(part, content_body))
            .collect();
        let joined = match operator {
            "&&" => values.into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join(""),
            // `||` 取第一个非空
            "||" => values.into_iter().find(|v| !v.is_empty()).unwrap_or_default(),
            // `%%` 依次拼接（字段级每段是单值）
            "%%" => values.into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join(""),
            _ => return self.extract_content_single(content_rule, content_body),
        };
        // 替换作用于组合后的整体（legado 对每个 SourceRule 的最终结果做替换）。
        match replace_part {
            Some(reg) => apply_legado_regex(&joined, reg),
            None => joined,
        }
    }

    fn extract_content_single(&self, content_rule: &str, content_body: &str) -> String {
        match self.detect_mode(content_rule, content_body) {
            // legado `AnalyzeByJSonPath.getString` / `getStringList`：值是 List 时
            // `joinToString("\n")`，多值**换行拼接**（不是只取第一个）。
            //
            // 书源脚本依赖这条：该源 featured 分支写
            // `String(java.getString("$..todayWorksIds[*]")).split("\n")`，
            // 期望拿到**全部** id。只取第一个会让 POST 的 worksIds 少两项，
            // 服务端只回 1 本书 —— 症状就是「今日限免只有一本书」。
            ParseMode::JsonPath => serde_json::from_str::<Value>(content_body)
                .ok()
                .map(|v| {
                    jsonpath::jsonpath_query(&v, self.strip_mode_prefix(content_rule))
                        .into_iter()
                        .filter_map(|item| jsonpath::value_to_string(&item))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
            // XPath 同理：`AnalyzeByXPath.getString` 也是多值换行拼接。
            ParseMode::XPath => html::select_xpath(content_body, self.strip_mode_prefix(content_rule))
                .into_iter()
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
            ParseMode::Regex => {
                // `@regex:pattern##find##replace`: 先提取, 再对结果做替换
                let raw = self.strip_mode_prefix(content_rule);
                let (pattern, replace) = split_regex_rule(raw);
                let matched = regex_extract_first(content_body, pattern).unwrap_or_default();
                match replace {
                    Some(part) => apply_legado_regex(&matched, part),
                    None => matched,
                }
            }
            _ => {
                let doc = html::parse_document(content_body);
                html::select_all_text(&doc, self.strip_mode_prefix(content_rule)).unwrap_or_default()
            }
        }
    }

    /// `extract_content` 的多值版本: 取所有匹配, 而不是只取第一个或用换行拼接。
    fn extract_content_list(&self, content_rule: &str, content_body: &str) -> Vec<String> {
        match self.detect_mode(content_rule, content_body) {
            ParseMode::JsonPath => serde_json::from_str::<Value>(content_body)
                .ok()
                .map(|value| {
                    jsonpath::jsonpath_query(&value, self.strip_mode_prefix(content_rule))
                        .iter()
                        .filter_map(jsonpath::value_to_string)
                        .collect()
                })
                .unwrap_or_default(),
            ParseMode::XPath => html::select_xpath(content_body, self.strip_mode_prefix(content_rule)),
            ParseMode::Regex => {
                let raw = self.strip_mode_prefix(content_rule);
                let (pattern, replace) = split_regex_rule(raw);
                let matched = regex_extract_all(content_body, pattern);
                match replace {
                    Some(part) => matched
                        .into_iter()
                        .map(|value| apply_legado_regex(&value, part))
                        .collect(),
                    None => matched,
                }
            }
            _ => {
                let doc = html::parse_document(content_body);
                html::select_text_list(&doc, self.strip_mode_prefix(content_rule))
            }
        }
    }

    /// Process inline JavaScript {{...}} in rules
    fn process_inline_js(&self, rule: &str, body: &str, base_url: &str) -> String {
        let mut result = rule.to_string();

        // Find all {{...}} blocks and evaluate them
        let re = inline_js_re();
        for cap in re.captures_iter(rule) {
            if let Some(js_code) = cap.get(1) {
                match eval_js(js_code.as_str(), body, base_url) {
                    Ok(js_result) => {
                        result = result.replace(cap.get(0).unwrap().as_str(), &js_result);
                    }
                    Err(e) => push_js_log(format!("[{{{{js}}}}] {e}")),
                }
            }
        }

        result
    }

    /// Get the next content page URL if pagination exists
    pub fn next_content_url(
        &self,
        source: &BookSource,
        body: &str,
        base_url: &str,
    ) -> Option<String> {
        let rule = source.rule_content.clone().unwrap_or_default();
        let next_rule = rule.next_content_url.as_deref()?;
        if next_rule.trim().is_empty() {
            return None;
        }

        // `nextContentUrl` 和正文规则一样支持 `选择器@js:脚本` 组合
        // (`#linkNext@href@js:/_/.test(result) ? result : null`)。整条当 CSS
        // 选择器解析的话，分页链接永远取不到，多页正文只剩第一页。
        let mode = self.detect_mode(next_rule, body);
        let next_url = match mode {
            ParseMode::JsonPath => serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|v| jsonpath::jsonpath_first_string(&v, self.strip_mode_prefix(next_rule))),
            ParseMode::XPath => html::select_xpath(body, self.strip_mode_prefix(next_rule))
                .into_iter()
                .find(|value| !value.trim().is_empty()),
            ParseMode::Js | ParseMode::Regex => RuleEngine::eval_rule_on(next_rule, body, base_url, false)
                .into_iter()
                .find(|value| !value.trim().is_empty()),
            ParseMode::Css => {
                // 组合规则(`@href@js:…`)先按 `@js:` 切分：前半段是选择器,
                // 后半段以取到的值为 `result` 求值。
                let (pure, script) = extract_js(next_rule);
                let doc = html::parse_document(body);
                let extracted = if pure.trim().is_empty() {
                    String::new()
                } else {
                    html::select_text(&doc, self.strip_mode_prefix(pure)).unwrap_or_default()
                };
                match script {
                    Some(script) if !script.trim().is_empty() => match eval_js(script, &extracted, base_url) {
                        Ok(value) => Some(value),
                        Err(error) => {
                            push_js_log(format!("[nextContentUrl JS] {error}"));
                            None
                        }
                    },
                    _ => Some(extracted),
                }
            }
        };

        let next_url = next_url
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty() && value != "null" && value != "undefined")?;
        Some(resolve_url(base_url, &next_url))
    }

    fn search_detail_fallback(
        &self,
        source: &BookSource,
        body: &str,
        base_url: &str,
    ) -> Option<SearchBook> {
        let book = self.book_info(source, body, base_url, base_url);
        search_book_from_book(book)
    }

    fn search_books_js(
        &self,
        source: &BookSource,
        body: &str,
        base_url: &str,
        rule: &SearchRule,
        list_rule: &str,
    ) -> Vec<SearchBook> {
        let (output, remaining_rule) = match eval_leading_js_rule(list_rule, body, base_url) {
            Ok(result) => result,
            Err(_) => return vec![],
        };
        // Legado executes each segment in order: the result of <js> feeds the
        // following JSONPath/CSS rule instead of becoming JavaScript source.
        if !remaining_rule.is_empty() {
            return self.parse_search_list(source, &output, base_url, rule, remaining_rule);
        }

        if let Some(items) = parse_js_output_items(&output) {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                if let Some(book) = build_search_book_from_json(source, &item, base_url, rule) {
                    out.push(book);
                }
            }
            return out;
        }

        let doc = html::parse_document(&output);
        let sel = match scraper::Selector::parse("body > *") {
            Ok(sel) => sel,
            Err(_) => return vec![],
        };
        let mut out = Vec::new();
        for el in doc.select(&sel) {
            let name = rule
                .name
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url))
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let author = rule
                .author
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url))
                .unwrap_or_default();
            let book_url = rule
                .book_url
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url))
                .unwrap_or_default();
            let cover_url = rule
                .cover_url
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url))
                .map(|u| resolve_url(base_url, &u));
            let intro = rule
                .intro
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url));
            let kind = rule
                .kind
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url));
            let last_chapter = rule
                .last_chapter
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url));
            let update_time = rule
                .update_time
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url));
            let word_count = rule
                .word_count
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url));
            out.push(SearchBook {
                name,
                author,
                book_url: resolve_url(base_url, &book_url),
                origin: source.book_source_url.clone(),
                cover_url,
                intro,
                kind,
                last_chapter,
                update_time,
                word_count,
                book_source_urls: None,
            });
        }
        out
    }

    fn search_books_regex(
        &self,
        source: &BookSource,
        body: &str,
        base_url: &str,
        rule: &SearchRule,
        list_rule: &str,
    ) -> Vec<SearchBook> {
        let pattern = self
            .strip_mode_prefix(list_rule)
            .trim_start_matches(':')
            .trim();
        let re = match regex::Regex::new(pattern) {
            Ok(re) => re,
            Err(_) => return vec![],
        };

        let mut out = Vec::new();
        for captures in re.captures_iter(body) {
            let name = capture_rule_value(rule.name.as_deref(), &captures).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let author = capture_rule_value(rule.author.as_deref(), &captures).unwrap_or_default();
            let book_url =
                capture_rule_value(rule.book_url.as_deref(), &captures).unwrap_or_default();
            let cover_url = capture_rule_value(rule.cover_url.as_deref(), &captures)
                .map(|u| resolve_url(base_url, &u));
            let intro = capture_rule_value(rule.intro.as_deref(), &captures);
            let kind = capture_rule_value(rule.kind.as_deref(), &captures);
            let last_chapter = capture_rule_value(rule.last_chapter.as_deref(), &captures);
            let update_time = capture_rule_value(rule.update_time.as_deref(), &captures);
            let word_count = capture_rule_value(rule.word_count.as_deref(), &captures);
            out.push(SearchBook {
                name,
                author,
                book_url: resolve_url(base_url, &book_url),
                origin: source.book_source_url.clone(),
                cover_url,
                intro,
                kind,
                last_chapter,
                update_time,
                word_count,
                book_source_urls: None,
            });
        }
        out
    }

    fn parse_chapter_list_js(
        &self,
        body: &str,
        base_url: &str,
        rule: &TocRule,
        list_rule: &str,
        ctx: &mut HashMap<String, String>,
    ) -> (Vec<BookChapter>, Vec<String>) {
        let (output, remaining_rule) = match eval_leading_js_rule(list_rule, body, base_url) {
            Ok(result) => result,
            Err(_) => return (vec![], vec![]),
        };
        if !remaining_rule.is_empty() {
            return self.parse_toc_list(&output, base_url, rule, remaining_rule, ctx);
        }
        // 分页目录: `chapterList` 是脚本时, `nextTocUrl` 仍要在**原始页面**上
        // 求值 —— 脚本输出是元素快照列表, 里面已经没有 `option` 分页控件。
        // 分页站点(目录按 60 章/页切分)只解析第一页会让书架少掉后续页面。
        let next_urls = toc_next_urls(body, base_url, rule);

        if let Some(items) = parse_js_output_items(&output) {
            let mut out = Vec::with_capacity(items.len());
            let mut seen_urls = std::collections::HashSet::new();
            for item in items {
                // 脚本返回的是**包着列表的对象**时(如 `{"data":{"chapters":[...]}}`),
                // legado 的 `getElements` 会在整个对象上继续按字段规则求值; 字段
                // 规则是 `$.name` 这种相对路径时, 必须先下钻到真正的列表节点,
                // 否则外层对象上取不到 `name`, 整本目录解析出 0 章。
                for candidate in expand_toc_item(item) {
                    if let Some(chapter) =
                        build_chapter_from_json(&candidate, base_url, rule, ctx, out.len())
                    {
                        if seen_urls.insert(chapter.url.clone()) {
                            out.push(chapter);
                        }
                    }
                }
            }
            return (out, next_urls);
        }

        let doc = html::parse_document(&output);
        let sel = match scraper::Selector::parse("body > *") {
            Ok(sel) => sel,
            Err(_) => return (vec![], vec![]),
        };
        let mut out = Vec::new();
        let mut seen_urls = std::collections::HashSet::new();
        for el in doc.select(&sel) {
            let title = rule
                .chapter_name
                .as_ref()
                .and_then(|r| eval_field_html_with_ctx(r, &el, base_url, ctx))
                .unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            let raw_url = rule
                .chapter_url
                .as_ref()
                .and_then(|r| eval_field_html_with_ctx(r, &el, base_url, ctx))
                .unwrap_or_default();
            let tag = rule
                .update_time
                .as_ref()
                .and_then(|r| eval_field_html_with_ctx(r, &el, base_url, ctx));
            let is_volume = rule
                .is_volume
                .as_ref()
                .and_then(|r| eval_field_html_with_ctx(r, &el, base_url, ctx))
                .map(is_truthy)
                .unwrap_or(false);
            let is_vip = rule
                .is_vip
                .as_ref()
                .and_then(|r| eval_field_html_with_ctx(r, &el, base_url, ctx))
                .map(is_truthy)
                .unwrap_or(false);
            let is_pay = rule
                .is_pay
                .as_ref()
                .and_then(|r| eval_field_html_with_ctx(r, &el, base_url, ctx))
                .map(is_truthy)
                .unwrap_or(false);
            let url = finalize_chapter_url(base_url, &raw_url, &title, is_volume, out.len());
            if !seen_urls.insert(url.clone()) {
                continue;
            }
            out.push(BookChapter {
                title,
                url,
                index: out.len() as i32,
                tag,
                is_vip,
                is_pay,
                is_volume,
                ..Default::default()
            });
        }
        (out, next_urls)
    }

    fn parse_chapter_list_regex(
        &self,
        body: &str,
        base_url: &str,
        rule: &TocRule,
        list_rule: &str,
    ) -> (Vec<BookChapter>, Vec<String>) {
        let pattern = self
            .strip_mode_prefix(list_rule)
            .trim_start_matches(':')
            .trim();
        let re = match regex::Regex::new(pattern) {
            Ok(re) => re,
            Err(_) => return (vec![], vec![]),
        };

        let mut out = Vec::new();
        let mut seen_urls = std::collections::HashSet::new();
        for captures in re.captures_iter(body) {
            let title =
                capture_rule_value(rule.chapter_name.as_deref(), &captures).unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            let raw_url =
                capture_rule_value(rule.chapter_url.as_deref(), &captures).unwrap_or_default();
            let tag = capture_rule_value(rule.update_time.as_deref(), &captures);
            let is_volume = capture_rule_value(rule.is_volume.as_deref(), &captures)
                .map(is_truthy)
                .unwrap_or(false);
            let is_vip = capture_rule_value(rule.is_vip.as_deref(), &captures)
                .map(is_truthy)
                .unwrap_or(false);
            let is_pay = capture_rule_value(rule.is_pay.as_deref(), &captures)
                .map(is_truthy)
                .unwrap_or(false);
            let url = finalize_chapter_url(base_url, &raw_url, &title, is_volume, out.len());
            if !seen_urls.insert(url.clone()) {
                continue;
            }
            out.push(BookChapter {
                title,
                url,
                index: out.len() as i32,
                tag,
                is_vip,
                is_pay,
                is_volume,
                ..Default::default()
            });
        }
        (out, vec![])
    }

    fn search_books_html(
        &self,
        source: &BookSource,
        body: &str,
        base_url: &str,
        rule: &SearchRule,
        list_sel: &str,
    ) -> Vec<SearchBook> {
        if list_sel.trim().is_empty() {
            return vec![];
        }
        let doc = html::parse_document(body);
        let items = html::select_list(&doc, self.strip_mode_prefix(list_sel));
        let mut out = Vec::with_capacity(items.len());

        for el in items {
            let name = rule
                .name
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url))
                .unwrap_or_default();
            let author = rule
                .author
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url))
                .unwrap_or_default();
            let book_url = rule
                .book_url
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url))
                .unwrap_or_default();
            let cover_url = rule
                .cover_url
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url));
            let intro = rule
                .intro
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url));
            let kind = rule
                .kind
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url));
            let last_chapter = rule
                .last_chapter
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url));
            let update_time = rule
                .update_time
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url));
            let word_count = rule
                .word_count
                .as_ref()
                .and_then(|r| eval_field_html(r, &el, base_url));
            let book_url_abs = resolve_url(base_url, &book_url);
            let cover_url_abs = cover_url.map(|u| resolve_url(base_url, &u));
            out.push(SearchBook {
                name,
                author,
                book_url: book_url_abs,
                origin: source.book_source_url.clone(),
                cover_url: cover_url_abs,
                intro,
                kind,
                last_chapter,
                update_time,
                word_count,
                book_source_urls: None,
            });
        }
        out
    }

    fn search_books_xpath(
        &self,
        source: &BookSource,
        body: &str,
        base_url: &str,
        rule: &SearchRule,
        list_rule: &str,
    ) -> Vec<SearchBook> {
        let package = match sxd_document::parser::parse(body) {
            Ok(p) => p,
            Err(_) => return vec![],
        };
        let document = package.as_document();
        let items = xpath_select_nodes(
            sxd_xpath::nodeset::Node::Root(document.root()),
            self.strip_mode_prefix(list_rule),
        );
        let mut out = Vec::with_capacity(items.len());

        for item in items {
            let name = eval_field_xpath(rule.name.as_deref().unwrap_or(""), item, base_url);
            let author = eval_field_xpath(rule.author.as_deref().unwrap_or(""), item, base_url);
            let book_url = eval_field_xpath(rule.book_url.as_deref().unwrap_or(""), item, base_url);
            let cover_url =
                eval_field_xpath(rule.cover_url.as_deref().unwrap_or(""), item, base_url);
            let intro = eval_field_xpath(rule.intro.as_deref().unwrap_or(""), item, base_url);
            let kind = eval_field_xpath(rule.kind.as_deref().unwrap_or(""), item, base_url);
            let last_chapter =
                eval_field_xpath(rule.last_chapter.as_deref().unwrap_or(""), item, base_url);
            let update_time =
                eval_field_xpath(rule.update_time.as_deref().unwrap_or(""), item, base_url);
            let word_count =
                eval_field_xpath(rule.word_count.as_deref().unwrap_or(""), item, base_url);
            out.push(SearchBook {
                name: name.unwrap_or_default(),
                author: author.unwrap_or_default(),
                book_url: resolve_url(base_url, &book_url.unwrap_or_default()),
                origin: source.book_source_url.clone(),
                cover_url: cover_url.map(|u| resolve_url(base_url, &u)),
                intro,
                kind,
                last_chapter,
                update_time,
                word_count,
                book_source_urls: None,
            });
        }

        out
    }

    fn search_books_json(
        &self,
        source: &BookSource,
        body: &str,
        base_url: &str,
        rule: &SearchRule,
        list_rule: &str,
    ) -> Vec<SearchBook> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => return vec![],
        };
        let items = jsonpath::jsonpath_query(&v, self.strip_mode_prefix(list_rule));
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let name = eval_field_json(rule.name.as_deref().unwrap_or(""), &item, base_url);
            let author = eval_field_json(rule.author.as_deref().unwrap_or(""), &item, base_url);
            let book_url = eval_field_json(rule.book_url.as_deref().unwrap_or(""), &item, base_url);
            let cover_url =
                eval_field_json(rule.cover_url.as_deref().unwrap_or(""), &item, base_url);
            let intro = eval_field_json(rule.intro.as_deref().unwrap_or(""), &item, base_url);
            let kind = eval_field_json(rule.kind.as_deref().unwrap_or(""), &item, base_url);
            let last_chapter =
                eval_field_json(rule.last_chapter.as_deref().unwrap_or(""), &item, base_url);
            let update_time =
                eval_field_json(rule.update_time.as_deref().unwrap_or(""), &item, base_url);
            let word_count =
                eval_field_json(rule.word_count.as_deref().unwrap_or(""), &item, base_url);
            let book_url = book_url.map(|u| resolve_url(base_url, &u));
            let cover_url = cover_url.map(|u| resolve_url(base_url, &u));
            out.push(SearchBook {
                name: name.unwrap_or_default(),
                author: author.unwrap_or_default(),
                book_url: book_url.unwrap_or_default(),
                origin: source.book_source_url.clone(),
                cover_url,
                intro,
                kind,
                last_chapter,
                update_time,
                word_count,
                book_source_urls: None,
            });
        }
        out
    }
}

fn parse_book_info_html(
    source: &BookSource,
    body: &str,
    base_url: &str,
    rule: &BookInfoRule,
    book_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Book {
    let doc = html::parse_document(body);

    // Execute init rule if present
    if let Some(init) = &rule.init {
        let _ = eval_field_html_doc_with_ctx(init, &doc, base_url, ctx);
    }

    // 字段规则之间互相独立：某条脚本抛错只应该让**那个字段**为空，不能连带毁掉
    // 后面的字段。
    //
    // legado 的 `BookInfo.analyzeBookInfo`（BookInfo.kt:84-153）给每个字段都包了
    // try/catch，失败只记日志、继续下一项。书源脚本常在开头做**登录态假设**
    //（例如该源 `name` 的第一行 `result.match(/Ark.CSRF_TOKEN = '([^']+)'/)[1]`），
    // 未登录时抛 `cannot read property of null` —— 若不隔离，书名会空、整本书
    // 被判成「详情解析失败」，用户看到的就是「打开详细页报错」。
    let mut field = |rule: Option<&String>| -> Option<String> {
        let rule = rule?;
        eval_field_html_doc_with_ctx(rule, &doc, base_url, ctx).filter(|v| !v.trim().is_empty())
    };

    let name = field(rule.name.as_ref()).unwrap_or_default();
    let author = field(rule.author.as_ref()).unwrap_or_default();
    let intro = field(rule.intro.as_ref());
    let kind = field(rule.kind.as_ref());
    let last_chapter = field(rule.last_chapter.as_ref());
    let update_time = field(rule.update_time.as_ref());
    let cover_url = field(rule.cover_url.as_ref()).map(|u| resolve_url(base_url, &u));
    let word_count = field(rule.word_count.as_ref());
    let toc_url = field(rule.toc_url.as_ref()).map(|u| resolve_url(base_url, &u));
    let can_re_name = rule
        .can_re_name
        .as_ref()
        .and_then(|r| eval_field_html_doc_with_ctx(r, &doc, base_url, ctx));
    let download_urls = rule
        .download_urls
        .as_ref()
        .and_then(|r| eval_field_html_doc_with_ctx(r, &doc, base_url, ctx));

    let final_toc_url = toc_url.or_else(|| Some(book_url.to_string()));

    Book {
        name,
        author,
        book_url: book_url.to_string(),
        origin: source.book_source_url.clone(),
        origin_name: Some(source.book_source_name.clone()),
        cover_url,
        toc_url: final_toc_url,
        intro,
        latest_chapter_title: last_chapter,
        word_count,
        info_html: None,
        toc_html: None,
        kind,
        update_time,
        can_re_name,
        download_urls,
        ..Default::default()
    }
}

fn parse_book_info_xpath(
    source: &BookSource,
    body: &str,
    base_url: &str,
    rule: &BookInfoRule,
    book_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Book {
    let package = match sxd_document::parser::parse(body) {
        Ok(p) => p,
        Err(_) => return parse_book_info_html(source, body, base_url, rule, book_url, ctx),
    };
    let document = package.as_document();
    let scope = select_xpath_scope(
        sxd_xpath::nodeset::Node::Root(document.root()),
        rule.init.as_deref(),
    );

    let name = eval_field_xpath_with_ctx(rule.name.as_deref().unwrap_or(""), scope, base_url, ctx)
        .unwrap_or_default();
    let author =
        eval_field_xpath_with_ctx(rule.author.as_deref().unwrap_or(""), scope, base_url, ctx)
            .unwrap_or_default();
    let intro =
        eval_field_xpath_with_ctx(rule.intro.as_deref().unwrap_or(""), scope, base_url, ctx);
    let kind = eval_field_xpath_with_ctx(rule.kind.as_deref().unwrap_or(""), scope, base_url, ctx);
    let last_chapter = eval_field_xpath_with_ctx(
        rule.last_chapter.as_deref().unwrap_or(""),
        scope,
        base_url,
        ctx,
    );
    let update_time = eval_field_xpath_with_ctx(
        rule.update_time.as_deref().unwrap_or(""),
        scope,
        base_url,
        ctx,
    );
    let cover_url = eval_field_xpath_with_ctx(
        rule.cover_url.as_deref().unwrap_or(""),
        scope,
        base_url,
        ctx,
    )
    .map(|u| resolve_url(base_url, &u));
    let word_count = eval_field_xpath_with_ctx(
        rule.word_count.as_deref().unwrap_or(""),
        scope,
        base_url,
        ctx,
    );
    let toc_url =
        eval_field_xpath_with_ctx(rule.toc_url.as_deref().unwrap_or(""), scope, base_url, ctx)
            .map(|u| resolve_url(base_url, &u));
    let can_re_name = eval_field_xpath_with_ctx(
        rule.can_re_name.as_deref().unwrap_or(""),
        scope,
        base_url,
        ctx,
    );
    let download_urls = eval_field_xpath_with_ctx(
        rule.download_urls.as_deref().unwrap_or(""),
        scope,
        base_url,
        ctx,
    );

    Book {
        name,
        author,
        book_url: book_url.to_string(),
        origin: source.book_source_url.clone(),
        origin_name: Some(source.book_source_name.clone()),
        cover_url,
        toc_url: toc_url.or_else(|| Some(book_url.to_string())),
        intro,
        latest_chapter_title: last_chapter,
        word_count,
        info_html: None,
        toc_html: None,
        kind,
        update_time,
        can_re_name,
        download_urls,
        ..Default::default()
    }
}

fn parse_book_info_json(
    source: &BookSource,
    v: &Value,
    base_url: &str,
    rule: &BookInfoRule,
    book_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Book {
    let scope = select_json_scope(v, rule.init.as_deref(), base_url, ctx);
    let name = eval_field_json_with_ctx(rule.name.as_deref().unwrap_or(""), &scope, base_url, ctx)
        .unwrap_or_default();
    let author =
        eval_field_json_with_ctx(rule.author.as_deref().unwrap_or(""), &scope, base_url, ctx)
            .unwrap_or_default();
    let intro =
        eval_field_json_with_ctx(rule.intro.as_deref().unwrap_or(""), &scope, base_url, ctx);
    let kind = eval_field_json_with_ctx(rule.kind.as_deref().unwrap_or(""), &scope, base_url, ctx);
    let last_chapter = eval_field_json_with_ctx(
        rule.last_chapter.as_deref().unwrap_or(""),
        &scope,
        base_url,
        ctx,
    );
    let update_time = eval_field_json_with_ctx(
        rule.update_time.as_deref().unwrap_or(""),
        &scope,
        base_url,
        ctx,
    );
    let cover_url = eval_field_json_with_ctx(
        rule.cover_url.as_deref().unwrap_or(""),
        &scope,
        base_url,
        ctx,
    )
    .map(|u| resolve_url(base_url, &u));
    let word_count = eval_field_json_with_ctx(
        rule.word_count.as_deref().unwrap_or(""),
        &scope,
        base_url,
        ctx,
    );
    let toc_url =
        eval_field_json_with_ctx(rule.toc_url.as_deref().unwrap_or(""), &scope, base_url, ctx)
            .map(|u| resolve_url(base_url, &u));
    let can_re_name = eval_field_json_with_ctx(
        rule.can_re_name.as_deref().unwrap_or(""),
        &scope,
        base_url,
        ctx,
    );
    let download_urls = eval_field_json_with_ctx(
        rule.download_urls.as_deref().unwrap_or(""),
        &scope,
        base_url,
        ctx,
    );
    Book {
        name,
        author,
        book_url: book_url.to_string(),
        origin: source.book_source_url.clone(),
        origin_name: Some(source.book_source_name.clone()),
        cover_url,
        toc_url: toc_url.or_else(|| Some(book_url.to_string())),
        intro,
        latest_chapter_title: last_chapter,
        word_count,
        info_html: None,
        toc_html: None,
        kind,
        update_time,
        can_re_name,
        download_urls,
        ..Default::default()
    }
}

fn parse_chapter_list_html(
    body: &str,
    base_url: &str,
    rule: &TocRule,
    list_sel: &str,
    ctx: &mut HashMap<String, String>,
) -> (Vec<BookChapter>, Vec<String>) {
    if list_sel.trim().is_empty() {
        return (vec![], vec![]);
    }
    let doc = html::parse_document(body);

    // Execute init rule if present
    if let Some(init) = &rule.init {
        let _ = eval_field_html_doc_with_ctx(init, &doc, base_url, ctx);
    }

    let items = html::select_list(&doc, strip_mode_prefix(list_sel));

    // Use a set to deduplicate chapters by URL
    let mut seen_urls = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(items.len());

    for el in items {
        let title = rule
            .chapter_name
            .as_ref()
            .and_then(|r| eval_field_html_with_ctx(r, &el, base_url, ctx))
            .unwrap_or_default();
        let url = rule
            .chapter_url
            .as_ref()
            .and_then(|r| eval_field_html_with_ctx(r, &el, base_url, ctx))
            .unwrap_or_default();
        let tag = rule
            .update_time
            .as_ref()
            .and_then(|r| eval_field_html_with_ctx(r, &el, base_url, ctx));
        let is_volume = rule
            .is_volume
            .as_ref()
            .and_then(|r| eval_field_html_with_ctx(r, &el, base_url, ctx))
            .map(is_truthy)
            .unwrap_or(false);
        let is_vip = rule
            .is_vip
            .as_ref()
            .and_then(|r| eval_field_html_with_ctx(r, &el, base_url, ctx))
            .map(is_truthy)
            .unwrap_or(false);
        let is_pay = rule
            .is_pay
            .as_ref()
            .and_then(|r| eval_field_html_with_ctx(r, &el, base_url, ctx))
            .map(is_truthy)
            .unwrap_or(false);
        let url_abs = finalize_chapter_url(base_url, &url, &title, is_volume, out.len());

        // Skip duplicate chapters (same URL)
        if seen_urls.contains(&url_abs) {
            continue;
        }

        seen_urls.insert(url_abs.clone());

        out.push(BookChapter {
            title,
            url: url_abs,
            index: out.len() as i32,
            tag,
            is_vip,
            is_pay,
            is_volume,
            ..Default::default()
        });
    }

    // Extract next_toc_url(s)
    //
    // 没有配 `nextTocUrl` 时直接给空 —— 不能把空串交给 `select_text_list`：
    // 空选择器在元素级语义里表示"当前元素自身"，会凭空产出一个 next_url，
    // 让本来单页的目录被当成有下一页。
    let next_urls = toc_next_urls(body, base_url, rule);

    (out, next_urls)
}

/// 求值目录分页规则 `nextTocUrl`（`option:not([selected])@value` 这类 CSS、
/// `@js:` 脚本、或 `||` 组合），返回绝对地址。
///
/// 必须在**原始页面**上求值：`chapterList` 是脚本时，脚本输出已经是元素快照，
/// 里面不再包含分页控件。
fn toc_next_urls(body: &str, base_url: &str, rule: &TocRule) -> Vec<String> {
    let Some(rule_str) = rule.next_toc_url.as_deref().map(str::trim) else {
        return Vec::new();
    };
    if rule_str.is_empty() {
        return Vec::new();
    }
    let engine = RuleEngine;
    let values = match engine.detect_mode(rule_str, body) {
        // 脚本分页规则按 legado 语义取全部结果
        ParseMode::Js | ParseMode::Regex | ParseMode::JsonPath => {
            RuleEngine::eval_rule_on(rule_str, body, base_url, true)
        }
        _ => {
            let doc = html::parse_document(body);
            html::select_text_list(&doc, engine.strip_mode_prefix(rule_str))
        }
    };
    values
        .into_iter()
        .filter(|u| !u.trim().is_empty())
        .map(|u| resolve_url(base_url, u.trim()))
        .collect()
}

fn parse_chapter_list_xpath(
    body: &str,
    base_url: &str,
    rule: &TocRule,
    list_rule: &str,
    ctx: &mut HashMap<String, String>,
) -> (Vec<BookChapter>, Vec<String>) {
    let package = match sxd_document::parser::parse(body) {
        Ok(p) => p,
        Err(_) => return parse_chapter_list_html(body, base_url, rule, list_rule, ctx),
    };
    let document = package.as_document();
    let scope = select_xpath_scope(
        sxd_xpath::nodeset::Node::Root(document.root()),
        rule.init.as_deref(),
    );
    let items = xpath_select_nodes(scope, list_rule);

    let mut seen_urls = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let title = eval_field_xpath_with_ctx(
            rule.chapter_name.as_deref().unwrap_or(""),
            item,
            base_url,
            ctx,
        )
        .unwrap_or_default();
        let url = eval_field_xpath_with_ctx(
            rule.chapter_url.as_deref().unwrap_or(""),
            item,
            base_url,
            ctx,
        )
        .unwrap_or_default();
        let tag = eval_field_xpath_with_ctx(
            rule.update_time.as_deref().unwrap_or(""),
            item,
            base_url,
            ctx,
        );
        let is_volume =
            eval_field_xpath_with_ctx(rule.is_volume.as_deref().unwrap_or(""), item, base_url, ctx)
                .map(is_truthy)
                .unwrap_or(false);
        let is_vip =
            eval_field_xpath_with_ctx(rule.is_vip.as_deref().unwrap_or(""), item, base_url, ctx)
                .map(is_truthy)
                .unwrap_or(false);
        let is_pay =
            eval_field_xpath_with_ctx(rule.is_pay.as_deref().unwrap_or(""), item, base_url, ctx)
                .map(is_truthy)
                .unwrap_or(false);
        let url_abs = finalize_chapter_url(base_url, &url, &title, is_volume, out.len());
        if seen_urls.contains(&url_abs) {
            continue;
        }
        seen_urls.insert(url_abs.clone());
        out.push(BookChapter {
            title,
            url: url_abs,
            index: out.len() as i32,
            tag,
            is_vip,
            is_pay,
            is_volume,
            ..Default::default()
        });
    }

    let next_urls = rule
        .next_toc_url
        .as_deref()
        .map(|xpath| xpath_eval_strings(scope, xpath))
        .unwrap_or_default()
        .into_iter()
        .filter(|u| !u.is_empty())
        .map(|u| resolve_url(base_url, &u))
        .collect();

    (out, next_urls)
}

fn parse_chapter_list_json(
    body: &str,
    base_url: &str,
    rule: &TocRule,
    list_rule: &str,
    ctx: &mut HashMap<String, String>,
) -> (Vec<BookChapter>, Vec<String>) {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return (vec![], vec![]),
    };
    let scope = select_json_scope(&v, rule.init.as_deref(), base_url, ctx);
    let items = jsonpath::jsonpath_query(&scope, list_rule);

    let mut seen_urls = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let title = eval_field_json_with_ctx(
            rule.chapter_name.as_deref().unwrap_or(""),
            &item,
            base_url,
            ctx,
        )
        .unwrap_or_default();
        let url = eval_field_json_with_ctx(
            rule.chapter_url.as_deref().unwrap_or(""),
            &item,
            base_url,
            ctx,
        )
        .unwrap_or_default();
        let tag = eval_field_json_with_ctx(
            rule.update_time.as_deref().unwrap_or(""),
            &item,
            base_url,
            ctx,
        );
        let is_volume = eval_field_json_with_ctx(
            rule.is_volume.as_deref().unwrap_or(""),
            &item,
            base_url,
            ctx,
        )
        .map(is_truthy)
        .unwrap_or(false);
        let is_vip =
            eval_field_json_with_ctx(rule.is_vip.as_deref().unwrap_or(""), &item, base_url, ctx)
                .map(is_truthy)
                .unwrap_or(false);
        let is_pay =
            eval_field_json_with_ctx(rule.is_pay.as_deref().unwrap_or(""), &item, base_url, ctx)
                .map(is_truthy)
                .unwrap_or(false);
        let url_abs = finalize_chapter_url(base_url, &url, &title, is_volume, out.len());

        if seen_urls.contains(&url_abs) {
            continue;
        }
        seen_urls.insert(url_abs.clone());

        out.push(BookChapter {
            title,
            url: url_abs,
            index: out.len() as i32,
            tag,
            is_vip,
            is_pay,
            is_volume,
            ..Default::default()
        });
    }

    let next_urls: Vec<String> = rule
        .next_toc_url
        .as_ref()
        .map(|r| jsonpath::jsonpath_query(&scope, r))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .filter(|u| !u.is_empty())
        .map(|u| resolve_url(base_url, &u))
        .collect();

    (out, next_urls)
}

fn select_json_scope(
    v: &Value,
    init_rule: Option<&str>,
    base_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Value {
    let Some(init_rule) = init_rule.map(str::trim).filter(|s| !s.is_empty()) else {
        return v.clone();
    };

    if let Some(res) = try_put_get_json(init_rule, v, base_url, ctx) {
        let _ = res;
        return v.clone();
    }

    let interpolated = interpolate_json_templates(init_rule, v, base_url, ctx);
    let (pure_rule, _) = split_legado_regex(&interpolated);
    let (pure, _) = extract_js(&pure_rule);
    if pure.is_empty() {
        return v.clone();
    }

    jsonpath::jsonpath_query(v, pure)
        .into_iter()
        .next()
        .unwrap_or_else(|| v.clone())
}

fn pick_json_field(v: &Value, rule: Option<&str>) -> Option<String> {
    let rule = rule?;
    if rule.trim_start().starts_with('$') {
        return jsonpath::jsonpath_first_string(v, rule);
    }
    // 管道串 item 是数组: 字段规则为纯数字时按索引取第 N 段
    if let Some(arr) = v.as_array() {
        if let Ok(idx) = rule.trim().parse::<usize>() {
            return arr.get(idx).and_then(jsonpath::value_to_string);
        }
    }
    if let Some(obj) = v.as_object() {
        if let Some(val) = obj.get(rule) {
            return jsonpath::value_to_string(val);
        }
        // 元素快照兼容: `<js>` 规则返回 JSoup 元素后, 字段规则常写裸属性名
        // (`href` / `src`) 或 `text`。快照把属性放在 `attrs` 下、文本放在
        // `text`, 直接按顶层键取会空 → bookUrl 退化成列表页地址(列表页规则的常见症状)。
        //
        // 守卫要求 `attrs` + `tag` + `outerHtml` **三个键同时存在**: 只看
        // `attrs` 会误伤普通 JSON 源 —— `attrs` 是常见字段名, 一旦响应里有它
        // 且规则名恰好是裸属性名, 就会取到错误的值。三者齐备是元素快照的
        // 充分特征(见 element_snapshot / snapshot_from_fragment)。
        let looks_like_snapshot = obj.contains_key("attrs")
            && obj.contains_key("tag")
            && obj.contains_key("outerHtml");
        if looks_like_snapshot {
            if let Some(attr) = obj.get("attrs").and_then(|a| a.get(rule)) {
                return jsonpath::value_to_string(attr);
            }
            if rule.eq_ignore_ascii_case("text") || rule.eq_ignore_ascii_case("ownText") {
                if let Some(text) = obj.get(rule) {
                    return jsonpath::value_to_string(text);
                }
            }
        }
    }
    None
}

fn resolve_url(base: &str, url: &str) -> String {
    let base = normalize_source_url(base);
    let url = normalize_source_url(strip_url_config(url));

    if url.is_empty() {
        return base.to_string();
    }
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("data:") {
        return url.to_string();
    }
    if url.starts_with("//") {
        return format!("https:{}", url);
    }

    let mut base_url = match url::Url::parse(&base) {
        Ok(u) => u,
        Err(_) => return url.to_string(),
    };
    base_url.set_fragment(None);

    match base_url.join(&url) {
        Ok(u) => u.to_string(),
        Err(_) => {
            let base = base.trim_end_matches('/');
            format!("{}/{}", base, url.trim_start_matches('/'))
        }
    }
}

fn interpolate_json_templates(
    rule: &str,
    v: &Value,
    base_url: &str,
    ctx: &HashMap<String, String>,
) -> String {
    let re = inline_js_lazy_re();
    re.replace_all(rule, |caps: &regex::Captures| {
        let expr = caps.get(1).map(|m| m.as_str().trim()).unwrap_or_default();
        if expr.is_empty() {
            return String::new();
        }

        if let Some(key) = expr
            .strip_prefix("@get:{")
            .and_then(|s| s.strip_suffix('}'))
        {
            return ctx.get(key.trim()).cloned().unwrap_or_default();
        }

        // `{{@规则}}` / `{{@@规则}}`：legado 的 `RuleAnalyzer.trim()`
        //（RuleAnalyzer.kt:15-22）把**一个或多个** `@` 一律修剪掉再当规则解析，
        // 两者语义相同（`@` 是"取属性/取字段"标记，可重复写）。
        //
        // 该源 ruleExplore.kind 就是这种写法：
        //   `{{@@class.tag@text}},{{@@class.score@text}},{{$.finalizedCn}}`
        // 同一条规则里既有 `@`/`@@`（对当前节点取字段）又有 `$.`（JSONPath），都要支持。
        // 落到 JS 引擎会得到 `unexpected token in expression: '@'`。
        let trimmed_rule = expr.trim_start_matches('@');
        if trimmed_rule.len() != expr.len() {
            // 修剪掉 `@` 后按规则解析。优先直接取 JSON 字段（`$.kind` / `kind` 都走这里），
            // 取不到再交给通用规则求值（`class.tag@text` 这类 CSS 规则的场景）。
            if let Some(val) = pick_json_field(v, Some(trimmed_rule)) {
                return val;
            }
            let json_text = serde_json::to_string(v).unwrap_or_default();
            let values = RuleEngine::eval_rule_on(trimmed_rule, &json_text, base_url, false);
            return values.into_iter().find(|s| !s.is_empty()).unwrap_or_default();
        }

        if expr.starts_with('$') {
            return pick_json_field(v, Some(expr)).unwrap_or_default();
        }

        if let Some(val) = ctx.get(expr) {
            return val.clone();
        }

        if let Some(val) = pick_json_field(v, Some(expr)) {
            return val;
        }

        match eval_js(
            expr,
            &serde_json::to_string(v).unwrap_or_default(),
            base_url,
        ) {
            Ok(res) => res,
            Err(_) => String::new(),
        }
    })
    .into_owned()
}

fn interpolate_common_templates(
    rule: &str,
    input: &str,
    base_url: &str,
    ctx: &HashMap<String, String>,
) -> String {
    let get_re = get_template_re();
    let with_get = get_re.replace_all(rule, |caps: &regex::Captures| {
        let key = caps.get(1).map(|m| m.as_str().trim()).unwrap_or_default();
        ctx.get(key).cloned().unwrap_or_default()
    });

    let js_re = inline_js_lazy_re();
    js_re
        .replace_all(&with_get, |caps: &regex::Captures| {
            let expr = caps.get(1).map(|m| m.as_str().trim()).unwrap_or_default();
            if expr.is_empty() {
                return String::new();
            }
            // `{{@规则}}` / `{{@@规则}}`：legado 的 `RuleAnalyzer.trim()`
            //（RuleAnalyzer.kt:15-22）把**一个或多个** `@` 一律修剪掉再当规则解析，
            // 两者语义相同（`@` 是"取属性/取字段"标记，可重复写）。
            //
            // 只认 `@@` 是不够的：该源 `ruleBookInfo.intro` 写的是
            // `{{@class.section-abstract@html||…}}`（单 `@`），落不到这里就会被当
            // JS 求值，报 `unexpected token in expression: '@'`。
            let trimmed_rule = expr.trim_start_matches('@');
            if trimmed_rule.len() != expr.len() {
                // 单值语义：`{{@@class.tag@text}}` 取第一个命中。
                //（`list=true` 会把整页同类元素拼成一串，模板里只需要一个值。）
                let values = RuleEngine::eval_rule_on(trimmed_rule, input, base_url, false);
                return values.into_iter().find(|s| !s.is_empty()).unwrap_or_default();
            }
            if let Some(val) = ctx.get(expr) {
                return val.clone();
            }
            // `{{$.xxx}}` 是 JSON 语境的当前节点引用; HTML 模板路径不能把它当 JS 求值
            // （`$ is not defined`）。尝试把 input 按 JSON 解析后走 JSONPath, 失败则留空。
            if let Some(json_path) = expr.strip_prefix("$.") {
                if let Ok(v) = serde_json::from_str::<Value>(input) {
                    if let Some(val) = jsonpath::jsonpath_first_string(&v, &format!("$.{json_path}")) {
                        return val;
                    }
                }
                return String::new();
            }
            if expr.starts_with('$') {
                return String::new();
            }
            eval_js(expr, input, base_url).unwrap_or_default()
        })
        .into_owned()
}

fn strip_url_config(url: &str) -> &str {
    if let Some(idx) = url.find("##$##") {
        &url[..idx]
    } else if let Some(idx) = url.find(",{'webView'") {
        &url[..idx]
    } else if let Some(idx) = url.find(",{\"webView\"") {
        &url[..idx]
    } else {
        url
    }
}

/// 把元素转成 JS 侧可用的只读快照(`java.getElement` / `getElements` 的返回值)。
fn element_snapshot(element: &scraper::ElementRef) -> Value {
    let mut attrs = serde_json::Map::new();
    for (name, value) in element.value().attrs() {
        attrs.insert(name.to_string(), Value::String(value.to_string()));
    }
    json!({
        "tag": element.value().name(),
        "text": html::extract_text(element, "text").unwrap_or_default(),
        "ownText": html::extract_text(element, "ownText").unwrap_or_default(),
        "innerHtml": element.inner_html(),
        "outerHtml": element.html(),
        "attrs": attrs,
    })
}

/// 非 CSS 模式取到的是字符串, 包成同样的快照; 纯文本则直接当文本处理。
fn snapshot_from_fragment(fragment: &str) -> Value {
    let fragment = fragment.trim();
    let doc = html::parse_document(fragment);
    // 用 scraper 原生选择器取 body 下的第一个元素: 这里要的是"这个片段本身",
    // 不该经过 legado 风格的选择器改写。
    let first_element = scraper::Selector::parse("body > *")
        .ok()
        .and_then(|selector| doc.select(&selector).next());
    if let Some(element) = first_element {
        return element_snapshot(&element);
    }
    json!({
        "tag": "",
        "text": fragment,
        "ownText": fragment,
        "innerHtml": fragment,
        "outerHtml": fragment,
        "attrs": serde_json::Map::<String, Value>::new(),
    })
}

/// 正则提取: 取出 `##` 之前的 pattern 与其后的替换指令
/// (legado 用 `rule##replaceRegex##replacement` 表示"提取后替换")。
fn split_regex_rule(rule: &str) -> (&str, Option<&str>) {
    match rule.find("##") {
        Some(index) => (rule[..index].trim(), Some(&rule[index..])),
        None => (rule.trim(), None),
    }
}

/// 取第一个匹配: 有捕获组时取第 1 组, 否则取整个匹配。
fn regex_extract_first(text: &str, pattern: &str) -> Option<String> {
    if pattern.is_empty() {
        return None;
    }
    let re = regex::Regex::new(pattern).ok()?;
    let caps = re.captures(text)?;
    Some(caps
        .get(1)
        .or_else(|| caps.get(0))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default())
}

/// 取全部匹配(与 `regex_extract_first` 同样的取值规则)。
fn regex_extract_all(text: &str, pattern: &str) -> Vec<String> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let Ok(re) = regex::Regex::new(pattern) else {
        return Vec::new();
    };
    re.captures_iter(text)
        .map(|caps| {
            caps.get(1)
                .or_else(|| caps.get(0))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default()
        })
        .collect()
}

fn extract_js(rule: &str) -> (&str, Option<&str>) {
    if let Some(idx) = rule.find("<js>") {
        if let Some(end_idx) = rule.rfind("</js>") {
            if end_idx > idx {
                let pure = rule[..idx].trim();
                let js = &rule[idx + 4..end_idx];
                return (pure, Some(js));
            }
        }
    }
    if let Some(idx) = rule.find("@js:") {
        let pure = rule[..idx].trim();
        let js = &rule[idx + 4..];
        return (pure, Some(js));
    }
    if let Some(script) = rule.trim().strip_prefix("js:") {
        return ("", Some(script));
    }
    (rule, None)
}

fn eval_field_html(rule: &str, el: &scraper::ElementRef, base_url: &str) -> Option<String> {
    eval_field_html_with_ctx(rule, el, base_url, &mut HashMap::new())
}

fn eval_field_html_with_ctx(
    rule: &str,
    el: &scraper::ElementRef,
    base_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Option<String> {
    // legado 对空规则直接给空值: `AnalyzeRule.getString` 开头 `if (TextUtils.isEmpty(ruleStr)) return ""`,
    // `AnalyzeByJSoup.getString` 也 `if (ruleStr.isEmpty()) return null`.
    // 空规则若继续往下走, `extract_js` 得到的 `pure` 为空, 会落到
    // "纯 JS 规则"分支返回**当前元素的 outerHtml** — 某书源
    // `ruleExplore.kind` / `updateTime` 就是空串, 结果是整段 `<li>` 被当成
    // 字段值, 详情页"标签"里于是显示 HTML 源码.
    if rule.trim().is_empty() {
        return None;
    }

    // legado 字段级链式: `&&`(拼接) / `||`(首个非空) / `%%`(按索引交织)
    //
    // 拆分层级必须与 legado 一致：**先拆 `##`**（rule / replaceRegex /
    // replacement），rule 部分才按组合符拆，最后把替换作用在组合后的整体上。
    // 见 `AnalyzeRule.SourceRule.init`（`splitRegex` 在组合符之外）与
    // `AnalyzeRule.getString`（每轮取值后 `replaceRegex(result, sourceRule)`）。
    //
    // 反过来的话，该源的 `class.title@html||$.title##<span …>|<.*?>` 会把
    // `##<span…>|<.*?>` 并进第二个候选，`||` 取走第一个非空的 `class.title@html`
    // 之后替换永远不执行 —— 书名里于是留着 `<h4 class="title">…` 标签。
    let has_combo = split_top_level(rule, &["&&", "||", "%%"]).delimiter.is_some();
    if has_combo {
        // `##` 之前的部分才是可组合的规则主体，之后是替换段（可能只有一段 =
        // 删除匹配内容）。
        let (rule_body, regex_rule) = split_legado_regex(rule);
        let combo = split_top_level(&rule_body, &["&&", "||", "%%"]);
        let Some(operator) = combo.delimiter.as_deref() else {
            // `##` 前没有组合符，交给单值分支（它会自己处理替换）。
            return eval_field_html_single_with_ctx(rule, el, base_url, ctx);
        };
        let mut results: Vec<String> = Vec::new();
        let mut or_results: Vec<String> = Vec::new();
        for part in combo.parts {
            let part_value = eval_field_html_single_with_ctx(&part, el, base_url, ctx);
            match operator {
                "&&" => {
                    if let Some(v) = part_value {
                        if !v.is_empty() {
                            results.push(v);
                        }
                    }
                }
                "||" => {
                    or_results.push(part_value.unwrap_or_default());
                }
                "%%" => {
                    results.push(part_value.unwrap_or_default());
                }
                _ => {}
            }
        }
        let combined = match operator {
            // && 拼接所有非空
            "&&" if !results.is_empty() => Some(results.join("")),
            // || 取第一个非空
            "||" => or_results
                .into_iter()
                .find(|v| !v.is_empty()),
            // %% 按索引交织(字段级这里的每一项是单值, 等价于依次拼接)
            "%%" if !results.is_empty() => Some(results.join("")),
            _ => None,
        };
        // 替换作用在组合后的整体上（legado 对每个 SourceRule 的最终结果做替换）。
        return combined.map(|text| match regex_rule {
            Some(reg) => apply_legado_regex(&text, reg),
            None => text,
        });
    }

    eval_field_html_single_with_ctx(rule, el, base_url, ctx)
}

fn eval_field_html_single_with_ctx(
    rule: &str,
    el: &scraper::ElementRef,
    base_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Option<String> {
    // Handle mode forcing prefixes
    let rule = rule.trim();
    // 同上: 空规则在 legado 里就是空值, 不能落进"纯 JS 规则"分支拿到 outerHtml.
    if rule.is_empty() {
        return None;
    }
    // 先分离规则中任意位置的 @put 段
    let (stripped, puts) = split_inline_put_rules(rule);
    for (key, val_rule) in puts {
        let val = eval_field_html_with_ctx(&val_rule, el, base_url, ctx).unwrap_or_default();
        ctx.insert(key.clone(), val.clone());
        put_rule_variable(&key, &val, base_url);
    }
    let rule = stripped.as_str();
    if rule.starts_with("@css:") {
        let pure = &rule[5..];
        return eval_field_html_with_ctx(pure, el, base_url, ctx);
    }
    if rule.starts_with("@xpath:") {
        // XPath from element - not directly supported, return None
        return None;
    }
    if rule.starts_with("@json:") {
        // JSON from element - not applicable
        return None;
    }

    // Handle @put/@get
    if let Some(res) = try_put_get_html(rule, el, base_url, ctx) {
        return Some(res);
    }

    // 模板插值(`{{...}}`)的上下文必须是**元素 HTML**，不能是纯文本。
    //
    // `{{@@class.tag@text}}` 是该源的写法：`@@` 后面是 CSS 选择器，要作用在
    // 当前元素上再取 `@text`。早先这里传 `textNodes`（纯文本），选择器在纯文本上
    // 什么都选不到，于是该模板恒为空 —— `ruleExplore.kind` 因此变成 `",,,,,"`。
    let input = el.html();
    let interpolated_rule = interpolate_common_templates(rule, &input, base_url, ctx);
    let had_templates = interpolated_rule != rule;
    let (pure_rule, regex_part) = split_legado_regex(&interpolated_rule);
    let (pure, js) = extract_js(&pure_rule);

    let mut text = if pure.is_empty() {
        // 纯 JS 规则(`@js:` / `<js>`)：legado 里 `result` 就是**当前元素**
        //（JSoup Element 会字符串化成 outerHtml）。脚本普遍依赖这一点，例如该源
        // 的 bookUrl 规则 `java.getString("@href||$.url")` 就是从当前元素取 href。
        //
        // 早先这里给空串，脚本面对空内容什么也取不到，`bookUrl` 于是落到
        // fallback 拼出 `.../column//`（ID 为空），详情页/目录/正文全部失败。
        el.html()
    } else {
        html::select_text_from_element(el, pure).unwrap_or_default()
    };
    if text.is_empty() && had_templates && !pure.is_empty() {
        text = pure.to_string();
    }

    if let Some(script) = js {
        if let Ok(res) = eval_js(script, &text, base_url) {
            text = res;
        }
    }

    if let Some(reg) = regex_part {
        text = apply_legado_regex(&text, reg);
    }

    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn eval_field_html_doc_with_ctx(
    rule: &str,
    doc: &scraper::Html,
    base_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Option<String> {
    // Handle mode forcing prefixes
    let rule = rule.trim();
    // 同上: 空规则 = 空值(legado AnalyzeRule.getString 首行即返回 "").
    if rule.is_empty() {
        return None;
    }
    if rule.starts_with("@css:") {
        let pure = &rule[5..];
        return eval_field_html_doc_with_ctx(pure, doc, base_url, ctx);
    }
    if rule.starts_with("@xpath:") {
        let pure = &rule[7..];
        return html::select_xpath(&doc.html(), pure).first().cloned();
    }

    if let Some(res) = try_put_get_html_doc(rule, doc, base_url, ctx) {
        return Some(res);
    }

    let interpolated_rule = interpolate_common_templates(rule, &doc.html(), base_url, ctx);
    let had_templates = interpolated_rule != rule;
    let (pure, js) = extract_js(&interpolated_rule);
    let mut text = if pure.is_empty() {
        "".to_string()
    } else {
        html::select_text(doc, pure).unwrap_or_default()
    };
    if text.is_empty() && had_templates && !pure.is_empty() {
        text = pure.to_string();
    }

    if let Some(script) = js {
        if let Ok(res) = eval_js(script, &text, base_url) {
            return Some(res);
        }
        return Some(text);
    }

    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn eval_field_json(rule: &str, v: &Value, base_url: &str) -> Option<String> {
    eval_field_json_with_ctx(rule, v, base_url, &mut HashMap::new())
}

fn eval_field_xpath(
    rule: &str,
    node: sxd_xpath::nodeset::Node<'_>,
    base_url: &str,
) -> Option<String> {
    eval_field_xpath_with_ctx(rule, node, base_url, &mut HashMap::new())
}

fn eval_field_xpath_with_ctx(
    rule: &str,
    node: sxd_xpath::nodeset::Node<'_>,
    base_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Option<String> {
    if rule.trim().is_empty() {
        return None;
    }
    if let Some(res) = try_put_get_xpath(rule, node, base_url, ctx) {
        return Some(res);
    }

    // legado 字段级链式: `&&`(拼接) / `||`(首个非空) / `%%`(按索引交织)
    let combo = split_top_level(rule, &["&&", "||", "%%"]);
    if let Some(operator) = combo.delimiter.as_deref() {
        let parts = combo.parts;
        match operator {
            "&&" => {
                let mut joined = String::new();
                let mut any = false;
                for part in parts {
                    if let Some(v) = eval_field_xpath_with_ctx(&part, node, base_url, ctx) {
                        if !v.is_empty() {
                            joined.push_str(&v);
                            any = true;
                        }
                    }
                }
                return if any { Some(joined) } else { None };
            }
            "||" => {
                for part in parts {
                    if let Some(v) = eval_field_xpath_with_ctx(&part, node, base_url, ctx) {
                        if !v.is_empty() {
                            return Some(v);
                        }
                    }
                }
                return None;
            }
            "%%" => {
                let mut zipped = String::new();
                let mut any = false;
                for part in parts {
                    if let Some(v) = eval_field_xpath_with_ctx(&part, node, base_url, ctx) {
                        if !v.is_empty() {
                            zipped.push_str(&v);
                            any = true;
                        }
                    }
                }
                return if any { Some(zipped) } else { None };
            }
            _ => {}
        }
    }

    eval_field_xpath_single_with_ctx(rule, node, base_url, ctx)
}

fn eval_field_xpath_single_with_ctx(
    rule: &str,
    node: sxd_xpath::nodeset::Node<'_>,
    base_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Option<String> {
    if rule.trim().is_empty() {
        return None;
    }

    // 先分离规则中任意位置的 @put 段
    let (stripped, puts) = split_inline_put_rules(rule);
    for (key, val_rule) in puts {
        let val = eval_field_xpath_with_ctx(&val_rule, node, base_url, ctx).unwrap_or_default();
        ctx.insert(key.clone(), val.clone());
        put_rule_variable(&key, &val, base_url);
    }
    let rule = stripped.as_str();
    if rule.trim().is_empty() {
        return None;
    }
    let interpolated_rule = interpolate_common_templates(rule, &node.string_value(), base_url, ctx);
    let had_templates = interpolated_rule != rule;
    let (pure_rule, regex_part) = split_legado_regex(&interpolated_rule);
    let (pure, js) = extract_js(&pure_rule);
    // 拆分 legado 提取器后缀(::text / ::html / ::outerHtml / ::ownText 等)。
    // xpath_eval_strings 只做纯 XPath 求值; 有提取器时改用 node_extract 精确取值,
    // 否则 ::html 会退化为 string_value()(纯文本), 与 Legado 语义不符。
    let (xpath_part, extractor) = html::split_xpath_axis(pure);
    let xpath_str = xpath_part.trim();
    let mut text = if xpath_str.is_empty() {
        node.string_value()
    } else if let Some(ext) = extractor {
        // 有提取器: 先用 XPath 选出节点, 再按提取器取值
        xpath_select_nodes(node, xpath_str)
            .into_iter()
            .find_map(|n| html::node_extract(&n, ext))
            .unwrap_or_default()
    } else {
        xpath_eval_strings(node, xpath_str)
            .into_iter()
            .next()
            .unwrap_or_default()
    };
    if text.is_empty() && had_templates && !pure.is_empty() {
        text = pure.to_string();
    }

    if let Some(script) = js {
        if let Ok(res) = eval_js(script, &text, base_url) {
            text = res;
        }
    }
    if let Some(reg) = regex_part {
        text = apply_legado_regex(&text, reg);
    }
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn select_xpath_scope<'a>(
    node: sxd_xpath::nodeset::Node<'a>,
    init_rule: Option<&str>,
) -> sxd_xpath::nodeset::Node<'a> {
    let Some(init_rule) = init_rule.map(str::trim).filter(|s| !s.is_empty()) else {
        return node;
    };
    xpath_select_nodes(node, init_rule)
        .into_iter()
        .next()
        .unwrap_or(node)
}

fn xpath_select_nodes<'a>(
    node: sxd_xpath::nodeset::Node<'a>,
    xpath: &str,
) -> Vec<sxd_xpath::nodeset::Node<'a>> {
    let (path, _extractor) = html::split_xpath_axis(xpath);
    let xpath = path.trim();
    if xpath.is_empty() {
        return vec![];
    }
    let context = XPathContext::new();
    match XPathFactory::new().build(xpath) {
        Ok(Some(expr)) => match expr.evaluate(&context, node) {
            Ok(XPathValue::Nodeset(ns)) => ns.document_order(),
            _ => vec![],
        },
        _ => vec![],
    }
}

fn xpath_eval_strings(node: sxd_xpath::nodeset::Node<'_>, xpath: &str) -> Vec<String> {
    let (path, _extractor) = html::split_xpath_axis(xpath);
    let xpath = path.trim();
    if xpath.is_empty() {
        return vec![];
    }
    let context = XPathContext::new();
    match XPathFactory::new().build(xpath) {
        Ok(Some(expr)) => match expr.evaluate(&context, node) {
            Ok(XPathValue::Nodeset(ns)) => ns
                .document_order()
                .into_iter()
                .map(|n| n.string_value())
                .collect(),
            Ok(XPathValue::String(s)) => vec![s],
            Ok(XPathValue::Number(n)) => vec![n.to_string()],
            Ok(XPathValue::Boolean(b)) => vec![b.to_string()],
            Err(_) => vec![],
        },
        _ => vec![],
    }
}

fn eval_json_item_js(script: &str, item: &Value, base_url: &str) -> anyhow::Result<String> {
    let input = item_raw_string(item);
    let pipe_row = item.as_array().is_some_and(|items| items.iter().all(Value::is_string));
    if !item.is_object() && (!item.is_array() || pipe_row) {
        return eval_js(script, &input, base_url);
    }
    let bindings = HashMap::from([("result".to_string(), item.clone())]);
    // Gson/JSON objects in Legado support member access and stringify to JSON.
    // Keep JSON.parse(result) rules working alongside result.book_id rules.
    let script = format!(
        "Object.defineProperty(globalThis.result, Symbol.toPrimitive, {{value: function () {{ return JSON.stringify(this); }}}});\n{script}"
    );
    eval_js_with_bindings(&script, &input, base_url, &bindings)
}

fn eval_field_json_with_ctx(
    rule: &str,
    v: &Value,
    base_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Option<String> {
    if let Some(res) = try_put_get_json(rule, v, base_url, ctx) {
        return Some(res);
    }
    let trimmed = rule.trim();
    if trimmed.starts_with("<js>") || trimmed.starts_with("@js:") || trimmed.starts_with("js:") {
        // || and && inside a script are JavaScript operators, not field fallbacks.
        return eval_field_json_single_with_ctx(trimmed, v, base_url, ctx);
    }

    // legado 字段级链式: `&&`(拼接) / `||`(首个非空) / `%%`(按索引交织)
    let combo = split_top_level(rule, &["&&", "||", "%%"]);
    if let Some(operator) = combo.delimiter.as_deref() {
        let parts = combo.parts;
        match operator {
            "&&" => {
                let mut joined = String::new();
                let mut any = false;
                for part in parts {
                    if let Some(v) = eval_field_json_with_ctx(&part, v, base_url, ctx) {
                        if !v.is_empty() {
                            joined.push_str(&v);
                            any = true;
                        }
                    }
                }
                return if any { Some(joined) } else { None };
            }
            "||" => {
                for part in parts {
                    if let Some(v) = eval_field_json_with_ctx(&part, v, base_url, ctx) {
                        if !v.is_empty() {
                            return Some(v);
                        }
                    }
                }
                return None;
            }
            "%%" => {
                let mut zipped = String::new();
                let mut any = false;
                for part in parts {
                    if let Some(v) = eval_field_json_with_ctx(&part, v, base_url, ctx) {
                        if !v.is_empty() {
                            zipped.push_str(&v);
                            any = true;
                        }
                    }
                }
                return if any { Some(zipped) } else { None };
            }
            _ => {}
        }
    }

    eval_field_json_single_with_ctx(rule, v, base_url, ctx)
}

fn eval_field_json_single_with_ctx(
    rule: &str,
    v: &Value,
    base_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Option<String> {
    // 先分离规则中任意位置的 @put 段（如 `title@put:{"bid":"$.id"}`）
    let (stripped, puts) = split_inline_put_rules(rule);
    for (key, val_rule) in puts {
        let val = eval_field_json_with_ctx(&val_rule, v, base_url, ctx).unwrap_or_default();
        ctx.insert(key.clone(), val.clone());
        put_rule_variable(&key, &val, base_url);
    }
    let rule = stripped.as_str();
    let interpolated_rule = interpolate_json_templates(rule, v, base_url, ctx);
    let had_templates = interpolated_rule != rule;
    let (pure_rule, regex_part) = split_legado_regex(&interpolated_rule);
    let (pure, js) = extract_js(&pure_rule);

    let mut text = if pure.is_empty() {
        "".to_string()
    } else if pure.contains("{{") && pure.contains("}}") {
        pure.to_string()
    } else if pure.contains('/')
        || pure.contains('?')
        || pure.contains('&')
        || pure.contains('=')
        || pure.contains(',')
    {
        pure.to_string()
    } else {
        pick_json_field(v, Some(pure)).unwrap_or_default()
    };
    if text.is_empty() && had_templates && !pure.is_empty() {
        text = pure.to_string();
    }

    if let Some(script) = js {
        let result = if pure.is_empty() {
            eval_json_item_js(script, v, base_url)
        } else {
            eval_js(script, &text, base_url)
        };
        if let Ok(res) = result {
            text = res;
        }
    }

    if let Some(reg) = regex_part {
        text = apply_legado_regex(&text, reg);
    }

    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn inline_put_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?i)@put:(\{[^}]+?\})").unwrap())
}

/// 分离规则中**任意位置**的 `@put:{...}` 段（legado putPattern 语义，
/// 如书源的 `title@put:{"bid":"$.id"}`），返回剩余取值规则与
/// `(key, 值规则)` 列表，由调用方在当前节点上求值后写入。
fn split_inline_put_rules(rule: &str) -> (String, Vec<(String, String)>) {
    if !rule.contains("@put:") {
        return (rule.to_string(), Vec::new());
    }
    let re = inline_put_re();
    let mut rest = String::with_capacity(rule.len());
    let mut puts = Vec::new();
    let mut last = 0usize;
    for captures in re.captures_iter(rule) {
        let Some(matched) = captures.get(0) else { continue };
        rest.push_str(&rule[last..matched.start()]);
        last = matched.end();
        let Some(inner) = captures
            .get(1)
            .and_then(|g| g.as_str().strip_prefix('{'))
            .and_then(|s| s.strip_suffix('}'))
        else {
            continue;
        };
        // 先按 JSON 对象解析（值里可以含逗号），失败再降级按逗号劈开。
        // 与 legado 的 GSON.fromJsonObject 行为一致。
        let parsed = serde_json::from_str::<HashMap<String, String>>(&format!("{{{inner}}}"));
        if let Ok(map) = parsed {
            puts.extend(map);
        } else {
            for part in inner.split(',') {
                if let Some(idx) = part.find(':') {
                    let key = part[..idx].trim().trim_matches('"').to_string();
                    let val_rule = part[idx + 1..].trim().trim_matches('"').to_string();
                    puts.push((key, val_rule));
                }
            }
        }
    }
    rest.push_str(&rule[last..]);
    (rest, puts)
}

/// legado 的 `@put:{...}` 除写入本次解析上下文外, 还落到
/// chapter/book/source 变量链（与 JS `java.get(key)` 同一存储）,
/// 否则跨阶段（详情 @put → 正文 java.get）取不到值。
fn put_rule_variable(key: &str, val: &str, base_url: &str) {
    let script = format!("java.put({}, {})", serde_json::json!(key), serde_json::json!(val));
    let _ = eval_js(&script, "", base_url);
}

fn try_put_get_html(
    rule: &str,
    el: &scraper::ElementRef,
    base_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Option<String> {
    if rule.starts_with("@put:") {
        let content = &rule[5..];
        if content.starts_with('{') && content.ends_with('}') {
            let inner = &content[1..content.len() - 1];
            for part in inner.split(',') {
                if let Some(idx) = part.find(':') {
                    let key = part[..idx].trim().trim_matches('"').to_string();
                    let val_rule = part[idx + 1..].trim().trim_matches('"');
                    let val =
                        eval_field_html_with_ctx(val_rule, el, base_url, ctx).unwrap_or_default();
                    ctx.insert(key.clone(), val.clone());
                    put_rule_variable(&key, &val, base_url);
                }
            }
        }
        return Some("".to_string());
    }
    if rule.starts_with("@get:") {
        let content = &rule[5..];
        if content.starts_with('{') && content.ends_with('}') {
            let key = &content[1..content.len() - 1].trim();
            return ctx.get(*key).cloned();
        }
    }
    None
}

fn try_put_get_html_doc(
    rule: &str,
    doc: &scraper::Html,
    base_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Option<String> {
    if rule.starts_with("@put:") {
        let content = &rule[5..];
        if content.starts_with('{') && content.ends_with('}') {
            let inner = &content[1..content.len() - 1];
            for part in inner.split(',') {
                if let Some(idx) = part.find(':') {
                    let key = part[..idx].trim().trim_matches('"').to_string();
                    let val_rule = part[idx + 1..].trim().trim_matches('"');
                    let val = eval_field_html_doc_with_ctx(val_rule, doc, base_url, ctx)
                        .unwrap_or_default();
                    ctx.insert(key.clone(), val.clone());
                    put_rule_variable(&key, &val, base_url);
                }
            }
        }
        return Some("".to_string());
    }
    if rule.starts_with("@get:") {
        let content = &rule[5..];
        if content.starts_with('{') && content.ends_with('}') {
            let key = &content[1..content.len() - 1].trim();
            return ctx.get(*key).cloned();
        }
    }
    None
}

fn try_put_get_json(
    rule: &str,
    v: &Value,
    base_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Option<String> {
    if rule.starts_with("@put:") {
        let content = &rule[5..];
        if content.starts_with('{') && content.ends_with('}') {
            let inner = &content[1..content.len() - 1];
            for part in inner.split(',') {
                if let Some(idx) = part.find(':') {
                    let key = part[..idx].trim().trim_matches('"').to_string();
                    let val_rule = part[idx + 1..].trim().trim_matches('"');
                    let val =
                        eval_field_json_with_ctx(val_rule, v, base_url, ctx).unwrap_or_default();
                    ctx.insert(key.clone(), val.clone());
                    put_rule_variable(&key, &val, base_url);
                }
            }
        }
        return Some("".to_string());
    }
    if rule.starts_with("@get:") {
        let content = &rule[5..];
        if content.starts_with('{') && content.ends_with('}') {
            let key = &content[1..content.len() - 1].trim();
            return ctx.get(*key).cloned();
        }
    }
    None
}

fn try_put_get_xpath(
    rule: &str,
    node: sxd_xpath::nodeset::Node<'_>,
    base_url: &str,
    ctx: &mut HashMap<String, String>,
) -> Option<String> {
    if rule.starts_with("@put:") {
        let content = &rule[5..];
        if content.starts_with('{') && content.ends_with('}') {
            let inner = &content[1..content.len() - 1];
            for part in inner.split(',') {
                if let Some(idx) = part.find(':') {
                    let key = part[..idx].trim().trim_matches('"').to_string();
                    let val_rule = part[idx + 1..].trim().trim_matches('"');
                    let val = eval_field_xpath_with_ctx(val_rule, node, base_url, ctx)
                        .unwrap_or_default();
                    ctx.insert(key.clone(), val.clone());
                    put_rule_variable(&key, &val, base_url);
                }
            }
        }
        return Some(String::new());
    }
    if rule.starts_with("@get:") {
        let content = &rule[5..];
        if content.starts_with('{') && content.ends_with('}') {
            let key = &content[1..content.len() - 1].trim();
            return ctx.get(*key).cloned();
        }
    }
    None
}

fn split_legado_regex(rule: &str) -> (String, Option<&str>) {
    if let Some(idx) = rule.find("##") {
        let (pure, reg) = rule.split_at(idx);
        return (pure.trim().to_string(), Some(reg));
    }
    (rule.to_string(), None)
}

fn apply_legado_regex(text: &str, regex_part: &str) -> String {
    if regex_part.trim().is_empty() {
        return text.to_string();
    }

    // `###` 后缀 = 只替换第一个匹配（legado `replaceFirst`）。
    let (regex_part, first_only) = if regex_part.ends_with("###") {
        (&regex_part[..regex_part.len() - 3], true)
    } else {
        (regex_part, false)
    };

    // legado `SourceRule`（AnalyzeRule.kt:762-773）把 `##` 之后**只当两段**看：
    //   `ruleStrS[1]` = replaceRegex（匹配模式）
    //   `ruleStrS[2]` = replacement（替换文本，缺省为空 = 删除匹配）
    //   `size > 3` 只置 `replaceFirst`，**不是**再多做一轮替换。
    //
    // 所以这里必须严格取前两段，不能循环处理任意多段：`约||\s|字|,` 这类正则是
    // 一整段内容，若按 `##` 循环并 `i += 2` 跳着走，第二段之后的匹配模式会被整个
    // 跳过/错位，替换结果不完整（症状：` 12.5万 字` 而非 `12.5万`）。
    let after = regex_part.strip_prefix("##").unwrap_or(regex_part);
    let mut segments = after.splitn(3, "##");
    let pattern = segments.next().unwrap_or_default();
    let replacement = segments.next().unwrap_or("");
    // 第 3 段存在即代表 `##a##b##c` 形态 → legado 只做首次替换。
    let first_only = first_only || segments.next().is_some();

    if pattern.is_empty() {
        return text.to_string();
    }

    if first_only {
        apply_regex_replace_first(text, pattern, replacement)
    } else {
        apply_regex_replace(text, pattern, replacement)
    }
}

/// Apply regex replacement to first match only
fn apply_regex_replace_first(text: &str, pattern: &str, replacement: &str) -> String {
    let re = match regex::Regex::new(pattern) {
        Ok(r) => r,
        Err(_) => return text.to_string(),
    };
    re.replace(text, replacement).to_string()
}

fn normalize_list_rule(rule: &str) -> (&str, bool) {
    let rule = rule.trim();
    if let Some(rest) = rule.strip_prefix('-') {
        return (rest.trim(), true);
    }
    if let Some(rest) = rule.strip_prefix('+') {
        return (rest.trim(), false);
    }
    (rule, false)
}

fn strip_mode_prefix(rule: &str) -> &str {
    let rule = rule.trim();
    if let Some(rest) = rule
        .strip_prefix("<js>")
        .and_then(|s| s.strip_suffix("</js>"))
    {
        return rest;
    }
    for prefix in [
        "@css:", "@CSS:", "@xpath:", "@XPath:", "@XPATH:", "@json:", "@Json:", "@JSON:", "@regex:",
        "@Regex:", "@js:", "js:",
    ] {
        if let Some(rest) = rule.strip_prefix(prefix) {
            return rest;
        }
    }
    rule
}

fn eval_leading_js_rule<'a>(rule: &'a str, body: &str, base_url: &str) -> anyhow::Result<(String, &'a str)> {
    let (script, remaining_rule) = split_leading_js_rule(rule);
    Ok((eval_js(script, body, base_url)?, remaining_rule))
}

fn split_leading_js_rule(rule: &str) -> (&str, &str) {
    let rule = rule.trim();
    if let Some(rest) = rule.strip_prefix("<js>") {
        if let Some((script, tail)) = rest.split_once("</js>") {
            return (script, tail.trim());
        }
    }
    (strip_mode_prefix(rule), "")
}

fn strip_js_rule(rule: &str) -> &str {
    let rule = rule.trim();
    if let Some(rest) = rule
        .strip_prefix("<js>")
        .and_then(|s| s.strip_suffix("</js>"))
    {
        return rest;
    }
    if let Some(rest) = rule.strip_prefix("@js:") {
        return rest;
    }
    if let Some(rest) = rule.strip_prefix("js:") {
        return rest;
    }
    rule
}

fn prepare_toc_body(body: &str, base_url: &str, rule: &TocRule) -> String {
    let Some(script) = rule
        .pre_update_js
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    else {
        return body.to_string();
    };
    match eval_js(strip_js_rule(script), body, base_url) {
        Ok(result) if !result.trim().is_empty() => result,
        _ => body.to_string(),
    }
}

fn apply_toc_format_js(chapters: &mut [BookChapter], format_js: Option<&str>, base_url: &str) {
    let Some(script) = format_js.filter(|s| !s.trim().is_empty()) else {
        return;
    };
    let script = strip_js_rule(script);
    for (index, chapter) in chapters.iter_mut().enumerate() {
        let mut bindings = HashMap::new();
        bindings.insert("index".to_string(), json!(index + 1));
        bindings.insert("title".to_string(), json!(chapter.title.clone()));
        bindings.insert(
            "chapter".to_string(),
            serde_json::to_value(&*chapter).unwrap_or_else(|_| json!({})),
        );
        if let Ok(result) = eval_js_with_bindings(script, &chapter.title, base_url, &bindings) {
            if !result.trim().is_empty() {
                chapter.title = result;
            }
        }
    }
}

/// 返回 item 的"原始串"形式, 供 `@js:` 字段规则作为 `result`/`input`:
/// - 管道串数组(→ 重拼回 `|` 分隔串, 支持 `result.split('|')`)
/// - JSON 对象/数组(→ JSON 字符串, 支持 `JSON.parse(result)`)
/// - 纯字符串(→ 原样, 不带引号)
fn item_raw_string(v: &Value) -> String {
    if let Some(arr) = v.as_array() {
        if arr.iter().all(|e| e.is_string()) {
            let joined = arr
                .iter()
                .filter_map(|e| e.as_str())
                .collect::<Vec<_>>()
                .join("|");
            if !joined.is_empty() {
                return joined;
            }
        }
    }
    match v {
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}

fn parse_js_output_items(output: &str) -> Option<Vec<Value>> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }
    // JSON 数组 / 对象 → 现有行为
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return match value {
            Value::Array(items) => Some(items.into_iter().map(|item| {
                if let Value::String(raw) = &item {
                    if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
                        if parsed.is_object() || parsed.is_array() { return parsed; }
                    }
                }
                item
            }).collect()),
            Value::Object(_) => Some(vec![value]),
            _ => None,
        };
    }
    // legado 常见: 每行一个独立 JSON 对象(如 `JSON.stringify(r)` 逐行 join)。
    // 整体不是合法 JSON, 但逐行解析成功 → 每行作为一条 item。
    let mut json_items: Vec<Value> = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if v.is_object() || v.is_array() {
                json_items.push(v);
            }
        }
    }
    if !json_items.is_empty() {
        return Some(json_items);
    }
    // legado 管道串兼容: 每行 `名称|作者|链接|封面|简介`(行内至少一个 `|`)
    // 解析成 Vec<Value>, 每项是 JSON 数组, 字段规则用索引取值(0/1/2...)
    let mut items = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts = line
            .split('|')
            .map(|s| Value::String(s.trim().to_string()))
            .collect::<Vec<_>>();
        if parts.len() >= 2 {
            items.push(Value::Array(parts));
        }
    }
    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

/// 把脚本输出的单个 item 展开成"真正的章节节点"列表。
///
/// 不少聚合源的 `chapterList` 脚本会 `return resStr` —— 交回的是**完整响应**，
/// 形如 `{"code":0,"data":{"chapters":[...]}}`，章节数组埋在里面。legado 的
/// `getElements` 拿到这种对象后，字段规则（`$.name` / `$.id`）是在每个元素上
/// 求值的，因此这里必须先把对象里唯一/最像章节列表的数组找出来；找不到就保持
/// 原样（对象自身就是一个 item，如 JSONPath 直接给出的单章）。
fn expand_toc_item(item: Value) -> Vec<Value> {
    fn push_array(value: Value, out: &mut Vec<Value>) -> bool {
        match value {
            Value::Array(items) => {
                if items.is_empty() {
                    return false;
                }
                out.extend(items);
                true
            }
            _ => false,
        }
    }

    // 已经是数组: 只有在元素**都是对象**时才当章节列表展开。
    // 管道串兼容路径产出的数组项形如 ["101","V","第一章","2026-08-01"], 那是
    // `id|名称|…` 的单章字段集合, 由 `@js:result.split('|')[n]` 按索引取值,
    // 直接展开会把每根管道段变成一章。
    if let Value::Array(items) = &item {
        if items.iter().all(Value::is_object) {
            return items.clone();
        }
        return vec![item];
    }
    let Value::Object(map) = &item else {
        return vec![item];
    };

    // 先找顶层里"元素都是对象、且数量最多"的数组, 再逐层下钻一层
    // (`{"data": {"chapters": [...]}}`)。只下钻两层: 再深的嵌套属于
    // 正常字段取值, 交给字段规则自己的 JSONPath 处理。
    let mut best: Vec<Value> = Vec::new();
    for value in map.values() {
        let mut found = Vec::new();
        if push_array(value.clone(), &mut found) {
            if found.iter().all(Value::is_object) && found.len() > best.len() {
                best = found;
            }
            continue;
        }
        if let Value::Object(inner) = value {
            for inner_value in inner.values() {
                let mut nested = Vec::new();
                if push_array(inner_value.clone(), &mut nested)
                    && nested.iter().all(Value::is_object)
                    && nested.len() > best.len()
                {
                    best = nested;
                }
            }
        }
    }
    if best.is_empty() {
        vec![item]
    } else {
        best
    }
}

fn search_book_from_book(book: Book) -> Option<SearchBook> {
    if book.name.trim().is_empty() {
        return None;
    }
    Some(SearchBook {
        name: book.name,
        author: book.author,
        book_url: book.book_url,
        origin: book.origin,
        cover_url: book.cover_url,
        intro: book.intro,
        kind: book.kind,
        last_chapter: book.latest_chapter_title,
        update_time: book.update_time,
        word_count: book.word_count,
        book_source_urls: None,
    })
}

fn build_search_book_from_json(
    source: &BookSource,
    item: &Value,
    base_url: &str,
    rule: &SearchRule,
) -> Option<SearchBook> {
    let mut ctx = HashMap::new();
    let name =
        eval_field_json_with_ctx(rule.name.as_deref().unwrap_or(""), item, base_url, &mut ctx)
            .unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    let author = eval_field_json_with_ctx(
        rule.author.as_deref().unwrap_or(""),
        item,
        base_url,
        &mut ctx,
    )
    .unwrap_or_default();
    let book_url = eval_field_json_with_ctx(
        rule.book_url.as_deref().unwrap_or(""),
        item,
        base_url,
        &mut ctx,
    )
    .unwrap_or_default();
    let cover_url = eval_field_json_with_ctx(
        rule.cover_url.as_deref().unwrap_or(""),
        item,
        base_url,
        &mut ctx,
    )
    .map(|u| resolve_url(base_url, &u));
    let intro = eval_field_json_with_ctx(
        rule.intro.as_deref().unwrap_or(""),
        item,
        base_url,
        &mut ctx,
    );
    let kind =
        eval_field_json_with_ctx(rule.kind.as_deref().unwrap_or(""), item, base_url, &mut ctx);
    let last_chapter = eval_field_json_with_ctx(
        rule.last_chapter.as_deref().unwrap_or(""),
        item,
        base_url,
        &mut ctx,
    );
    let update_time = eval_field_json_with_ctx(
        rule.update_time.as_deref().unwrap_or(""),
        item,
        base_url,
        &mut ctx,
    );
    let word_count = eval_field_json_with_ctx(
        rule.word_count.as_deref().unwrap_or(""),
        item,
        base_url,
        &mut ctx,
    );
    Some(SearchBook {
        name,
        author,
        book_url: resolve_url(base_url, &book_url),
        origin: source.book_source_url.clone(),
        cover_url,
        intro,
        kind,
        last_chapter,
        update_time,
        word_count,
        book_source_urls: None,
    })
}

fn build_chapter_from_json(
    item: &Value,
    base_url: &str,
    rule: &TocRule,
    ctx: &mut HashMap<String, String>,
    index: usize,
) -> Option<BookChapter> {
    let title = eval_field_json_with_ctx(
        rule.chapter_name.as_deref().unwrap_or(""),
        item,
        base_url,
        ctx,
    )
    .unwrap_or_default();
    if title.is_empty() {
        return None;
    }
    let raw_url = eval_field_json_with_ctx(
        rule.chapter_url.as_deref().unwrap_or(""),
        item,
        base_url,
        ctx,
    )
    .unwrap_or_default();
    let tag = eval_field_json_with_ctx(
        rule.update_time.as_deref().unwrap_or(""),
        item,
        base_url,
        ctx,
    );
    let is_volume =
        eval_field_json_with_ctx(rule.is_volume.as_deref().unwrap_or(""), item, base_url, ctx)
            .map(is_truthy)
            .unwrap_or(false);
    let is_vip =
        eval_field_json_with_ctx(rule.is_vip.as_deref().unwrap_or(""), item, base_url, ctx)
            .map(is_truthy)
            .unwrap_or(false);
    let is_pay =
        eval_field_json_with_ctx(rule.is_pay.as_deref().unwrap_or(""), item, base_url, ctx)
            .map(is_truthy)
            .unwrap_or(false);
    Some(BookChapter {
        title: title.clone(),
        url: finalize_chapter_url(base_url, &raw_url, &title, is_volume, index),
        index: index as i32,
        tag,
        is_vip,
        is_pay,
        is_volume,
    })
}

fn capture_rule_value(rule: Option<&str>, captures: &regex::Captures<'_>) -> Option<String> {
    let rule = rule?.trim();
    if rule.is_empty() {
        return None;
    }
    let placeholder = placeholder_re();
    let replaced = placeholder.replace_all(rule, |cap: &regex::Captures| {
        let index = cap
            .get(1)
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .unwrap_or(0);
        if index == 0 {
            return cap
                .get(0)
                .map(|m| m.as_str())
                .unwrap_or_default()
                .to_string();
        }
        captures
            .get(index)
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| {
                cap.get(0)
                    .map(|m| m.as_str())
                    .unwrap_or_default()
                    .to_string()
            })
    });
    let (pure, regex_part) = split_legado_regex(&replaced);
    let mut output = pure;
    if let Some(regex_part) = regex_part {
        output = apply_legado_regex(&output, regex_part);
    }
    if output.is_empty() {
        None
    } else {
        Some(output)
    }
}

fn finalize_chapter_url(
    base_url: &str,
    raw_url: &str,
    title: &str,
    is_volume: bool,
    index: usize,
) -> String {
    if !raw_url.trim().is_empty() {
        return resolve_url(base_url, raw_url);
    }
    if is_volume {
        return format!("{}{}", title, index);
    }
    base_url.to_string()
}

fn is_truthy(value: String) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    !matches!(
        value.to_ascii_lowercase().as_str(),
        "0" | "false" | "null" | "none" | "no" | "off"
    )
}

/// Extract chapter number from title
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::book_source::BookSource;
    use crate::model::rule::{BookInfoRule, ContentRule, SearchRule, TocRule};

    /// 空字段规则必须给空值。某书源 `ruleExplore.kind` / `updateTime` 是空串,
    /// 早先会落到"纯 JS 规则"分支返回**当前元素的 outerHtml**, 于是整段 `<li>`
    /// 被当成字段值, 详情页标签显示 HTML 源码。对齐 legado
    /// (`AnalyzeRule.getString` 首行 `TextUtils.isEmpty(ruleStr) -> ""`).
    #[test]
    fn test_empty_field_rule_yields_none_not_element_html() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "Test".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_explore: Some(SearchRule {
                book_list: Some("class.list@tag.li".to_string()),
                name: Some("class.name@tag.a@text".to_string()),
                kind: Some(String::new()),
                update_time: Some(String::new()),
                intro: Some("class.intro@text".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"<html><body><div class="list"><ul><li><div class="name"><a href="/b/1.html">书名</a></div><div class="intro">简介</div></li></ul></div></body></html>"#;
        let books = engine.explore_books(&source, body, "https://source.example/rank.html");
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].name, "书名");
        assert_eq!(books[0].intro.as_deref(), Some("简介"));
        assert_eq!(books[0].kind, None, "空规则不能返回元素 HTML");
        assert_eq!(books[0].update_time, None, "空规则不能返回元素 HTML");
    }

    #[test]
    fn test_detect_mode() {
        let engine = RuleEngine::new().unwrap();

        assert_eq!(engine.detect_mode("@css:.test", ""), ParseMode::Css);
        assert_eq!(engine.detect_mode("@xpath://div", ""), ParseMode::XPath);
        assert_eq!(engine.detect_mode("$.data.list", ""), ParseMode::JsonPath);
        assert_eq!(engine.detect_mode("/html/body/div", ""), ParseMode::XPath);
        assert_eq!(engine.detect_mode(".class", ""), ParseMode::Css);
        assert_eq!(engine.detect_mode("js:return 1", ""), ParseMode::Js);
        assert_eq!(engine.detect_mode("<js>return 1</js>", ""), ParseMode::Js);
    }

    #[test]
    fn test_apply_legado_regex() {
        let text = "Hello World 123 456";

        // Test basic replacement (all matches)
        let result = apply_legado_regex(text, "##\\d+##NUM");
        assert_eq!(result, "Hello World NUM NUM");

        // Test first match only (###)
        let result = apply_legado_regex(text, "##\\d+##NUM###");
        assert_eq!(result, "Hello World NUM 456");
    }

    #[test]
    fn test_search_detail_fallback_uses_book_info_rules() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "Test".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_search: Some(SearchRule {
                book_list: Some(String::new()),
                ..Default::default()
            }),
            rule_book_info: Some(BookInfoRule {
                name: Some(".name@text".to_string()),
                author: Some(".author@text".to_string()),
                intro: Some(".intro@text".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"
            <div class="name">Fallback Book</div>
            <div class="author">Fallback Author</div>
            <div class="intro">Fallback Intro</div>
        "#;

        let results = engine.search_books(&source, body, "https://books.example/detail/1");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Fallback Book");
        assert_eq!(results[0].author, "Fallback Author");
        assert_eq!(results[0].intro.as_deref(), Some("Fallback Intro"));
    }

    #[test]
    fn test_search_books_regex_list() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "Regex".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_search: Some(SearchRule {
                book_list: Some(r#":<a href="([^"]+)">([^<]+)</a>"#.to_string()),
                name: Some("$2".to_string()),
                book_url: Some("$1".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"<a href="/book/1">One</a><a href="/book/2">Two</a>"#;

        let results = engine.search_books(&source, body, "https://books.example");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "One");
        assert_eq!(results[0].book_url, "https://books.example/book/1");
        assert_eq!(results[1].name, "Two");
    }

    #[test]
    fn test_search_books_js_json_list() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "JS".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_search: Some(SearchRule {
                book_list: Some(
                    "js:JSON.stringify([{name:'Alpha',author:'Tester',bookUrl:'/alpha'}])"
                        .to_string(),
                ),
                name: Some("name".to_string()),
                author: Some("author".to_string()),
                book_url: Some("bookUrl".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let results = engine.search_books(&source, "<html></html>", "https://books.example");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Alpha");
        assert_eq!(results[0].author, "Tester");
        assert_eq!(results[0].book_url, "https://books.example/alpha");
    }

    #[test]
    fn test_search_books_js_pipe_kv() {
        // legado 书源常见写法: bookList 返回 `名称|作者|链接` 管道串(逐行),
        // 字段规则用索引取(0/1/2)。此前引擎只认 JSON 数组, 此测试验证兼容。
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "JS Pipe".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_search: Some(SearchRule {
                book_list: Some(
                    "js:['Alpha|Tester|/alpha','Beta|Writer|/beta','Gamma|Author|/gamma'].join('\\n')"
                        .to_string(),
                ),
                name: Some("0".to_string()),
                author: Some("1".to_string()),
                book_url: Some("2".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let results = engine.search_books(&source, "<html></html>", "https://books.example");
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].name, "Alpha");
        assert_eq!(results[0].author, "Tester");
        assert_eq!(results[0].book_url, "https://books.example/alpha");
        assert_eq!(results[2].name, "Gamma");
    }

    #[test]
    fn test_search_books_js_java_get_content() {
        // legado 书源用 java.getContent() 拿页面内容, 本引擎应返回 input
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "JS GetContent".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_search: Some(SearchRule {
                book_list: Some(
                    "js:JSON.stringify([{name: 'Alpha', author: 'Tester', bookUrl: '/a', intro: java.getContent().slice(0, 10)}])"
                        .to_string(),
                ),
                name: Some("name".to_string()),
                author: Some("author".to_string()),
                book_url: Some("bookUrl".to_string()),
                intro: Some("intro".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let results = engine.search_books(&source, "0123456789", "https://books.example");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Alpha");
        // java.getContent() 返回 input(即页面 body "0123456789"), 前 10 字符
        assert_eq!(results[0].intro.as_deref(), Some("0123456789"));
    }

    #[test]
    fn test_search_books_js_newline_json_with_js_field_rules() {
        // 某些书源的 _arr 写法: bookList 逐行输出独立 JSON 对象,
        // 字段规则用 `@js:result=JSON.parse(result).xxx`(legado 风格)。
        // 此前引擎既不解析"逐行 JSON", 也不把 item 传给 @js 字段规则 → 搜索全空。
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "JS LineJSON".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_search: Some(SearchRule {
                book_list: Some(
                    "js:['{\"bName\":\"Alpha\",\"bAuth\":\"Tester\",\"bid\":\"1001\",\"imgUrl\":\"/c1.jpg\"}','{\"bName\":\"Beta\",\"bAuth\":\"Writer\",\"bid\":\"1002\",\"imgUrl\":\"/c2.jpg\"}'].join('\\n')"
                        .to_string(),
                ),
                name: Some("@js:result=JSON.parse(result).bName".to_string()),
                author: Some("@js:result=JSON.parse(result).bAuth".to_string()),
                book_url: Some("@js:result=JSON.parse(result).bid;result='https://m.example.com/book/'+result+'/'".to_string()),
                cover_url: Some("@js:var o=JSON.parse(result);result=o.imgUrl||o.cover||''".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let results = engine.search_books(&source, "<html></html>", "https://source.example");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "Alpha");
        assert_eq!(results[0].author, "Tester");
        assert_eq!(results[0].book_url, "https://m.example.com/book/1001/");
        assert_eq!(results[0].cover_url.as_deref(), Some("https://source.example/c1.jpg"));
        assert_eq!(results[1].name, "Beta");
        assert_eq!(results[1].book_url, "https://m.example.com/book/1002/");
    }

    #[test]
    fn test_chapter_list_js_and_format_js() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "JS TOC".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_toc: Some(TocRule {
                chapter_list: Some("js:JSON.stringify([{chapterName:'One',chapterUrl:'/1',isVip:'1'},{chapterName:'Two',chapterUrl:'/2',isPay:'true'}])".to_string()),
                chapter_name: Some("chapterName".to_string()),
                chapter_url: Some("chapterUrl".to_string()),
                is_vip: Some("isVip".to_string()),
                is_pay: Some("isPay".to_string()),
                format_js: Some("`${index}.${title}`".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let (chapters, next_urls) =
            engine.chapter_list(&source, "<html></html>", "https://books.example", None);
        assert!(next_urls.is_empty());
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title, "1.One");
        assert_eq!(chapters[0].url, "https://books.example/1");
        assert!(chapters[0].is_vip);
        assert_eq!(chapters[1].title, "2.Two");
        assert!(chapters[1].is_pay);
    }

    #[test]
    fn test_chapter_list_js_pipe_with_js_field_rules() {
        // 某些书源的 _arr 目录写法: chapterList 逐行输出 `id|V|name|tag` 管道串,
        // 字段规则用 `@js:result=result.split('|')[n]`。验证管道串经 @js 字段规则可解析。
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "JS TOC Pipe".to_string(),
            book_source_url: "https://m.example.com/book/1001/".to_string(),
            rule_toc: Some(TocRule {
                chapter_list: Some(
                    "js:['101|V|第一章 测试|2026-08-01','102|F|第二章 继续|2026-08-02'].join('\\n')"
                        .to_string(),
                ),
                chapter_name: Some("@js:result=result.split('|')[2]".to_string()),
                // legado: source.getKey() 返回**书源地址**（不是书籍地址）。
                chapter_url: Some("@js:var bid=source.getKey().match(/\\d+/)[0];result='https://m.example.com/chapter/'+bid+'/'+result.split('|')[0]+'/'".to_string()),
                is_vip: Some("@js:result=result.split('|')[1].replace(/V/,'true').replace(/F/,'false')".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        // 书源地址里的 1001 就是 source.getKey() 取到的 id（legado 语义：书源主键）
        let (chapters, _) = engine.chapter_list(
            &source,
            "<html></html>",
            "https://m.example.com/book/1001/",
            Some("https://m.example.com/book/1001/"),
        );
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title, "第一章 测试");
        assert!(chapters[0].is_vip);
        assert_eq!(chapters[0].url, "https://m.example.com/chapter/1001/101/");
        assert_eq!(chapters[1].title, "第二章 继续");
        assert!(!chapters[1].is_vip);
    }

    #[test]
    fn test_java_crypto_and_encoding_api() {
        // legado java.* 加密/编码 API 兼容性: digest/HMac/hex/htmlFormat/toNumChapter/timeFormatUTC
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "JS Crypto".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_search: Some(SearchRule {
                book_list: Some(
                    "js:JSON.stringify([{name:java.md5Encode16('abc'),author:java.digestHex('abc','MD5'),bookUrl:'/a',intro:java.htmlFormat('a&amp;b'),kind:java.toNumChapter('第12章'),lastChapter:java.hexEncodeToString('hi'),updateTime:java.HMacHex('abc','SHA-256','key')}])"
                        .to_string(),
                ),
                name: Some("name".to_string()),
                author: Some("author".to_string()),
                book_url: Some("bookUrl".to_string()),
                intro: Some("intro".to_string()),
                kind: Some("kind".to_string()),
                last_chapter: Some("lastChapter".to_string()),
                update_time: Some("updateTime".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let results = engine.search_books(&source, "<html></html>", "https://books.example");
        assert_eq!(results.len(), 1);
        // md5('abc') = 900150983cd24fb0d6963f7d28e17f72, 取 [8..24] 中间16位
        assert_eq!(results[0].name, "3cd24fb0d6963f7d");
        // md5 hex 全量
        assert_eq!(results[0].author, "900150983cd24fb0d6963f7d28e17f72");
        // htmlFormat 解码实体
        assert_eq!(results[0].intro.as_deref(), Some("a&b"));
        // toNumChapter("第12章") → "12"
        assert_eq!(results[0].kind.as_deref(), Some("12"));
        // hexEncodeToString("hi") → "6869"
        assert_eq!(results[0].last_chapter.as_deref(), Some("6869"));
        // HMAC-SHA256("abc", key="key") 非空且为 64 位 hex
        let hmac = results[0].update_time.as_deref().unwrap_or("");
        assert_eq!(hmac.len(), 64);
        assert!(hmac.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_field_level_chain_and_or() {
        // legado 字段级链式: `&&` 拼接、`||` 取首个非空
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "JS Chain".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_search: Some(SearchRule {
                book_list: Some(
                    "js:JSON.stringify([{name:'A',author:'X',bookUrl:'/a',b:'B1',c:'C1',x:'',y:'Y1'}])"
                        .to_string(),
                ),
                name: Some("name&&author".to_string()),
                // b 字段可能有值, 用 `c||b` 验证取第一个非空(c 非空)
                intro: Some("c||b".to_string()),
                // `x||y` 中 x 为空 xy下取 y
                kind: Some("x||y".to_string()),
                book_url: Some("bookUrl".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let results = engine.search_books(&source, "<html></html>", "https://books.example");
        assert_eq!(results.len(), 1);
        // name&&author → "AX"
        assert_eq!(results[0].name, "AX");
        // c||b → c 非空取 "C1"
        assert_eq!(results[0].intro.as_deref(), Some("C1"));
        // x||y → x 为空取 "Y1"
        assert_eq!(results[0].kind.as_deref(), Some("Y1"));
    }

    #[test]
    fn test_chapter_list_keeps_real_chapterlist_container() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "HTML TOC".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_toc: Some(TocRule {
                chapter_list: Some("#chapterlist a".to_string()),
                chapter_name: Some("@text".to_string()),
                chapter_url: Some("@href".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"
            <div id="chapterlist">
                <a href="/1">第一章</a>
                <a href="/2">第二章</a>
            </div>
        "#;

        let (chapters, next_urls) = engine.chapter_list(&source, body, "https://books.example", None);
        assert!(next_urls.is_empty());
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].url, "https://books.example/1");
        assert_eq!(chapters[1].url, "https://books.example/2");
    }

    #[test]
    fn test_search_books_js_uses_js_lib() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "JS Lib".to_string(),
            book_source_url: "https://source.example".to_string(),
            js_lib: Some("function buildName(v){ return v + '-lib'; }".to_string()),
            rule_search: Some(SearchRule {
                book_list: Some("js:JSON.stringify([{name:buildName('Alpha'),author:'Tester',bookUrl:'/alpha'}])".to_string()),
                name: Some("name".to_string()),
                author: Some("author".to_string()),
                book_url: Some("bookUrl".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let results = engine.search_books(&source, "<html></html>", "https://books.example");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Alpha-lib");
    }

    #[test]
    fn test_book_info_html_interpolates_get_template() {
        let source = BookSource {
            book_source_name: "Info".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_book_info: Some(BookInfoRule {
                init: Some("@put:{alias:.name@text}".to_string()),
                name: Some("Book-@get:{alias}".to_string()),
                author: Some(".author@text".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"<div class="name">Alias</div><div class="author">Tester</div>"#;
        let mut ctx = HashMap::new();
        let book = parse_book_info_html(
            &source,
            body,
            "https://books.example/detail/1",
            &source.rule_book_info.clone().unwrap(),
            "https://books.example/detail/1",
            &mut ctx,
        );
        assert_eq!(book.name, "Book-Alias");
        assert_eq!(book.author, "Tester");
    }

    /// 段评类书源: `class.con@html@js:addComment(result)`。
    /// 关键点 1: 选择器部分保留 HTML(否则 JS 拿不到 `<p>` 结构)。
    /// 关键点 2: `addComment` 来自 jsLib, 规则 JS 必须能调用到它(验证成功传递)。
    #[test]
    fn test_content_rule_html_then_js_receives_html_in_js_lib() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "段评".to_string(),
            book_source_url: "https://source.example".to_string(),
            // 模拟书源 jsLib: 插件在 jsLib 里定义 addComment, 正文规则再调用
            js_lib: Some(
                "function addComment(html){ return html.replace(/<\\/p>/g, ',{\"click\":\"showCommentPanel(1)\"}</p>'); }"
                    .to_string(),
            ),
            rule_content: Some(ContentRule {
                content: Some("class.con@html@js:addComment(result)".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"<div class="con"><p>第一段</p><p>第二段</p></div>"#;

        let content = engine.content(
            &source,
            body,
            "https://source.example/1/2",
            Some("https://source.example/book/1"),
        );
        // 证明: (1) @html 把 HTML 传进了 JS; (2) jsLib 里的 addComment 被成功调用
        assert_eq!(
            content,
            r#"<div class="con"><p>第一段,{"click":"showCommentPanel(1)"}</p><p>第二段,{"click":"showCommentPanel(1)"}</p></div>"#
        );
    }

    /// `@html` 提取后交给 JS, 未定义函数时回退为提取结果(legado 语义是 outerHtml)。
    #[test]
    fn test_content_rule_html_then_js_keeps_html() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "段评".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_content: Some(ContentRule {
                content: Some("class.con@html@js:addComment(result)".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"<div class="con"><p>第一段</p><p>第二段</p></div>"#;

        let content = engine.content(&source, body, "https://source.example/1/2", Some("https://source.example/book/1"));
        // addComment 未定义 → JS 抛错 → 回退保留选择器提取结果
        // legado 的 `@html` 语义是 outerHtml(含元素自身标签)
        assert_eq!(
            content,
            r#"<div class="con"><p>第一段</p><p>第二段</p></div>"#
        );
    }

    /// 正文规则 `@regex:` 应能提取 —— 此前没有 Regex 分支, 会落进默认的 CSS 分支
    /// (`@regex:...` 不是合法 CSS 选择器)而恒为空。
    #[test]
    fn test_content_rule_regex_extracts() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "正则".to_string(),
            book_source_url: "https://regex.example".to_string(),
            rule_content: Some(ContentRule {
                content: Some("@regex:正文：([^<]+)".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"<div>正文：这是内容</div>"#;

        let content = engine.content(&source, body, "https://regex.example/1", None);
        assert_eq!(content, "这是内容");
    }

    /// `@js:` 脚本为空时, 应直接返回选择器提取结果(不执行 JS)。
    #[test]
    fn test_content_rule_empty_js_keeps_selector_result() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "空JS".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_content: Some(ContentRule {
                content: Some("class.con@html@js:".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"<div class="con"><p>正文</p></div>"#;

        let content = engine.content(&source, body, "https://source.example/1/2", None);
        assert_eq!(content, r#"<div class="con"><p>正文</p></div>"#);
    }

    /// **纯 JS** 正文规则(没有选择器部分)在 JS 失败时, 整条正文变空。
    ///
    /// 这是缓存路径「跳过书源脚本后正文为空 → 回退完整解析」那条判断的前提:
    /// 书源把解析全写在 JS 里(如 `@js:clean(result)`)时, 不加载 jsLib 会让规则
    /// 报错并得到空内容 —— 缓存必须能识别这种情况, 而不是把每章都记成失败。
    #[test]
    fn test_content_rule_pure_js_failure_yields_empty() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "纯JS".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_content: Some(ContentRule {
                // 模拟跳过 jsLib 后插件函数不存在的情形
                content: Some("@js:pluginFnThatNeedsJsLib(result)".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"<div class="con"><p>正文</p></div>"#;

        let content = engine.content(&source, body, "https://source.example/1/2", None);
        assert!(
            content.trim().is_empty(),
            "纯 JS 规则失败时应得到空内容(而不是整页 HTML), 实际: {content:?}"
        );
    }

    /// `@js:` 为空/JS 抛错时, 保留选择器提取结果, 不应整条正文变空。
    #[test]
    fn test_content_rule_keeps_extract_when_js_missing() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "退化".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_content: Some(ContentRule {
                content: Some("class.con@html@js:throw new Error('boom')".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"<div class="con"><p>正文</p></div>"#;

        let content = engine.content(&source, body, "https://source.example/1/2", None);
        assert_eq!(content, r#"<div class="con"><p>正文</p></div>"#);
    }

    /// 链式规则: `选择器@html` 再 `@js:` 里能改 result 并返回(模拟插件插图标)。
    #[test]
    fn test_content_rule_html_then_js_can_transform() {
        let engine = RuleEngine::new().unwrap();
        let source = BookSource {
            book_source_name: "插图标".to_string(),
            book_source_url: "https://source.example".to_string(),
            rule_content: Some(ContentRule {
                content: Some(
                    "class.con@html@js:result.replace('</p>', ',{\"click\":\"showCommentPanel(1)\"}</p>')"
                        .to_string(),
                ),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"<div class="con"><p>第一段</p></div>"#;

        let content = engine.content(&source, body, "https://source.example/1/2", None);
        assert_eq!(
            content,
            r#"<div class="con"><p>第一段,{"click":"showCommentPanel(1)"}</p></div>"#
        );
    }
}
