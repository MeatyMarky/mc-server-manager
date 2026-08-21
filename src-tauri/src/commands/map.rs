//! The Map tab's commands: what is installed, installing it, and the port.

use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::events;
use crate::map::{self, MapStatus};
use crate::state::AppState;
use crate::{instance, mods};

#[tauri::command]
pub async fn map_status(state: State<'_, AppState>, id: i64) -> AppResult<MapStatus> {
    map::status(&state, id).await
}

/// Whether this server type can run squaremap, for the create dialog's box.
#[tauri::command]
pub async fn map_supported(server_type: crate::db::models::ServerType) -> AppResult<bool> {
    Ok(map::supported(server_type))
}

/// Installs squaremap through the ordinary mod path, then puts it on a port
/// nothing else is using.
///
/// Returns a task id; progress arrives as `task://progress` like every other
/// download, because this *is* a mod install — the only difference is that the
/// project was chosen by this app rather than browsed for.
#[tauri::command]
pub async fn map_install(app: AppHandle, state: State<'_, AppState>, id: i64) -> AppResult<String> {
    let (task_id, cancel) = state.tasks.register();
    let handle = app.clone();
    let returned = task_id.clone();

    tauri::async_runtime::spawn(async move {
        let state = handle.state::<AppState>();
        let result = map::install(&state, id, &cancel, |done, total, message| {
            events::task_progress(
                &handle,
                events::TaskProgressEvent {
                    task_id: task_id.clone(),
                    kind: "map_install".to_string(),
                    phase: "download".to_string(),
                    done,
                    total,
                    message,
                    instance_id: Some(id),
                },
            );
        })
        .await;

        state.tasks.finish(&task_id);
        events::task_done(
            &handle,
            events::TaskDoneEvent {
                task_id: task_id.clone(),
                kind: "map_install".to_string(),
                ok: result.is_ok(),
                cancelled: cancel.is_cancelled(),
                error: result.as_ref().err().map(|err| err.user_message()),
                error_kind: result.as_ref().err().map(|err| err.kind().to_string()),
                log_path: None,
                log_tail: result.as_ref().ok().cloned(),
                instance_id: Some(id),
            },
        );
        events::instances_changed(&handle);
    });

    Ok(returned)
}

/// Renders the parts of the world that were played before the map existed.
///
/// squaremap draws chunks as they are loaded and saved, so a world with history
/// behind it stays blank until it is asked. The command goes through the same
/// console path a typed one does, so it is echoed and its output is visible.
#[tauri::command]
pub async fn map_render_world(state: State<'_, AppState>, id: i64) -> AppResult<String> {
    let row = instance::get(&state.db, id).await?;
    if map::detect(&row)?.is_none() {
        return Err(AppError::Other(format!(
            "\"{}\" has no web map installed.",
            row.name
        )));
    }
    if !state.status_of(&row.uuid).is_live() {
        return Err(AppError::Other(format!(
            "\"{}\" has to be running for its map to render.",
            row.name
        )));
    }

    let command = map::render_command();
    crate::process::supervisor::send_command(&state, id, &command).await?;
    Ok(command)
}

/// Moves the map onto a free port, effective at the next start.
#[tauri::command]
pub async fn map_move_port(state: State<'_, AppState>, id: i64) -> AppResult<Option<u16>> {
    let row = instance::get(&state.db, id).await?;
    if state.status_of(&row.uuid).is_live() {
        // A running map rewrites its config on shutdown, so an edit now would
        // be thrown away — the same rule `server.properties` follows.
        return Err(AppError::Other(format!(
            "Stop \"{}\" before moving its map: a running server rewrites the file on shutdown.",
            row.name
        )));
    }
    map::move_to_free_port(&state, id).await
}

/// Removes the map mod again, and forgets the intent to have one.
#[tauri::command]
pub async fn map_uninstall(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let row = instance::get(&state.db, id).await?;
    let Some(found) = map::detect(&row)? else {
        return Ok(());
    };

    // The same removal the Mods tab performs, so the file and its row go
    // together and anything depending on it is reported the same way.
    mods::uninstall(&state, id, &found.file_name).await?;
    map::forget(&state, id).await
}
