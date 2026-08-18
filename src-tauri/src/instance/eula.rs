//! `eula.txt` handling.
//!
//! The file is only ever written after the user explicitly accepts in the UI,
//! and the acceptance is timestamped in the database. Nothing in this codebase
//! writes `eula=true` on its own.

use serde::Serialize;
use ts_rs::TS;

use crate::db::{now_rfc3339, record_event};
use crate::error::{AppError, AppResult, IoContext};
use crate::paths;
use crate::state::AppState;

pub const EULA_URL: &str = "https://aka.ms/MinecraftEULA";

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct EulaStatus {
    pub accepted: bool,
    /// Whether `eula.txt` exists at all yet.
    pub file_exists: bool,
    pub path: String,
    pub accepted_at: Option<String>,
    pub url: String,
}

pub async fn status(state: &AppState, id: i64) -> AppResult<EulaStatus> {
    let instance = super::get(&state.db, id).await?;
    let path = paths::eula_path(&instance.path_buf());
    let on_disk = tokio::fs::read_to_string(&path)
        .await
        .map(|text| super::import::parse_eula(&text))
        .unwrap_or(false);

    Ok(EulaStatus {
        accepted: on_disk && instance.eula_accepted,
        file_exists: path.is_file(),
        path: path.to_string_lossy().to_string(),
        accepted_at: instance.eula_accepted_at,
        url: EULA_URL.to_string(),
    })
}

/// Writes `eula.txt` to match the user's decision and records it. Declining
/// rewrites the file to `eula=false` rather than deleting it, so the server
/// stays refusing to start for an obvious reason.
pub async fn set(state: &AppState, id: i64, accepted: bool) -> AppResult<EulaStatus> {
    let instance = super::get(&state.db, id).await?;
    let dir = instance.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: instance.name.clone(),
            path: dir,
        });
    }

    let now = now_rfc3339();
    let body = format!(
        "# Accepted through Minecraft Server Manager on {now}\n\
         # By changing this you agree to the Minecraft EULA: {EULA_URL}\n\
         eula={accepted}\n"
    );
    let path = paths::eula_path(&dir);
    tokio::fs::write(&path, body.as_bytes())
        .await
        .ctx("write eula.txt", &path)?;

    sqlx::query(
        "UPDATE instances SET eula_accepted = ?, eula_accepted_at = ?, updated_at = ? WHERE id = ?",
    )
    .bind(accepted)
    .bind(accepted.then(|| now.clone()))
    .bind(&now)
    .bind(id)
    .execute(&state.db)
    .await?;

    record_event(
        &state.db,
        id,
        "eula",
        Some(if accepted { "accepted" } else { "declined" }),
    )
    .await?;

    let instance = super::get(&state.db, id).await?;
    super::crud::write_manifest(&instance).await?;
    status(state, id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::ServerType;
    use crate::instance::{crud, CreateInstanceInput};

    async fn instance_in(dir: &std::path::Path) -> (AppState, i64) {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, dir.to_path_buf());
        let created = crud::create(
            &state,
            CreateInstanceInput {
                name: "Eula".into(),
                path: dir.join("server").to_string_lossy().to_string(),
                server_type: ServerType::Paper,
                mc_version: "1.21.4".into(),
                loader_version: None,
                min_ram_mb: None,
                max_ram_mb: None,
                notes: None,
                color: None,
            },
        )
        .await
        .unwrap();
        (state, created.id)
    }

    #[tokio::test]
    async fn a_new_instance_has_no_eula_file() {
        let dir = tempfile::tempdir().unwrap();
        let (state, id) = instance_in(dir.path()).await;
        let status = status(&state, id).await.unwrap();
        assert!(!status.accepted);
        assert!(!status.file_exists, "eula.txt is never written implicitly");
    }

    #[tokio::test]
    async fn accepting_writes_the_file_and_timestamps_it() {
        let dir = tempfile::tempdir().unwrap();
        let (state, id) = instance_in(dir.path()).await;

        let status = set(&state, id, true).await.unwrap();
        assert!(status.accepted);
        assert!(status.file_exists);
        assert!(status.accepted_at.is_some());

        let text = std::fs::read_to_string(dir.path().join("server").join("eula.txt")).unwrap();
        assert!(text.contains("eula=true"));
        assert!(text.contains(EULA_URL), "the file points at the EULA");
    }

    #[tokio::test]
    async fn declining_writes_eula_false_rather_than_removing_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (state, id) = instance_in(dir.path()).await;
        set(&state, id, true).await.unwrap();

        let status = set(&state, id, false).await.unwrap();
        assert!(!status.accepted);
        assert!(status.file_exists);
        let text = std::fs::read_to_string(&status.path).unwrap();
        assert!(text.contains("eula=false"));
    }

    #[tokio::test]
    async fn an_externally_reverted_file_wins_over_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let (state, id) = instance_in(dir.path()).await;
        set(&state, id, true).await.unwrap();

        // Someone edited eula.txt by hand outside the app.
        std::fs::write(dir.path().join("server").join("eula.txt"), b"eula=false\n").unwrap();
        assert!(!status(&state, id).await.unwrap().accepted);
    }
}
