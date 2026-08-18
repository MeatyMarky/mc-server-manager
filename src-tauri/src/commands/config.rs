//! `server.properties` editing.

use tauri::{AppHandle, State};

use crate::config::{self, KeyInfo, PropertiesUpdate, PropertiesView, SaveReport};
use crate::error::AppResult;
use crate::events;
use crate::state::AppState;

#[tauri::command]
pub async fn properties_read(state: State<'_, AppState>, id: i64) -> AppResult<PropertiesView> {
    config::view(&state, id).await
}

/// Applies changes atomically, keeping comments, ordering and unknown keys.
#[tauri::command]
pub async fn properties_write(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    input: PropertiesUpdate,
) -> AppResult<SaveReport> {
    let report = config::save(&state, id, input).await?;
    if !report.changed.is_empty() {
        events::instances_changed(&app);
    }
    Ok(report)
}

/// The metadata the editor uses to pick a control per key.
#[tauri::command]
pub async fn properties_schema() -> AppResult<Vec<KeyInfo>> {
    Ok(config::schema::known_keys())
}
