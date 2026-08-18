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
    // Mojang's own metadata, recorded at install time, beats the fallback table.
    let required = instance
        .java_major
        .unwrap_or_else(|| java::required_java_for(&instance.mc_version));

    if let Some(pinned) = instance.java_path.clone() {
        let known = java::list(&state.db)
            .await?
            .into_iter()
            .find(|runtime| runtime.path == pinned);

        return Ok(match known {
            Some(runtime) => {
                let ok = java::satisfies(runtime.major, required);
                JavaStatus {
                    required_major: required,
                    message: (!ok).then(|| {
                        format!(
                            "This instance is pinned to Java {}, but Minecraft {} needs Java {required}.",
                            runtime.major, instance.mc_version
                        )
                    }),
                    selected: Some(runtime),
                    pinned_path: Some(pinned),
                    pinned_valid: true,
                    mismatch: !ok,
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
            },
        });
    }

    let best = java::best_for(&state.db, required).await?;
    Ok(JavaStatus {
        required_major: required,
        message: best.is_none().then(|| {
            format!(
                "Minecraft {} needs Java {required}; no installed runtime satisfies that.",
                instance.mc_version
            )
        }),
        mismatch: best.is_none(),
        selected: best,
        pinned_path: None,
        pinned_valid: true,
    })
}

/// The Java a given Minecraft version needs, for the create dialog, before an
/// instance exists.
#[tauri::command]
pub async fn java_required_for(mc_version: String) -> AppResult<i64> {
    Ok(java::required_java_for(&mc_version))
}
