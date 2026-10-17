pub mod repo;

use sqlx::{
    migrate::{Migration, Migrator},
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    Connection, SqliteConnection, SqlitePool,
};
use std::{borrow::Cow, str::FromStr, time::Duration};

static EMBEDDED_MIGRATOR: Migrator = sqlx::migrate!("src/storage/db/migrations");

fn stable_migrator() -> Migrator {
    let migrations = EMBEDDED_MIGRATOR
        .iter()
        .map(|migration| {
            Migration::new(
                migration.version,
                migration.description.clone(),
                migration.migration_type,
                Cow::Owned(migration.sql.replace("\r\n", "\n")),
            )
        })
        .collect::<Vec<_>>();
    Migrator {
        migrations: Cow::Owned(migrations),
        ..Migrator::DEFAULT
    }
}

/// Build connect options from either a `sqlite:` URL or a bare filesystem path.
///
/// URL parsing rejects Windows paths that contain a drive letter, backslashes or
/// spaces (`D:\Reader Data\reader.db`), so callers that already hold an absolute
/// path can pass it through directly instead of encoding it into a URL.
fn connect_options(database_url: &str) -> anyhow::Result<SqliteConnectOptions> {
    let options = if database_url.starts_with("sqlite:") {
        SqliteConnectOptions::from_str(database_url)?
    } else {
        SqliteConnectOptions::new().filename(database_url)
    };
    Ok(options
        .create_if_missing(true)
        .foreign_keys(true)
        // Concurrent readers should not block a short user/session write.
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        // Windows can retain file locks briefly after a writer completes. Wait
        // instead of immediately returning SQLITE_BUSY to the application.
        .busy_timeout(Duration::from_secs(15)))
}

pub async fn init_pool(database_url: &str) -> anyhow::Result<SqlitePool> {
    // 内存库的每条连接都是**独立的**数据库, 关掉池就等于销毁整库。
    // VACUUM 对内存库也没有意义(退出即失), 所以这条路径必须在关池前退出,
    // 否则 `init_pool("sqlite::memory:")` 会返回一个「可用但空无一物」的池子。
    let in_memory = is_in_memory_database(database_url);

    let options = connect_options(database_url)?;
    let pool = SqlitePoolOptions::new()
        .max_connections(if in_memory { 1 } else { 5 })
        .connect_with(options)
        .await?;
    let migrator = stable_migrator();
    repair_legacy_line_ending_checksums(&pool, &migrator).await?;
    migrator.run(&pool).await?;
    if in_memory {
        return Ok(pool);
    }
    let has_free_pages = free_page_count(&pool).await? > 0;
    pool.close().await;
    if has_free_pages {
        if let Err(err) = reclaim_sqlite_space(database_url).await {
            // Startup correctness must not depend on an optional compaction.
            // Windows can still hold WAL/SHM handles briefly after a prior
            // process exits, and VACUUM is safe to retry next launch.
            tracing::warn!("sqlite vacuum skipped: {err}");
        }
    }
    let options = connect_options(database_url)?;
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    Ok(pool)
}

/// 判断连接串是否指向内存库。
///
/// 覆盖 SQLx 支持的两种写法: `sqlite::memory:` 与 `sqlite:file::memory:?cache=shared`
/// (后者常被集成测试用来跨连接共享同一份内存库)。
fn is_in_memory_database(database_url: &str) -> bool {
    let trimmed = database_url.trim();
    if !trimmed.starts_with("sqlite:") {
        return false;
    }
    let rest = trimmed.trim_start_matches("sqlite:");
    rest.starts_with(":memory:") || rest.starts_with("file::memory:") || rest.contains("mode=memory&") || rest.starts_with("//:memory:")
}

/// Migrations and long-lived JSON documents can leave a large freelist in an
/// otherwise small SQLite file. `VACUUM` rewrites it compactly and requires an
/// exclusive connection, so the migration pool is fully closed first.
async fn free_page_count(pool: &SqlitePool) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(pool)
        .await?)
}

async fn reclaim_sqlite_space(database_url: &str) -> anyhow::Result<()> {
    let mut connection = SqliteConnection::connect(database_url).await?;
    let freelist: i64 = sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(&mut connection)
        .await?;
    if freelist == 0 {
        return Ok(());
    }
    // VACUUM owns its own locking and cannot run inside an explicit
    // transaction. This standalone connection is not shared with a pool, so
    // no other connection can hold a reader lock while it rewrites the file.
    sqlx::query("VACUUM").execute(&mut connection).await?;
    tracing::info!(freelist_pages = freelist, "sqlite vacuum reclaimed free pages");
    Ok(())
}

/// 测试专用的内存库: 跑完全部迁移, 不需要清理任何文件。
///
/// 必须 `max_connections(1)` + `shared_cache` 语义: SQLite 的 `:memory:` 库是
/// **每个连接一个独立数据库**, 池里有多条连接时, 建表连接与查询连接看到的表
/// 会不是同一个, 测试会莫名其妙报 "no such table"。
#[cfg(test)]
pub(crate) async fn test_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    let migrator = stable_migrator();
    migrator.run(&pool).await.expect("run migrations");
    pool
}

/// Older Windows packages could embed CRLF migration scripts while CI packages
/// embedded LF. SQLx hashes raw bytes, so the same SQL then looked modified.
/// Only repair a checksum when it exactly matches the alternate line-ending form.
async fn repair_legacy_line_ending_checksums(
    pool: &SqlitePool,
    migrator: &Migrator,
) -> anyhow::Result<()> {
    let table_exists: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if table_exists == 0 {
        return Ok(());
    }

    for migration in migrator.iter() {
        let stored: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
                .bind(migration.version)
                .fetch_optional(pool)
                .await?;
        let Some(stored) = stored else {
            continue;
        };
        if stored.as_slice() == migration.checksum.as_ref() {
            continue;
        }

        let normalized = migration.sql.replace("\r\n", "\n");
        let crlf = normalized.replace('\n', "\r\n");
        let matches_alternate_line_endings = [normalized, crlf].into_iter().any(|sql| {
            if sql.as_bytes() == migration.sql.as_bytes() {
                return false;
            }
            let alternate = Migration::new(
                migration.version,
                migration.description.clone(),
                migration.migration_type,
                Cow::Owned(sql),
            );
            stored.as_slice() == alternate.checksum.as_ref()
        });

        if matches_alternate_line_endings {
            sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
                .bind(migration.checksum.as_ref())
                .bind(migration.version)
                .execute(pool)
                .await?;
            tracing::warn!(
                version = migration.version,
                "repaired legacy migration checksum caused by line endings"
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn migrated_memory_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        stable_migrator().run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn repairs_checksum_created_with_windows_line_endings() {
        let pool = migrated_memory_pool().await;
        let migrator = stable_migrator();
        let migration = migrator.iter().find(|item| item.version == 4).unwrap();
        let crlf_sql = migration.sql.replace("\r\n", "\n").replace('\n', "\r\n");
        let crlf_migration = Migration::new(
            migration.version,
            migration.description.clone(),
            migration.migration_type,
            Cow::Owned(crlf_sql),
        );
        sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = 4")
            .bind(crlf_migration.checksum.as_ref())
            .execute(&pool)
            .await
            .unwrap();

        repair_legacy_line_ending_checksums(&pool, &migrator)
            .await
            .unwrap();
        migrator.run(&pool).await.unwrap();

        let checksum: Vec<u8> =
            sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = 4")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(checksum.as_slice(), migration.checksum.as_ref());
    }

    #[tokio::test]
    async fn keeps_rejecting_genuinely_modified_migrations() {
        let pool = migrated_memory_pool().await;
        let migrator = stable_migrator();
        sqlx::query("UPDATE _sqlx_migrations SET checksum = x'010203' WHERE version = 4")
            .execute(&pool)
            .await
            .unwrap();

        repair_legacy_line_ending_checksums(&pool, &migrator)
            .await
            .unwrap();
        let error = migrator.run(&pool).await.unwrap_err();

        assert!(error.to_string().contains("migration 4"));
    }

    #[tokio::test]
    async fn drops_legacy_cache_tables() {
        let pool = migrated_memory_pool().await;
        for table in ["book_cache", "chapter_cache"] {
            let exists: i64 = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(exists, 0, "遗留表 {table} 应在迁移后删除");
        }
    }
    /// `init_pool` 在文件库上会关池重开以做 VACUUM; 内存库不能走这条路,
    /// 否则返回的池子「可用但空无一物」(迁移建的表随连接一起消失)。
    #[tokio::test]
    async fn init_pool_keeps_in_memory_database_intact() {
        let pool = init_pool("sqlite::memory:").await.unwrap();
        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(users, 0, "迁移建出的 users 表必须还在");
    }

    #[test]
    fn in_memory_database_detection_covers_sqlx_forms() {
        assert!(is_in_memory_database("sqlite::memory:"));
        assert!(is_in_memory_database("sqlite:file::memory:?cache=shared"));
        assert!(!is_in_memory_database("sqlite:storage/reader.db?mode=rwc"));
        assert!(!is_in_memory_database("E:/Desktop/reader/reader.db"));
    }

}
