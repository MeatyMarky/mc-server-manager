//! Worlds: listing, sizing, switching, import/export and deletion.
//!
//! Sizing and the archive operations are long enough to need a task id, progress
//! events and cancellation, exactly like an install.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::events;
use crate::instance;
use crate::state::AppState;
use crate::worlds::{self, World};

#[tauri::command]
pub async fn worlds_list(state: State<'_, AppState>, id: i64) -> AppResult<Vec<World>> {
    worlds::list(&state, id).await
}

/// Measures a world in the background. Progress arrives as `task://progress`
/// with `phase = "measure"`, the total as `task://done`.
#[tauri::command]
pub async fn world_measure(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    folder: String,
) -> AppResult<String> {
    let row = instance::get(&state.db, id).await?;
    let dir = worlds::resolve_world(&row.path_buf(), &folder)?;

    let (task_id, cancel) = state.tasks.register();
    let handle = app.clone();
    let returned = task_id.clone();

    tauri::async_runtime::spawn(async move {
        let progress_handle = handle.clone();
        let progress_task = task_id.clone();
        let label = folder.clone();

        // Walking a big world is thousands of stat calls: never on the runtime.
        let result = tokio::task::spawn_blocking(move || {
            worlds::measure(&dir, &cancel, |files, bytes| {
                events::task_progress(
                    &progress_handle,
                    events::TaskProgressEvent {
                        task_id: progress_task.clone(),
                        kind: "measure".to_string(),
                        phase: "measure".to_string(),
                        done: bytes,
                        total: None,
                        message: format!("{label}: {files} files"),
                        instance_id: Some(id),
                    },
                );
            })
        })
        .await;

        let state = handle.state::<AppState>();
        state.tasks.finish(&task_id);

        let done = match result {
            Ok(Ok(total)) => events::TaskDoneEvent {
                task_id: task_id.clone(),
                kind: "measure".into(),
                ok: true,
                cancelled: false,
                error: None,
                error_kind: None,
                log_path: None,
                // The measured size travels in log_tail so the UI can show it
                // without another round trip.
                log_tail: Some(total.to_string()),
                instance_id: Some(id),
            },
            Ok(Err(err)) => done_event(&task_id, "measure", id, err),
            Err(err) => done_event(
                &task_id,
                "measure",
                id,
                AppError::Other(format!("measuring failed: {err}")),
            ),
        };
        events::task_done(&handle, done);
    });

    Ok(returned)
}

#[tauri::command]
pub async fn world_switch(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    folder: String,
) -> AppResult<()> {
    worlds::switch(&state, id, &folder).await?;
    events::instances_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn world_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    folder: String,
) -> AppResult<()> {
    worlds::delete(&state, id, &folder).await?;
    events::instances_changed(&app);
    Ok(())
}

/// Zips a world to the path the user picked. Cancellable.
#[tauri::command]
pub async fn world_export(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    folder: String,
    target: String,
) -> AppResult<String> {
    let row = instance::get(&state.db, id).await?;
    let world_dir = worlds::resolve_world(&row.path_buf(), &folder)?;
    let target = PathBuf::from(target);

    let (task_id, cancel) = state.tasks.register();
    let handle = app.clone();
    let returned = task_id.clone();

    tauri::async_runtime::spawn(async move {
        let progress_handle = handle.clone();
        let progress_task = task_id.clone();

        let result = tokio::task::spawn_blocking(move || {
            worlds::archive::export(&world_dir, &target, &cancel, |progress| {
                events::task_progress(
                    &progress_handle,
                    events::TaskProgressEvent {
                        task_id: progress_task.clone(),
                        kind: "world_export".to_string(),
                        phase: "archive".to_string(),
                        done: progress.entries_done,
                        total: Some(progress.entries_total),
                        message: format!("Archiving {folder}"),
                        instance_id: Some(id),
                    },
                );
            })
        })
        .await;

        let outcome = match result {
            Ok(Ok(_bytes)) => Ok(()),
            Ok(Err(err)) => Err(err),
            Err(err) => Err(AppError::Other(format!("export task failed: {err}"))),
        };
        finish(&handle, task_id, "world_export", id, outcome);
    });

    Ok(returned)
}

/// Unpacks a world zip into the instance. Cancellable; a cancelled import
/// removes what it had written.
#[tauri::command]
pub async fn world_import(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    archive: String,
    folder: Option<String>,
) -> AppResult<String> {
    let row = instance::get(&state.db, id).await?;
    let dir = row.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: row.name,
            path: dir,
        });
    }
    let archive = PathBuf::from(archive);

    let (task_id, cancel) = state.tasks.register();
    let handle = app.clone();
    let returned = task_id.clone();

    tauri::async_runtime::spawn(async move {
        let progress_handle = handle.clone();
        let progress_task = task_id.clone();

        let result = tokio::task::spawn_blocking(move || {
            worlds::archive::import(&archive, &dir, folder.as_deref(), &cancel, |progress| {
                events::task_progress(
                    &progress_handle,
                    events::TaskProgressEvent {
                        task_id: progress_task.clone(),
                        kind: "world_import".to_string(),
                        phase: "extract".to_string(),
                        done: progress.entries_done,
                        total: Some(progress.entries_total),
                        message: "Unpacking world".to_string(),
                        instance_id: Some(id),
                    },
                );
            })
        })
        .await;

        let outcome = match result {
            Ok(Ok(_folder)) => Ok(()),
            Ok(Err(err)) => Err(err),
            Err(err) => Err(AppError::Other(format!("import task failed: {err}"))),
        };
        finish(&handle, task_id, "world_import", id, outcome);
        events::instances_changed(&handle);
    });

    Ok(returned)
}

/// Shared completion handling for the archive tasks.
fn finish(app: &AppHandle, task_id: String, kind: &str, instance_id: i64, outcome: AppResult<()>) {
    let state = app.state::<AppState>();
    state.tasks.finish(&task_id);

    let done = match outcome {
        Ok(()) => events::TaskDoneEvent {
            task_id,
            kind: kind.to_string(),
            ok: true,
            cancelled: false,
            error: None,
            error_kind: None,
            log_path: None,
            log_tail: None,
            instance_id: Some(instance_id),
        },
        Err(err) => done_event(&task_id, kind, instance_id, err),
    };
    events::task_done(app, done);
}

fn done_event(
    task_id: &str,
    kind: &str,
    instance_id: i64,
    err: AppError,
) -> events::TaskDoneEvent {
    events::TaskDoneEvent {
        task_id: task_id.to_string(),
        kind: kind.to_string(),
        ok: false,
        cancelled: matches!(err, AppError::Cancelled),
        error: Some(err.to_string()),
        error_kind: Some(err.kind().to_string()),
        log_path: None,
        log_tail: None,
        instance_id: Some(instance_id),
    }
}
