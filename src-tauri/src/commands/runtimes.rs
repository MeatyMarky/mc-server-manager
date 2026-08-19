//! Managed JDKs: what is installed, what could be downloaded, and doing it.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::events;
use crate::java::adoptium::{self, Candidate};
use crate::java::managed::{self, ManagedRuntime};
use crate::state::AppState;

/// A JDK this app could download, as the confirmation shows it.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct DownloadOffer {
    #[ts(type = "number")]
    pub feature_version: i64,
    pub release_name: String,
    pub openjdk_version: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    pub os: String,
    pub arch: String,
    pub file_name: String,
}

impl From<&Candidate> for DownloadOffer {
    fn from(candidate: &Candidate) -> Self {
        Self {
            feature_version: candidate.feature_version,
            release_name: candidate.release_name.clone(),
            openjdk_version: candidate.openjdk_version.clone(),
            size_bytes: candidate.size_bytes as i64,
            os: candidate.os.clone(),
            arch: candidate.arch.clone(),
            file_name: candidate.file_name.clone(),
        }
    }
}

/// What the machine can offer for a required version, and what it would cost.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct JavaPlan {
    #[ts(type = "number")]
    pub required_major: i64,
    /// True when a pin, a managed runtime or a system JDK already satisfies it.
    pub satisfied: bool,
    /// Where the runtime that would be used came from.
    pub origin: Option<String>,
    pub java_path: Option<String>,
    /// The download to offer when nothing suitable is installed. `None` when
    /// downloads are switched off or the API could not be reached.
    pub offer: Option<DownloadOffer>,
    /// Why there is no offer, when there is none.
    pub offer_error: Option<String>,
    pub downloads_allowed: bool,
}

#[tauri::command]
pub async fn managed_runtimes_list(state: State<'_, AppState>) -> AppResult<Vec<ManagedRuntime>> {
    managed::list(&state).await
}

#[tauri::command]
pub async fn managed_runtimes_size(state: State<'_, AppState>) -> AppResult<i64> {
    managed::total_size(&state).await
}

#[tauri::command]
pub async fn managed_runtime_delete(
    state: State<'_, AppState>,
    feature_version: i64,
) -> AppResult<()> {
    managed::remove(&state, feature_version).await
}

/// What would run this Minecraft version, and what to download if nothing can.
///
/// Called when creating or importing an instance, so the download is offered
/// there rather than at the first failed start.
#[tauri::command]
pub async fn java_plan_for(
    state: State<'_, AppState>,
    mc_version: String,
    recorded_major: Option<i64>,
    pinned: Option<String>,
) -> AppResult<JavaPlan> {
    let required = crate::java::required_for(recorded_major, &mc_version);
    let selection = crate::java::select_for(&state, pinned.as_deref(), required).await?;
    let allowed = managed::downloads_allowed(&state).await;

    if let Some(selection) = selection {
        return Ok(JavaPlan {
            required_major: required,
            satisfied: true,
            origin: Some(
                match selection.origin {
                    crate::java::Origin::Pinned => "pinned",
                    crate::java::Origin::Managed => "managed",
                    crate::java::Origin::System => "system",
                }
                .to_string(),
            ),
            java_path: Some(selection.path.to_string_lossy().to_string()),
            offer: None,
            offer_error: None,
            downloads_allowed: allowed,
        });
    }

    // Nothing suitable: resolve what could be downloaded, naming the version
    // and the size, so the user is asked rather than told at launch time.
    let (offer, offer_error) = if allowed {
        match adoptium::resolve(
            &state.http,
            required,
            adoptium::current_os(),
            adoptium::current_arch(),
        )
        .await
        {
            Ok(candidate) => (Some(DownloadOffer::from(&candidate)), None),
            Err(err) => (None, Some(err.user_message())),
        }
    } else {
        (
            None,
            Some("This app is set to use only the Java already installed.".into()),
        )
    };

    Ok(JavaPlan {
        required_major: required,
        satisfied: false,
        origin: None,
        java_path: None,
        offer,
        offer_error,
        downloads_allowed: allowed,
    })
}

/// Downloads and installs a JDK. Returns a task id; progress arrives as
/// `task://progress`, the finished runtime as `task://done`.
#[tauri::command]
pub async fn managed_runtime_install(
    app: AppHandle,
    state: State<'_, AppState>,
    feature_version: i64,
) -> AppResult<String> {
    if !managed::downloads_allowed(&state).await {
        return Err(AppError::Other(
            "This app is set to use only the Java already installed. Turn that off in Settings \
             to download a runtime."
                .into(),
        ));
    }

    let candidate = adoptium::resolve(
        &state.http,
        feature_version,
        adoptium::current_os(),
        adoptium::current_arch(),
    )
    .await?;

    let (task_id, cancel) = state.tasks.register();
    let handle = app.clone();
    let returned = task_id.clone();

    tauri::async_runtime::spawn(async move {
        let progress_handle = handle.clone();
        let progress_task = task_id.clone();
        let total = candidate.size_bytes;
        let label = candidate.release_name.clone();

        let state = handle.state::<AppState>();
        let result = managed::install(&state, &candidate, &cancel, move |progress| {
            events::task_progress(
                &progress_handle,
                events::TaskProgressEvent {
                    task_id: progress_task.clone(),
                    kind: "java_download".to_string(),
                    phase: "download".to_string(),
                    done: progress.downloaded,
                    total: progress.total.or(Some(total)),
                    message: format!("Downloading {label}"),
                    instance_id: None,
                },
            );
        })
        .await;

        state.tasks.finish(&task_id);
        events::instances_changed(&handle);

        let done = match result {
            Ok(runtime) => events::TaskDoneEvent {
                task_id: task_id.clone(),
                kind: "java_download".into(),
                ok: true,
                cancelled: false,
                error: None,
                error_kind: None,
                log_path: None,
                log_tail: Some(runtime.java_path),
                instance_id: None,
            },
            Err(err) => events::TaskDoneEvent {
                task_id: task_id.clone(),
                kind: "java_download".into(),
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
