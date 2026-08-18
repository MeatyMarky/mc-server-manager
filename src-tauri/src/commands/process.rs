//! Start, stop, restart, console.

use tauri::{AppHandle, State};

use crate::db::models::InstanceView;
use crate::error::AppResult;
use crate::events;
use crate::instance;
use crate::logparse::ParsedLine;
use crate::process::{port, supervisor, StopStage};
use crate::state::AppState;

#[tauri::command]
pub async fn instance_start(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<InstanceView> {
    supervisor::start(&app, &state, id).await?;
    let row = instance::get(&state.db, id).await?;
    events::instances_changed(&app);
    Ok(state.view(&row))
}

/// Graceful stop. The returned stage says how far it had to go: `graceful`,
/// `terminated` or `killed`.
#[tauri::command]
pub async fn instance_stop(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<StopStage> {
    supervisor::stop(&app, &state, id).await
}

#[tauri::command]
pub async fn instance_kill(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<StopStage> {
    supervisor::kill(&app, &state, id).await
}

#[tauri::command]
pub async fn instance_restart(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<InstanceView> {
    supervisor::restart(&app, &state, id).await?;
    let row = instance::get(&state.db, id).await?;
    events::instances_changed(&app);
    Ok(state.view(&row))
}

#[tauri::command]
pub async fn instance_send_command(
    state: State<'_, AppState>,
    id: i64,
    command: String,
) -> AppResult<()> {
    supervisor::send_command(&state, id, &command).await
}

/// The last `count` console lines held in memory. New lines arrive as
/// `instance://console` events; this is only for the initial paint.
#[tauri::command]
pub async fn console_tail(
    state: State<'_, AppState>,
    id: i64,
    count: Option<usize>,
) -> AppResult<Vec<ParsedLine>> {
    let row = instance::get(&state.db, id).await?;
    Ok(state
        .supervisor
        .tail(&row.uuid, count.unwrap_or(500).min(5_000)))
}

#[tauri::command]
pub async fn command_history(state: State<'_, AppState>, id: i64) -> AppResult<Vec<String>> {
    supervisor::command_history(&state, id).await
}

/// What port this instance would bind, and whether anything holds it. Shown
/// before the user hits Start rather than 20 seconds into a failed boot.
#[tauri::command]
pub async fn port_status(state: State<'_, AppState>, id: i64) -> AppResult<Option<String>> {
    let row = instance::get(&state.db, id).await?;
    let check = port::check(&state.db, id, &row.path_buf(), &state.live_uuids()).await?;
    Ok(check.message())
}
