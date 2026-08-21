pub mod crud;
pub mod eula;
pub mod import;
pub mod install;
pub mod reconcile;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use ts_rs::TS;

use crate::db::models::{Instance, LaunchKind, ServerType};
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct CreateInstanceInput {
    pub name: String,
    /// Absolute folder for this instance. Instances are not confined to a shared
    /// root; each one records its own path.
    pub path: String,
    pub server_type: ServerType,
    pub mc_version: String,
    pub loader_version: Option<String>,
    #[ts(type = "number | null")]
    pub min_ram_mb: Option<i64>,
    #[ts(type = "number | null")]
    pub max_ram_mb: Option<i64>,
    pub notes: Option<String>,
    pub color: Option<String>,
    /// Whether to install the web map. It goes in after the server itself,
    /// because it lands in a folder that does not exist yet.
    #[serde(default)]
    pub web_map: bool,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct CloneInstanceInput {
    #[ts(type = "number")]
    pub source_id: i64,
    pub name: String,
    pub path: String,
    /// Copying worlds is the common case; skipping them gives a fresh map with
    /// the same mods and configuration.
    pub include_worlds: bool,
}

/// Every field is optional: only what the UI sends is written.
#[derive(Debug, Clone, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct UpdateInstanceInput {
    pub name: Option<String>,
    pub mc_version: Option<String>,
    pub loader_version: Option<String>,
    pub java_path: Option<Option<String>>,
    pub jvm_args: Option<Vec<String>>,
    pub server_args: Option<Vec<String>>,
    #[ts(type = "number | null")]
    pub min_ram_mb: Option<i64>,
    #[ts(type = "number | null")]
    pub max_ram_mb: Option<i64>,
    pub auto_start: Option<bool>,
    pub auto_restart: Option<bool>,
    #[ts(type = "number | null")]
    pub restart_max: Option<i64>,
    #[ts(type = "number | null")]
    pub restart_window_s: Option<i64>,
    #[ts(type = "number | null")]
    pub stop_timeout_s: Option<i64>,
    pub notes: Option<Option<String>>,
    pub color: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct DeleteReport {
    pub name: String,
    pub files_deleted: bool,
    pub path: String,
}

pub async fn list(pool: &SqlitePool) -> AppResult<Vec<Instance>> {
    let rows = sqlx::query_as::<_, Instance>("SELECT * FROM instances ORDER BY name COLLATE NOCASE")
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

pub async fn get(pool: &SqlitePool, id: i64) -> AppResult<Instance> {
    sqlx::query_as::<_, Instance>("SELECT * FROM instances WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::InstanceNotFound(id.to_string()))
}

pub async fn get_by_uuid(pool: &SqlitePool, uuid: &str) -> AppResult<Instance> {
    sqlx::query_as::<_, Instance>("SELECT * FROM instances WHERE uuid = ?")
        .bind(uuid)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::InstanceNotFound(uuid.to_string()))
}

/// The launch target a freshly created instance starts out with. Phase 2 replaces
/// it with whatever the provider's installer actually produced.
pub fn default_launch(server_type: ServerType) -> (LaunchKind, Option<String>) {
    match server_type {
        ServerType::Forge | ServerType::NeoForge => (LaunchKind::Script, None),
        _ => (LaunchKind::Jar, Some("server.jar".to_string())),
    }
}
