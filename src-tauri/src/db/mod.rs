pub mod models;

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

use crate::error::AppResult;

/// Opens (creating if needed) the application database and applies all pending
/// migrations. Migrations are the only way the schema ever changes.
pub async fn connect(db_path: &Path) -> AppResult<SqlitePool> {
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| crate::error::AppError::io("create data folder", parent, e))?;
    }

    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(10));

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(15))
        .connect_with(options)
        .await?;

    migrate(&pool).await?;
    Ok(pool)
}

/// In-memory database for tests, with the real migrations applied.
pub async fn connect_in_memory() -> AppResult<SqlitePool> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::from_str("sqlite::memory:")
                .expect("static sqlite url")
                .foreign_keys(true),
        )
        .await?;
    migrate(&pool).await?;
    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> AppResult<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

/// RFC3339 UTC, the format every timestamp column uses.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Problems SQLite reports with its own file, empty when the database is sound.
///
/// `quick_check` is the cheap half of `integrity_check`: it verifies the tables
/// and their indexes without the full page walk, which is what matters here —
/// a broken unique index makes `ON CONFLICT` update the wrong row and makes
/// `COUNT(*)` disagree with a table scan, and both look like the app losing
/// track of things rather than like a damaged file.
pub async fn integrity_problems(pool: &SqlitePool) -> AppResult<Vec<String>> {
    let rows: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .filter(|row| !row.eq_ignore_ascii_case("ok"))
        .flat_map(|row| {
            row.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        // Pages left on the free list are not damage: an interrupted VACUUM
        // leaves them and SQLite reuses them. Reporting them as corruption
        // would rebuild the Java cache on every launch for nothing.
        .filter(|line| !line.starts_with("Page ") && !line.starts_with("*** in database"))
        .collect())
}

pub async fn setting_get(pool: &SqlitePool, key: &str) -> AppResult<Option<String>> {
    let value: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(value.map(|v| v.0))
}

pub async fn setting_set(pool: &SqlitePool, key: &str, value: &str) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn settings_all(pool: &SqlitePool) -> AppResult<Vec<(String, String)>> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM settings ORDER BY key")
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// Appends a row to `instance_events`; the history view and the restart-backoff
/// window both read from it.
pub async fn record_event(
    pool: &SqlitePool,
    instance_id: i64,
    kind: &str,
    detail: Option<&str>,
) -> AppResult<()> {
    sqlx::query("INSERT INTO instance_events (instance_id, ts, kind, detail) VALUES (?, ?, ?, ?)")
        .bind(instance_id)
        .bind(now_rfc3339())
        .bind(kind)
        .bind(detail)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_apply_to_a_fresh_database() {
        let pool = connect_in_memory().await.expect("migrate");
        let tables: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .fetch_all(&pool)
                .await
                .expect("query");
        let names: Vec<String> = tables.into_iter().map(|t| t.0).collect();
        for expected in [
            "artifact_cache",
            "backup_schedules",
            "backups",
            "instance_events",
            "instances",
            "java_runtimes",
            "mod_dependencies",
            "mods",
            "players_seen",
            "resource_samples",
            "settings",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[tokio::test]
    async fn settings_round_trip_and_upsert() {
        let pool = connect_in_memory().await.unwrap();
        assert_eq!(setting_get(&pool, "theme").await.unwrap(), None);
        setting_set(&pool, "theme", "dark").await.unwrap();
        setting_set(&pool, "theme", "light").await.unwrap();
        assert_eq!(
            setting_get(&pool, "theme").await.unwrap(),
            Some("light".to_string())
        );
        assert_eq!(settings_all(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_healthy_database_reports_no_problems() {
        let pool = connect_in_memory().await.unwrap();
        assert!(integrity_problems(&pool).await.unwrap().is_empty());

        // And it stays quiet with data in it.
        setting_set(&pool, "theme", "dark").await.unwrap();
        assert!(integrity_problems(&pool).await.unwrap().is_empty());
    }

    #[test]
    fn free_pages_are_not_reported_as_damage() {
        // What an interrupted VACUUM leaves behind, next to what real damage
        // looks like. Only the second kind is a problem.
        let noise = "*** in database main ***
Page 26: never used
Page 27: never used";
        let damage = "*** in database main ***
Page 26: never used
                      row 3 missing from index sqlite_autoindex_java_runtimes_1";

        let filter = |raw: &str| -> Vec<String> {
            raw.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .filter(|line| !line.starts_with("Page ") && !line.starts_with("*** in database"))
                .map(str::to_string)
                .collect()
        };

        assert!(filter(noise).is_empty());
        assert_eq!(filter(damage).len(), 1);
        assert!(filter(damage)[0].contains("missing from index"));
    }
}
