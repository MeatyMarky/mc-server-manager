//! Finding usable Java runtimes.
//!
//! Candidates come from PATH, `JAVA_HOME`, `CLASSPATH` entries, the standard
//! install roots for the platform, and the Windows registry. Every candidate is
//! then resolved by *running* it — folder names are never trusted. Results are
//! cached in `java_runtimes` keyed on the binary's mtime and size, so a rescan
//! only executes binaries that actually changed.

pub mod detect;
pub mod version;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use ts_rs::TS;

use crate::db::now_rfc3339;
use crate::error::{AppError, AppResult};

pub use version::{required_java_for, satisfies, JavaVersionInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, TS)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum JavaSource {
    Path,
    JavaHome,
    Classpath,
    Registry,
    CommonDir,
    Manual,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct JavaRuntime {
    #[ts(type = "number")]
    pub id: i64,
    /// Absolute path to the `java` binary.
    pub path: String,
    #[ts(type = "number")]
    pub major: i64,
    pub full_version: Option<String>,
    pub vendor: Option<String>,
    pub arch: Option<String>,
    pub source: JavaSource,
    pub valid: bool,
    pub detected_at: String,
}

pub async fn list(pool: &SqlitePool) -> AppResult<Vec<JavaRuntime>> {
    let rows = sqlx::query_as::<_, JavaRuntime>(
        "SELECT id, path, major, full_version, vendor, arch, source, valid, detected_at
         FROM java_runtimes WHERE valid = 1 ORDER BY major DESC, path",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Best runtime for a required major version: the lowest major that still
/// satisfies the requirement, so a 1.16 server does not get Java 26.
pub async fn best_for(pool: &SqlitePool, required: i64) -> AppResult<Option<JavaRuntime>> {
    let mut runtimes = list(pool).await?;
    runtimes.retain(|r| satisfies(r.major, required));
    runtimes.sort_by_key(|r| r.major);
    Ok(runtimes.into_iter().next())
}

/// Runs detection and replaces the cache. Returns everything now known.
pub async fn rescan(pool: &SqlitePool) -> AppResult<Vec<JavaRuntime>> {
    let cached = sqlx::query_as::<_, detect::CachedProbe>(
        "SELECT path, mtime, size_bytes, major, full_version, vendor, arch, valid FROM java_runtimes",
    )
    .fetch_all(pool)
    .await?;

    let probes = tokio::task::spawn_blocking(move || detect::detect_all(&cached))
        .await
        .map_err(|e| AppError::internal("Java detection", e))?;

    let now = now_rfc3339();
    for probe in &probes {
        sqlx::query(
            "INSERT INTO java_runtimes
                (path, major, vendor, arch, source, valid, detected_at, mtime, size_bytes, full_version, error)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(path) DO UPDATE SET
                major = excluded.major, vendor = excluded.vendor, arch = excluded.arch,
                source = excluded.source, valid = excluded.valid, detected_at = excluded.detected_at,
                mtime = excluded.mtime, size_bytes = excluded.size_bytes,
                full_version = excluded.full_version, error = excluded.error",
        )
        .bind(&probe.path)
        .bind(probe.major.unwrap_or(0))
        .bind(&probe.vendor)
        .bind(&probe.arch)
        .bind(probe.source)
        .bind(probe.major.is_some())
        .bind(&now)
        .bind(probe.mtime)
        .bind(probe.size_bytes)
        .bind(&probe.full_version)
        .bind(&probe.error)
        .execute(pool)
        .await?;
    }

    // Runtimes that vanished from disk stop being offered.
    let found: Vec<String> = probes.iter().map(|p| p.path.clone()).collect();
    let rows = sqlx::query_as::<_, (i64, String)>("SELECT id, path FROM java_runtimes")
        .fetch_all(pool)
        .await?;
    for (id, path) in rows {
        if !found.contains(&path) && !std::path::Path::new(&path).is_file() {
            sqlx::query("UPDATE java_runtimes SET valid = 0, error = ? WHERE id = ?")
                .bind("the binary is no longer on disk")
                .bind(id)
                .execute(pool)
                .await?;
        }
    }

    // Recorded so the first-run screen can tell "no Java on this machine" apart
    // from "detection has not run yet"; the two need different words.
    crate::db::setting_set(pool, "java_scanned_at", &now).await?;

    list(pool).await
}

/// The "browse for a JDK" fallback: adds one path the user picked, after
/// checking that it really is a Java runtime.
pub async fn add_manual(pool: &SqlitePool, path: &str) -> AppResult<JavaRuntime> {
    let candidate = std::path::PathBuf::from(path);
    let binary = detect::java_binary_in(&candidate).unwrap_or(candidate);

    let probe = tokio::task::spawn_blocking({
        let binary = binary.clone();
        move || detect::probe(&binary, JavaSource::Manual)
    })
    .await
    .map_err(|e| AppError::Other(format!("Java probe failed: {e}")))?;

    let Some(major) = probe.major else {
        return Err(AppError::Other(format!(
            "{} is not a usable Java runtime{}",
            binary.display(),
            probe
                .error
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        )));
    };

    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO java_runtimes
            (path, major, vendor, arch, source, valid, detected_at, mtime, size_bytes, full_version, error)
         VALUES (?, ?, ?, ?, 'manual', 1, ?, ?, ?, ?, NULL)
         ON CONFLICT(path) DO UPDATE SET
            major = excluded.major, vendor = excluded.vendor, arch = excluded.arch,
            source = 'manual', valid = 1, detected_at = excluded.detected_at,
            mtime = excluded.mtime, size_bytes = excluded.size_bytes,
            full_version = excluded.full_version, error = NULL",
    )
    .bind(&probe.path)
    .bind(major)
    .bind(&probe.vendor)
    .bind(&probe.arch)
    .bind(&now)
    .bind(probe.mtime)
    .bind(probe.size_bytes)
    .bind(&probe.full_version)
    .execute(pool)
    .await?;

    sqlx::query_as::<_, JavaRuntime>(
        "SELECT id, path, major, full_version, vendor, arch, source, valid, detected_at
         FROM java_runtimes WHERE path = ?",
    )
    .bind(&probe.path)
    .fetch_one(pool)
    .await
    .map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed(pool: &SqlitePool, path: &str, major: i64) {
        sqlx::query(
            "INSERT INTO java_runtimes (path, major, source, valid, detected_at)
             VALUES (?, ?, 'common_dir', 1, ?)",
        )
        .bind(path)
        .bind(major)
        .bind(now_rfc3339())
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn best_for_picks_the_lowest_runtime_that_satisfies() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        seed(&pool, "/jdk8/bin/java", 8).await;
        seed(&pool, "/jdk17/bin/java", 17).await;
        seed(&pool, "/jdk21/bin/java", 21).await;
        seed(&pool, "/jdk26/bin/java", 26).await;

        assert_eq!(best_for(&pool, 21).await.unwrap().unwrap().major, 21);
        assert_eq!(best_for(&pool, 17).await.unwrap().unwrap().major, 17);
        // Nothing satisfies a Java 25 requirement except the 26 runtime.
        assert_eq!(best_for(&pool, 25).await.unwrap().unwrap().major, 26);
    }

    #[tokio::test]
    async fn best_for_reports_nothing_when_every_runtime_is_too_old() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        seed(&pool, "/jdk8/bin/java", 8).await;
        assert!(best_for(&pool, 21).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn invalid_runtimes_are_not_offered() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        seed(&pool, "/broken/bin/java", 0).await;
        sqlx::query("UPDATE java_runtimes SET valid = 0")
            .execute(&pool)
            .await
            .unwrap();
        assert!(list(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_scan_records_that_it_happened_even_when_it_finds_nothing() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        assert_eq!(
            crate::db::setting_get(&pool, "java_scanned_at").await.unwrap(),
            None
        );

        rescan(&pool).await.unwrap();

        // Whatever this machine has, the fact that we looked is now on record —
        // that is what separates "no Java here" from "not looked yet".
        assert!(crate::db::setting_get(&pool, "java_scanned_at")
            .await
            .unwrap()
            .is_some());
    }
}
