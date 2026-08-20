//! Instance lifecycle on disk and in the database.
//!
//! The database row is authoritative; `.msm/instance.json` is written after every
//! mutation purely as a recovery mirror (see CLAUDE.md).

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use super::{CloneInstanceInput, CreateInstanceInput, DeleteReport, UpdateInstanceInput};
use crate::db::models::{
    default_jvm_args, default_server_args, Instance, InstanceStatus, ServerType,
};
use crate::db::{now_rfc3339, record_event};
use crate::error::{AppError, AppResult, IoContext};
use crate::paths;
use crate::state::AppState;

/// Files and folders that are never worth copying into a clone: they are
/// regenerated, machine-specific, or actively locked while a server runs.
const CLONE_SKIP_DIRS: &[&str] = &["logs", "crash-reports", "cache", "versions"];
const CLONE_SKIP_FILES: &[&str] = &["session.lock", "usercache.json", "usernamecache.json"];

/// Decides whether one entry, given as a path relative to the instance folder,
/// belongs in a clone. Pure so the rules can be tested without touching a disk.
pub fn should_copy(relative: &Path, world_dirs: &[String], include_worlds: bool) -> bool {
    let components: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let Some(first) = components.first() else {
        return false;
    };

    // Our own console captures are noise; the manifest is rewritten for the clone.
    if first == paths::MSM_DIR {
        return false;
    }
    if CLONE_SKIP_DIRS.contains(&first.as_str()) {
        return false;
    }
    if !include_worlds && world_dirs.iter().any(|w| w == first) {
        return false;
    }

    let Some(last) = components.last() else {
        return false;
    };
    if CLONE_SKIP_FILES.contains(&last.as_str()) {
        return false;
    }
    let lower = last.to_ascii_lowercase();
    if lower.ends_with(".log") || lower.ends_with(".log.gz") {
        return false;
    }
    true
}

/// Top-level folders that look like worlds, i.e. contain a `level.dat`.
pub fn world_dir_names(instance_path: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(instance_path) else {
        return Vec::new();
    };
    let mut worlds = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("level.dat").is_file() {
            worlds.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    worlds.sort();
    worlds
}

pub async fn create(state: &AppState, input: CreateInstanceInput) -> AppResult<Instance> {
    paths::validate_instance_name(&input.name)?;
    let path = PathBuf::from(&input.path);
    if !path.is_absolute() {
        return Err(AppError::Other(
            "the instance folder must be an absolute path".into(),
        ));
    }
    ensure_name_free(&state.db, &input.name, None).await?;
    ensure_path_free(&state.db, &path, None).await?;

    if !paths::dir_is_empty(&path)? {
        return Err(AppError::FolderNotEmpty(path));
    }

    scaffold(&path, input.server_type).await?;

    let (launch_kind, launch_target) = super::default_launch(input.server_type);
    let now = now_rfc3339();
    let uuid = uuid::Uuid::new_v4().to_string();
    let min_ram = input.min_ram_mb.unwrap_or(1024).max(512);
    let max_ram = input.max_ram_mb.unwrap_or(4096).max(min_ram);

    // A web map is a wish at this point, not a mod: the folder it installs into
    // does not exist until the server is installed, so the choice is recorded
    // and acted on when that finishes.
    // Only a map this server type can actually run: a choice that arrived for
    // the wrong type is dropped rather than installed and left to fail.
    let map_kind = input
        .web_map
        .filter(|kind| kind.supports(input.server_type))
        .map(|kind| kind.as_str().to_string());

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO instances (
            uuid, name, path, server_type, mc_version, loader_version,
            launch_kind, launch_target, jvm_args, server_args,
            min_ram_mb, max_ram_mb, notes, color, map_kind, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         RETURNING id",
    )
    .bind(&uuid)
    .bind(input.name.trim())
    .bind(path.to_string_lossy().to_string())
    .bind(input.server_type)
    .bind(&input.mc_version)
    .bind(&input.loader_version)
    .bind(launch_kind)
    .bind(&launch_target)
    .bind(serde_json::to_string(&default_jvm_args())?)
    .bind(serde_json::to_string(&default_server_args())?)
    .bind(min_ram)
    .bind(max_ram)
    .bind(&input.notes)
    .bind(&input.color)
    .bind(&map_kind)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;

    let instance = super::get(&state.db, id).await?;
    write_manifest(&instance).await?;
    record_event(&state.db, id, "created", Some(input.server_type.as_str())).await?;
    Ok(instance)
}

/// Creates the folder skeleton. No jar, no `eula.txt`: the EULA is only ever
/// written after an explicit acceptance, and jars arrive in Phase 2.
pub async fn scaffold(path: &Path, server_type: ServerType) -> AppResult<()> {
    tokio::fs::create_dir_all(path)
        .await
        .ctx("create instance folder", path)?;
    let console = paths::console_dir(path);
    tokio::fs::create_dir_all(&console)
        .await
        .ctx("create metadata folder", &console)?;
    if server_type.loads_mods() {
        let content = paths::content_dir(path, server_type.content_dir_name());
        tokio::fs::create_dir_all(&content)
            .await
            .ctx("create content folder", &content)?;
    }
    Ok(())
}

/// Writes `.msm/instance.json`. Failure to write the mirror is logged, never
/// fatal: the database row is the authority.
pub async fn write_manifest(instance: &Instance) -> AppResult<()> {
    let dir = paths::msm_dir(&instance.path_buf());
    if tokio::fs::create_dir_all(&dir).await.is_err() {
        tracing::warn!(path = %dir.display(), "could not create metadata folder");
        return Ok(());
    }
    let target = paths::instance_json_path(&instance.path_buf());
    let json = serde_json::to_string_pretty(&instance.to_manifest())?;
    if let Err(err) = write_atomic(&target, json.as_bytes()).await {
        tracing::warn!(path = %target.display(), error = %err, "could not write instance manifest");
    }
    Ok(())
}

/// Temp file plus rename, so a crash mid-write cannot truncate the original.
pub async fn write_atomic(target: &Path, bytes: &[u8]) -> AppResult<()> {
    let tmp = target.with_extension("tmp");
    tokio::fs::write(&tmp, bytes).await.ctx("write file", &tmp)?;
    tokio::fs::rename(&tmp, target)
        .await
        .ctx("replace file", target)?;
    Ok(())
}

pub async fn clone_instance(state: &AppState, input: CloneInstanceInput) -> AppResult<Instance> {
    let source = super::get(&state.db, input.source_id).await?;
    let source_path = source.path_buf();
    if !source_path.is_dir() {
        return Err(AppError::FolderMissing {
            name: source.name.clone(),
            path: source_path,
        });
    }
    if state.status_of(&source.uuid).is_live() {
        return Err(AppError::InstanceRunning(source.name.clone()));
    }

    paths::validate_instance_name(&input.name)?;
    let target_path = PathBuf::from(&input.path);
    ensure_name_free(&state.db, &input.name, None).await?;
    ensure_path_free(&state.db, &target_path, None).await?;
    if !paths::dir_is_empty(&target_path)? {
        return Err(AppError::FolderNotEmpty(target_path));
    }

    let include_worlds = input.include_worlds;
    let from = source_path.clone();
    let to = target_path.clone();
    tokio::task::spawn_blocking(move || copy_tree(&from, &to, include_worlds))
        .await
        .map_err(|e| AppError::Other(format!("copy task failed: {e}")))??;

    let now = now_rfc3339();
    let uuid = uuid::Uuid::new_v4().to_string();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO instances (
            uuid, name, path, server_type, mc_version, loader_version, launch_kind, launch_target,
            java_path, java_major, jvm_args, server_args, min_ram_mb, max_ram_mb,
            eula_accepted, eula_accepted_at, auto_restart, restart_max, restart_window_s,
            stop_timeout_s, color, notes, created_at, updated_at
         )
         SELECT ?, ?, ?, server_type, mc_version, loader_version, launch_kind, launch_target,
            java_path, java_major, jvm_args, server_args, min_ram_mb, max_ram_mb,
            eula_accepted, eula_accepted_at, auto_restart, restart_max, restart_window_s,
            stop_timeout_s, color, notes, ?, ?
         FROM instances WHERE id = ?
         RETURNING id",
    )
    .bind(&uuid)
    .bind(input.name.trim())
    .bind(target_path.to_string_lossy().to_string())
    .bind(&now)
    .bind(&now)
    .bind(input.source_id)
    .fetch_one(&state.db)
    .await?;

    let instance = super::get(&state.db, id).await?;
    write_manifest(&instance).await?;
    record_event(
        &state.db,
        id,
        "created",
        Some(&format!("cloned from {}", source.name)),
    )
    .await?;
    Ok(instance)
}

fn copy_tree(from: &Path, to: &Path, include_worlds: bool) -> AppResult<()> {
    let worlds = world_dir_names(from);
    std::fs::create_dir_all(to).ctx("create clone folder", to)?;

    for entry in walkdir::WalkDir::new(from).min_depth(1).into_iter() {
        let entry = entry.map_err(|e| {
            AppError::Other(format!("could not read {}: {e}", from.display()))
        })?;
        let relative = entry
            .path()
            .strip_prefix(from)
            .map_err(|_| AppError::Other("unexpected path outside the source folder".into()))?;
        if !should_copy(relative, &worlds, include_worlds) {
            continue;
        }
        let destination = to.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&destination).ctx("create folder", &destination)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).ctx("create folder", parent)?;
            }
            std::fs::copy(entry.path(), &destination).ctx("copy file", entry.path())?;
        }
        // Symlinks are skipped deliberately: following them can escape the folder.
    }
    Ok(())
}

pub async fn rename(state: &AppState, id: i64, new_name: &str) -> AppResult<Instance> {
    paths::validate_instance_name(new_name)?;
    ensure_name_free(&state.db, new_name, Some(id)).await?;
    sqlx::query("UPDATE instances SET name = ?, updated_at = ? WHERE id = ?")
        .bind(new_name.trim())
        .bind(now_rfc3339())
        .bind(id)
        .execute(&state.db)
        .await?;
    let instance = super::get(&state.db, id).await?;
    write_manifest(&instance).await?;
    Ok(instance)
}

pub async fn update(
    state: &AppState,
    id: i64,
    input: UpdateInstanceInput,
) -> AppResult<Instance> {
    let current = super::get(&state.db, id).await?;

    if let Some(name) = input.name.as_deref() {
        paths::validate_instance_name(name)?;
        ensure_name_free(&state.db, name, Some(id)).await?;
    }
    let min_ram = input.min_ram_mb.unwrap_or(current.min_ram_mb).max(512);
    let max_ram = input.max_ram_mb.unwrap_or(current.max_ram_mb).max(min_ram);

    sqlx::query(
        "UPDATE instances SET
            name = ?, mc_version = ?, loader_version = ?, java_path = ?,
            jvm_args = ?, server_args = ?, min_ram_mb = ?, max_ram_mb = ?,
            auto_start = ?, auto_restart = ?, restart_max = ?, restart_window_s = ?,
            stop_timeout_s = ?, notes = ?, color = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(input.name.as_deref().map(str::trim).unwrap_or(&current.name))
    .bind(input.mc_version.as_ref().unwrap_or(&current.mc_version))
    .bind(
        input
            .loader_version
            .clone()
            .or_else(|| current.loader_version.clone()),
    )
    .bind(input.java_path.clone().unwrap_or(current.java_path.clone()))
    .bind(match &input.jvm_args {
        Some(args) => serde_json::to_string(args)?,
        None => current.jvm_args.clone(),
    })
    .bind(match &input.server_args {
        Some(args) => serde_json::to_string(args)?,
        None => current.server_args.clone(),
    })
    .bind(min_ram)
    .bind(max_ram)
    .bind(input.auto_start.unwrap_or(current.auto_start))
    .bind(input.auto_restart.unwrap_or(current.auto_restart))
    .bind(input.restart_max.unwrap_or(current.restart_max))
    .bind(input.restart_window_s.unwrap_or(current.restart_window_s))
    .bind(input.stop_timeout_s.unwrap_or(current.stop_timeout_s))
    .bind(input.notes.clone().unwrap_or(current.notes.clone()))
    .bind(input.color.clone().unwrap_or(current.color.clone()))
    .bind(now_rfc3339())
    .bind(id)
    .execute(&state.db)
    .await?;

    let instance = super::get(&state.db, id).await?;
    write_manifest(&instance).await?;
    Ok(instance)
}

/// Repoints a `Missing` instance at the folder the user located.
pub async fn relocate(state: &AppState, id: i64, new_path: &Path) -> AppResult<Instance> {
    if !new_path.is_dir() {
        return Err(AppError::io(
            "open folder",
            new_path,
            std::io::Error::new(std::io::ErrorKind::NotFound, "no such folder"),
        ));
    }
    ensure_path_free(&state.db, new_path, Some(id)).await?;
    sqlx::query("UPDATE instances SET path = ?, updated_at = ? WHERE id = ?")
        .bind(paths::normalize(new_path).to_string_lossy().to_string())
        .bind(now_rfc3339())
        .bind(id)
        .execute(&state.db)
        .await?;
    let instance = super::get(&state.db, id).await?;
    write_manifest(&instance).await?;
    record_event(&state.db, id, "imported", Some("folder relocated")).await?;
    Ok(instance)
}

pub async fn delete(state: &AppState, id: i64, delete_files: bool) -> AppResult<DeleteReport> {
    let instance = super::get(&state.db, id).await?;
    if state.status_of(&instance.uuid).is_live() {
        return Err(AppError::InstanceRunning(instance.name.clone()));
    }

    let path = instance.path_buf();
    let mut files_deleted = false;
    if delete_files && path.is_dir() {
        // Only ever delete a folder we know we manage: it must carry our marker.
        if paths::msm_dir(&path).is_dir() {
            tokio::fs::remove_dir_all(&path)
                .await
                .ctx("delete instance folder", &path)?;
            files_deleted = true;
        } else {
            tracing::warn!(
                path = %path.display(),
                "refusing to delete a folder without a .msm marker"
            );
        }
    }

    sqlx::query("DELETE FROM instances WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    state.forget(&instance.uuid);

    Ok(DeleteReport {
        name: instance.name,
        files_deleted,
        path: instance.path,
    })
}

/// Sets the status the supervisor should report for an instance. Phase 1 only
/// uses this for orphan reconciliation.
pub fn set_status(state: &AppState, uuid: &str, status: InstanceStatus) {
    state.set_status(uuid, status);
}

async fn ensure_name_free(pool: &SqlitePool, name: &str, except: Option<i64>) -> AppResult<()> {
    let taken: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM instances WHERE name = ? COLLATE NOCASE AND id IS NOT ?",
    )
    .bind(name.trim())
    .bind(except)
    .fetch_optional(pool)
    .await?;
    if taken.is_some() {
        return Err(AppError::NameInUse(name.trim().to_string()));
    }
    Ok(())
}

async fn ensure_path_free(pool: &SqlitePool, path: &Path, except: Option<i64>) -> AppResult<()> {
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, path FROM instances")
        .fetch_all(pool)
        .await?;
    for (id, existing) in rows {
        if Some(id) == except {
            continue;
        }
        if paths::same_path(Path::new(&existing), path) {
            return Err(AppError::PathInUse(path.to_path_buf()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(parts: &[&str]) -> PathBuf {
        parts.iter().collect()
    }

    #[test]
    fn clone_skips_regenerated_and_locked_entries() {
        let worlds = vec!["world".to_string()];
        assert!(!should_copy(&rel(&["logs", "latest.log"]), &worlds, true));
        assert!(!should_copy(&rel(&[".msm", "console", "a.log"]), &worlds, true));
        assert!(!should_copy(&rel(&["crash-reports"]), &worlds, true));
        assert!(!should_copy(&rel(&["world", "session.lock"]), &worlds, true));
        assert!(!should_copy(&rel(&["debug.log"]), &worlds, true));
        assert!(!should_copy(&rel(&["usercache.json"]), &worlds, true));
    }

    #[test]
    fn clone_keeps_configuration_and_content() {
        let worlds = vec!["world".to_string()];
        assert!(should_copy(&rel(&["server.properties"]), &worlds, true));
        assert!(should_copy(&rel(&["mods", "sodium.jar"]), &worlds, true));
        assert!(should_copy(&rel(&["plugins", "config.yml"]), &worlds, true));
        assert!(should_copy(&rel(&["libraries", "net", "x.jar"]), &worlds, true));
        assert!(should_copy(&rel(&["ops.json"]), &worlds, true));
    }

    #[test]
    fn worlds_are_excluded_only_when_asked() {
        let worlds = vec!["world".to_string(), "creative_map".to_string()];
        assert!(should_copy(&rel(&["world", "level.dat"]), &worlds, true));
        assert!(!should_copy(&rel(&["world", "level.dat"]), &worlds, false));
        assert!(!should_copy(&rel(&["creative_map", "region", "r.0.0.mca"]), &worlds, false));
        // A folder that is not a world is unaffected by the toggle.
        assert!(should_copy(&rel(&["config", "paper.yml"]), &worlds, false));
    }

    #[tokio::test]
    async fn create_rejects_a_non_empty_folder() {
        let (state, tmp) = test_state().await;
        let dir = tmp.join("busy");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("something.txt"), b"x").unwrap();

        let err = create(&state, input(&dir, "Busy")).await.unwrap_err();
        assert_eq!(err.kind(), "folder_not_empty");
        cleanup(tmp);
    }

    #[tokio::test]
    async fn create_scaffolds_and_writes_the_manifest() {
        let (state, tmp) = test_state().await;
        let dir = tmp.join("survival");
        let instance = create(&state, input(&dir, "Survival")).await.unwrap();

        assert!(dir.join("plugins").is_dir(), "paper gets plugins/");
        assert!(paths::console_dir(&dir).is_dir());
        assert!(paths::instance_json_path(&dir).is_file());
        // The EULA is never written implicitly.
        assert!(!paths::eula_path(&dir).exists());
        assert!(!instance.eula_accepted);
        cleanup(tmp);
    }

    #[tokio::test]
    async fn duplicate_names_and_paths_are_rejected() {
        let (state, tmp) = test_state().await;
        create(&state, input(&tmp.join("a"), "Same")).await.unwrap();

        let err = create(&state, input(&tmp.join("b"), "same")).await.unwrap_err();
        assert_eq!(err.kind(), "name_in_use");

        let err = create(&state, input(&tmp.join("a"), "Other")).await.unwrap_err();
        assert_eq!(err.kind(), "path_in_use");
        cleanup(tmp);
    }

    #[tokio::test]
    async fn delete_refuses_folders_without_our_marker() {
        let (state, tmp) = test_state().await;
        let dir = tmp.join("adopted");
        let instance = create(&state, input(&dir, "Adopted")).await.unwrap();
        std::fs::remove_dir_all(paths::msm_dir(&dir)).unwrap();

        let report = delete(&state, instance.id, true).await.unwrap();
        assert!(!report.files_deleted, "files must survive without the marker");
        assert!(dir.is_dir());
        cleanup(tmp);
    }

    #[tokio::test]
    async fn delete_removes_row_and_files_when_marked() {
        let (state, tmp) = test_state().await;
        let dir = tmp.join("gone");
        let instance = create(&state, input(&dir, "Gone")).await.unwrap();

        let report = delete(&state, instance.id, true).await.unwrap();
        assert!(report.files_deleted);
        assert!(!dir.exists());
        assert!(super::super::get(&state.db, instance.id).await.is_err());
        cleanup(tmp);
    }

    #[tokio::test]
    async fn relocate_repoints_a_missing_instance() {
        let (state, tmp) = test_state().await;
        let dir = tmp.join("moved-from");
        let instance = create(&state, input(&dir, "Moved")).await.unwrap();
        let moved = tmp.join("moved-to");
        std::fs::rename(&dir, &moved).unwrap();

        assert_eq!(
            state.view(&super::super::get(&state.db, instance.id).await.unwrap()).status,
            InstanceStatus::Missing
        );
        let updated = relocate(&state, instance.id, &moved).await.unwrap();
        assert!(updated.folder_exists());
        assert_eq!(state.view(&updated).status, InstanceStatus::Stopped);
        cleanup(tmp);
    }

    #[tokio::test]
    async fn clone_copies_content_but_not_logs() {
        let (state, tmp) = test_state().await;
        let dir = tmp.join("source");
        let source = create(&state, input(&dir, "Source")).await.unwrap();
        std::fs::write(dir.join("server.properties"), b"motd=hi").unwrap();
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        std::fs::write(dir.join("logs").join("latest.log"), b"noise").unwrap();
        std::fs::create_dir_all(dir.join("world")).unwrap();
        std::fs::write(dir.join("world").join("level.dat"), b"dat").unwrap();

        let target = tmp.join("copy");
        let clone = clone_instance(
            &state,
            CloneInstanceInput {
                source_id: source.id,
                name: "Copy".into(),
                path: target.to_string_lossy().to_string(),
                include_worlds: false,
            },
        )
        .await
        .unwrap();

        assert!(target.join("server.properties").is_file());
        assert!(!target.join("logs").exists());
        assert!(!target.join("world").exists(), "worlds excluded");
        assert_eq!(clone.server_type, source.server_type);
        assert_ne!(clone.uuid, source.uuid);
        cleanup(tmp);
    }

    fn input(path: &Path, name: &str) -> CreateInstanceInput {
        CreateInstanceInput {
            name: name.to_string(),
            path: path.to_string_lossy().to_string(),
            server_type: ServerType::Paper,
            mc_version: "1.21.4".into(),
            loader_version: None,
            min_ram_mb: None,
            max_ram_mb: None,
            notes: None,
            color: None,
            web_map: None,
        }
    }

    async fn test_state() -> (AppState, PathBuf) {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let tmp = std::env::temp_dir().join(format!("msm-crud-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        (AppState::new(pool, tmp.clone()), tmp)
    }

    fn cleanup(tmp: PathBuf) {
        std::fs::remove_dir_all(tmp).ok();
    }
}
