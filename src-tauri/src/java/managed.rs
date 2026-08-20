//! JDKs this app downloads and owns.
//!
//! One runtime per feature version, shared by every instance that needs it:
//! `<data>/runtimes/temurin-25/`. Instances never own a copy, so a second
//! server needing Java 25 costs nothing.
//!
//! The install is atomic in the way that matters: the archive is verified
//! before it is opened, unpacked into a sibling temp folder, and only then
//! renamed into place. A folder under `runtimes/` therefore never holds a
//! half-extracted JDK, whatever happens mid-install.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::db::now_rfc3339;
use crate::error::{AppError, AppResult, IoContext};
use crate::state::AppState;

use super::adoptium::Candidate;

/// Setting that keeps this app from downloading anything.
pub const SYSTEM_ONLY_SETTING: &str = "use_system_java_only";

/// A runtime this app downloaded, as the UI lists it.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct ManagedRuntime {
    #[ts(type = "number")]
    pub feature_version: i64,
    /// `jdk-25.0.4+7`.
    pub release_name: String,
    pub vendor: String,
    /// Absolute path to the `java` binary inside the install.
    pub java_path: String,
    pub installed_at: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    /// Names of instances that would break if this were removed.
    pub used_by: Vec<String>,
}

/// Where every managed runtime lives.
pub fn runtimes_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("runtimes")
}

/// The folder for one feature version. Keyed by version, never by instance.
pub fn install_dir(data_dir: &Path, feature_version: i64) -> PathBuf {
    runtimes_dir(data_dir).join(format!("temurin-{feature_version}"))
}

/// Finds the `java` binary inside an extracted JDK.
///
/// Adoptium archives contain a single top-level folder (`jdk-25.0.4+7/`), and
/// macOS puts the runtime another two levels down inside a bundle.
pub fn java_binary_within(root: &Path) -> Option<PathBuf> {
    let name = super::detect::java_executable_name();
    let direct = root.join("bin").join(name);
    if direct.is_file() {
        return Some(direct);
    }

    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        for candidate in [
            path.join("bin").join(name),
            path.join("Contents").join("Home").join("bin").join(name),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether the user has asked for system Java only.
///
/// The setting says "use only the Java installed on this computer", and that is
/// the whole of what it means: no downloads, and the runtimes this app already
/// downloaded stop being an answer. A switch that quietly kept using them would
/// be answering a different question from the one it asks.
pub async fn system_java_only(state: &AppState) -> bool {
    matches!(
        crate::db::setting_get(&state.db, SYSTEM_ONLY_SETTING)
            .await
            .ok()
            .flatten()
            .as_deref(),
        Some("true")
    )
}

/// Whether this app may download runtimes at all.
pub async fn downloads_allowed(state: &AppState) -> bool {
    !system_java_only(state).await
}

/// Every runtime this app has downloaded, newest feature version first.
pub async fn list(state: &AppState) -> AppResult<Vec<ManagedRuntime>> {
    let rows: Vec<(i64, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT feature_version, release_name, vendor, java_path, installed_at, size_bytes
         FROM managed_runtimes ORDER BY feature_version DESC",
    )
    .fetch_all(&state.db)
    .await?;

    let mut out = Vec::new();
    for (feature_version, release_name, vendor, java_path, installed_at, size_bytes) in rows {
        out.push(ManagedRuntime {
            used_by: users_of(state, feature_version, &java_path).await?,
            feature_version,
            release_name,
            vendor,
            java_path,
            installed_at,
            size_bytes,
        });
    }
    Ok(out)
}

/// Whether a path is inside the folder this app downloads runtimes into.
///
/// A managed runtime is also registered in the detected list, so the pin
/// dropdown can offer it — which means "system Java only" has to recognise it
/// there too, or the setting is honoured in one code path and quietly ignored
/// in the other.
pub fn is_managed_path(data_dir: &Path, java_path: &Path) -> bool {
    java_path.starts_with(runtimes_dir(data_dir))
}

/// The managed runtime for a feature version, if one is installed, its binary
/// is still on disk, and the user has not asked for system Java only.
///
/// The setting is read here rather than at each call site, so selection,
/// preflight and the create dialog's plan cannot disagree about it.
pub async fn for_version(
    state: &AppState,
    required: i64,
    fit: super::JavaFit,
) -> AppResult<Option<ManagedRuntime>> {
    if system_java_only(state).await {
        return Ok(None);
    }

    Ok(list(state)
        .await?
        .into_iter()
        .filter(|runtime| fit.accepts(runtime.feature_version, required))
        .find(|runtime| Path::new(&runtime.java_path).is_file()))
}

/// Instances that would lose their Java if this runtime went away: those pinned
/// to its binary, and those whose requirement only it satisfies.
async fn users_of(state: &AppState, feature_version: i64, java_path: &str) -> AppResult<Vec<String>> {
    let rows: Vec<(String, Option<String>, Option<i64>, String)> =
        sqlx::query_as("SELECT name, java_path, java_major, mc_version FROM instances")
            .fetch_all(&state.db)
            .await?;

    Ok(rows
        .into_iter()
        .filter(|(_, pinned, recorded, mc_version)| {
            pinned.as_deref() == Some(java_path)
                || super::required_for(*recorded, mc_version) == feature_version
        })
        .map(|(name, _, _, _)| name)
        .collect())
}

/// Total bytes the managed runtimes occupy.
pub async fn total_size(state: &AppState) -> AppResult<i64> {
    Ok(sqlx::query_scalar::<_, Option<i64>>("SELECT SUM(size_bytes) FROM managed_runtimes")
        .fetch_one(&state.db)
        .await?
        .unwrap_or(0))
}

/// Downloads, verifies, unpacks and registers a JDK.
pub async fn install<P>(
    state: &AppState,
    candidate: &Candidate,
    cancel: &CancellationToken,
    mut report: P,
) -> AppResult<ManagedRuntime>
where
    P: FnMut(crate::download::Progress) + Send,
{
    let artifact = candidate.artifact();
    let archive = crate::instance::install::cache_path(&state.data_dir, &artifact);

    // The Phase 2 engine: resumable, cancellable, checksum-verified, cached.
    crate::download::download(&state.http, &artifact, &archive, cancel, &mut report).await?;

    let target = install_dir(&state.data_dir, candidate.feature_version);
    let staging = runtimes_dir(&state.data_dir).join(format!(
        ".staging-{}-{}",
        candidate.feature_version,
        uuid::Uuid::new_v4()
    ));

    let unpack_from = archive.clone();
    let unpack_to = staging.clone();
    let token = cancel.clone();
    // Unpacking a 300 MB archive is thousands of blocking writes.
    let extracted = tokio::task::spawn_blocking(move || extract(&unpack_from, &unpack_to, &token))
        .await
        .map_err(|e| AppError::internal("unpacking the JDK", e))?;

    if let Err(err) = extracted {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(err);
    }

    let Some(binary) = java_binary_within(&staging) else {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(AppError::Other(format!(
            "the downloaded {} archive did not contain a java binary",
            candidate.release_name
        )));
    };
    let relative = binary
        .strip_prefix(&staging)
        .unwrap_or(&binary)
        .to_path_buf();

    // Replace any previous install of this version, then rename into place. The
    // final folder only ever appears complete.
    if target.exists() {
        let _ = tokio::fs::remove_dir_all(&target).await;
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .ctx("create the runtimes folder", parent)?;
    }
    tokio::fs::rename(&staging, &target)
        .await
        .ctx("move the unpacked JDK into place", &target)?;

    let java_path = target.join(relative);
    let size_bytes = dir_size(&target);

    // What it really is, read from the binary rather than from the API.
    let probed = super::probe_major(&java_path).await;
    if let Some(major) = probed {
        if !super::satisfies(major, candidate.feature_version) {
            return Err(AppError::Other(format!(
                "the downloaded JDK reports Java {major}, not {}",
                candidate.feature_version
            )));
        }
    }

    sqlx::query(
        "INSERT INTO managed_runtimes
            (feature_version, release_name, vendor, java_path, installed_at, size_bytes)
         VALUES (?, ?, 'Eclipse Temurin', ?, ?, ?)
         ON CONFLICT(feature_version) DO UPDATE SET
            release_name = excluded.release_name, java_path = excluded.java_path,
            installed_at = excluded.installed_at, size_bytes = excluded.size_bytes",
    )
    .bind(candidate.feature_version)
    .bind(&candidate.release_name)
    .bind(java_path.to_string_lossy().to_string())
    .bind(now_rfc3339())
    .bind(size_bytes)
    .execute(&state.db)
    .await?;

    // Registered as a runtime like any other, so every existing path — the
    // picker, `best_for`, the problem report — sees it without special cases.
    let _ = super::add_manual(&state.db, &java_path.to_string_lossy()).await;

    Ok(ManagedRuntime {
        feature_version: candidate.feature_version,
        release_name: candidate.release_name.clone(),
        vendor: "Eclipse Temurin".into(),
        java_path: java_path.to_string_lossy().to_string(),
        installed_at: now_rfc3339(),
        size_bytes,
        used_by: users_of(
            state,
            candidate.feature_version,
            &java_path.to_string_lossy(),
        )
        .await?,
    })
}

/// Removes a managed runtime, refusing while an instance depends on it.
pub async fn remove(state: &AppState, feature_version: i64) -> AppResult<()> {
    let Some(runtime) = list(state)
        .await?
        .into_iter()
        .find(|runtime| runtime.feature_version == feature_version)
    else {
        return Err(AppError::Other(format!(
            "no managed Java {feature_version} is installed"
        )));
    };

    if !runtime.used_by.is_empty() {
        return Err(AppError::Other(format!(
            "Java {feature_version} is what {} would run on. Remove or repoint {} first.",
            runtime.used_by.join(", "),
            if runtime.used_by.len() == 1 {
                "that server"
            } else {
                "those servers"
            }
        )));
    }

    let dir = install_dir(&state.data_dir, feature_version);
    if dir.is_dir() {
        tokio::fs::remove_dir_all(&dir)
            .await
            .ctx("delete the managed runtime", &dir)?;
    }

    sqlx::query("DELETE FROM managed_runtimes WHERE feature_version = ?")
        .bind(feature_version)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM java_runtimes WHERE path = ?")
        .bind(&runtime.java_path)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Unpacks a `.zip` or `.tar.gz` into `target`.
fn extract(archive: &Path, target: &Path, cancel: &CancellationToken) -> AppResult<()> {
    std::fs::create_dir_all(target).ctx("create the staging folder", target)?;
    let name = archive.to_string_lossy().to_ascii_lowercase();

    if name.ends_with(".zip") {
        let file = std::fs::File::open(archive).ctx("open the JDK archive", archive)?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| AppError::archive(archive.display(), e))?;
        for index in 0..zip.len() {
            if cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            let mut entry = zip
                .by_index(index)
                .map_err(|e| AppError::archive(archive.display(), e))?;
            // An archive naming ../.. must never write outside the target.
            let Some(relative) =
                crate::worlds::archive::safe_entry_path(entry.name(), None)
            else {
                continue;
            };
            let out = target.join(relative);
            if entry.is_dir() {
                std::fs::create_dir_all(&out).ctx("unpack the JDK", &out)?;
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).ctx("unpack the JDK", parent)?;
            }
            let mut writer = std::fs::File::create(&out).ctx("unpack the JDK", &out)?;
            std::io::copy(&mut entry, &mut writer).ctx("unpack the JDK", &out)?;
        }
        return Ok(());
    }

    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let file = std::fs::File::open(archive).ctx("open the JDK archive", archive)?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(decoder);
        tar.set_preserve_permissions(true);
        for entry in tar
            .entries()
            .map_err(|e| AppError::archive(archive.display(), e))?
        {
            if cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            let mut entry = entry.map_err(|e| AppError::archive(archive.display(), e))?;
            let path = entry
                .path()
                .map_err(|e| AppError::archive(archive.display(), e))?
                .to_string_lossy()
                .to_string();
            let Some(relative) = crate::worlds::archive::safe_entry_path(&path, None) else {
                continue;
            };
            entry
                .unpack(target.join(relative))
                .ctx("unpack the JDK", target)?;
        }
        return Ok(());
    }

    Err(AppError::Other(format!(
        "{} is not an archive this app can unpack",
        archive.display()
    )))
}

/// Bytes on disk, walked once at install time.
fn dir_size(dir: &Path) -> i64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| meta.len() as i64)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn state_in(dir: &Path) -> AppState {
        let pool = crate::db::connect_in_memory().await.unwrap();
        AppState::new(pool, dir.to_path_buf())
    }

    async fn add_instance(state: &AppState, name: &str, mc_version: &str, pinned: Option<&str>) {
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                java_path, jvm_args, server_args, created_at, updated_at)
             VALUES (?, ?, ?, 'fabric', ?, 'jar', ?, '[]', '[]', ?, ?)",
        )
        .bind(format!("uuid-{name}"))
        .bind(name)
        .bind(format!("Z:/{name}"))
        .bind(mc_version)
        .bind(pinned)
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();
    }

    async fn register(state: &AppState, feature: i64, java_path: &str, size: i64) {
        sqlx::query(
            "INSERT INTO managed_runtimes
                (feature_version, release_name, vendor, java_path, installed_at, size_bytes)
             VALUES (?, ?, 'Eclipse Temurin', ?, ?, ?)",
        )
        .bind(feature)
        .bind(format!("jdk-{feature}.0.1+9"))
        .bind(java_path)
        .bind(now_rfc3339())
        .bind(size)
        .execute(&state.db)
        .await
        .unwrap();
    }

    #[test]
    fn runtimes_are_keyed_by_version_and_shared() {
        let data = Path::new("/data");
        assert_eq!(
            install_dir(data, 25),
            Path::new("/data").join("runtimes").join("temurin-25")
        );
        // Two instances needing 25 resolve to the same folder: one download.
        assert_eq!(install_dir(data, 25), install_dir(data, 25));
        assert_ne!(install_dir(data, 25), install_dir(data, 21));
    }

    #[test]
    fn the_java_binary_is_found_inside_an_adoptium_layout() {
        let dir = tempfile::tempdir().unwrap();
        let name = crate::java::detect::java_executable_name();

        // Adoptium archives wrap everything in one top-level folder.
        let inner = dir.path().join("jdk-25.0.4+7").join("bin");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join(name), b"java").unwrap();

        let found = java_binary_within(dir.path()).expect("found the binary");
        assert!(found.ends_with(Path::new("bin").join(name)));

        // An archive with nothing in it is not silently accepted.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(java_binary_within(empty.path()), None);
    }

    #[tokio::test]
    async fn a_runtime_in_use_is_refused_and_an_unused_one_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(dir.path()).await;

        let java_25 = install_dir(dir.path(), 25).join("bin").join("java");
        std::fs::create_dir_all(java_25.parent().unwrap()).unwrap();
        std::fs::write(&java_25, b"java").unwrap();
        register(&state, 25, &java_25.to_string_lossy(), 300_000_000).await;
        register(&state, 21, "Z:/runtimes/temurin-21/bin/java", 200_000_000).await;

        // A 26.2 server needs Java 25, so that runtime is spoken for.
        add_instance(&state, "idk", "26.2", None).await;

        let listed = list(&state).await.unwrap();
        assert_eq!(listed[0].feature_version, 25, "newest first");
        assert_eq!(listed[0].used_by, vec!["idk".to_string()]);
        assert!(listed[1].used_by.is_empty());
        assert_eq!(total_size(&state).await.unwrap(), 500_000_000);

        let err = remove(&state, 25).await.unwrap_err();
        assert!(err.to_string().contains("idk"), "{err}");
        assert!(java_25.is_file(), "nothing was deleted");

        // The unused one goes, files and row together.
        remove(&state, 21).await.unwrap();
        assert_eq!(list(&state).await.unwrap().len(), 1);
        assert!(remove(&state, 21).await.is_err(), "already gone");
    }

    #[tokio::test]
    async fn a_pinned_instance_also_counts_as_a_user() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(dir.path()).await;
        let java_17 = "Z:/runtimes/temurin-17/bin/java";
        register(&state, 17, java_17, 1).await;

        // Its Minecraft version needs 8, but it is pinned to the managed 17.
        add_instance(&state, "legacy", "1.16.5", Some(java_17)).await;

        let listed = list(&state).await.unwrap();
        assert_eq!(listed[0].used_by, vec!["legacy".to_string()]);
        assert!(remove(&state, 17).await.is_err());
    }

    #[tokio::test]
    async fn a_missing_binary_means_the_runtime_is_not_offered() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(dir.path()).await;
        register(&state, 25, "Z:/gone/bin/java", 1).await;

        // The row survives so the UI can show it, but nothing will be launched
        // with a binary that is not there.
        assert_eq!(list(&state).await.unwrap().len(), 1);
        assert!(for_version(&state, 25, crate::java::JavaFit::Floor).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn downloads_can_be_switched_off_entirely() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(dir.path()).await;
        assert!(downloads_allowed(&state).await, "on by default");

        crate::db::setting_set(&state.db, SYSTEM_ONLY_SETTING, "true")
            .await
            .unwrap();
        assert!(!downloads_allowed(&state).await);

        crate::db::setting_set(&state.db, SYSTEM_ONLY_SETTING, "false")
            .await
            .unwrap();
        assert!(downloads_allowed(&state).await);
    }

    #[test]
    fn extraction_refuses_an_archive_that_writes_outside_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("evil.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("../escaped.txt", options).unwrap();
            use std::io::Write;
            zip.write_all(b"nope").unwrap();
            // The name the platform actually looks for, so the check below is
            // the same one an install does.
            let binary = format!("jdk-25/bin/{}", crate::java::detect::java_executable_name());
            zip.start_file(&binary, options).unwrap();
            zip.write_all(b"java").unwrap();
            zip.finish().unwrap();
        }

        let target = dir.path().join("staging");
        extract(&archive, &target, &CancellationToken::new()).unwrap();

        assert!(!dir.path().join("escaped.txt").exists(), "traversal blocked");
        assert!(target
            .join("jdk-25")
            .join("bin")
            .join(crate::java::detect::java_executable_name())
            .is_file());
        assert!(java_binary_within(&target).is_some());
    }

    #[test]
    fn a_cancelled_extraction_stops_and_an_unknown_format_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("jdk.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file(
                format!("jdk-25/bin/{}", crate::java::detect::java_executable_name()),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            use std::io::Write;
            zip.write_all(b"java").unwrap();
            zip.finish().unwrap();
        }

        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = extract(&archive, &dir.path().join("staging"), &cancel).unwrap_err();
        assert_eq!(err.kind(), "cancelled");

        let odd = dir.path().join("jdk.7z");
        std::fs::write(&odd, b"nope").unwrap();
        assert!(extract(&odd, &dir.path().join("s2"), &CancellationToken::new()).is_err());
    }
}
