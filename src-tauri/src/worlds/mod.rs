//! Worlds: listing, metadata, sizing, switching, import/export and deletion.
//!
//! Sizing walks every region file, which on a large world is thousands of files
//! and real seconds, so it runs in `spawn_blocking` behind a task id and reports
//! progress — the same shape downloads and installs use.

pub mod archive;
pub mod nbt;

use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::db::record_event;
use crate::error::{AppError, AppResult, IoContext};
use crate::instance;
use crate::state::AppState;

/// Game modes, as `level.dat` stores them.
fn game_type_name(value: i64) -> &'static str {
    match value {
        0 => "survival",
        1 => "creative",
        2 => "adventure",
        3 => "spectator",
        _ => "unknown",
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct World {
    /// Folder name, which is what `level-name` refers to.
    pub folder: String,
    pub path: String,
    /// `LevelName` from level.dat, which the player chose and can differ.
    pub display_name: Option<String>,
    #[ts(type = "number | null")]
    pub seed: Option<i64>,
    pub game_type: Option<String>,
    pub hardcore: bool,
    /// Milliseconds since the epoch, as level.dat stores it.
    #[ts(type = "number | null")]
    pub last_played: Option<i64>,
    /// Minecraft version that last wrote the world.
    pub version: Option<String>,
    /// True for the world the server will load.
    pub active: bool,
    /// Set when level.dat could not be read; the world is still listed.
    pub problem: Option<String>,
    /// Where players appear, from `SpawnX`/`SpawnY`/`SpawnZ`. A map centred on
    /// 0,0 is centred on nothing in particular; this is where the world is.
    #[ts(type = "number | null")]
    pub spawn_x: Option<i64>,
    #[ts(type = "number | null")]
    pub spawn_y: Option<i64>,
    #[ts(type = "number | null")]
    pub spawn_z: Option<i64>,
}

/// Reads what `level.dat` says about a world. A world with an unreadable
/// `level.dat` is still returned — it exists, and hiding it helps nobody.
pub fn read_world(dir: &Path, active_name: &str) -> World {
    let folder = dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut world = World {
        active: folder == active_name,
        path: dir.to_string_lossy().to_string(),
        folder,
        display_name: None,
        spawn_x: None,
        spawn_y: None,
        spawn_z: None,
        seed: None,
        game_type: None,
        hardcore: false,
        last_played: None,
        version: None,
        problem: None,
    };

    let level_dat = dir.join("level.dat");
    let bytes = match std::fs::read(&level_dat) {
        Ok(bytes) => bytes,
        Err(err) => {
            world.problem = Some(format!("level.dat could not be read: {err}"));
            return world;
        }
    };

    match nbt::parse(&bytes) {
        Ok(root) => {
            world.display_name = root
                .get_path(&["Data", "LevelName"])
                .and_then(nbt::Value::as_string)
                .map(str::to_string);
            world.seed = root
                .get_path(&["Data", "WorldGenSettings", "seed"])
                .or_else(|| root.get_path(&["Data", "RandomSeed"]))
                .and_then(nbt::Value::as_i64);
            world.game_type = root
                .get_path(&["Data", "GameType"])
                .and_then(nbt::Value::as_i64)
                .map(|value| game_type_name(value).to_string());
            world.hardcore = root
                .get_path(&["Data", "hardcore"])
                .and_then(nbt::Value::as_i64)
                .map(|value| value != 0)
                .unwrap_or(false);
            world.last_played = root
                .get_path(&["Data", "LastPlayed"])
                .and_then(nbt::Value::as_i64);
            world.spawn_x = root.get_path(&["Data", "SpawnX"]).and_then(nbt::Value::as_i64);
            world.spawn_y = root.get_path(&["Data", "SpawnY"]).and_then(nbt::Value::as_i64);
            world.spawn_z = root.get_path(&["Data", "SpawnZ"]).and_then(nbt::Value::as_i64);
            world.version = root
                .get_path(&["Data", "Version", "Name"])
                .and_then(nbt::Value::as_string)
                .map(str::to_string);
        }
        Err(err) => world.problem = Some(err.to_string()),
    }

    world
}

/// Every folder in the instance that holds a `level.dat`.
pub fn scan(instance_path: &Path, active_name: &str) -> Vec<World> {
    let Ok(entries) = std::fs::read_dir(instance_path) else {
        return Vec::new();
    };

    let mut worlds: Vec<World> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("level.dat").is_file())
        .map(|path| read_world(&path, active_name))
        .collect();

    // Active first, then most recently played.
    worlds.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| b.last_played.cmp(&a.last_played))
            .then_with(|| a.folder.cmp(&b.folder))
    });
    worlds
}

pub async fn list(state: &AppState, id: i64) -> AppResult<Vec<World>> {
    let row = instance::get(&state.db, id).await?;
    let dir = row.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: row.name,
            path: dir,
        });
    }

    let active = crate::config::read(&dir)
        .await?
        .get("level-name")
        .unwrap_or("world")
        .to_string();

    let scan_dir = dir.clone();
    tokio::task::spawn_blocking(move || scan(&scan_dir, &active))
        .await
        .map_err(|e| AppError::internal("scanning for worlds", e))
}

/// Total size in bytes, reported as it goes. Cancellable, and never run on the
/// async runtime: a large world is tens of thousands of files.
pub fn measure<P>(dir: &Path, cancel: &CancellationToken, mut report: P) -> AppResult<u64>
where
    P: FnMut(u64, u64),
{
    let mut total = 0u64;
    let mut files = 0u64;

    for entry in walkdir::WalkDir::new(dir).into_iter() {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_file() {
            continue;
        }
        total += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        files += 1;
        if files % 256 == 0 {
            report(files, total);
        }
    }

    report(files, total);
    Ok(total)
}

/// Points `level-name` at another world. Refused while the server is running:
/// it would keep writing the old world and overwrite the change on shutdown.
pub async fn switch(state: &AppState, id: i64, folder: &str) -> AppResult<()> {
    let row = instance::get(&state.db, id).await?;
    if state.status_of(&row.uuid).is_live() {
        return Err(AppError::InstanceRunning(row.name));
    }

    let dir = row.path_buf();
    let target = dir.join(folder);
    if !target.join("level.dat").is_file() {
        return Err(AppError::Other(format!(
            "\"{folder}\" is not a world folder in this instance"
        )));
    }

    crate::config::save(
        state,
        id,
        crate::config::PropertiesUpdate {
            changes: std::collections::BTreeMap::from([(
                "level-name".to_string(),
                folder.to_string(),
            )]),
        },
    )
    .await?;

    record_event(&state.db, id, "world", Some(&format!("switched to {folder}"))).await?;
    Ok(())
}

/// Deletes a world folder. Refused while running; the UI additionally makes the
/// user type the folder name.
pub async fn delete(state: &AppState, id: i64, folder: &str) -> AppResult<()> {
    let row = instance::get(&state.db, id).await?;
    if state.status_of(&row.uuid).is_live() {
        return Err(AppError::InstanceRunning(row.name));
    }

    let dir = row.path_buf();
    let target = resolve_world(&dir, folder)?;

    let active = crate::config::read(&dir)
        .await?
        .get("level-name")
        .unwrap_or("world")
        .to_string();
    if active == folder {
        return Err(AppError::Other(format!(
            "\"{folder}\" is the active world; switch to another one first"
        )));
    }

    tokio::fs::remove_dir_all(&target)
        .await
        .ctx("delete world", &target)?;
    record_event(&state.db, id, "world", Some(&format!("deleted {folder}"))).await?;
    Ok(())
}

/// Resolves a world folder name to a path inside the instance, refusing
/// anything that escapes it.
pub fn resolve_world(instance_path: &Path, folder: &str) -> AppResult<PathBuf> {
    let trimmed = folder.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
    {
        return Err(AppError::Other(format!(
            "\"{folder}\" is not a valid world folder name"
        )));
    }

    let target = instance_path.join(trimmed);
    if !target.join("level.dat").is_file() {
        return Err(AppError::Other(format!(
            "\"{trimmed}\" is not a world folder in this instance"
        )));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_at(root: &Path, folder: &str, name: &str, seed: i64) -> PathBuf {
        let dir = root.join(folder);
        std::fs::create_dir_all(dir.join("region")).unwrap();
        std::fs::write(
            dir.join("level.dat"),
            nbt::build::gzip(&nbt::build::level_dat(name, seed, 0, 1_700_000_000_000)),
        )
        .unwrap();
        std::fs::write(dir.join("region").join("r.0.0.mca"), vec![0u8; 4096]).unwrap();
        dir
    }

    #[test]
    fn reads_metadata_out_of_level_dat() {
        let root = tempfile::tempdir().unwrap();
        let dir = world_at(root.path(), "world", "My Survival World", 987654321);

        let world = read_world(&dir, "world");
        assert_eq!(world.folder, "world");
        assert_eq!(world.display_name.as_deref(), Some("My Survival World"));
        assert_eq!(world.seed, Some(987654321));
        assert_eq!(world.game_type.as_deref(), Some("survival"));
        assert_eq!(world.last_played, Some(1_700_000_000_000));
        assert_eq!(world.version.as_deref(), Some("1.21.4"));
        assert!(world.active);
        assert!(world.problem.is_none());
    }

    #[test]
    fn an_uncompressed_level_dat_reads_the_same() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("plain");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("level.dat"),
            nbt::build::level_dat("Uncompressed", 5, 1, 0),
        )
        .unwrap();

        let world = read_world(&dir, "world");
        assert_eq!(world.display_name.as_deref(), Some("Uncompressed"));
        assert_eq!(world.game_type.as_deref(), Some("creative"));
        assert!(!world.active);
    }

    #[test]
    fn a_corrupt_level_dat_still_lists_the_world_with_a_problem() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("level.dat"), b"not nbt").unwrap();

        let world = read_world(&dir, "world");
        assert_eq!(world.folder, "broken");
        assert!(world.problem.is_some(), "the reason is reported");
        assert!(world.display_name.is_none());
    }

    #[test]
    fn scanning_finds_worlds_and_puts_the_active_one_first() {
        let root = tempfile::tempdir().unwrap();
        world_at(root.path(), "world", "Main", 1);
        world_at(root.path(), "creative_map", "Creative", 2);
        // Not a world: no level.dat.
        std::fs::create_dir_all(root.path().join("plugins")).unwrap();

        let worlds = scan(root.path(), "creative_map");
        assert_eq!(worlds.len(), 2);
        assert_eq!(worlds[0].folder, "creative_map");
        assert!(worlds[0].active);
        assert!(!worlds[1].active);
    }

    #[test]
    fn measuring_adds_up_every_file_and_reports_progress() {
        let root = tempfile::tempdir().unwrap();
        let dir = world_at(root.path(), "world", "Main", 1);
        std::fs::write(dir.join("region").join("r.0.1.mca"), vec![0u8; 2048]).unwrap();

        let mut reports = 0;
        let total = measure(&dir, &CancellationToken::new(), |_files, _bytes| reports += 1).unwrap();
        assert!(total >= 6144, "region files are counted: {total}");
        assert!(reports >= 1, "progress is reported at least once");
    }

    #[test]
    fn measuring_stops_when_cancelled() {
        let root = tempfile::tempdir().unwrap();
        let dir = world_at(root.path(), "world", "Main", 1);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = measure(&dir, &cancel, |_, _| {}).unwrap_err();
        assert_eq!(err.kind(), "cancelled");
    }

    #[test]
    fn folder_names_that_escape_the_instance_are_refused() {
        let root = tempfile::tempdir().unwrap();
        world_at(root.path(), "world", "Main", 1);

        assert!(resolve_world(root.path(), "world").is_ok());
        for bad in ["../world", "..", "sub/world", r"sub\world", "  "] {
            let err = resolve_world(root.path(), bad).unwrap_err();
            assert!(
                err.to_string().contains("not a valid world folder name")
                    || err.to_string().contains("not a world folder"),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn a_folder_without_level_dat_is_not_a_world() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("mods")).unwrap();
        assert!(resolve_world(root.path(), "mods").is_err());
    }
}
