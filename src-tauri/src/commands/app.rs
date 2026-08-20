use std::collections::HashMap;

use serde::Serialize;
use tauri::{AppHandle, State};
use ts_rs::TS;

use crate::db;
use crate::error::AppResult;
use crate::instance;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    pub platform: String,
    /// Suggested parent folder for new instances. Instances may live anywhere;
    /// this only pre-fills the create dialog.
    pub default_instance_root: String,
    /// Physical RAM in megabytes, for the "you are giving the server more than
    /// this machine can spare" warning.
    #[ts(type = "number")]
    pub total_ram_mb: i64,
}

#[tauri::command]
pub async fn app_info(state: State<'_, AppState>) -> AppResult<AppInfo> {
    let default_root = db::setting_get(&state.db, "default_instance_root")
        .await?
        // Settings clears the override by storing an empty string, which is
        // still a row: an empty root would be a path of nothing at all.
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            state
                .data_dir
                .join("instances")
                .to_string_lossy()
                .to_string()
        });

    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: state.data_dir.to_string_lossy().to_string(),
        platform: std::env::consts::OS.to_string(),
        default_instance_root: default_root,
        total_ram_mb: total_ram_mb(),
    })
}

/// Physical RAM, in megabytes.
///
/// Only the total: what is free right now says nothing useful about what a
/// server can be given, because the OS hands back cache on demand.
fn total_ram_mb() -> i64 {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    (system.total_memory() / 1024 / 1024) as i64
}

#[tauri::command]
pub async fn settings_get_all(state: State<'_, AppState>) -> AppResult<HashMap<String, String>> {
    Ok(db::settings_all(&state.db).await?.into_iter().collect())
}

#[tauri::command]
pub async fn settings_set(
    state: State<'_, AppState>,
    key: String,
    value: String,
) -> AppResult<()> {
    db::setting_set(&state.db, &key, &value).await
}

/// Names of instances that are still alive. The frontend uses this for the
/// quit confirmation; closing the window only hides it to the tray.
#[tauri::command]
pub async fn live_instances(state: State<'_, AppState>) -> AppResult<Vec<String>> {
    let live = state.live_uuids();
    if live.is_empty() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for uuid in live {
        if let Ok(row) = instance::get_by_uuid(&state.db, &uuid).await {
            names.push(row.name);
        }
    }
    names.sort();
    Ok(names)
}

/// Quit for real. Phase 3 stops running servers gracefully here; for now the
/// only live instances are adopted orphans, which the user is warned about.
#[tauri::command]
pub async fn app_quit(app: AppHandle) -> AppResult<()> {
    tracing::info!("exiting on user request");
    app.exit(0);
    Ok(())
}
