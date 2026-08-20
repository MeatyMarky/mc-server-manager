//! Finding usable Java runtimes.
//!
//! Candidates come from PATH, `JAVA_HOME`, `CLASSPATH` entries, the standard
//! install roots for the platform, and the Windows registry. Every candidate is
//! then resolved by *running* it — folder names are never trusted. Results are
//! cached in `java_runtimes` keyed on the binary's mtime and size, so a rescan
//! only executes binaries that actually changed.

pub mod adoptium;
pub mod detect;
pub mod managed;
pub mod version;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use ts_rs::TS;

use crate::db::now_rfc3339;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

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
    /// 64 or 32, as the JVM reported it. `None` only for rows detected by a
    /// build that predates this column; those are re-probed on the next scan.
    #[ts(type = "number | null")]
    pub bits: Option<i64>,
    pub source: JavaSource,
    pub valid: bool,
    pub detected_at: String,
}

impl JavaRuntime {
    /// Whether this runtime may be chosen automatically.
    ///
    /// A 32-bit JVM tops out around 1.5 GB of heap and refuses to start with the
    /// `-Xmx` a server is normally given, so it is never picked on its own. It
    /// stays in the list — a user who deliberately pins one gets a warning, not
    /// a disappearance.
    pub fn usable_for_servers(&self) -> bool {
        self.valid && self.bits == Some(64)
    }

    /// Why this runtime is not offered, for the UI to show next to it.
    pub fn unsuitable_reason(&self) -> Option<&'static str> {
        match self.bits {
            Some(64) => None,
            Some(_) => Some("32-bit, not suitable for servers"),
            None => Some("width unknown until the next scan"),
        }
    }
}

pub async fn list(pool: &SqlitePool) -> AppResult<Vec<JavaRuntime>> {
    let rows = sqlx::query_as::<_, JavaRuntime>(
        "SELECT id, path, major, full_version, vendor, arch, bits, source, valid, detected_at
         FROM java_runtimes WHERE valid = 1 ORDER BY major DESC, path",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Best runtime for a required major version: the lowest major that still
/// satisfies the requirement, so a 1.16 server does not get Java 26.
pub async fn best_for(pool: &SqlitePool, required: i64) -> AppResult<Option<JavaRuntime>> {
    Ok(best_of(list(pool).await?, required))
}

/// The same choice, over a list a caller has already narrowed.
///
/// Pure, and separate from `best_for`, because "system Java only" has to drop
/// the managed runtimes *before* the pick rather than after it: the managed
/// Java 8 is the lowest major that satisfies a 1.16 server, so rejecting the
/// winner afterwards would answer "nothing suitable" while a perfectly good
/// system Java 17 sat behind it in the list.
pub fn best_of(mut runtimes: Vec<JavaRuntime>, required: i64) -> Option<JavaRuntime> {
    // 32-bit runtimes are excluded here rather than at launch: picking one
    // automatically produces "Invalid maximum heap size" from the JVM, which
    // tells the user nothing about which Java was used or why.
    runtimes.retain(|r| satisfies(r.major, required) && r.usable_for_servers());
    runtimes.sort_by_key(|r| r.major);
    runtimes.into_iter().next()
}

/// The feature version of the runtime at `path`, asked of the binary itself.
///
/// The cache is not consulted: a row can be stale, can predate an upgrade in
/// place, or — as happened here — can be wrong outright, and the whole point of
/// this call is to know what will really run a moment before it runs.
pub async fn probe_major(path: &std::path::Path) -> Option<i64> {
    let binary = path.to_path_buf();
    tokio::task::spawn_blocking(move || detect::probe(&binary, JavaSource::Manual))
        .await
        .ok()
        .and_then(|probe| probe.major)
}

/// The bitness of the runtime at `path`, from the cache or by asking it.
///
/// A pinned path may never have been through detection, and preflight still has
/// to know before it spawns anything.
pub async fn bits_of(pool: &SqlitePool, path: &std::path::Path) -> Option<i64> {
    let text = path.to_string_lossy().to_string();
    if let Ok(Some(bits)) =
        sqlx::query_scalar::<_, Option<i64>>("SELECT bits FROM java_runtimes WHERE path = ?")
            .bind(&text)
            .fetch_optional(pool)
            .await
            .map(|row| row.flatten())
    {
        return Some(bits);
    }

    let binary = path.to_path_buf();
    tokio::task::spawn_blocking(move || detect::probe(&binary, JavaSource::Manual))
        .await
        .ok()
        .and_then(|probe| probe.bits)
}

/// Where a launch's Java came from, for the log line and the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum Origin {
    /// The user chose this exact binary for this instance.
    Pinned,
    /// A JDK this app downloaded, shared by every instance needing that version.
    Managed,
    /// A JDK that was already on the machine.
    System,
}

/// The runtime a launch will use, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub path: std::path::PathBuf,
    pub origin: Origin,
    #[allow(dead_code)]
    pub major: Option<i64>,
}

/// Picks the runtime for an instance, in the order that respects the user's
/// choices before this app's own:
///
/// 1. the pin, when it is set and points at something;
/// 2. a managed runtime for the required version — this app downloaded it for
///    exactly this, and it cannot have been changed underneath;
/// 3. a system JDK that satisfies the requirement.
///
/// `None` means nothing suitable exists, which is the cue to offer a download.
pub async fn select_for(
    state: &AppState,
    pinned: Option<&str>,
    required: i64,
) -> AppResult<Option<Selection>> {
    if let Some(pinned) = pinned.filter(|path| std::path::Path::new(path).is_file()) {
        return Ok(Some(Selection {
            path: std::path::PathBuf::from(pinned),
            origin: Origin::Pinned,
            major: None,
        }));
    }

    if let Some(runtime) = managed::for_version(state, required).await? {
        return Ok(Some(Selection {
            path: std::path::PathBuf::from(runtime.java_path),
            origin: Origin::Managed,
            major: Some(runtime.feature_version),
        }));
    }

    // A managed runtime is in the detected list as well, so this is where the
    // "system Java only" setting has to be applied a second time: without it
    // the runtime this app downloaded comes back through the system route,
    // labelled as though the machine had it all along.
    let mut candidates = list(&state.db).await?;
    if managed::system_java_only(state).await {
        candidates.retain(|runtime| {
            !managed::is_managed_path(&state.data_dir, std::path::Path::new(&runtime.path))
        });
    }

    Ok(best_of(candidates, required).map(|runtime| Selection {
        path: std::path::PathBuf::from(runtime.path),
        origin: Origin::System,
        major: Some(runtime.major),
    }))
}

/// What this instance really needs, taking the recorded number and the version
/// table as a floor apiece.
///
/// The per-version JSON at install time is authoritative when it is *higher*:
/// Mojang knows about a requirement the table has not learned yet. It is not
/// authoritative when it is lower, because a wrong or stale row then silently
/// downgrades the requirement — a recorded 8 against a 26.2 server let Java 17
/// be chosen and the server refused its own class files.
pub fn required_for(recorded: Option<i64>, mc_version: &str) -> i64 {
    let from_table = required_java_for(mc_version);
    recorded.unwrap_or(from_table).max(from_table)
}

/// The setting holding when detection last ran.
pub const SCANNED_AT: &str = "java_scanned_at";
/// How long a scan is trusted before the next launch redoes it.
pub const CACHE_MAX_AGE_HOURS: i64 = 24;

/// When detection last ran, if it ever has.
pub async fn last_scan_at(pool: &SqlitePool) -> AppResult<Option<String>> {
    crate::db::setting_get(pool, SCANNED_AT).await
}

/// Whether the cached list is old enough to be worth rebuilding.
///
/// A JDK installed after the last scan is invisible until someone presses
/// Rescan, and "the app is choosing from a list that predates the thing I just
/// installed" is not a state a user can be expected to work out.
pub fn scan_is_stale(last: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    let Some(last) = last else {
        return true;
    };
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(last) else {
        // An unreadable timestamp is not a reason to trust the cache forever.
        return true;
    };
    now.signed_duration_since(parsed.with_timezone(&chrono::Utc))
        >= chrono::Duration::hours(CACHE_MAX_AGE_HOURS)
}

/// Throws the detected list away and builds it again from scratch.
///
/// The table is a cache, so this is always safe — it exists for the case where
/// the rows cannot be trusted at all, such as a database whose indexes SQLite
/// reports as damaged.
pub async fn rebuild_cache(pool: &SqlitePool) -> AppResult<Vec<JavaRuntime>> {
    sqlx::query("DELETE FROM java_runtimes").execute(pool).await?;
    rescan(pool).await
}

/// Rescans when the cache is stale, and says whether it did.
pub async fn rescan_if_stale(pool: &SqlitePool) -> AppResult<bool> {
    let last = last_scan_at(pool).await?;
    if !scan_is_stale(last.as_deref(), chrono::Utc::now()) {
        return Ok(false);
    }

    let found = rescan(pool).await?;
    tracing::info!(
        count = found.len(),
        previous_scan = last.as_deref().unwrap_or("never"),
        "Java cache was stale; rescanned"
    );
    Ok(true)
}

/// Runs detection and replaces the cache. Returns everything now known.
pub async fn rescan(pool: &SqlitePool) -> AppResult<Vec<JavaRuntime>> {
    let cached = sqlx::query_as::<_, detect::CachedProbe>(
        "SELECT path, mtime, size_bytes, major, full_version, vendor, arch, bits, valid
         FROM java_runtimes",
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
                (path, major, vendor, arch, bits, source, valid, detected_at, mtime, size_bytes,
                 full_version, error)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(path) DO UPDATE SET
                major = excluded.major, vendor = excluded.vendor, arch = excluded.arch,
                bits = excluded.bits, source = excluded.source, valid = excluded.valid,
                detected_at = excluded.detected_at, mtime = excluded.mtime,
                size_bytes = excluded.size_bytes, full_version = excluded.full_version,
                error = excluded.error",
        )
        .bind(&probe.path)
        .bind(probe.major.unwrap_or(0))
        .bind(&probe.vendor)
        .bind(&probe.arch)
        .bind(probe.bits)
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
    // from "detection has not run yet"; the two need different words. It is
    // also what `scan_is_stale` reads and what Settings shows.
    crate::db::setting_set(pool, SCANNED_AT, &now).await?;

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
            (path, major, vendor, arch, bits, source, valid, detected_at, mtime, size_bytes,
             full_version, error)
         VALUES (?, ?, ?, ?, ?, 'manual', 1, ?, ?, ?, ?, NULL)
         ON CONFLICT(path) DO UPDATE SET
            major = excluded.major, vendor = excluded.vendor, arch = excluded.arch,
            bits = excluded.bits, source = 'manual', valid = 1,
            detected_at = excluded.detected_at, mtime = excluded.mtime,
            size_bytes = excluded.size_bytes, full_version = excluded.full_version, error = NULL",
    )
    .bind(&probe.path)
    .bind(major)
    .bind(&probe.vendor)
    .bind(&probe.arch)
    .bind(probe.bits)
    .bind(&now)
    .bind(probe.mtime)
    .bind(probe.size_bytes)
    .bind(&probe.full_version)
    .execute(pool)
    .await?;

    sqlx::query_as::<_, JavaRuntime>(
        "SELECT id, path, major, full_version, vendor, arch, bits, source, valid, detected_at
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

    fn runtime(major: i64, path: &str, bits: Option<i64>) -> JavaRuntime {
        JavaRuntime {
            id: major,
            path: path.to_string(),
            major,
            full_version: None,
            vendor: None,
            arch: None,
            bits,
            source: JavaSource::Path,
            valid: true,
            detected_at: String::new(),
        }
    }

    #[test]
    fn the_lowest_runtime_that_satisfies_the_floor_wins() {
        let found = best_of(
            vec![
                runtime(26, "C:/jdk-26/bin/java.exe", Some(64)),
                runtime(17, "C:/jdk-17/bin/java.exe", Some(64)),
                runtime(21, "C:/jdk-21/bin/java.exe", Some(64)),
            ],
            8,
        );
        // A 1.16 server takes the oldest thing that can still run it.
        assert_eq!(found.map(|runtime| runtime.major), Some(17));
    }

    #[test]
    fn a_32_bit_runtime_is_never_the_answer_even_when_it_is_the_only_match() {
        let found = best_of(vec![runtime(8, "C:/Program Files (x86)/java.exe", Some(32))], 8);
        assert!(found.is_none(), "it cannot address the heap a server is given");

        // And an unknown width is treated the same until the next scan proves it.
        let unknown = best_of(vec![runtime(8, "C:/java.exe", None)], 8);
        assert!(unknown.is_none());
    }

    #[test]
    fn a_managed_runtime_is_recognised_by_where_it_lives() {
        let data_dir = std::path::Path::new("C:/Users/x/AppData/Roaming/dev.msm.manager");
        let managed = data_dir.join("runtimes/temurin-8/jdk8u502-b07/bin/java.exe");
        assert!(managed::is_managed_path(data_dir, &managed));

        // A system install is not, however similar the name looks.
        assert!(!managed::is_managed_path(
            data_dir,
            std::path::Path::new("C:/Program Files/Eclipse Adoptium/jdk-8/bin/java.exe")
        ));
    }

    async fn seed(pool: &SqlitePool, path: &str, major: i64) {
        seed_bits(pool, path, major, Some(64)).await;
    }

    async fn seed_bits(pool: &SqlitePool, path: &str, major: i64, bits: Option<i64>) {
        sqlx::query(
            "INSERT INTO java_runtimes (path, major, bits, source, valid, detected_at)
             VALUES (?, ?, ?, 'common_dir', 1, ?)",
        )
        .bind(path)
        .bind(major)
        .bind(bits)
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
    async fn a_32_bit_runtime_is_never_chosen_automatically() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        // The shape this machine actually has: Program Files (x86) Java 8.
        seed_bits(&pool, "C:/Program Files (x86)/Java/jre1.8.0_451/bin/java.exe", 8, Some(32)).await;

        assert!(
            best_for(&pool, 8).await.unwrap().is_none(),
            "a 32-bit JVM cannot hold a server heap, so it is not a candidate"
        );

        // It stays visible, with a reason, rather than vanishing from the list.
        let listed = list(&pool).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].usable_for_servers());
        assert_eq!(
            listed[0].unsuitable_reason(),
            Some("32-bit, not suitable for servers")
        );

        // A 64-bit runtime of the same version is picked instead.
        seed_bits(&pool, "C:/Program Files/Java/jdk-8/bin/java.exe", 8, Some(64)).await;
        let chosen = best_for(&pool, 8).await.unwrap().expect("the 64-bit one");
        assert!(chosen.path.contains("Program Files/Java"));
        assert!(!chosen.path.contains("(x86)"));
    }

    #[tokio::test]
    async fn a_runtime_of_unknown_width_is_not_chosen_either() {
        // Rows written before bitness was recorded. Assuming 64-bit is exactly
        // the assumption that produced "Invalid maximum heap size".
        let pool = crate::db::connect_in_memory().await.unwrap();
        seed_bits(&pool, "/jdk21/bin/java", 21, None).await;

        assert!(best_for(&pool, 21).await.unwrap().is_none());
        assert_eq!(
            list(&pool).await.unwrap()[0].unsuitable_reason(),
            Some("width unknown until the next scan")
        );
    }

    #[tokio::test]
    async fn bits_of_reads_the_cache_before_probing_anything() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        seed_bits(&pool, "/jdk21/bin/java", 21, Some(32)).await;

        assert_eq!(
            bits_of(&pool, std::path::Path::new("/jdk21/bin/java")).await,
            Some(32)
        );
        // An unknown path with no binary behind it answers nothing rather than
        // guessing, and preflight treats that as "not proven 32-bit".
        assert_eq!(
            bits_of(&pool, std::path::Path::new("/nowhere/bin/java")).await,
            None
        );
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

    #[test]
    fn a_cache_older_than_a_day_is_stale() {
        let now = chrono::Utc::now();
        let stamp = |hours: i64| (now - chrono::Duration::hours(hours)).to_rfc3339();

        assert!(!scan_is_stale(Some(&stamp(1)), now), "an hour old is current");
        assert!(!scan_is_stale(Some(&stamp(23)), now));
        assert!(scan_is_stale(Some(&stamp(24)), now), "a day old is rescanned");
        assert!(scan_is_stale(Some(&stamp(72)), now));

        // Never scanned, and a timestamp nothing can read, both mean "look now".
        assert!(scan_is_stale(None, now));
        assert!(scan_is_stale(Some("last tuesday"), now));
    }

    #[tokio::test]
    async fn a_stale_cache_is_rescanned_at_startup_and_a_current_one_is_not() {
        let pool = crate::db::connect_in_memory().await.unwrap();

        // Nothing recorded: the first launch scans.
        assert!(rescan_if_stale(&pool).await.unwrap());
        let first = last_scan_at(&pool).await.unwrap().expect("a timestamp");

        // Immediately afterwards there is nothing to redo.
        assert!(!rescan_if_stale(&pool).await.unwrap());
        assert_eq!(last_scan_at(&pool).await.unwrap().as_deref(), Some(first.as_str()));

        // A day later, the JDK installed in the meantime is worth looking for.
        let long_ago = (chrono::Utc::now() - chrono::Duration::hours(30)).to_rfc3339();
        crate::db::setting_set(&pool, SCANNED_AT, &long_ago).await.unwrap();

        assert!(rescan_if_stale(&pool).await.unwrap());
        let after = last_scan_at(&pool).await.unwrap().unwrap();
        assert_ne!(after, long_ago, "the stamp moved to now");
        // Second-resolution stamps can tie with the first scan of this test, so
        // the check is that the cache is current again, not that the text moved.
        assert!(!scan_is_stale(Some(&after), chrono::Utc::now()));
    }

    #[test]
    fn a_recorded_requirement_can_raise_the_table_but_never_lower_it() {
        // The bug this exists for: a 26.2 install recorded java_major = 8,
        // which let a Java 17 runtime be chosen for a server built for 25.
        assert_eq!(required_for(Some(8), "26.2"), 25);
        assert_eq!(required_for(None, "26.2"), 25);

        // Mojang knowing better than the table is exactly why the record exists.
        assert_eq!(required_for(Some(26), "26.2"), 26);
        assert_eq!(required_for(Some(21), "1.20.6"), 21);

        // Old versions keep their low requirement.
        assert_eq!(required_for(Some(8), "1.16.5"), 8);
        assert_eq!(required_for(None, "1.16.5"), 8);
        assert_eq!(required_for(Some(17), "1.16.5"), 17, "a pinned newer record wins");
    }

    #[tokio::test]
    async fn the_version_a_binary_reports_is_read_from_the_binary() {
        // A path that is not a JVM answers nothing rather than guessing.
        assert_eq!(
            probe_major(std::path::Path::new("/nowhere/bin/java")).await,
            None
        );
    }

    /// A file that exists, standing in for a java binary.
    fn touch(path: &std::path::Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"java").unwrap();
    }

    async fn managed_state(dir: &std::path::Path) -> AppState {
        let pool = crate::db::connect_in_memory().await.unwrap();
        AppState::new(pool, dir.to_path_buf())
    }

    async fn register_managed(state: &AppState, feature: i64, path: &std::path::Path) {
        touch(path);
        sqlx::query(
            "INSERT INTO managed_runtimes
                (feature_version, release_name, vendor, java_path, installed_at, size_bytes)
             VALUES (?, ?, 'Eclipse Temurin', ?, ?, 1)",
        )
        .bind(feature)
        .bind(format!("jdk-{feature}.0.1+9"))
        .bind(path.to_string_lossy().to_string())
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn selection_prefers_a_pin_then_a_managed_runtime_then_the_system() {
        let dir = tempfile::tempdir().unwrap();
        let state = managed_state(dir.path()).await;

        // Only a system JDK to start with.
        seed(&state.db, "/system/jdk-25/bin/java", 25).await;
        let system = select_for(&state, None, 25).await.unwrap().unwrap();
        assert_eq!(system.origin, Origin::System);
        assert_eq!(system.path, std::path::PathBuf::from("/system/jdk-25/bin/java"));

        // A managed runtime for the same version takes over: this app put it
        // there for exactly this, and nothing else can have changed it.
        let managed_path = managed::install_dir(dir.path(), 25).join("bin").join("java");
        register_managed(&state, 25, &managed_path).await;
        let chosen = select_for(&state, None, 25).await.unwrap().unwrap();
        assert_eq!(chosen.origin, Origin::Managed);
        assert_eq!(chosen.path, managed_path);

        // An explicit pin beats both, because the user asked for it.
        let pinned = dir.path().join("pinned").join("bin").join("java");
        touch(&pinned);
        let chosen = select_for(&state, Some(&pinned.to_string_lossy()), 25)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(chosen.origin, Origin::Pinned);
        assert_eq!(chosen.path, pinned);
    }

    #[tokio::test]
    async fn a_pin_that_no_longer_exists_does_not_silently_become_something_else() {
        let dir = tempfile::tempdir().unwrap();
        let state = managed_state(dir.path()).await;
        seed(&state.db, "/system/jdk-25/bin/java", 25).await;

        // preflight turns this into JavaPinnedMissing; selection simply does not
        // pretend the pin was honoured.
        let chosen = select_for(&state, Some("Z:/gone/bin/java"), 25)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(chosen.origin, Origin::Pinned);
    }

    #[tokio::test]
    async fn nothing_suitable_is_none_which_is_what_offers_a_download() {
        let dir = tempfile::tempdir().unwrap();
        let state = managed_state(dir.path()).await;

        // A machine with only Java 17 cannot run a server needing 25.
        seed(&state.db, "/system/jdk-17/bin/java", 17).await;
        assert!(select_for(&state, None, 25).await.unwrap().is_none());

        // A managed 21 does not satisfy 25 either.
        let managed_path = managed::install_dir(dir.path(), 21).join("bin").join("java");
        register_managed(&state, 21, &managed_path).await;
        assert!(select_for(&state, None, 25).await.unwrap().is_none());

        // But it does satisfy 17.
        let chosen = select_for(&state, None, 17).await.unwrap().unwrap();
        assert_eq!(chosen.origin, Origin::Managed);
    }

    #[tokio::test]
    async fn a_managed_runtime_whose_files_are_gone_falls_back_to_the_system() {
        let dir = tempfile::tempdir().unwrap();
        let state = managed_state(dir.path()).await;
        seed(&state.db, "/system/jdk-25/bin/java", 25).await;

        sqlx::query(
            "INSERT INTO managed_runtimes
                (feature_version, release_name, vendor, java_path, installed_at, size_bytes)
             VALUES (25, 'jdk-25.0.1+9', 'Eclipse Temurin', 'Z:/deleted/bin/java', ?, 1)",
        )
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();

        let chosen = select_for(&state, None, 25).await.unwrap().unwrap();
        assert_eq!(chosen.origin, Origin::System, "a deleted folder is not a runtime");
    }
}
