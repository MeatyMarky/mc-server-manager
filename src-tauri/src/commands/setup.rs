//! Commands for getting a server *into* an instance: version lists, installs,
//! the EULA gate, and task cancellation.

use tauri::{AppHandle, Manager, State};

use crate::db::models::ServerType;
use crate::error::{AppError, AppResult};
use crate::events;
use crate::instance::{self, eula::EulaStatus, install};
use crate::providers::{self, BuildEntry, VersionEntry};
use crate::state::AppState;

#[tauri::command]
pub async fn provider_versions(
    state: State<'_, AppState>,
    server_type: ServerType,
) -> AppResult<Vec<VersionEntry>> {
    // Release chronology first: it decides the order of everything below.
    let index = providers::index::ensure_fresh(&state.db, &state.http).await?;
    providers::list_versions(server_type, &state.http, &index).await
}

#[tauri::command]
pub async fn provider_builds(
    state: State<'_, AppState>,
    server_type: ServerType,
    mc_version: String,
) -> AppResult<Vec<BuildEntry>> {
    providers::list_builds(server_type, &state.http, &mc_version).await
}

/// Starts an install and returns its task id immediately. Progress arrives as
/// `task://progress`, the outcome as `task://done`.
#[tauri::command]
pub async fn install_server(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    mc_version: String,
    build: Option<String>,
) -> AppResult<String> {
    // Fail fast on the obvious problems, before a task id exists.
    let instance = instance::get(&state.db, id).await?;
    if state.status_of(&instance.uuid).is_live() {
        return Err(AppError::InstanceRunning(instance.name));
    }

    let (task_id, cancel) = state.tasks.register();
    let handle = app.clone();
    let returned = task_id.clone();

    tauri::async_runtime::spawn(async move {
        let state = handle.state::<AppState>();
        let progress_handle = handle.clone();
        let progress_task = task_id.clone();

        let result = install::install(
            &state,
            &state.http,
            &instance,
            &mc_version,
            build.as_deref(),
            &cancel,
            |phase, done, total, message| {
                events::task_progress(
                    &progress_handle,
                    events::TaskProgressEvent {
                        task_id: progress_task.clone(),
                        kind: "install".to_string(),
                        phase: phase.as_str().to_string(),
                        done,
                        total,
                        message,
                        instance_id: Some(id),
                    },
                );
            },
        )
        .await;

        // A web map asked for at create time is installed now, because now is
        // when the folder it goes into exists. A failure here is reported on
        // its own rather than failing the server install, which worked.
        if result.is_ok() {
            if let Ok(row) = instance::get(&state.db, id).await {
                if let Some(kind) = crate::map::wanted(&row) {
                    if crate::map::detect(&row).ok().flatten().is_none() {
                        let map_cancel = tokio_util::sync::CancellationToken::new();
                        match crate::map::install(&state, id, kind, &map_cancel, |_, _, _| {}).await
                        {
                            Ok(message) => tracing::info!(
                                instance = %row.name,
                                instance_id = id,
                                "{message}"
                            ),
                            Err(err) => tracing::warn!(
                                instance = %row.name,
                                instance_id = id,
                                error = %err,
                                "the web map could not be installed; the server itself is fine"
                            ),
                        }
                    }
                }
            }
        }

        // install() already recorded the outcome on the instance row.
        let done = match result {
            Ok(_) => events::TaskDoneEvent {
                task_id: task_id.clone(),
                kind: "install".into(),
                ok: true,
                cancelled: false,
                error: None,
                error_kind: None,
                log_path: None,
                log_tail: None,
                instance_id: Some(id),
            },
            Err(err) => done_from_error(&task_id, id, err),
        };

        state.tasks.finish(&task_id);
        events::task_done(&handle, done);
        events::instances_changed(&handle);
    });

    Ok(returned)
}

/// Installer failures carry their log through to the UI rather than collapsing
/// into a generic message.
fn done_from_error(task_id: &str, instance_id: i64, err: AppError) -> events::TaskDoneEvent {
    let cancelled = matches!(err, AppError::Cancelled);
    let (log_path, log_tail) = match &err {
        AppError::InstallerFailed {
            log_path, log_tail, ..
        } => (Some(log_path.clone()), Some(log_tail.clone())),
        _ => (None, None),
    };

    events::TaskDoneEvent {
        task_id: task_id.to_string(),
        kind: "install".into(),
        ok: false,
        cancelled,
        error: Some(err.to_string()),
        error_kind: Some(err.kind().to_string()),
        log_path,
        log_tail,
        instance_id: Some(instance_id),
    }
}

/// Cancels a running task. Returns false when it had already finished.
#[tauri::command]
pub async fn task_cancel(state: State<'_, AppState>, task_id: String) -> AppResult<bool> {
    Ok(state.tasks.cancel(&task_id))
}

#[tauri::command]
pub async fn eula_get(state: State<'_, AppState>, id: i64) -> AppResult<EulaStatus> {
    instance::eula::status(&state, id).await
}

/// Only ever called from an explicit user action in the EULA dialog.
#[tauri::command]
pub async fn eula_set(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    accepted: bool,
) -> AppResult<EulaStatus> {
    let status = instance::eula::set(&state, id, accepted).await?;
    events::instances_changed(&app);
    Ok(status)
}

/// Reads the installer log the UI was told about, so the failure state can show
/// more than the tail.
#[tauri::command]
pub async fn read_installer_log(path: String) -> AppResult<String> {
    let path = std::path::PathBuf::from(path);
    let text = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| AppError::io("read installer log", &path, e))?;
    Ok(text)
}
