//! 聚合源目录解析回归：`chapterList` 脚本交回**完整响应**，章节数组埋在内层。
//!
//! 典型写法：脚本末尾 `resStr;`（或 `result = java.ajax(...)` 后直接收尾），交回的是
//! `{"code":0,"data":{"chapters":[...]}}` 这种整包响应。章节字段规则（`$.name` /
//! `$.id`）是相对路径，只在章节节点上才有意义。若把整包响应当成一个 item 求值，
//! 目录会解析出 0 章 —— 表现为「无法加载目录」。

use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::js;
use reader_rust::parser::rule_engine::RuleEngine;
use serde_json::json;

fn source() -> reader_rust::model::book_source::BookSource {
    book_source_from_value(json!({
        "bookSourceName": "聚合示例源",
        "bookSourceUrl": "https://aggregate.example.test",
        "ruleToc": {
            // 脚本原样交回整包响应，章节数组在 data.chapters
            "chapterList": "@js:\nJSON.stringify({code:0,data:{chapters:[\
{cid:'c1',name:'第一章',src:'plat'},\
{cid:'c2',name:'第二章',src:'plat'}]}});",
            "chapterName": "$.name",
            "chapterUrl": "/api/chapter?source={{$.src}}&id={{$.cid}}"
        }
    }))
    .unwrap()
}

#[test]
fn toc_script_returning_wrapped_response_still_yields_chapters() {
    let src = source();
    let base = "https://aggregate.example.test";
    js::with_source(&src, || {
        let engine = RuleEngine::new().unwrap();
        let (chapters, _) = engine.chapter_list(&src, "ignored", base, Some(base));
        assert_eq!(
            chapters.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(),
            ["第一章", "第二章"],
            "脚本交回整包响应时, 章节数组要能下钻取到"
        );
        assert_eq!(
            chapters[0].url,
            "https://aggregate.example.test/api/chapter?source=plat&id=c1"
        );
        assert_eq!(
            chapters[1].url,
            "https://aggregate.example.test/api/chapter?source=plat&id=c2"
        );
    });
}

/// 脚本直接交回章节数组时行为不变。
#[test]
fn toc_script_returning_plain_array_is_unchanged() {
    let src = book_source_from_value(json!({
        "bookSourceName": "数组示例源",
        "bookSourceUrl": "https://array.example.test",
        "ruleToc": {
            "chapterList": "@js:\nJSON.stringify([{cid:'c1',name:'第一章'},{cid:'c2',name:'第二章'}]);",
            "chapterName": "$.name",
            "chapterUrl": "/c/{{$.cid}}"
        }
    }))
    .unwrap();
    let base = "https://array.example.test";
    js::with_source(&src, || {
        let engine = RuleEngine::new().unwrap();
        let (chapters, _) = engine.chapter_list(&src, "ignored", base, Some(base));
        assert_eq!(
            chapters.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(),
            ["第一章", "第二章"]
        );
        assert_eq!(chapters[0].url, "https://array.example.test/c/c1");
    });
}

/// 单章对象（JSONPath 直达）不能被误当成列表候选而丢失。
#[test]
fn toc_single_object_item_is_kept() {
    let src = book_source_from_value(json!({
        "bookSourceName": "单章示例源",
        "bookSourceUrl": "https://single.example.test",
        "ruleToc": {
            "chapterList": "@js:\nJSON.stringify({cid:'only',name:'唯一章'});",
            "chapterName": "$.name",
            "chapterUrl": "/c/{{$.cid}}"
        }
    }))
    .unwrap();
    let base = "https://single.example.test";
    js::with_source(&src, || {
        let engine = RuleEngine::new().unwrap();
        let (chapters, _) = engine.chapter_list(&src, "ignored", base, Some(base));
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].title, "唯一章");
        assert_eq!(chapters[0].url, "https://single.example.test/c/only");
    });
}
