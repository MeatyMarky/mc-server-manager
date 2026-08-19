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
    self, mrpack, resolve, source::ModSource, AnySource, Category, ContentType, InstallPlan,
    Loader, ModView, ModsView, SearchPage, SearchQuery, SortBy, SourceId, SourceVersion,
    VersionFilter,
};
use crate::providers;
use crate::state::AppState;

/// What a source can do right now, for the picker.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct SourceStatus {
    pub id: SourceId,
    pub name: String,
    /// False when the source needs something before it can be used.
    pub configured: bool,
    /// What is missing, and what to do about it.
    pub needs: Option<String>,
    /// Where to get what it needs.
    pub setup_url: Option<String>,
}

/// The two implementations, side by side. A source that needs a key is listed
/// whether or not one is set: hiding it would look like the app cannot do it.
#[tauri::command]
pub async fn mods_sources(state: State<'_, AppState>) -> AppResult<Vec<SourceStatus>> {
    let key = curseforge_key(&state).await;

    Ok(vec![
        SourceStatus {
            id: SourceId::Modrinth,
            name: "Modrinth".into(),
            configured: true,
            needs: None,
            setup_url: None,
        },
        SourceStatus {
            id: SourceId::CurseForge,
            name: "CurseForge".into(),
            configured: key.is_some(),
            needs: key.is_none().then(|| {
                "CurseForge requires every application to use its own API key, so this app \
                 cannot ship one. Create a free key and paste it into Settings."
                    .to_string()
            }),
            setup_url: Some(crate::mods::curseforge::KEY_URL.to_string()),
        },
    ])
}

async fn curseforge_key(state: &AppState) -> Option<String> {
    crate::db::setting_get(&state.db, crate::mods::curseforge::KEY_SETTING)
        .await
        .ok()
        .flatten()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

#[tauri::command]
pub async fn mods_list(state: State<'_, AppState>, id: i64) -> AppResult<ModsView> {
    mods::list(&state, id).await
}

/// One page of the browser.
///
/// Filtered to the instance's loader and Minecraft version by default; the UI
/// can ask for everything instead, which is what the "show all" toggle sends.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn mods_search(
    state: State<'_, AppState>,
    id: i64,
    source: SourceId,
    text: String,
    content_type: ContentType,
    sort: SortBy,
    categories: Vec<String>,
    filter_to_instance: bool,
    limit: Option<u32>,
    offset: Option<u32>,
) -> AppResult<SearchPage> {
    let row = instance::get(&state.db, id).await?;

    // A client-only kind has no loader to filter by, and a pack is not filtered
    // by the loader of the instance browsing for it.
    let filtering = filter_to_instance
        && !content_type.is_client_only()
        && content_type != ContentType::Modpack;

    let loaders = if filtering {
        Loader::for_server_type(row.server_type)
            .map(|loader| {
                loader
                    .accepted()
                    .iter()
                    .map(|loader| loader.to_string())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let query = SearchQuery {
        text,
        loaders,
        game_versions: if filter_to_instance {
            vec![row.mc_version]
        } else {
            Vec::new()
        },
        limit,
        offset,
        sort,
        categories,
        content_type,
    };

    AnySource::build(&state, source).await?.search(&query).await
}

/// The local file for a project's icon, fetching it once.
///
/// Returns `null` for a project with no icon; the card draws a placeholder
/// rather than an empty box.
#[tauri::command]
pub async fn mods_icon(state: State<'_, AppState>, url: Option<String>) -> AppResult<Option<String>> {
    let cached = mods::icons::ensure_cached(&state.http, &state.data_dir, url.as_deref()).await?;
    Ok(cached.map(|path| path.to_string_lossy().to_string()))
}

/// The categories a source offers for a kind of content.
#[tauri::command]
pub async fn mods_categories(
    state: State<'_, AppState>,
    source: SourceId,
    content_type: ContentType,
) -> AppResult<Vec<Category>> {
    AnySource::build(&state, source)
        .await?
        .categories(content_type)
        .await
}

/// The content kinds worth offering for this instance, and whether each is
/// something it could install.
#[tauri::command]
pub async fn mods_content_types(
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<Vec<ContentTypeOption>> {
    let row = instance::get(&state.db, id).await?;
    Ok(ContentType::for_server_type(row.server_type)
        .into_iter()
        .map(|content_type| ContentTypeOption {
            content_type,
            installable: content_type.installable_on(row.server_type),
            client_only: content_type.is_client_only(),
        })
        .collect())
}

/// One entry of the content-type dropdown.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct ContentTypeOption {
    pub content_type: ContentType,
    /// False for what this server cannot load — shown, and marked.
    pub installable: bool,
    pub client_only: bool,
}

/// Versions of a project that suit this instance, newest first.
#[tauri::command]
pub async fn mods_versions(
    state: State<'_, AppState>,
    id: i64,
    source: SourceId,
    project_id: String,
) -> AppResult<Vec<SourceVersion>> {
    let row = instance::get(&state.db, id).await?;
    let loader = mods::loader_of(row.server_type, &row.name)?;
    let index = providers::index::ensure_fresh(&state.db, &state.http).await?;
    let filter = VersionFilter {
        loaders: loader
            .accepted()
            .iter()
            .map(|loader| loader.to_string())
            .collect(),
        game_versions: vec![row.mc_version],
    };

    let mut versions = AnySource::build(&state, source)
        .await?
        .versions(&project_id, &filter)
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
    source: SourceId,
    project_id: String,
    version_id: Option<String>,
) -> AppResult<InstallPlan> {
    let row = instance::get(&state.db, id).await?;
    let loader = mods::loader_of(row.server_type, &row.name)?;
    let index = providers::index::ensure_fresh(&state.db, &state.http).await?;
    let source = AnySource::build(&state, source).await?;

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

            let source = match AnySource::build(&state, planned.source).await {
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

    // Per source: a version id from one means nothing to the other, and an
    // unconfigured CurseForge simply has nothing of its own to check.
    for source_id in [SourceId::Modrinth, SourceId::CurseForge] {
        match AnySource::build(&state, source_id).await {
            Ok(source) => {
                mods::check_updates(
                    &state,
                    id,
                    source_id,
                    &source,
                    loader,
                    &row.mc_version,
                    &index,
                )
                .await?;
            }
            Err(err) => tracing::debug!(error = %err, "no update check for this source"),
        }
    }
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
