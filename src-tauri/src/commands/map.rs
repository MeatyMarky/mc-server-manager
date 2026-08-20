//! The Map tab's commands: what is installed, installing one, and the port.

use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::events;
use crate::map::{self, MapKind, MapStatus};
use crate::state::AppState;
use crate::{instance, mods};

#[tauri::command]
pub async fn map_status(state: State<'_, AppState>, id: i64) -> AppResult<MapStatus> {
    map::status(&state, id).await
}

/// The maps a server type can run, for the create dialog's checkbox.
#[tauri::command]
pub async fn map_kinds_for(server_type: crate::db::models::ServerType) -> AppResult<Vec<MapKind>> {
    Ok(map::kinds_for(server_type))
}

/// Installs a map mod through the ordinary mod path, then puts it on a port
/// nothing else is using.
///
/// Returns a task id; progress arrives as `task://progress` like every other
/// download, because this *is* a mod install — the only difference is that the
/// project was chosen by this app rather than browsed for.
#[tauri::command]
pub async fn map_install(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    kind: MapKind,
) -> AppResult<String> {
    let row = instance::get(&state.db, id).await?;
    if !kind.supports(row.server_type) {
        return Err(AppError::Other(format!(
            "{} does not run on a {} server.",
            kind.label(),
            row.server_type.label()
        )));
    }

    let (task_id, cancel) = state.tasks.register();
    let handle = app.clone();
    let returned = task_id.clone();

    tauri::async_runtime::spawn(async move {
        let state = handle.state::<AppState>();
        let result = map::install(&state, id, kind, &cancel, |done, total, message| {
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

/// Lets BlueMap download the resources it renders with.
///
/// Its own default is to refuse, because the download is a Minecraft client jar
/// from Mojang: turning it on says the user owns Minecraft: Java Edition and
/// accepts Mojang's EULA. So it happens on a click, next to a sentence saying
/// exactly that, and never on this app's own initiative.
#[tauri::command]
pub async fn map_accept_download(state: State<'_, AppState>, id: i64) -> AppResult<bool> {
    let row = instance::get(&state.db, id).await?;
    crate::map::config::accept_download(&row).await
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
