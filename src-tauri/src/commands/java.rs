use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::error::AppResult;
use crate::instance;
use crate::java::{self, JavaRuntime};
use crate::state::AppState;

/// What the instance's Java situation looks like right now: what it needs, what
/// it would use, and whether that combination works.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct JavaStatus {
    #[ts(type = "number")]
    pub required_major: i64,
    /// The runtime that would be used: the pinned one, or the best match.
    pub selected: Option<JavaRuntime>,
    /// Set when the instance pins a path that is missing or unusable.
    pub pinned_path: Option<String>,
    pub pinned_valid: bool,
    /// True when nothing installed satisfies the requirement.
    pub mismatch: bool,
    pub message: Option<String>,
    /// When detection last ran, so the picker can say how old its list is.
    pub last_scan_at: Option<String>,
    /// True once that scan is old enough that a new JDK could be missing.
    pub scan_is_stale: bool,
}

/// When detection last ran, without asking about a particular instance.
///
/// App settings show the detected list before any instance exists, so the age
/// of that list cannot be read out of one instance's status.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct ScanInfo {
    pub last_scan_at: Option<String>,
    pub scan_is_stale: bool,
}

#[tauri::command]
pub async fn java_scan_info(state: State<'_, AppState>) -> AppResult<ScanInfo> {
    let last_scan_at = java::last_scan_at(&state.db).await?;
    Ok(ScanInfo {
        scan_is_stale: java::scan_is_stale(last_scan_at.as_deref(), chrono::Utc::now()),
        last_scan_at,
    })
}

#[tauri::command]
pub async fn java_list(state: State<'_, AppState>) -> AppResult<Vec<JavaRuntime>> {
    java::list(&state.db).await
}

#[tauri::command]
pub async fn java_rescan(state: State<'_, AppState>) -> AppResult<Vec<JavaRuntime>> {
    java::rescan(&state.db).await
}

/// The "browse for a JDK" fallback. Accepts a JDK home, its `bin` folder, or
/// the binary itself.
#[tauri::command]
pub async fn java_add_manual(state: State<'_, AppState>, path: String) -> AppResult<JavaRuntime> {
    java::add_manual(&state.db, &path).await
}

#[tauri::command]
pub async fn java_status(state: State<'_, AppState>, id: i64) -> AppResult<JavaStatus> {
    let instance = instance::get(&state.db, id).await?;
    let last_scan_at = java::last_scan_at(&state.db).await?;
    let scan_is_stale = java::scan_is_stale(last_scan_at.as_deref(), chrono::Utc::now());
    // Mojang's own metadata, recorded at install time, raises the table's
    // answer but never lowers it — a stale or wrong record would otherwise let
    // a server be run on a JVM that cannot load its class files.
    let required = java::required_for(instance.java_major, &instance.mc_version);
    // The mod loaders want the major their release was built against; vanilla
    // and the Bukkit family take anything newer.
    let fit = java::fit_for(instance.server_type);

    if let Some(pinned) = instance.java_path.clone() {
        let known = java::list(&state.db)
            .await?
            .into_iter()
            .find(|runtime| runtime.path == pinned);

        return Ok(match known {
            Some(runtime) => {
                // Too old is a mismatch under either rule. Newer than a loader
                // wants is a pin the user is entitled to make, so it reads as a
                // caution rather than a fault.
                let version_ok = java::satisfies(runtime.major, required);
                let fits_rule = fit.accepts(runtime.major, required);
                // Bitness is checked here too: a pinned 32-bit runtime satisfies
                // the version and still cannot run the server, which is exactly
                // how it went unnoticed.
                let width_ok = runtime.usable_for_servers();
                let message = if !version_ok {
                    Some(format!(
                        "This instance is pinned to Java {}, but Minecraft {} needs Java {required}.",
                        runtime.major, instance.mc_version
                    ))
                } else if !fits_rule {
                    Some(format!(
                        "This server is pinned to Java {}, and {} {} is tested on Java {required}. \
                         It will run, but a mod loader on the wrong Java usually fails somewhere \
                         inside a mod rather than saying what is wrong.",
                        runtime.major,
                        instance.server_type.label(),
                        instance.mc_version
                    ))
                } else {
                    runtime.unsuitable_reason().map(|reason| {
                        format!(
                            "The pinned Java at {} is {reason}. Pick a 64-bit runtime, or the \
                             server will refuse the memory it is given.",
                            runtime.path
                        )
                    })
                };

                JavaStatus {
                    required_major: required,
                    message,
                    selected: Some(runtime),
                    pinned_path: Some(pinned),
                    pinned_valid: true,
                    // A pin that merely disagrees with the loader rule is not a
                    // mismatch: the server starts, and the sentence above says why
                    // it might not behave.
                    mismatch: !version_ok || !width_ok,
                    last_scan_at: last_scan_at.clone(),
                    scan_is_stale,
                }
            }
            None => JavaStatus {
                required_major: required,
                selected: None,
                pinned_path: Some(pinned.clone()),
                pinned_valid: false,
                mismatch: true,
                message: Some(format!(
                    "The pinned Java at {pinned} is not usable; pick another one."
                )),
                last_scan_at: last_scan_at.clone(),
                scan_is_stale,
            },
        });
    }

    let best = java::best_of(java::list(&state.db).await?, required, fit);
    // When nothing is selectable, say whether the machine has *no* Java of that
    // version or only 32-bit ones — the fix is different.
    let excluded_32bit = best.is_none()
        && java::list(&state.db)
            .await?
            .iter()
            .any(|runtime| java::satisfies(runtime.major, required) && !runtime.usable_for_servers());

    Ok(JavaStatus {
        required_major: required,
        message: best.is_none().then(|| {
            if excluded_32bit {
                format!(
                    "The only Java {required} on this computer is 32-bit, which cannot run a \
                     server. Install a 64-bit JDK and rescan."
                )
            } else if fit == java::JavaFit::Exact {
                // Not "no Java": the machine may have plenty, none of it the
                // major this loader was built against.
                format!(
                    "{} {} is tested on Java {required}, and no Java {required} is installed.                      The app can download it.",
                    instance.server_type.label(),
                    instance.mc_version
                )
            } else {
                format!(
                    "Minecraft {} needs Java {required}; no installed runtime satisfies that.",
                    instance.mc_version
                )
            }
        }),
        mismatch: best.is_none(),
        selected: best,
        pinned_path: None,
        pinned_valid: true,
        last_scan_at,
        scan_is_stale,
    })
}

/// The Java a given Minecraft version needs, for the create dialog, before an
/// instance exists.
#[tauri::command]
pub async fn java_required_for(mc_version: String) -> AppResult<i64> {
    Ok(java::required_java_for(&mc_version))
}
