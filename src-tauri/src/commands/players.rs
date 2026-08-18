//! Ops, whitelist and bans. Every mutation goes through `players::mutate`.

use tauri::{AppHandle, State};

use crate::error::AppResult;
use crate::events;
use crate::players::{self, MutationReport, PlayerLists};
use crate::state::AppState;

#[tauri::command]
pub async fn players_read(state: State<'_, AppState>, id: i64) -> AppResult<PlayerLists> {
    players::lists(&state, id).await
}

/// One change to one list. The gate decides whether it goes over stdin or to
/// the file, and reports which happened.
#[tauri::command]
pub async fn players_mutate(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    mutation: players::Mutation,
) -> AppResult<MutationReport> {
    let report = players::mutate(&state, id, mutation).await?;
    events::instances_changed(&app);
    Ok(report)
}

/// Resolves a name to a Mojang UUID, falling back to the offline UUID. Used by
/// the "add player" field so the UI can show which one it got.
#[tauri::command]
pub async fn players_resolve_uuid(
    state: State<'_, AppState>,
    name: String,
) -> AppResult<(String, bool)> {
    Ok(players::files::resolve_uuid(&state.http, name.trim()).await)
}
