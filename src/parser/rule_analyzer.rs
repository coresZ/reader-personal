#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitResult {
    pub parts: Vec<String>,
    pub delimiter: Option<String>,
}

pub fn split_top_level(rule: &str, delimiters: &[&str]) -> SplitResult {
    let Some((_, delimiter)) = find_next_delimiter(rule, delimiters, 0) else {
        return SplitResult {
            parts: vec![rule.trim().to_string()],
            delimiter: None,
        };
    };

    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut search_from = 0usize;
    while let Some((idx, _)) = find_next_delimiter(rule, &[delimiter], search_from) {
        parts.push(rule[start..idx].trim().to_string());
        start = idx + delimiter.len();
        search_from = start;
    }
    parts.push(rule[start..].trim().to_string());

    SplitResult {
        parts,
        delimiter: Some(delimiter.to_string()),
    }
}

fn find_next_delimiter<'a>(
    rule: &'a str,
    delimiters: &[&'a str],
    from: usize,
) -> Option<(usize, &'a str)> {
    let mut square_depth = 0i32;
    let mut paren_depth = 0i32;
    let mut brace_depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    // `<js>…</js>` 是**不可分割的脚本段**：段内的 `||` / `&&` 是 JS 运算符，段外的
    // 才是字段组合符。legado `AnalyzeRule.splitSourceRule` 用 `JS_PATTERN` 先把
    // `<js>` 段整体切成一个 `Mode.Js` 的 `SourceRule`，剩余文本才按 `&&`/`||`/`%%`
    // 处理。
    //
    // 少了这层隔离，该源的 `bookList`
    // （`<js>…</js>\n.item||$.data.worksList[*]||$.[?(@.alias_id)]`）会因为 JS 段里
    // 没有 `||`（或者有，被当成普通代码）而把整条规则算作一段，`</js>` 之后的 `||`
    // 链整个被吞进脚本，「今日限免」分类于是永远 0 本。
    let mut in_js = false;

    for (idx, ch) in rule.char_indices().filter(|(idx, _)| *idx >= from) {
        // 脚本段内只看结束标签：脚本内容不参与组合符判定。
        //
        // 关键是**不要**在段内跟踪引号/括号 —— 书源脚本里常出现模板字符串
        //（`` `...` ``）和正则（`/a|b/`），它们会让引号状态错位；一旦引号状态在
        // `</js>` 之后仍是「未闭合」，`||` 的检测就永远走不到，
        // `.item||$.data.worksList[*]` 这类尾链会被整条吞掉。
        if in_js {
            if rule[idx..].starts_with("</js>") {
                in_js = false;
            }
            continue;
        }
        if rule[idx..].starts_with("<js>") {
            in_js = true;
            // 进入脚本段：外部残留的引号/转义状态作废
            quote = None;
            escaped = false;
            continue;
        }

        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                continue;
            }
            '[' => square_depth += 1,
            ']' => square_depth -= 1,
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            '{' => brace_depth += 1,
            '}' => brace_depth -= 1,
            '\\' => {
                escaped = true;
                continue;
            }
            _ => {}
        }

        if square_depth == 0 && paren_depth == 0 && brace_depth == 0 {
            if let Some(delimiter) = delimiters.iter().find(|delimiter| {
                rule[idx..].starts_with(**delimiter)
                    && !(**delimiter == "@" && rule[idx..].starts_with("@@"))
            }) {
                return Some((idx, *delimiter));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_ignores_delimiters_inside_attribute_selector() {
        let result = split_top_level(r#"div[a="x&&y"]&&span"#, &["&&", "||", "%%"]);

        assert_eq!(result.delimiter.as_deref(), Some("&&"));
        assert_eq!(result.parts, vec![r#"div[a="x&&y"]"#, "span"]);
    }
}
