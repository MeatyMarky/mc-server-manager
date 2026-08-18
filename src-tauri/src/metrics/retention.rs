//! Retention for `resource_samples`.
//!
//! Full resolution for 24 h, one row per minute after that, nothing older than
//! 30 days. Runs once at startup and every 24 h afterwards.

use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;

use crate::error::AppResult;

pub const FULL_RESOLUTION_HOURS: i64 = 24;
pub const MAX_AGE_DAYS: i64 = 30;
const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneStats {
    /// Rows removed by collapsing older samples to one per minute.
    pub downsampled: u64,
    /// Rows removed for being older than the retention window.
    pub expired: u64,
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Timestamps are fixed-width RFC3339 UTC, so string comparison is chronological
/// and `substr(ts, 1, 16)` is the minute bucket (`YYYY-MM-DDTHH:MM`).
pub async fn prune(pool: &SqlitePool, now: DateTime<Utc>) -> AppResult<PruneStats> {
    let downsample_before = stamp(now - Duration::hours(FULL_RESOLUTION_HOURS));
    let delete_before = stamp(now - Duration::days(MAX_AGE_DAYS));

    let expired = sqlx::query("DELETE FROM resource_samples WHERE ts < ?")
        .bind(&delete_before)
        .execute(pool)
        .await?
        .rows_affected();

    // Keep the earliest sample in each minute bucket, drop the rest.
    let downsampled = sqlx::query(
        "DELETE FROM resource_samples AS r
         WHERE r.ts < ?
           AND EXISTS (
               SELECT 1 FROM resource_samples AS keep
               WHERE keep.instance_id = r.instance_id
                 AND substr(keep.ts, 1, 16) = substr(r.ts, 1, 16)
                 AND keep.ts < r.ts
           )",
    )
    .bind(&downsample_before)
    .execute(pool)
    .await?
    .rows_affected();

    if downsampled > 0 || expired > 0 {
        tracing::debug!(downsampled, expired, "pruned resource samples");
    }
    Ok(PruneStats {
        downsampled,
        expired,
    })
}

/// Prunes now, then once a day for as long as the app runs. The caller owns the
/// spawning: `setup` runs outside a Tokio runtime context, so this is handed to
/// `tauri::async_runtime::spawn` rather than `tokio::spawn`.
pub async fn pruner_loop(pool: SqlitePool) {
    loop {
        if let Err(err) = prune(&pool, Utc::now()).await {
            tracing::warn!(error = %err, "resource sample prune failed");
        }
        tokio::time::sleep(PRUNE_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed(pool: &SqlitePool) {
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'A', 'Z:/a', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_sample(pool: &SqlitePool, at: DateTime<Utc>) {
        sqlx::query("INSERT INTO resource_samples (instance_id, ts, cpu_pct, rss_bytes) VALUES (1, ?, 1.0, 1024)")
            .bind(stamp(at))
            .execute(pool)
            .await
            .unwrap();
    }

    async fn count(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM resource_samples")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn recent_samples_keep_full_resolution() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        seed(&pool).await;
        let now = Utc::now();
        for offset in 0..12 {
            insert_sample(&pool, now - Duration::seconds(offset * 5)).await;
        }

        let stats = prune(&pool, now).await.unwrap();
        assert_eq!(stats, PruneStats::default());
        assert_eq!(count(&pool).await, 12);
    }

    #[tokio::test]
    async fn older_samples_collapse_to_one_per_minute() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        seed(&pool).await;
        let now = Utc::now();
        let old = now - Duration::hours(30);
        // Twelve samples inside one minute, plus one in the next minute.
        for offset in 0..12 {
            insert_sample(&pool, old + Duration::seconds(offset * 5)).await;
        }
        insert_sample(&pool, old + Duration::seconds(65)).await;

        let stats = prune(&pool, now).await.unwrap();
        assert_eq!(stats.downsampled, 11);
        assert_eq!(count(&pool).await, 2, "one row per minute survives");
    }

    #[tokio::test]
    async fn samples_past_thirty_days_are_deleted() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        seed(&pool).await;
        let now = Utc::now();
        insert_sample(&pool, now - Duration::days(31)).await;
        insert_sample(&pool, now - Duration::days(29)).await;
        insert_sample(&pool, now).await;

        let stats = prune(&pool, now).await.unwrap();
        assert_eq!(stats.expired, 1);
        assert_eq!(count(&pool).await, 2);
    }

    #[tokio::test]
    async fn pruning_is_idempotent() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        seed(&pool).await;
        let now = Utc::now();
        let old = now - Duration::hours(48);
        for offset in 0..6 {
            insert_sample(&pool, old + Duration::seconds(offset * 5)).await;
        }

        prune(&pool, now).await.unwrap();
        let after_first = count(&pool).await;
        let stats = prune(&pool, now).await.unwrap();
        assert_eq!(stats, PruneStats::default());
        assert_eq!(count(&pool).await, after_first);
    }

    #[tokio::test]
    async fn instances_are_downsampled_independently() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        seed(&pool).await;
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u2', 'B', 'Z:/b', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();

        let then = Utc::now() - Duration::hours(48);
        for instance_id in [1, 2] {
            for offset in 0..4 {
                sqlx::query(
                    "INSERT INTO resource_samples (instance_id, ts, cpu_pct, rss_bytes) VALUES (?, ?, 1.0, 1)",
                )
                .bind(instance_id)
                .bind(stamp(then + Duration::seconds(offset * 5)))
                .execute(&pool)
                .await
                .unwrap();
            }
        }

        prune(&pool, Utc::now()).await.unwrap();
        assert_eq!(count(&pool).await, 2, "one surviving row per instance");
    }
}
