//! Backups, schedules and resource metrics.
//!
//! Creating and restoring run in the background behind a task id, because both
//! move gigabytes and both must stay cancellable.

use tauri::{AppHandle, Manager, State};

use crate::backup::archive::{ArchiveEntry, Estimate};
use crate::backup::schedule::{Schedule, ScheduleInput};
use crate::backup::{self, Backup, BackupOptions, SpaceCheck};
use crate::error::{AppError, AppResult};
use crate::events;
use crate::instance;
use crate::metrics::collector::{self, Sample, Window};
use crate::state::AppState;

#[tauri::command]
pub async fn backups_list(state: State<'_, AppState>, id: i64) -> AppResult<Vec<Backup>> {
    backup::list(&state, id).await
}

/// The size estimate and the free-space verdict, shown before anything starts.
#[tauri::command]
pub async fn backup_plan(
    state: State<'_, AppState>,
    id: i64,
    options: BackupOptions,
) -> AppResult<SpaceCheck> {
    backup::plan(&state, id, &options).await
}

/// Starts a backup. Returns the task id; progress arrives as `task://progress`.
#[tauri::command]
pub async fn backup_create(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    options: BackupOptions,
) -> AppResult<String> {
    let (task_id, cancel) = state.tasks.register();
    let handle = app.clone();
    let returned = task_id.clone();

    tauri::async_runtime::spawn(async move {
        let progress_handle = handle.clone();
        let progress_task = task_id.clone();

        let state = handle.state::<AppState>();
        let result = backup::create(&state, id, options, "manual", None, &cancel, move |progress| {
            events::task_progress(
                &progress_handle,
                events::TaskProgressEvent {
                    task_id: progress_task.clone(),
                    kind: "backup".to_string(),
                    phase: "archive".to_string(),
                    done: progress.bytes,
                    total: None,
                    message: format!("{} of {} files", progress.files_done, progress.files_total),
                    instance_id: Some(id),
                },
            );
        })
        .await;

        state.tasks.finish(&task_id);
        if let Ok(row) = instance::get(&state.db, id).await {
            events::backups_changed(&handle, &row.uuid);
        }
        finish(&handle, &task_id, "backup", id, result.map(|_| ()));
    });

    Ok(returned)
}

#[tauri::command]
pub async fn backup_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    backup_id: i64,
) -> AppResult<()> {
    let row = backup::get(&state, backup_id).await?;
    backup::delete(&state, backup_id).await?;
    if let Ok(instance) = instance::get(&state.db, row.instance_id).await {
        events::backups_changed(&app, &instance.uuid);
    }
    Ok(())
}

/// What is inside an archive, so a restore can be previewed before it is run.
#[tauri::command]
pub async fn backup_preview(
    state: State<'_, AppState>,
    backup_id: i64,
) -> AppResult<Vec<ArchiveEntry>> {
    backup::preview(&state, backup_id).await
}

/// Restores an archive over the instance folder. The current state is backed up
/// first; the confirmation by typed name happens in the UI.
#[tauri::command]
pub async fn backup_restore(
    app: AppHandle,
    state: State<'_, AppState>,
    backup_id: i64,
) -> AppResult<String> {
    let (task_id, cancel) = state.tasks.register();
    let handle = app.clone();
    let returned = task_id.clone();
    let instance_id = backup::get(&state, backup_id).await?.instance_id;

    tauri::async_runtime::spawn(async move {
        let progress_handle = handle.clone();
        let progress_task = task_id.clone();

        let state = handle.state::<AppState>();
        let result = backup::restore(&state, backup_id, &cancel, move |progress| {
            events::task_progress(
                &progress_handle,
                events::TaskProgressEvent {
                    task_id: progress_task.clone(),
                    kind: "restore".to_string(),
                    phase: "extract".to_string(),
                    done: progress.bytes,
                    total: None,
                    message: format!("{} of {} files", progress.files_done, progress.files_total),
                    instance_id: Some(instance_id),
                },
            );
        })
        .await;

        state.tasks.finish(&task_id);
        if let Ok(row) = instance::get(&state.db, instance_id).await {
            events::backups_changed(&handle, &row.uuid);
        }
        events::instances_changed(&handle);
        finish(&handle, &task_id, "restore", instance_id, result);
    });

    Ok(returned)
}

/// Applies retention by hand, outside a schedule.
#[tauri::command]
pub async fn backups_prune(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    keep_count: Option<i64>,
    keep_days: Option<i64>,
) -> AppResult<usize> {
    let removed = backup::prune(&state, id, keep_count, keep_days).await?;
    if let Ok(row) = instance::get(&state.db, id).await {
        events::backups_changed(&app, &row.uuid);
    }
    Ok(removed)
}

#[tauri::command]
pub async fn backup_estimate(
    state: State<'_, AppState>,
    id: i64,
    options: BackupOptions,
) -> AppResult<Estimate> {
    Ok(backup::plan(&state, id, &options).await?.estimate)
}

#[tauri::command]
pub async fn schedules_list(state: State<'_, AppState>, id: i64) -> AppResult<Vec<Schedule>> {
    backup::schedule::list(&state, id).await
}

#[tauri::command]
pub async fn schedule_save(
    state: State<'_, AppState>,
    id: i64,
    input: ScheduleInput,
) -> AppResult<Schedule> {
    backup::schedule::upsert(&state, id, input).await
}

#[tauri::command]
pub async fn schedule_delete(state: State<'_, AppState>, schedule_id: i64) -> AppResult<()> {
    backup::schedule::delete(&state, schedule_id).await
}

/// Runs a schedule now, ignoring whether it is due.
#[tauri::command]
pub async fn schedule_run_now(
    app: AppHandle,
    state: State<'_, AppState>,
    schedule_id: i64,
) -> AppResult<()> {
    let schedules = backup::schedule::all_enabled(&state).await?;
    let schedule = schedules
        .into_iter()
        .find(|schedule| schedule.id == schedule_id)
        .ok_or_else(|| AppError::Other("that schedule no longer exists".into()))?;

    backup::runner::run_one(&app, &state, &schedule).await
}

#[tauri::command]
pub async fn metrics_range(
    state: State<'_, AppState>,
    id: i64,
    window: Window,
) -> AppResult<Vec<Sample>> {
    collector::range(&state.db, id, window, chrono::Utc::now()).await
}

/// The heap the instance is configured to use, so the memory chart can plot RSS
/// against what was allocated rather than against the machine's total.
#[tauri::command]
pub async fn metrics_heap_bytes(state: State<'_, AppState>, id: i64) -> AppResult<Option<u64>> {
    let row = instance::get(&state.db, id).await?;
    let jvm_args: Vec<String> = serde_json::from_str(&row.jvm_args).unwrap_or_default();
    Ok(crate::process::launch::max_heap_bytes(row.max_ram_mb, &jvm_args))
}

fn finish(app: &AppHandle, task_id: &str, kind: &str, instance_id: i64, result: AppResult<()>) {
    let done = match result {
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
    };
    events::task_done(app, done);
}
