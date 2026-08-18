//! Every event the backend emits, in one place.
//!
//! The UI subscribes; it never polls. Phase 1 only needs list invalidation,
//! status changes and the quit handshake — console, metrics and task progress
//! events land in later phases and get added here.

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

use crate::db::models::InstanceStatus;

/// The instance list changed (created, cloned, renamed, deleted, imported).
pub const INSTANCES_CHANGED: &str = "instances://changed";
/// One instance changed status.
pub const INSTANCE_STATUS: &str = "instance://status";
/// The user asked to quit while servers are still alive.
pub const QUIT_REQUESTED: &str = "app://quit-requested";
/// A long-running task moved forward.
pub const TASK_PROGRESS: &str = "task://progress";
/// A long-running task finished, succeeded, failed or was cancelled.
pub const TASK_DONE: &str = "task://done";

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct InstanceStatusEvent {
    pub uuid: String,
    pub status: InstanceStatus,
    #[ts(type = "number | null")]
    pub exit_code: Option<i64>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct QuitRequestedEvent {
    /// Names of instances that are still alive, for the confirmation dialog.
    pub live_instances: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct TaskProgressEvent {
    pub task_id: String,
    /// What kind of work this is: "install" for now.
    pub kind: String,
    /// Which step: resolve, download, install, finalize.
    pub phase: String,
    #[ts(type = "number")]
    pub done: u64,
    #[ts(type = "number | null")]
    pub total: Option<u64>,
    pub message: String,
    #[ts(type = "number | null")]
    pub instance_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct TaskDoneEvent {
    pub task_id: String,
    pub kind: String,
    pub ok: bool,
    pub cancelled: bool,
    /// Rendered error message when `ok` is false.
    pub error: Option<String>,
    /// Structured detail for errors that carry one (installer logs).
    pub error_kind: Option<String>,
    pub log_path: Option<String>,
    pub log_tail: Option<String>,
    #[ts(type = "number | null")]
    pub instance_id: Option<i64>,
}

/// Emit failures are logged, never propagated: a missing listener must not turn
/// a successful backend operation into an error.
fn emit<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    if let Err(err) = app.emit(event, payload) {
        tracing::warn!(event, error = %err, "could not emit event");
    }
}

pub fn instances_changed(app: &AppHandle) {
    emit(app, INSTANCES_CHANGED, ());
}

pub fn instance_status(app: &AppHandle, uuid: &str, status: InstanceStatus, exit_code: Option<i64>) {
    emit(
        app,
        INSTANCE_STATUS,
        InstanceStatusEvent {
            uuid: uuid.to_string(),
            status,
            exit_code,
        },
    );
}

pub fn task_progress(app: &AppHandle, payload: TaskProgressEvent) {
    emit(app, TASK_PROGRESS, payload);
}

pub fn task_done(app: &AppHandle, payload: TaskDoneEvent) {
    emit(app, TASK_DONE, payload);
}

pub fn quit_requested(app: &AppHandle, live_instances: Vec<String>) {
    emit(app, QUIT_REQUESTED, QuitRequestedEvent { live_instances });
}
