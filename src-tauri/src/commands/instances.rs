use std::path::PathBuf;

use tauri::{AppHandle, State};

use crate::db::models::{InstanceStatus, InstanceView};
use crate::error::AppResult;
use crate::events;
use crate::instance::import::{ImportCandidate, ImportInstanceInput};
use crate::instance::{
    self, crud, CloneInstanceInput, CreateInstanceInput, DeleteReport, UpdateInstanceInput,
};
use crate::state::AppState;

#[tauri::command]
pub async fn instance_list(state: State<'_, AppState>) -> AppResult<Vec<InstanceView>> {
    let rows = instance::list(&state.db).await?;
    Ok(rows.iter().map(|i| state.view(i)).collect())
}

#[tauri::command]
pub async fn instance_get(state: State<'_, AppState>, id: i64) -> AppResult<InstanceView> {
    let row = instance::get(&state.db, id).await?;
    Ok(state.view(&row))
}

#[tauri::command]
pub async fn instance_create(
    app: AppHandle,
    state: State<'_, AppState>,
    input: CreateInstanceInput,
) -> AppResult<InstanceView> {
    let created = crud::create(&state, input).await?;
    events::instances_changed(&app);
    Ok(state.view(&created))
}

#[tauri::command]
pub async fn instance_clone(
    app: AppHandle,
    state: State<'_, AppState>,
    input: CloneInstanceInput,
) -> AppResult<InstanceView> {
    let created = crud::clone_instance(&state, input).await?;
    events::instances_changed(&app);
    Ok(state.view(&created))
}

#[tauri::command]
pub async fn instance_rename(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    name: String,
) -> AppResult<InstanceView> {
    let updated = crud::rename(&state, id, &name).await?;
    events::instances_changed(&app);
    Ok(state.view(&updated))
}

#[tauri::command]
pub async fn instance_update(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    input: UpdateInstanceInput,
) -> AppResult<InstanceView> {
    let updated = crud::update(&state, id, input).await?;
    events::instances_changed(&app);
    Ok(state.view(&updated))
}

#[tauri::command]
pub async fn instance_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    delete_files: bool,
) -> AppResult<DeleteReport> {
    let report = crud::delete(&state, id, delete_files).await?;
    events::instances_changed(&app);
    Ok(report)
}

/// Repoints a `Missing` instance after the user picks the folder it moved to.
#[tauri::command]
pub async fn instance_locate(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    path: String,
) -> AppResult<InstanceView> {
    let updated = crud::relocate(&state, id, &PathBuf::from(path)).await?;
    events::instances_changed(&app);
    Ok(state.view(&updated))
}

/// Builds the folder a new instance would get under `root`. Path construction
/// and name sanitization stay in Rust; the dialog only displays the result.
#[tauri::command]
pub async fn instance_suggest_path(root: String, name: String) -> AppResult<String> {
    let root = PathBuf::from(root);
    let folder = crate::paths::sanitize_folder_name(&name)?;
    Ok(crate::paths::unique_dir(&root, &folder)
        .to_string_lossy()
        .to_string())
}

/// Inspects a folder without changing anything, so the import dialog can show
/// what it found and let the user correct it.
#[tauri::command]
pub async fn instance_import_detect(path: String) -> AppResult<ImportCandidate> {
    let path = PathBuf::from(path);
    tokio::task::spawn_blocking(move || instance::import::detect(&path))
        .await
        .map_err(|e| crate::error::AppError::Other(format!("detection task failed: {e}")))?
}

#[tauri::command]
pub async fn instance_import(
    app: AppHandle,
    state: State<'_, AppState>,
    input: ImportInstanceInput,
) -> AppResult<InstanceView> {
    let imported = instance::import::import(&state, input).await?;
    events::instances_changed(&app);
    Ok(state.view(&imported))
}

/// Hard-kills an orphan adopted at startup. There is no stdin to send `stop` to,
/// so the UI presents this as "Force stop".
#[tauri::command]
pub async fn instance_force_stop(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<InstanceView> {
    instance::reconcile::force_stop_orphan(&state, id).await?;
    let updated = instance::get(&state.db, id).await?;
    events::instance_status(&app, &updated.uuid, InstanceStatus::Stopped, None);
    events::instances_changed(&app);
    Ok(state.view(&updated))
}

#[tauri::command]
pub async fn instance_open_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;

    let row = instance::get(&state.db, id).await?;
    let path = row.path_buf();
    if !path.is_dir() {
        return Err(crate::error::AppError::FolderMissing {
            name: row.name,
            path,
        });
    }
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| crate::error::AppError::Other(format!("could not open the folder: {e}")))
}
