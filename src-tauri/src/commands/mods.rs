//! Mods and plugins: search, plan, install, manage, and pack import.
//!
//! Long operations (installing a plan, importing a pack) register a task id and
//! report through `task://progress`, exactly like a server install.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::events;
use crate::instance;
use crate::mods::{
    self, modrinth::Modrinth, mrpack, resolve, source::ModSource, InstallPlan, Loader, ModView,
    ModsView, Project, SearchQuery, SourceVersion, VersionFilter,
};
use crate::providers;
use crate::state::AppState;

/// The source to use. One implementation today; adding CurseForge means adding
/// a variant here, not changing the callers.
fn source(state: &AppState) -> AppResult<Modrinth> {
    Modrinth::new(state.rate_limiter.clone())
}

#[tauri::command]
pub async fn mods_list(state: State<'_, AppState>, id: i64) -> AppResult<ModsView> {
    mods::list(&state, id).await
}

/// Searches, filtered by the instance's loader and Minecraft version.
#[tauri::command]
pub async fn mods_search(
    state: State<'_, AppState>,
    id: i64,
    text: String,
    limit: Option<u32>,
    offset: Option<u32>,
) -> AppResult<Vec<Project>> {
    let row = instance::get(&state.db, id).await?;
    let loader = mods::loader_of(row.server_type, &row.name)?;

    source(&state)?
        .search(&SearchQuery {
            text,
            loaders: loader
                .accepted()
                .iter()
                .map(|loader| loader.to_string())
                .collect(),
            game_versions: vec![row.mc_version],
            limit,
            offset,
        })
        .await
}

/// Versions of a project that suit this instance, newest first.
#[tauri::command]
pub async fn mods_versions(
    state: State<'_, AppState>,
    id: i64,
    project_id: String,
) -> AppResult<Vec<SourceVersion>> {
    let row = instance::get(&state.db, id).await?;
    let loader = mods::loader_of(row.server_type, &row.name)?;
    let index = providers::index::ensure_fresh(&state.db, &state.http).await?;

    let mut versions = source(&state)?
        .versions(
            &project_id,
            &VersionFilter {
                loaders: loader
                    .accepted()
                    .iter()
                    .map(|loader| loader.to_string())
                    .collect(),
                game_versions: vec![row.mc_version],
            },
        )
        .await?;
    mods::modrinth::sort_versions(&mut versions, &index);
    Ok(versions)
}

/// Resolves the dependency tree for a version. Nothing is downloaded here: the
/// UI shows this and asks the user to confirm.
#[tauri::command]
pub async fn mods_plan(
    state: State<'_, AppState>,
    id: i64,
    project_id: String,
    version_id: Option<String>,
) -> AppResult<InstallPlan> {
    let row = instance::get(&state.db, id).await?;
    let loader = mods::loader_of(row.server_type, &row.name)?;
    let index = providers::index::ensure_fresh(&state.db, &state.http).await?;
    let source = source(&state)?;

    let root = match version_id {
        Some(version_id) => source.version(&version_id).await?,
        None => {
            let versions = source
                .versions(
                    &project_id,
                    &VersionFilter {
                        loaders: loader
                            .accepted()
                            .iter()
                            .map(|loader| loader.to_string())
                            .collect(),
                        game_versions: vec![row.mc_version.clone()],
                    },
                )
                .await?;
            resolve::pick_version(&versions, loader, &row.mc_version, &index).ok_or_else(|| {
                AppError::Other(format!(
                    "no build of this project supports {} on Minecraft {}",
                    loader.as_str(),
                    row.mc_version
                ))
            })?
        }
    };

    let installed = mods::installed(&state, id).await?;
    resolve::plan(&source, root, loader, &row.mc_version, &index, &installed).await
}

/// Installs a confirmed plan in the background.
#[tauri::command]
pub async fn mods_install(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    plan: InstallPlan,
) -> AppResult<String> {
    let row = instance::get(&state.db, id).await?;
    mods::loader_of(row.server_type, &row.name)?;

    let (task_id, cancel) = state.tasks.register();
    let handle = app.clone();
    let returned = task_id.clone();

    tauri::async_runtime::spawn(async move {
        let state = handle.state::<AppState>();
        let total = plan.install.len() as u64;
        let mut outcome = Ok(());

        for (position, planned) in plan.install.iter().enumerate() {
            if cancel.is_cancelled() {
                outcome = Err(AppError::Cancelled);
                break;
            }
            if planned.already_installed {
                continue;
            }

            events::task_progress(
                &handle,
                events::TaskProgressEvent {
                    task_id: task_id.clone(),
                    kind: "mod_install".to_string(),
                    phase: "download".to_string(),
                    done: position as u64,
                    total: Some(total),
                    message: format!("Installing {}", planned.project_title),
                    instance_id: Some(id),
                },
            );

            let source = match source(&state) {
                Ok(source) => source,
                Err(err) => {
                    outcome = Err(err);
                    break;
                }
            };
            let version = match source.version(&planned.version_id).await {
                Ok(version) => version,
                Err(err) => {
                    outcome = Err(err);
                    break;
                }
            };
            if let Err(err) = mods::install_planned(&state, id, planned, &version, &cancel).await {
                outcome = Err(err);
                break;
            }
        }

        state.tasks.finish(&task_id);
        events::task_done(&handle, done_event(&task_id, "mod_install", id, outcome));
        events::instances_changed(&handle);
    });

    Ok(returned)
}

#[tauri::command]
pub async fn mods_set_enabled(
    state: State<'_, AppState>,
    id: i64,
    file_name: String,
    enabled: bool,
) -> AppResult<ModsView> {
    mods::set_enabled(&state, id, &file_name, enabled).await?;
    mods::list(&state, id).await
}

#[tauri::command]
pub async fn mods_set_pinned(
    state: State<'_, AppState>,
    id: i64,
    file_name: String,
    pinned: bool,
) -> AppResult<ModsView> {
    mods::set_pinned(&state, id, &file_name, pinned).await?;
    mods::list(&state, id).await
}

/// Removes a jar. The returned list names anything that depended on it.
#[tauri::command]
pub async fn mods_uninstall(
    state: State<'_, AppState>,
    id: i64,
    file_name: String,
) -> AppResult<Vec<String>> {
    mods::uninstall(&state, id, &file_name).await
}

/// Drag-and-drop or "add a jar" install.
#[tauri::command]
pub async fn mods_install_local(
    state: State<'_, AppState>,
    id: i64,
    path: String,
) -> AppResult<ModView> {
    mods::install_local(&state, id, &PathBuf::from(path)).await
}

/// Checks every tracked, unpinned mod for a newer version and records it.
#[tauri::command]
pub async fn mods_check_updates(state: State<'_, AppState>, id: i64) -> AppResult<ModsView> {
    let row = instance::get(&state.db, id).await?;
    let loader = mods::loader_of(row.server_type, &row.name)?;
    let index = providers::index::ensure_fresh(&state.db, &state.http).await?;

    mods::check_updates(&state, id, &source(&state)?, loader, &row.mc_version, &index).await?;
    mods::list(&state, id).await
}

/// Reads a `.mrpack` and reports what importing it would do.
#[tauri::command]
pub async fn mrpack_plan(
    state: State<'_, AppState>,
    id: i64,
    archive: String,
) -> AppResult<mrpack::PackPlan> {
    let row = instance::get(&state.db, id).await?;
    let path = PathBuf::from(archive);

    let index = tokio::task::spawn_blocking(move || mrpack::read_index(&path))
        .await
        .map_err(|e| AppError::internal("reading the pack", e))??;
    mrpack::plan(index, row.server_type, &row.mc_version)
}

/// Imports a `.mrpack`: staged, verified, then committed.
#[tauri::command]
pub async fn mrpack_import(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    archive: String,
) -> AppResult<String> {
    let archive = PathBuf::from(archive);
    let (task_id, cancel) = state.tasks.register();
    let handle = app.clone();
    let returned = task_id.clone();

    tauri::async_runtime::spawn(async move {
        let state = handle.state::<AppState>();
        let progress_handle = handle.clone();
        let progress_task = task_id.clone();

        let outcome = mrpack::import(&state, id, &archive, &cancel, |progress, file| {
            events::task_progress(
                &progress_handle,
                events::TaskProgressEvent {
                    task_id: progress_task.clone(),
                    kind: "mrpack_import".to_string(),
                    phase: "download".to_string(),
                    done: progress.done,
                    total: Some(progress.total),
                    message: format!("Fetching {file}"),
                    instance_id: Some(id),
                },
            );
        })
        .await;

        state.tasks.finish(&task_id);
        events::task_done(
            &handle,
            done_event(&task_id, "mrpack_import", id, outcome.map(|_| ())),
        );
        events::instances_changed(&handle);
    });

    Ok(returned)
}

fn done_event(
    task_id: &str,
    kind: &str,
    instance_id: i64,
    outcome: AppResult<()>,
) -> events::TaskDoneEvent {
    match outcome {
        Ok(()) => events::TaskDoneEvent {
            task_id: task_id.to_string(),
            kind: kind.to_string(),
            ok: true,
            cancelled: false,
            error: None,
            error_kind: None,
            log_path: None,
            log_tail: None,
            instance_id: Some(instance_id),
        },
        Err(err) => events::TaskDoneEvent {
            task_id: task_id.to_string(),
            kind: kind.to_string(),
            ok: false,
            cancelled: matches!(err, AppError::Cancelled),
            error: Some(err.to_string()),
            error_kind: Some(err.kind().to_string()),
            log_path: None,
            log_tail: None,
            instance_id: Some(instance_id),
        },
    }
}

/// The loader an instance uses, for the UI's labels. `None` means vanilla.
#[tauri::command]
pub async fn mods_loader(state: State<'_, AppState>, id: i64) -> AppResult<Option<Loader>> {
    let row = instance::get(&state.db, id).await?;
    Ok(Loader::for_server_type(row.server_type))
}
