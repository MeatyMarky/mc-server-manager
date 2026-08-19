//! Browsing modpacks, and installing one as a new server.
//!
//! Packs are not browsed inside an instance: installing one *creates* the
//! instance, with the loader, Minecraft version and Java the pack needs.

use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::events;
use crate::mods::{AnySource, ContentType, ModSource, SearchPage, SearchQuery, SortBy, SourceId};
use crate::packs::{self, InstallPackInput, PackDetail};
use crate::state::AppState;

/// One page of modpacks.
///
/// `server_only` asks the source to leave out what it knows cannot run on a
/// server. Modrinth can answer that; CurseForge cannot, so its packs are all
/// returned and each one's index decides when it is opened.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn packs_search(
    state: State<'_, AppState>,
    source: SourceId,
    text: String,
    sort: SortBy,
    categories: Vec<String>,
    game_versions: Vec<String>,
    server_only: bool,
    limit: Option<u32>,
    offset: Option<u32>,
) -> AppResult<SearchPage> {
    let query = SearchQuery {
        text,
        loaders: Vec::new(),
        game_versions,
        limit,
        offset,
        sort,
        categories,
        content_type: ContentType::Modpack,
    };

    let mut page = AnySource::build(&state, source).await?.search(&query).await?;
    if server_only {
        // Only what the source positively rules out is dropped here: a pack it
        // says nothing about stays, and its index answers later.
        page.projects
            .retain(|project| packs::declared_support(project) != packs::ServerSupport::No);
    }
    Ok(page)
}

/// Reads a pack's index and says whether it can run as a server.
///
/// Downloads the pack file to do it, because the index is the only real answer
/// — but not when the source has already said no.
#[tauri::command]
pub async fn pack_examine(
    state: State<'_, AppState>,
    source: SourceId,
    project_id: String,
    version_id: String,
) -> AppResult<PackDetail> {
    let (task_id, cancel) = state.tasks.register();
    let detail = packs::examine(&state, source, &project_id, &version_id, &cancel).await;
    state.tasks.finish(&task_id);
    detail
}

/// The versions of a pack, newest first, without an instance to filter against.
#[tauri::command]
pub async fn pack_versions(
    state: State<'_, AppState>,
    source: SourceId,
    project_id: String,
) -> AppResult<Vec<crate::mods::SourceVersion>> {
    let versions = AnySource::build(&state, source)
        .await?
        .versions(&project_id, &crate::mods::VersionFilter::default())
        .await?;
    Ok(versions)
}

/// Installs a pack as a new instance. Returns a task id; the new instance's id
/// arrives on `task://done`.
#[tauri::command]
pub async fn pack_install(
    app: AppHandle,
    state: State<'_, AppState>,
    input: InstallPackInput,
) -> AppResult<String> {
    let (task_id, cancel) = state.tasks.register();
    let handle = app.clone();
    let returned = task_id.clone();

    tauri::async_runtime::spawn(async move {
        let progress_handle = handle.clone();
        let progress_task = task_id.clone();

        let state = handle.state::<AppState>();
        let result = packs::install(&state, input, &cancel, move |message, done, total| {
            events::task_progress(
                &progress_handle,
                events::TaskProgressEvent {
                    task_id: progress_task.clone(),
                    kind: "pack_install".to_string(),
                    phase: "install".to_string(),
                    done,
                    total,
                    message: message.to_string(),
                    instance_id: None,
                },
            );
        })
        .await;

        state.tasks.finish(&task_id);
        events::instances_changed(&handle);

        let done = match result {
            Ok(instance_id) => events::TaskDoneEvent {
                task_id: task_id.clone(),
                kind: "pack_install".into(),
                ok: true,
                cancelled: false,
                error: None,
                error_kind: None,
                log_path: None,
                log_tail: None,
                instance_id: Some(instance_id),
            },
            Err(err) => events::TaskDoneEvent {
                task_id: task_id.clone(),
                kind: "pack_install".into(),
                ok: false,
                cancelled: matches!(err, AppError::Cancelled),
                error: Some(err.user_message()),
                error_kind: Some(err.kind().to_string()),
                log_path: None,
                log_tail: None,
                instance_id: None,
            },
        };
        events::task_done(&handle, done);
    });

    Ok(returned)
}
