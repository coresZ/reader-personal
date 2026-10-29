//! 对**本机已安装**的书源逐个跑一次真实搜索，报告失败者。
//!
//! 这是「主动扫一遍同类语义缺口」的抓手：单测只能覆盖我们想到的写法，真实书源
//! 里那些「写法合法但语义不对」的细节（缺 API、参数被截断、字符集被忽略…）
//! 只有真跑一遍才暴露。失败面往往是**静默的** —— 脚本里 `try { … } catch { return null }`
//! 把异常吞掉，界面只显示「搜索 0 条」。
//!
//! 需要网络，默认 `#[ignore]`。运行：
//!
//! ```powershell
//! $env:READER_DB = "E:\Desktop\reader\reader-rust\desktop\.dev-data\reader.db"
//! $env:READER_SMOKE_KEYWORD = "剑来"
//! cargo test -p reader-rust --test live_installed_sources -- --ignored --nocapture
//! ```
//!
//! 环境变量：
//! - `READER_DB`：SQLite 路径，默认用开发库。
//! - `READER_SMOKE_KEYWORD`：搜索关键词，默认「剑来」。
//! - `READER_SMOKE_ONLY`：只跑名字包含该子串的源（调试用）。
//! - `READER_SMOKE_MIN_BOOKS`：判定通过的最少结果数，默认 1。
use reader_rust::crawler::http_client::HttpClient;
use reader_rust::model::book_source::book_source_from_value;
use reader_rust::parser::rule_engine::RuleEngine;
use reader_rust::service::book_service::BookService;
use reader_rust::storage::cache::file_cache::FileCache;
use std::env;
use uuid::Uuid;

fn default_db() -> String {
    "desktop/.dev-data/reader.db".to_string()
}

/// 从 SQLite 里读出全部书源的 JSON。
async fn load_sources(path: &str) -> Vec<serde_json::Value> {
    use sqlx::sqlite::SqlitePoolOptions;
    let url = format!("sqlite://{}?mode=ro", path.replace('\\', "/"));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("open reader.db");
    let rows: Vec<(String,)> = sqlx::query_as("select json from book_sources")
        .fetch_all(&pool)
        .await
        .expect("query book_sources");
    rows.into_iter()
        .filter_map(|(text,)| serde_json::from_str(&text).ok())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "会向真实站点发请求"]
async fn live_installed_sources_search() {
    let db = env::var("READER_DB").unwrap_or_else(|_| default_db());
    let keyword = env::var("READER_SMOKE_KEYWORD").unwrap_or_else(|_| "剑来".to_string());
    let only = env::var("READER_SMOKE_ONLY").unwrap_or_default();
    let min_books: usize = env::var("READER_SMOKE_MIN_BOOKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);

    let sources = load_sources(&db).await;
    assert!(!sources.is_empty(), "数据库里没有书源: {db}");

    let http = HttpClient::new(20, None).expect("http client");
    let root = env::temp_dir().join(format!("reader-rust-smoke-{}", Uuid::new_v4()));
    let service = BookService::new(
        http.clone(),
        RuleEngine::new().expect("rule engine"),
        FileCache::new(root.join("cache")),
        root.to_string_lossy().as_ref(),
    );

    let mut ok = Vec::new();
    let mut failed = Vec::new();

    for value in sources {
        let Ok(source) = book_source_from_value(value) else {
            continue;
        };
        let name = source.book_source_name.clone();
        if !only.is_empty() && !name.contains(&only) {
            continue;
        }
        // 没有搜索规则的源（纯发现页）跳过。
        if source
            .search_url
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            continue;
        }
        print!("[{name}] ");
        match service.search_book("default", &source, &keyword, 1).await {
            Ok(books) if books.len() >= min_books => {
                let sample = books
                    .first()
                    .map(|b| b.name.as_str())
                    .unwrap_or_default();
                println!("OK  {} 本  首条={sample}", books.len());
                ok.push(name);
            }
            Ok(books) => {
                println!("EMPTY  {} 本", books.len());
                failed.push((name, format!("0 本（关键词 {keyword}）")));
            }
            Err(err) => {
                println!("ERR   {err:#}");
                failed.push((name, format!("{err:#}")));
            }
        }
    }

    println!("\n===== 汇总 =====");
    println!("通过 {} / 失败 {}", ok.len(), failed.len());
    for (name, reason) in &failed {
        let reason: String = reason.chars().take(200).collect();
        println!("  ✗ {name}\n      {reason}");
    }
}
