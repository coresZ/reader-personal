use serde_json::Value;

/// 把 legado（jayway JsonPath）的写法规范成 `jsonpath_lib` 能接受的写法。
///
/// 差异只在「前导点」上，但后果很严重：
///
/// - jayway 里 `.[?(@.x)]` 的 `.` 是**递归下降**的起点，等价于 `$..[?(@.x)]`，
///   会在整棵文档里找所有带 `x` 的对象；
/// - `jsonpath_lib` 不认 `.[?(…)]`，会报语法错误；而早先的实现在「不以 `$` 开头」
///   时把前导点**剥掉**再拼 `$.`，得到 `$.?(@.x)` —— 依然非法，于是恒定 0 项。
///
/// 书源里 `.[?(@.bookName)]||.[?(@.title)]` 这种写法很常见（不写 `$`、不写 `..`，
/// 靠 jayway 的宽松语义做「全文档找书」），桌面端因此搜索恒 0 条，而手机端正常。
///
/// 规则：
/// 1. 已经是 `$…` / `[` 开头的原样返回；
/// 2. `.[?(…)` / `.[*]` 这类「点 + 方括号」补成 `$..`（递归下降）；
/// 3. 其余 `.a.b` 普通路径补成 `$.a.b`（前导点只是省略的根，**不能**变递归）。
fn normalize_rule(rule: &str) -> String {
    if rule.starts_with('$') || rule.starts_with('[') {
        return rule.to_string();
    }
    if let Some(rest) = rule.strip_prefix('.') {
        // `.[?(…)]` / `.[*]` / `.[0]` —— 方括号紧跟点，jayway 按递归下降处理。
        if rest.starts_with('[') {
            return format!("$..{rest}");
        }
        return format!("$.{rest}");
    }
    format!("$.{rule}")
}

pub fn jsonpath_query(value: &Value, rule: &str) -> Vec<Value> {
    if let Some(rendered) = render_embedded_paths(value, rule) {
        return vec![Value::String(rendered)];
    }
    // legado 允许省略 `$` 前缀（如 `data.books`），统一补成 `$.data.books`；
    // `.[?(…)]` 则要补成 `$..[?(…)]`（见 `normalize_rule`）。
    let owned = normalize_rule(rule);
    let rule = owned.as_str();
    if let Ok(res) = jsonpath_lib::select(value, rule) {
        let mut out = Vec::new();
        for item in res {
            match item {
                Value::Array(items) => {
                    out.extend(items.iter().cloned());
                }
                other => out.push(other.clone()),
            }
        }
        out
    } else {
        vec![]
    }
}

pub fn jsonpath_first_string(value: &Value, rule: &str) -> Option<String> {
    if let Some(rendered) = render_embedded_paths(value, rule) {
        return Some(rendered);
    }
    let res = jsonpath_query(value, rule);
    res.first().and_then(|v| value_to_string(v))
}

pub fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(value_to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        Value::Object(_) => Some(v.to_string()),
    }
}

fn render_embedded_paths(value: &Value, rule: &str) -> Option<String> {
    if !rule.contains("{$") {
        return None;
    }
    let re = regex::Regex::new(r"\{\s*(\$[^}]+)\}").unwrap();
    let mut replaced_any = false;
    let rendered = re
        .replace_all(rule, |captures: &regex::Captures| {
            replaced_any = true;
            let path = captures.get(1).map(|m| m.as_str()).unwrap_or_default();
            resolve_inner_path(value, path)
        })
        .into_owned();
    if replaced_any {
        Some(rendered)
    } else {
        Some(String::new())
    }
}

/// 解析 `{{$.a||$.b}}` 这类内嵌路径里的**组合符**。
///
/// legado 的 `AnalyzeByJSonPath.getString` 会先用 `splitRule("&&", "||")` 把规则
/// 切成多段，再逐个求值、按组合符合并：
///
/// - `||`：**取第一个有值的结果**（回退链）。书源大量用
///   `{{$.bookId||$.bid}}` 兼容新旧两套字段名。
/// - `&&`：全部求值后按行拼接。
///
/// 早先这里把整串当成一条 JSONPath 直接查，`$.bookId||$.bid` 在引擎里是非法
/// 语法，结果恒为空 —— 于是 `bookId` 取空、`bookUrl` 拼出来就只剩接口路径，
/// 症状是「搜索有结果但每本书的地址都一样、点进去打不开」。
fn resolve_inner_path(value: &Value, path: &str) -> String {
    // `||` 优先: 回退链只要第一个非空即可; 没有再退到 `&&` 的拼接语义。
    if path.contains("||") {
        for part in path.split("||") {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some(found) = jsonpath_first_string(value, part) {
                if !found.is_empty() {
                    return found;
                }
            }
        }
        return String::new();
    }
    if path.contains("&&") {
        let joined = path
            .split("&&")
            .filter_map(|part| {
                let part = part.trim();
                if part.is_empty() {
                    None
                } else {
                    jsonpath_first_string(value, part).filter(|s| !s.is_empty())
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        return joined;
    }
    jsonpath_first_string(value, path).unwrap_or_default()
}
