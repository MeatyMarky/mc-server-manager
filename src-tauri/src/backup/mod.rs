//! Creating, listing, restoring and pruning backups.

pub mod archive;
pub mod runner;
pub mod saveguard;
pub mod schedule;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::db::{now_rfc3339, record_event};
use crate::error::{AppError, AppResult, IoContext};
use crate::instance;
use crate::state::AppState;

pub use archive::{ArchiveEntry, Estimate, Format, Scope};

/// Headroom required over the uncompressed estimate before a backup starts.
pub const FREE_SPACE_FACTOR: f64 = 1.2;

#[derive(Debug, Clone, Serialize, sqlx::FromRow, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Backup {
    #[ts(type = "number")]
    pub id: i64,
    #[ts(type = "number")]
    pub instance_id: i64,
    pub path: String,
    pub format: Format,
    pub scope: Scope,
    /// manual | scheduled | pre_restore
    pub kind: String,
    pub label: Option<String>,
    #[ts(type = "number")]
    pub size_bytes: i64,
    pub created_at: String,
}

/// Options for one backup run.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct BackupOptions {
    pub format: Format,
    pub scope: Scope,
    #[ts(type = "number | null")]
    pub level: Option<i32>,
    pub label: Option<String>,
    /// Extra paths or `*.ext` patterns to leave out.
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl Default for BackupOptions {
    fn default() -> Self {
        Self {
            // tar.zst by default: faster to write and smaller than zip.
            format: Format::TarZst,
            scope: Scope::Full,
            level: None,
            label: None,
            exclude: Vec::new(),
        }
    }
}

/// What a backup would cost, checked before it starts.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct SpaceCheck {
    pub estimate: Estimate,
    #[ts(type = "number")]
    pub required_bytes: u64,
    #[ts(type = "number | null")]
    pub free_bytes: Option<u64>,
    pub sufficient: bool,
    /// Set when there is not enough room, naming the shortfall.
    pub message: Option<String>,
}

/// Where an instance's backups live.
pub fn backup_dir(data_dir: &Path, instance_uuid: &str) -> PathBuf {
    data_dir.join("backups").join(instance_uuid)
}

/// A name no existing archive in `dir` already has.
///
/// Timestamps are only second-resolution, and a restore takes its safety backup
/// in the same second as it reads the archive it is restoring — without this,
/// the second write silently replaces the first.
pub fn unique_path(dir: &Path, instance_name: &str, format: Format, now: &str) -> PathBuf {
    let base = file_name(instance_name, format, now);
    let first = dir.join(&base);
    if !first.exists() {
        return first;
    }

    let stem = base.trim_end_matches(&format!(".{}", format.extension()));
    for suffix in 2..1000 {
        let candidate = dir.join(format!("{stem}-{suffix}.{}", format.extension()));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-{}.{}", uuid::Uuid::new_v4(), format.extension()))
}

/// `survival-2026-08-18-143000.tar.zst`
pub fn file_name(instance_name: &str, format: Format, now: &str) -> String {
    let stamp = now
        .replace(['-', ':'], "")
        .replace('T', "-")
        .replace('Z', "");
    let slug: String = instance_name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    format!(
        "{}-{stamp}.{}",
        slug.trim_matches('-').to_ascii_lowercase(),
        format.extension()
    )
}

/// Free space on the volume holding `path`, when the platform reports it.
pub fn free_space(path: &Path) -> Option<u64> {
    use sysinfo::Disks;

    let disks = Disks::new_with_refreshed_list();
    let target = crate::paths::normalize(path);

    // The disk with the longest matching mount point is the one this path is on.
    disks
        .list()
        .iter()
        .filter(|disk| target.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| disk.available_space())
}

/// Decides whether there is room, and says by how much when there is not.
pub fn check_space(estimate: Estimate, free: Option<u64>) -> SpaceCheck {
    let required = (estimate.bytes as f64 * FREE_SPACE_FACTOR) as u64;
    let sufficient = free.map(|free| free >= required).unwrap_or(true);

    let message = (!sufficient).then(|| {
        let free = free.unwrap_or(0);
        format!(
            "this backup needs about {} of free space ({} of data plus 20% headroom) \
             but only {} is free: {} short",
            human(required),
            human(estimate.bytes),
            human(free),
            human(required.saturating_sub(free))
        )
    });

    SpaceCheck {
        estimate,
        required_bytes: required,
        free_bytes: free,
        sufficient,
        message,
    }
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Measures an instance and checks there is room to back it up.
pub async fn plan(state: &AppState, id: i64, options: &BackupOptions) -> AppResult<SpaceCheck> {
    let row = instance::get(&state.db, id).await?;
    let dir = row.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: row.name,
            path: dir,
        });
    }

    let worlds = world_names(&dir).await;
    let scope = options.scope;
    let exclude = options.exclude.clone();
    let measure_dir = dir.clone();

    let estimate = tokio::task::spawn_blocking(move || {
        archive::estimate(&measure_dir, scope, &worlds, &exclude)
    })
    .await
    .map_err(|e| AppError::internal("measuring the instance", e))??;

    let target_dir = backup_dir(&state.data_dir, &row.uuid);
    let free = tokio::task::spawn_blocking(move || free_space(&target_dir))
        .await
        .unwrap_or(None);

    Ok(check_space(estimate, free))
}

async fn world_names(dir: &Path) -> Vec<String> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || crate::instance::crud::world_dir_names(&dir))
        .await
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes: u64,
}

/// Creates a backup.
///
/// When the server is running, saving is suspended first and **always** resumed,
/// whatever happens in between — that is the whole reason this function owns the
/// sequence rather than leaving it to callers.
pub async fn create<P>(
    state: &AppState,
    id: i64,
    options: BackupOptions,
    kind: &str,
    schedule_id: Option<i64>,
    cancel: &CancellationToken,
    mut report: P,
) -> AppResult<Backup>
where
    P: FnMut(Progress) + Send,
{
    let row = instance::get(&state.db, id).await?;
    let dir = row.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: row.name,
            path: dir,
        });
    }

    let space = plan(state, id, &options).await?;
    if let Some(message) = space.message {
        return Err(AppError::Other(message));
    }

    let live = state.status_of(&row.uuid).is_live();
    let running = state.supervisor.is_running(&row.uuid);
    if live && !running {
        return Err(AppError::Other(format!(
            "\"{}\" is running but its console is not owned by this app, so world saving \
             cannot be paused for a backup. Stop it, or start it from here, and try again.",
            row.name
        )));
    }

    let mut suspend_error = None;
    if running {
        // A failure here still leaves saving off, so the resume below must run.
        if let Err(err) = saveguard::suspend(state, id, &row.uuid).await {
            suspend_error = Some(err);
        }
    }

    let outcome = match suspend_error {
        Some(err) => Err(err),
        None => write_archive(state, &row, &dir, &options, cancel, &mut report).await,
    };

    // The finally block: whatever happened above, saving goes back on.
    if running {
        saveguard::resume(state, id).await;
    }

    let (path, size) = outcome?;
    let now = now_rfc3339();
    let level = options
        .level
        .unwrap_or_else(|| options.format.default_level());

    let backup_id: i64 = sqlx::query_scalar(
        "INSERT INTO backups (instance_id, path, format, scope, kind, label, size_bytes,
            created_at, compression_level, schedule_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         RETURNING id",
    )
    .bind(id)
    .bind(path.to_string_lossy().to_string())
    .bind(options.format)
    .bind(options.scope)
    .bind(kind)
    .bind(&options.label)
    .bind(size as i64)
    .bind(&now)
    .bind(level)
    .bind(schedule_id)
    .fetch_one(&state.db)
    .await?;

    record_event(
        &state.db,
        id,
        "backup",
        Some(&format!(
            "{kind} backup: {} ({})",
            path.file_name().unwrap_or_default().to_string_lossy(),
            human(size)
        )),
    )
    .await?;

    get(state, backup_id).await
}

async fn write_archive<P>(
    state: &AppState,
    row: &crate::db::models::Instance,
    dir: &Path,
    options: &BackupOptions,
    cancel: &CancellationToken,
    report: &mut P,
) -> AppResult<(PathBuf, u64)>
where
    P: FnMut(Progress) + Send,
{
    let worlds = world_names(dir).await;
    let backups = backup_dir(&state.data_dir, &row.uuid);
    tokio::fs::create_dir_all(&backups)
        .await
        .ctx("create the backup folder", &backups)?;
    let target = unique_path(&backups, &row.name, options.format, &now_rfc3339());

    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();
    let format = options.format;
    let spec = archive::Spec {
        instance_dir: dir.to_path_buf(),
        target: target.clone(),
        format,
        level: format.clamp_level(options.level.unwrap_or_else(|| format.default_level())),
        scope: options.scope,
        worlds,
        extra: options.exclude.clone(),
    };
    let cancel = cancel.clone();

    let handle = tokio::task::spawn_blocking(move || {
        archive::write(&spec, &cancel, |progress| {
            let _ = progress_tx.send(Progress {
                files_done: progress.files_done,
                files_total: progress.files_total,
                bytes: progress.bytes_read,
            });
        })
    });

    while let Some(progress) = progress_rx.recv().await {
        report(progress);
    }

    let size = handle
        .await
        .map_err(|e| AppError::internal("the backup", e))??;
    Ok((target, size))
}

pub async fn list(state: &AppState, id: i64) -> AppResult<Vec<Backup>> {
    let rows = sqlx::query_as::<_, Backup>(
        "SELECT id, instance_id, path, format, scope, kind, label, size_bytes, created_at
         FROM backups WHERE instance_id = ? ORDER BY created_at DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows)
}

pub async fn get(state: &AppState, backup_id: i64) -> AppResult<Backup> {
    sqlx::query_as::<_, Backup>(
        "SELECT id, instance_id, path, format, scope, kind, label, size_bytes, created_at
         FROM backups WHERE id = ?",
    )
    .bind(backup_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::Other(format!("no backup with id {backup_id}")))
}

/// Deletes a backup and its file.
pub async fn delete(state: &AppState, backup_id: i64) -> AppResult<()> {
    let backup = get(state, backup_id).await?;
    let path = PathBuf::from(&backup.path);
    if path.is_file() {
        tokio::fs::remove_file(&path)
            .await
            .ctx("delete the archive", &path)?;
    }
    sqlx::query("DELETE FROM backups WHERE id = ?")
        .bind(backup_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// What an archive holds, for the restore preview.
pub async fn preview(state: &AppState, backup_id: i64) -> AppResult<Vec<ArchiveEntry>> {
    let backup = get(state, backup_id).await?;
    let path = PathBuf::from(backup.path);
    tokio::task::spawn_blocking(move || archive::list(&path))
        .await
        .map_err(|e| AppError::internal("reading the archive", e))?
}

/// Restores a backup over the instance.
///
/// Refused while the server runs, and the current state is backed up first: a
/// restore that turns out to be the wrong archive must not be the end of the
/// story.
pub async fn restore<P>(
    state: &AppState,
    backup_id: i64,
    cancel: &CancellationToken,
    mut report: P,
) -> AppResult<()>
where
    P: FnMut(Progress) + Send,
{
    let backup = get(state, backup_id).await?;
    let instance_id = backup.instance_id;

    let row = instance::get(&state.db, instance_id).await?;
    if state.status_of(&row.uuid).is_live() {
        return Err(AppError::InstanceRunning(row.name));
    }

    let archive_path = PathBuf::from(&backup.path);
    if !archive_path.is_file() {
        return Err(AppError::Other(format!(
            "the archive {} is missing",
            backup.path
        )));
    }

    // Safety net, taken before anything is overwritten.
    create(
        state,
        instance_id,
        BackupOptions {
            label: Some(format!("before restoring {}", backup.created_at)),
            ..BackupOptions::default()
        },
        "pre_restore",
        None,
        cancel,
        |_| {},
    )
    .await?;

    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();
    let destination = row.path_buf();
    let cancel_clone = cancel.clone();

    let handle = tokio::task::spawn_blocking(move || {
        archive::extract(&archive_path, &destination, &cancel_clone, |progress| {
            let _ = progress_tx.send(Progress {
                files_done: progress.files_done,
                files_total: progress.files_total,
                bytes: progress.bytes_read,
            });
        })
    });

    while let Some(progress) = progress_rx.recv().await {
        report(progress);
    }
    handle
        .await
        .map_err(|e| AppError::internal("the restore", e))??;

    record_event(
        &state.db,
        instance_id,
        "restore",
        Some(&format!("restored {}", backup.created_at)),
    )
    .await?;
    Ok(())
}

/// Applies retention to one instance, deleting the archives that fall outside
/// both limits. Returns how many were removed.
pub async fn prune(
    state: &AppState,
    instance_id: i64,
    keep_count: Option<i64>,
    keep_days: Option<i64>,
) -> AppResult<usize> {
    let backups = list(state, instance_id).await?;
    let doomed = schedule::select_for_pruning(&backups, keep_count, keep_days, chrono::Utc::now());

    for backup_id in &doomed {
        if let Err(err) = delete(state, *backup_id).await {
            tracing::warn!(error = %err, backup = backup_id, "could not delete an old backup");
        }
    }
    Ok(doomed.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_are_readable_and_sortable() {
        let name = file_name("Survival 1.21", Format::TarZst, "2026-08-18T14:30:00Z");
        assert_eq!(name, "survival-1-21-20260818-143000.tar.zst");
        assert!(file_name("Creative", Format::Zip, "2026-01-02T03:04:05Z").ends_with(".zip"));
    }

    #[test]
    fn backups_live_under_the_instance_uuid() {
        let path = backup_dir(Path::new("data"), "abc-123");
        assert!(path.ends_with(Path::new("backups").join("abc-123")));
    }

    #[test]
    fn the_space_check_demands_headroom_and_names_the_shortfall() {
        let estimate = Estimate {
            files: 100,
            bytes: 1_000_000_000,
        };

        // Exactly the estimate is not enough: 20% headroom is required.
        let tight = check_space(estimate, Some(1_000_000_000));
        assert!(!tight.sufficient);
        let message = tight.message.unwrap();
        assert!(message.contains("short"), "{message}");
        assert!(message.contains("20%"), "{message}");

        let roomy = check_space(estimate, Some(2_000_000_000));
        assert!(roomy.sufficient);
        assert!(roomy.message.is_none());
        assert_eq!(roomy.required_bytes, 1_200_000_000);
    }

    #[test]
    fn an_unknown_free_space_does_not_block_a_backup() {
        let check = check_space(
            Estimate {
                files: 1,
                bytes: 10,
            },
            None,
        );
        assert!(check.sufficient, "a platform that will not say must not stop a backup");
    }

    #[test]
    fn sizes_read_as_something_a_person_can_act_on() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn free_space_is_reported_for_a_real_path() {
        let dir = tempfile::tempdir().unwrap();
        let free = free_space(dir.path());
        assert!(free.is_some(), "this platform reports free space");
        assert!(free.unwrap() > 0);
    }

    #[tokio::test]
    async fn a_backup_of_a_stopped_instance_round_trips_and_prunes() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, dir.path().to_path_buf());

        let server = dir.path().join("server");
        std::fs::create_dir_all(server.join("world")).unwrap();
        std::fs::write(server.join("world").join("level.dat"), b"dat").unwrap();
        std::fs::write(server.join("server.properties"), b"motd=hi").unwrap();

        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'Survival', ?, 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(server.to_string_lossy().to_string())
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        let backup = create(
            &state,
            1,
            BackupOptions::default(),
            "manual",
            None,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();

        assert!(PathBuf::from(&backup.path).is_file());
        assert!(backup.size_bytes > 0);
        assert_eq!(backup.kind, "manual");
        assert_eq!(list(&state, 1).await.unwrap().len(), 1);

        // The preview lists what would be restored.
        let entries = preview(&state, backup.id).await.unwrap();
        assert!(entries.iter().any(|entry| entry.path == "world/level.dat"));

        // A restore puts a deleted file back, and keeps a pre-restore copy.
        std::fs::remove_file(server.join("world").join("level.dat")).unwrap();
        restore(&state, backup.id, &CancellationToken::new(), |_| {})
            .await
            .unwrap();
        assert!(server.join("world").join("level.dat").is_file());

        let all = list(&state, 1).await.unwrap();
        assert_eq!(all.len(), 2, "the pre-restore backup is kept: {all:?}");
        assert!(all.iter().any(|entry| entry.kind == "pre_restore"));

        // Retention by count removes the surplus archive and its file. Both were
        // written in the same second, so the check is on what survives rather
        // than on which of the two it is.
        let backups_dir = backup_dir(&state.data_dir, "u1");
        assert_eq!(std::fs::read_dir(&backups_dir).unwrap().count(), 2);

        let removed = prune(&state, 1, Some(1), None).await.unwrap();
        assert_eq!(removed, 1);

        let remaining = list(&state, 1).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(PathBuf::from(&remaining[0].path).is_file());
        assert_eq!(
            std::fs::read_dir(&backups_dir).unwrap().count(),
            1,
            "the pruned archive's file is deleted too"
        );
    }

    #[tokio::test]
    async fn restoring_while_the_server_runs_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, dir.path().to_path_buf());

        let server = dir.path().join("server");
        std::fs::create_dir_all(&server).unwrap();
        std::fs::write(server.join("server.properties"), b"motd=hi").unwrap();

        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'Survival', ?, 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(server.to_string_lossy().to_string())
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        let backup = create(
            &state,
            1,
            BackupOptions::default(),
            "manual",
            None,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();

        state.set_status("u1", crate::db::models::InstanceStatus::Running);
        let err = restore(&state, backup.id, &CancellationToken::new(), |_| {})
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "instance_running");
    }

    #[tokio::test]
    async fn a_live_server_this_app_does_not_own_is_refused_rather_than_torn() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, dir.path().to_path_buf());

        let server = dir.path().join("server");
        std::fs::create_dir_all(&server).unwrap();
        std::fs::write(server.join("server.properties"), b"motd=hi").unwrap();

        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'Survival', ?, 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(server.to_string_lossy().to_string())
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        // An orphan adopted at startup is running, but this app owns no console
        // for it, so saving cannot be paused. Archiving anyway would produce a
        // torn world, so the backup is refused instead.
        state.set_status("u1", crate::db::models::InstanceStatus::Unmanaged);

        let err = create(
            &state,
            1,
            BackupOptions::default(),
            "manual",
            None,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("console is not owned"), "{message}");
        assert!(message.contains("Stop it"), "{message}");
        assert!(
            !saveguard::is_marked(&state, 1).await.unwrap(),
            "nothing was disabled, so nothing needs recovering"
        );
        assert!(list(&state, 1).await.unwrap().is_empty(), "no backup was recorded");
    }

    /// The finally path: whatever happens between `save-off` and the archive,
    /// `save-on` runs. Here the resume itself cannot be delivered, which is the
    /// case that must leave the marker behind for the next start to fix.
    #[tokio::test]
    async fn an_interrupted_backup_leaves_the_marker_for_recovery() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'Survival', 'Z:/survival', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        // What the app would have written just before sending save-off.
        saveguard::mark_disabled(&state, 1).await.unwrap();

        // The app died here. On the next start, recovery re-enables saving; with
        // no console to say it on, the marker survives for the start after that.
        saveguard::recover_on_start(&state, 1).await;
        assert!(saveguard::is_marked(&state, 1).await.unwrap());

        let events: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT kind, detail FROM instance_events WHERE instance_id = 1")
                .fetch_all(&state.db)
                .await
                .unwrap();
        assert!(events.iter().any(|(kind, _)| kind == "backup"));
        assert!(events.iter().any(|(kind, _)| kind == "error"));
    }
}
