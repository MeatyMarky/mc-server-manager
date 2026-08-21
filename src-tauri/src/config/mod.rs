//! Reading and writing an instance's `server.properties`.
//!
//! Every write is atomic (temp file plus rename) and the original file is kept
//! once, as `server.properties.orig`, before this app changes it for the first
//! time — a mis-set property should never cost someone the file they had.

pub mod properties;
pub mod schema;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult, IoContext};
use crate::instance;
use crate::paths;
use crate::state::AppState;

pub use properties::Properties;
pub use schema::{KeyInfo, ValueKind};

/// Name of the one-time backup taken before the first edit.
pub const BACKUP_SUFFIX: &str = "orig";

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct PropertyEntry {
    pub key: String,
    pub value: String,
    pub info: KeyInfo,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct PropertiesView {
    pub path: String,
    pub exists: bool,
    /// In file order, with unknown keys included.
    pub entries: Vec<PropertyEntry>,
    /// Known keys the file does not contain yet, offered as additions.
    pub missing: Vec<KeyInfo>,
    /// True when a restart is needed for edits to take effect.
    pub running: bool,
    pub backup_exists: bool,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct PropertiesUpdate {
    pub changes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct SaveReport {
    pub changed: Vec<String>,
    /// Set when the server is running, because the file is re-read only on boot.
    pub restart_required: bool,
    pub backup_created: bool,
}

pub fn backup_path(instance_path: &Path) -> PathBuf {
    let properties = paths::server_properties_path(instance_path);
    let mut name = properties.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(BACKUP_SUFFIX);
    properties.with_file_name(name)
}

/// Reads the file as UTF-8, falling back to Latin-1 for files written by older
/// servers, which used ISO-8859-1 with `\uXXXX` escapes.
pub fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|byte| *byte as char).collect(),
    }
}

pub async fn read(instance_path: &Path) -> AppResult<Properties> {
    let path = paths::server_properties_path(instance_path);
    match tokio::fs::read(&path).await {
        Ok(bytes) => Ok(Properties::parse(&decode(&bytes))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Properties::default()),
        Err(err) => Err(AppError::io("read server.properties", &path, err)),
    }
}

/// The whole editor payload: current values, their metadata, and what is missing.
pub async fn view(state: &AppState, id: i64) -> AppResult<PropertiesView> {
    let row = instance::get(&state.db, id).await?;
    let dir = row.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: row.name,
            path: dir,
        });
    }

    let path = paths::server_properties_path(&dir);
    let parsed = read(&dir).await?;
    let present = parsed.keys();

    let entries = present
        .iter()
        .map(|key| PropertyEntry {
            key: key.clone(),
            value: parsed.get(key).unwrap_or_default().to_string(),
            info: schema::describe(key),
        })
        .collect();

    let missing = schema::known_keys()
        .into_iter()
        .filter(|info| !present.contains(&info.key))
        .collect();

    Ok(PropertiesView {
        path: path.to_string_lossy().to_string(),
        exists: path.is_file(),
        entries,
        missing,
        running: state.status_of(&row.uuid).is_live(),
        backup_exists: backup_path(&dir).is_file(),
    })
}

/// Applies changes and writes them atomically.
///
/// Validation happens before anything is written, so a rejected value cannot
/// leave the file half-updated.
pub async fn save(state: &AppState, id: i64, update: PropertiesUpdate) -> AppResult<SaveReport> {
    let row = instance::get(&state.db, id).await?;
    let dir = row.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: row.name,
            path: dir,
        });
    }

    for (key, value) in &update.changes {
        if let Some(problem) = schema::validate(&schema::describe(key), value) {
            return Err(AppError::Other(problem));
        }
    }

    let mut parsed = read(&dir).await?;
    let changed = parsed.apply(&update.changes);
    if changed.is_empty() {
        return Ok(SaveReport {
            changed,
            restart_required: false,
            backup_created: false,
        });
    }

    let path = paths::server_properties_path(&dir);
    let backup_created = ensure_backup(&path).await?;
    write_atomic(&path, parsed.to_string().as_bytes()).await?;

    let running = state.status_of(&row.uuid).is_live();
    crate::db::record_event(
        &state.db,
        id,
        "config",
        Some(&format!("server.properties: {}", changed.join(", "))),
    )
    .await?;

    Ok(SaveReport {
        changed,
        restart_required: running,
        backup_created,
    })
}

/// Copies the original aside the first time this app edits the file.
async fn ensure_backup(path: &Path) -> AppResult<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let backup = {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".");
        name.push(BACKUP_SUFFIX);
        path.with_file_name(name)
    };
    if backup.exists() {
        return Ok(false);
    }
    tokio::fs::copy(path, &backup)
        .await
        .ctx("back up server.properties", &backup)?;
    Ok(true)
}

/// Temp file plus rename, so an interrupted write cannot truncate the file.
pub async fn write_atomic(target: &Path, bytes: &[u8]) -> AppResult<()> {
    let temp = target.with_extension("tmp");
    tokio::fs::write(&temp, bytes)
        .await
        .ctx("write configuration", &temp)?;
    tokio::fs::rename(&temp, target)
        .await
        .ctx("replace configuration", target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::ServerType;
    use crate::instance::{crud, CreateInstanceInput};

    async fn instance_with(properties: &str) -> (AppState, i64, PathBuf) {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let dir = std::env::temp_dir().join(format!("msm-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::new(pool, dir.clone());

        let server = dir.join("server");
        let created = crud::create(
            &state,
            CreateInstanceInput {
                name: "Config".into(),
                path: server.to_string_lossy().to_string(),
                server_type: ServerType::Paper,
                mc_version: "1.21.4".into(),
                loader_version: None,
                min_ram_mb: None,
                max_ram_mb: None,
                notes: None,
                color: None,
                web_map: false,
            },
        )
        .await
        .unwrap();

        std::fs::write(paths::server_properties_path(&server), properties).unwrap();
        (state, created.id, server)
    }

    const SAMPLE: &str = "#Minecraft server properties\n\
                          motd=A Minecraft Server\n\
                          difficulty=easy\n\
                          max-players=20\n\
                          paper.custom=42\n";

    #[tokio::test]
    async fn the_view_types_known_keys_and_keeps_unknown_ones() {
        let (state, id, _dir) = instance_with(SAMPLE).await;
        let view = view(&state, id).await.unwrap();

        assert!(view.exists);
        let keys: Vec<&str> = view.entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, vec!["motd", "difficulty", "max-players", "paper.custom"]);

        let custom = view.entries.iter().find(|e| e.key == "paper.custom").unwrap();
        assert!(!custom.info.known, "an unknown key is still editable");
        assert!(view.missing.iter().any(|info| info.key == "pvp"));
        assert!(!view.running);
    }

    #[tokio::test]
    async fn saving_rewrites_only_what_changed_and_backs_up_once() {
        let (state, id, dir) = instance_with(SAMPLE).await;

        let report = save(
            &state,
            id,
            PropertiesUpdate {
                changes: BTreeMap::from([("difficulty".into(), "hard".into())]),
            },
        )
        .await
        .unwrap();

        assert_eq!(report.changed, vec!["difficulty"]);
        assert!(report.backup_created);
        assert!(!report.restart_required, "the server is not running");

        let written =
            std::fs::read_to_string(paths::server_properties_path(&dir)).unwrap();
        assert!(written.contains("difficulty=hard"));
        assert!(written.starts_with("#Minecraft server properties\n"));
        assert!(written.contains("paper.custom=42"));

        // The backup holds the file as it was, and is not taken again.
        let backup = std::fs::read_to_string(backup_path(&dir)).unwrap();
        assert_eq!(backup, SAMPLE);

        let second = save(
            &state,
            id,
            PropertiesUpdate {
                changes: BTreeMap::from([("motd".into(), "Changed".into())]),
            },
        )
        .await
        .unwrap();
        assert!(!second.backup_created, "the original is only kept once");
        assert_eq!(std::fs::read_to_string(backup_path(&dir)).unwrap(), SAMPLE);
    }

    #[tokio::test]
    async fn a_no_op_save_touches_nothing() {
        let (state, id, dir) = instance_with(SAMPLE).await;
        let report = save(
            &state,
            id,
            PropertiesUpdate {
                changes: BTreeMap::from([("difficulty".into(), "easy".into())]),
            },
        )
        .await
        .unwrap();

        assert!(report.changed.is_empty());
        assert!(!report.backup_created);
        assert!(!backup_path(&dir).exists(), "no backup for a write that did not happen");
        assert_eq!(
            std::fs::read_to_string(paths::server_properties_path(&dir)).unwrap(),
            SAMPLE
        );
    }

    #[tokio::test]
    async fn an_invalid_value_is_refused_before_anything_is_written() {
        let (state, id, dir) = instance_with(SAMPLE).await;
        let err = save(
            &state,
            id,
            PropertiesUpdate {
                changes: BTreeMap::from([
                    ("motd".into(), "fine".into()),
                    ("max-players".into(), "lots".into()),
                ]),
            },
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("whole number"), "{err}");
        assert_eq!(
            std::fs::read_to_string(paths::server_properties_path(&dir)).unwrap(),
            SAMPLE,
            "the valid change was not written either"
        );
    }

    #[tokio::test]
    async fn a_non_ascii_motd_survives_read_edit_write() {
        let (state, id, dir) = instance_with(SAMPLE).await;
        let motd = "Čajovna — žíznivý šnek";

        save(
            &state,
            id,
            PropertiesUpdate {
                changes: BTreeMap::from([("motd".into(), motd.into())]),
            },
        )
        .await
        .unwrap();

        // On disk as UTF-8, and read back identical.
        let bytes = std::fs::read(paths::server_properties_path(&dir)).unwrap();
        assert!(std::str::from_utf8(&bytes).is_ok(), "written as valid UTF-8");
        let reread = read(&dir).await.unwrap();
        assert_eq!(reread.get("motd"), Some(motd));

        let view = view(&state, id).await.unwrap();
        assert_eq!(
            view.entries.iter().find(|e| e.key == "motd").unwrap().value,
            motd
        );
    }

    #[tokio::test]
    async fn a_latin1_file_from_an_older_server_still_reads() {
        // "motd=caf\xE9" in ISO-8859-1, which is not valid UTF-8.
        let dir = std::env::temp_dir().join(format!("msm-latin1-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            paths::server_properties_path(&dir),
            b"motd=caf\xE9\nmax-players=20\n",
        )
        .unwrap();

        let parsed = read(&dir).await.unwrap();
        assert_eq!(parsed.get("motd"), Some("café"));
        assert_eq!(parsed.get("max-players"), Some("20"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn a_missing_file_reads_as_empty_rather_than_failing() {
        let dir = std::env::temp_dir().join(format!("msm-noprops-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let parsed = read(&dir).await.unwrap();
        assert!(parsed.keys().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn saving_while_running_reports_that_a_restart_is_needed() {
        let (state, id, _dir) = instance_with(SAMPLE).await;
        let row = instance::get(&state.db, id).await.unwrap();
        state.set_status(&row.uuid, crate::db::models::InstanceStatus::Running);

        let report = save(
            &state,
            id,
            PropertiesUpdate {
                changes: BTreeMap::from([("motd".into(), "later".into())]),
            },
        )
        .await
        .unwrap();

        assert!(report.restart_required);
        assert!(view(&state, id).await.unwrap().running);
    }
}
