//! Persistence for the release chronology used by every version sort.
//!
//! Mojang's manifest is the only source that can say whether `1.21.11` predates
//! `26.2`. It is fetched at most every few hours and cached in
//! `mc_version_index`, so sorting keeps working with no network.

use sqlx::SqlitePool;

use crate::db::{now_rfc3339, setting_get, setting_set};
use crate::error::AppResult;
use crate::http::Fetch;
use crate::mcversion::{IndexedVersion, VersionIndex};

use super::vanilla;

/// How long a cached index is considered current.
pub const MAX_AGE_HOURS: i64 = 6;
const REFRESHED_AT_KEY: &str = "version_index_refreshed_at";

/// Reads the cached chronology. An empty index is valid: callers degrade to
/// component ordering rather than failing.
pub async fn load(pool: &SqlitePool) -> AppResult<VersionIndex> {
    let rows = sqlx::query_as::<_, (String, String, String, i64)>(
        "SELECT id, release_time, kind, position FROM mc_version_index",
    )
    .fetch_all(pool)
    .await?;

    Ok(VersionIndex::from_entries(rows.into_iter().map(
        |(id, release_time, kind, position)| IndexedVersion {
            id,
            release_time,
            kind,
            position,
        },
    )))
}

/// True when the cache is missing or older than [`MAX_AGE_HOURS`].
pub async fn is_stale(pool: &SqlitePool) -> AppResult<bool> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mc_version_index")
        .fetch_one(pool)
        .await?;
    if count == 0 {
        return Ok(true);
    }

    let Some(refreshed) = setting_get(pool, REFRESHED_AT_KEY).await? else {
        return Ok(true);
    };
    let Ok(refreshed) = chrono::DateTime::parse_from_rfc3339(&refreshed) else {
        return Ok(true);
    };
    Ok(chrono::Utc::now().signed_duration_since(refreshed.with_timezone(&chrono::Utc))
        > chrono::Duration::hours(MAX_AGE_HOURS))
}

/// Fetches the manifest and replaces the cached chronology.
pub async fn refresh<F: Fetch>(pool: &SqlitePool, fetch: &F) -> AppResult<VersionIndex> {
    let body = fetch.get_text(vanilla::MANIFEST_URL).await?;
    let entries = vanilla::parse_manifest_entries(&body)?;
    store(pool, &entries).await?;
    Ok(VersionIndex::from_entries(entries))
}

/// Refreshes when stale, and never lets a network failure break a version list:
/// a stale-but-present cache is better than no ordering at all.
pub async fn ensure_fresh<F: Fetch>(pool: &SqlitePool, fetch: &F) -> AppResult<VersionIndex> {
    if is_stale(pool).await.unwrap_or(true) {
        match refresh(pool, fetch).await {
            Ok(index) => return Ok(index),
            Err(err) => tracing::warn!(error = %err, "could not refresh the version index"),
        }
    }
    load(pool).await
}

async fn store(pool: &SqlitePool, entries: &[IndexedVersion]) -> AppResult<()> {
    let mut transaction = pool.begin().await?;
    sqlx::query("DELETE FROM mc_version_index")
        .execute(&mut *transaction)
        .await?;

    for entry in entries {
        sqlx::query(
            "INSERT INTO mc_version_index (id, release_time, kind, position)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&entry.id)
        .bind(&entry.release_time)
        .bind(&entry.kind)
        .bind(entry.position)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;

    setting_set(pool, REFRESHED_AT_KEY, &now_rfc3339()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::FixtureFetch;

    fn fixtures() -> FixtureFetch {
        FixtureFetch::new().route(vanilla::MANIFEST_URL, "vanilla_version_manifest_v2.json")
    }

    #[tokio::test]
    async fn a_fresh_database_has_no_index_and_reports_stale() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        assert!(load(&pool).await.unwrap().is_empty());
        assert!(is_stale(&pool).await.unwrap());
    }

    #[tokio::test]
    async fn refresh_stores_chronology_and_survives_a_reload() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let index = refresh(&pool, &fixtures()).await.unwrap();

        // Chronology, not string order: both are in the fixture manifest.
        assert!(index.is_newer("26.2", "1.21.4"));
        assert!(!is_stale(&pool).await.unwrap());

        let reloaded = load(&pool).await.unwrap();
        assert_eq!(reloaded.len(), index.len());
        assert!(reloaded.is_newer("26.2", "1.21.4"));
        assert_eq!(
            reloaded.get("1.21.4").map(|entry| entry.kind.as_str()),
            Some("release")
        );
    }

    #[tokio::test]
    async fn refresh_replaces_rather_than_accumulates() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let first = refresh(&pool, &fixtures()).await.unwrap();
        let second = refresh(&pool, &fixtures()).await.unwrap();
        assert_eq!(first.len(), second.len());
    }

    #[tokio::test]
    async fn ensure_fresh_keeps_the_cache_when_the_network_fails() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        refresh(&pool, &fixtures()).await.unwrap();
        // Force staleness, then offer a fetcher with no routes at all.
        setting_set(&pool, REFRESHED_AT_KEY, "2000-01-01T00:00:00Z")
            .await
            .unwrap();

        let index = ensure_fresh(&pool, &FixtureFetch::new()).await.unwrap();
        assert!(!index.is_empty(), "the stale cache is still usable");
        assert!(index.is_newer("26.2", "1.21.4"));
    }
}
