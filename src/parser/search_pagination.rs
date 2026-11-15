//! Infer whether a search/explore response has more pages.
//!
//! Priority: JSON metadata (common field names) → non-empty page heuristic.
//! Empty book list always means no more pages.

use serde_json::Value;

const CURRENT_PAGE_KEYS: &[&str] = &[
    "currentPage",
    "current_page",
    "page",
    "pageNo",
    "page_no",
    "pageNum",
    "page_num",
    "pageIndex",
    "page_index",
    "pn",
];

const TOTAL_PAGE_KEYS: &[&str] = &[
    "totalPage",
    "total_page",
    "totalPages",
    "total_pages",
    "pageCount",
    "page_count",
    "pages",
    "lastPage",
    "last_page",
];

const TOTAL_COUNT_KEYS: &[&str] = &[
    "totalCount",
    "total_count",
    "total",
    "count",
    "totalNum",
    "total_num",
    "recordCount",
    "record_count",
];

const PAGE_SIZE_KEYS: &[&str] = &[
    "pageSize",
    "page_size",
    "size",
    "limit",
    "perPage",
    "per_page",
];

const HAS_MORE_TRUE_KEYS: &[&str] = &["hasMore", "has_more", "hasNext", "has_next"];

const HAS_MORE_FALSE_KEYS: &[&str] = &["isEnd", "is_end", "finished", "eof", "isLast", "is_last"];

const CURSOR_KEYS: &[&str] = &[
    "next",
    "nextCursor",
    "next_cursor",
    "nextPageToken",
    "next_page_token",
    "continueToken",
    "continue_token",
];

const WRAPPER_KEYS: &[&str] = &["data", "model", "result", "pagination", "page"];

/// Infer has_more for a search response body.
///
/// - Empty `books_len` → false
/// - Recognized JSON metadata → use it
/// - Otherwise non-empty page assumes more pages may exist (caller stops on empty page)
pub fn infer_search_has_more(body: &str, request_page: i32, books_len: usize) -> bool {
    if books_len == 0 {
        return false;
    }
    let trimmed = body.trim_start();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        // HTML / text: keep paging until an empty page.
        return true;
    }
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return true;
    };
    if let Some(flag) = detect_from_value(&root, request_page) {
        return flag;
    }
    true
}

fn detect_from_value(root: &Value, request_page: i32) -> Option<bool> {
    let mut nodes: Vec<&Value> = vec![root];
    if let Value::Object(map) = root {
        for key in WRAPPER_KEYS {
            if let Some(child) = map.get(*key) {
                nodes.push(child);
                if let Value::Object(inner) = child {
                    for key2 in WRAPPER_KEYS {
                        if let Some(child2) = inner.get(*key2) {
                            nodes.push(child2);
                        }
                    }
                }
            }
        }
    }

    for node in &nodes {
        if let Some(v) = find_bool_like(node, HAS_MORE_TRUE_KEYS) {
            return Some(v);
        }
        if let Some(v) = find_bool_like(node, HAS_MORE_FALSE_KEYS) {
            // isEnd/finished true ⇒ no more
            return Some(!v);
        }
        if let Some(has) = find_cursor_has_more(node) {
            return Some(has);
        }
        let current = find_i64(node, CURRENT_PAGE_KEYS).unwrap_or(request_page as i64);
        if let Some(total_page) = find_i64(node, TOTAL_PAGE_KEYS) {
            if total_page > 0 {
                return Some(current < total_page);
            }
        }
        if let (Some(total), Some(size)) = (
            find_i64(node, TOTAL_COUNT_KEYS),
            find_i64(node, PAGE_SIZE_KEYS),
        ) {
            if size > 0 && total >= 0 {
                // request_page is 1-based in book sources
                let page = if current > 0 { current } else { request_page as i64 };
                return Some(page * size < total);
            }
        }
        if let Some(total) = find_i64(node, TOTAL_COUNT_KEYS) {
            // Without pageSize, if we know total and this page returned something,
            // we cannot know page size; leave to outer heuristic unless total is 0.
            if total == 0 {
                return Some(false);
            }
        }
    }
    None
}

fn find_bool_like(node: &Value, keys: &[&str]) -> Option<bool> {
    let Value::Object(map) = node else {
        return None;
    };
    for key in keys {
        if let Some(v) = map.get(*key) {
            match v {
                Value::Bool(b) => return Some(*b),
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        return Some(i != 0);
                    }
                }
                Value::String(s) => {
                    let s = s.trim().to_ascii_lowercase();
                    if matches!(s.as_str(), "1" | "true" | "yes" | "y") {
                        return Some(true);
                    }
                    if matches!(s.as_str(), "0" | "false" | "no" | "n") {
                        return Some(false);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn find_cursor_has_more(node: &Value) -> Option<bool> {
    let Value::Object(map) = node else {
        return None;
    };
    for key in CURSOR_KEYS {
        if let Some(v) = map.get(*key) {
            match v {
                Value::Null => return Some(false),
                Value::String(s) => return Some(!s.trim().is_empty()),
                Value::Number(_) => return Some(true),
                Value::Object(o) => return Some(!o.is_empty()),
                Value::Bool(b) => return Some(*b),
                _ => {}
            }
        }
    }
    None
}

fn find_i64(node: &Value, keys: &[&str]) -> Option<i64> {
    let Value::Object(map) = node else {
        return None;
    };
    for key in keys {
        if let Some(v) = map.get(*key) {
            match v {
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        return Some(i);
                    }
                    if let Some(f) = n.as_f64() {
                        return Some(f as i64);
                    }
                }
                Value::String(s) => {
                    if let Ok(i) = s.trim().parse::<i64>() {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banshanren_model_total() {
        let body = r#"{"result":"success","model":{"currentPage":1,"totalCount":40,"totalPage":2,"pageSize":24,"data":[1,2]}}"#;
        assert!(infer_search_has_more(body, 1, 24));
        let body2 = r#"{"result":"success","model":{"currentPage":2,"totalCount":40,"totalPage":2,"pageSize":24,"data":[1]}}"#;
        assert!(!infer_search_has_more(body2, 2, 16));
    }

    #[test]
    fn empty_books_always_false() {
        let body = r#"{"model":{"currentPage":1,"totalPage":9,"data":[]}}"#;
        assert!(!infer_search_has_more(body, 1, 0));
    }

    #[test]
    fn has_more_bool() {
        assert!(infer_search_has_more(r#"{"hasMore":true,"list":[1]}"#, 1, 1));
        assert!(!infer_search_has_more(r#"{"has_more":false,"list":[1]}"#, 1, 1));
    }

    #[test]
    fn is_end_inverted() {
        assert!(!infer_search_has_more(r#"{"isEnd":true,"list":[1]}"#, 1, 1));
        assert!(infer_search_has_more(r#"{"isEnd":false,"list":[1]}"#, 1, 1));
    }

    #[test]
    fn cursor_next() {
        assert!(infer_search_has_more(r#"{"nextCursor":"abc","items":[1]}"#, 1, 1));
        assert!(!infer_search_has_more(r#"{"nextCursor":null,"items":[1]}"#, 1, 1));
        assert!(!infer_search_has_more(r#"{"next":"","items":[1]}"#, 1, 1));
    }

    #[test]
    fn html_non_empty_assumes_more() {
        assert!(infer_search_has_more("<html><li>book</li></html>", 1, 10));
        assert!(!infer_search_has_more("<html></html>", 1, 0));
    }

    #[test]
    fn total_and_page_size() {
        let body = r#"{"data":{"total":100,"pageSize":20,"page":1,"list":[]}}"#;
        // books_len > 0 required for true path
        assert!(infer_search_has_more(
            r#"{"data":{"total":100,"pageSize":20,"page":1}}"#,
            1,
            20
        ));
        assert!(!infer_search_has_more(
            r#"{"data":{"total":100,"pageSize":20,"page":5}}"#,
            5,
            20
        ));
        let _ = body;
    }
}
