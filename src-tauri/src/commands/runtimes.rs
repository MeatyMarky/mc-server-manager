//! Managed JDKs: what is installed, what could be downloaded, and doing it.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;

use crate::db::models::ServerType;
use crate::error::{AppError, AppResult};
use crate::events;
use crate::java::adoptium::{self, Candidate};
use crate::java::managed::{self, ManagedRuntime};
use crate::java::JavaFit;
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
    /// Whether this server type takes anything newer or wants that exact major.
    pub fit: crate::java::JavaFit,
    /// The reasoning, as one sentence for the create dialog: which Java the
    /// release is tested on, what this computer has, and what follows.
    pub reason: String,
    /// Set when a runtime would be used that the rule does not prefer — a pin
    /// against a loader's exact major, or a server grandfathered by a start
    /// that already worked. Allowed, and worth saying.
    pub warning: Option<String>,
    /// The Java this server has already reached its Done line on, when it has.
    /// The reason a stricter rule does not get to stop it.
    #[ts(type = "number | null")]
    pub ran_before_on: Option<i64>,
    /// What would be pinned by the "keep using this Java" button, so the UI
    /// does not have to work it out.
    pub pinnable_path: Option<String>,
    /// The best major this computer could offer under a floor rule, when the
    /// exact one is missing. Names what the user already has.
    #[ts(type = "number | null")]
    pub installed_major: Option<i64>,
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
    server_type: ServerType,
    recorded_major: Option<i64>,
    pinned: Option<String>,
    // `instance_id` is the instance this is about, when there is one: a server
    // with a start behind it is judged on that, not on the rule alone.
    instance_id: Option<i64>,
) -> AppResult<JavaPlan> {
    let required = crate::java::required_for(recorded_major, &mc_version);
    let fit = crate::java::fit_for(server_type);
    let selection = crate::java::select_for(&state, pinned.as_deref(), required, fit).await?;
    let allowed = managed::downloads_allowed(&state).await;

    // What the machine has that is at least good enough under a floor rule.
    // Under `Exact` this is the version the app is declining to substitute, and
    // naming it is the difference between a reason and a refusal.
    let installed_major = crate::java::best_for(&state.db, required)
        .await?
        .map(|runtime| runtime.major);

    // A server that has already reached "Done" keeps starting, whatever the
    // rule now says — with the reason on screen and the download offered.
    let grandfathered = match instance_id {
        Some(id) if fit == JavaFit::Exact && selection.is_none() => {
            crate::java::ran_before(&state.db, id).await?
        }
        _ => None,
    };

    if let Some(previous) = grandfathered {
        let fallback =
            crate::java::select_for(&state, pinned.as_deref(), required, JavaFit::Floor).await?;
        if let Some(fallback) = fallback {
            let found = crate::java::probe_major(&fallback.path).await;
            let ran_on = previous.java_major.or(found);
            let (offer, offer_error) = offer_for(&state, required, allowed).await;
            return Ok(JavaPlan {
                required_major: required,
                fit,
                reason: format!(
                    "{mc_version} {} is tested on Java {required}.",
                    label_for(server_type)
                ),
                warning: Some(match ran_on {
                    Some(major) => format!(
                        "This server has run on Java {major} before, so it still starts on it. \
                         Java {required} is what {mc_version} {} is tested on — download it, or \
                         keep using Java {major}.",
                        label_for(server_type)
                    ),
                    None => format!(
                        "This server has run before on the Java it has, so it still starts. \
                         Java {required} is what {mc_version} {} is tested on.",
                        label_for(server_type)
                    ),
                }),
                ran_before_on: ran_on,
                pinnable_path: Some(fallback.path.to_string_lossy().to_string()),
                installed_major,
                satisfied: true,
                origin: Some("grandfathered".to_string()),
                java_path: Some(fallback.path.to_string_lossy().to_string()),
                offer,
                offer_error,
                downloads_allowed: allowed,
            });
        }
    }

    if let Some(selection) = selection {
        let pinned_major = match selection.origin {
            crate::java::Origin::Pinned => crate::java::probe_major(&selection.path).await,
            _ => selection.major,
        };
        // A pin is the user's call, so it is honoured — and said out loud when
        // it is not what the loader was tested against.
        let warning = match (selection.origin, pinned_major) {
            (crate::java::Origin::Pinned, Some(major)) if !fit.accepts(major, required) => {
                Some(format!(
                    "This server is pinned to Java {major}, and {} {} is tested on Java {required}.                      Mod loaders often fail in ways that are hard to read when the version differs.",
                    mc_version,
                    label_for(server_type)
                ))
            }
            _ => None,
        };

        let pinnable_path = warning
            .is_some()
            .then(|| selection.path.to_string_lossy().to_string());

        return Ok(JavaPlan {
            required_major: required,
            fit,
            reason: satisfied_reason(&mc_version, server_type, required, fit, &selection),
            warning,
            ran_before_on: None,
            pinnable_path,
            installed_major,
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
    let (offer, offer_error) = offer_for(&state, required, allowed).await;

    Ok(JavaPlan {
        required_major: required,
        fit,
        reason: missing_reason(&mc_version, server_type, required, fit, installed_major),
        warning: None,
        ran_before_on: None,
        pinnable_path: None,
        installed_major,
        satisfied: false,
        origin: None,
        java_path: None,
        offer,
        offer_error,
        downloads_allowed: allowed,
    })
}

/// The download to offer for a required version, or why there is none.
async fn offer_for(
    state: &AppState,
    required: i64,
    allowed: bool,
) -> (Option<DownloadOffer>, Option<String>) {
    if !allowed {
        return (
            None,
            Some("This app is set to use only the Java already installed.".into()),
        );
    }
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
}

/// How a server type is named in these sentences. "1.16.5 Forge", not
/// "1.16.5 forge" and not "Minecraft 1.16.5 (Forge)".
fn label_for(server_type: ServerType) -> &'static str {
    match server_type {
        ServerType::Vanilla => "Vanilla",
        ServerType::Paper => "Paper",
        ServerType::Purpur => "Purpur",
        ServerType::Fabric => "Fabric",
        ServerType::Forge => "Forge",
        ServerType::NeoForge => "NeoForge",
    }
}

/// The sentence when something suitable is already installed.
fn satisfied_reason(
    mc_version: &str,
    server_type: ServerType,
    required: i64,
    fit: JavaFit,
    selection: &crate::java::Selection,
) -> String {
    let what = label_for(server_type);
    let where_from = match selection.origin {
        crate::java::Origin::Pinned => "pinned for this server",
        crate::java::Origin::Managed => "downloaded by this app",
        crate::java::Origin::System => "already on this computer",
    };
    match fit {
        JavaFit::Exact => format!(
            "{mc_version} {what} is tested on Java {required}, and Java {required} is {where_from}."
        ),
        JavaFit::Floor => match selection.major {
            Some(major) if major != required => format!(
                "{mc_version} {what} needs Java {required} or newer; Java {major} is {where_from}."
            ),
            _ => format!(
                "{mc_version} {what} needs Java {required} or newer, and that is {where_from}."
            ),
        },
    }
}

/// The sentence when nothing installed fits, which is what the offer sits next
/// to. Under `Exact` it names the version the user has, because "you need Java
/// 8" reads like a mistake to somebody looking at their Java 17 install.
fn missing_reason(
    mc_version: &str,
    server_type: ServerType,
    required: i64,
    fit: JavaFit,
    installed_major: Option<i64>,
) -> String {
    let what = label_for(server_type);
    match (fit, installed_major) {
        (JavaFit::Exact, Some(installed)) => format!(
            "{mc_version} {what} is tested on Java {required}; this computer has Java {installed}.              Mod loaders rewrite bytecode as they load, so a newer Java tends to fail inside a mod              rather than say what is wrong."
        ),
        (JavaFit::Exact, None) => format!(
            "{mc_version} {what} is tested on Java {required}, and no Java was found on this              computer."
        ),
        (JavaFit::Floor, _) => format!(
            "{mc_version} {what} needs Java {required} or newer, and nothing suitable is installed."
        ),
    }
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
