//! About, first-run readiness, and the problem report.

use std::path::PathBuf;

use tauri::State;

use crate::diag::{self, BuildInfo, ReportPreview};
use crate::error::AppResult;
use crate::state::AppState;

#[tauri::command]
pub async fn build_info(state: State<'_, AppState>) -> AppResult<BuildInfo> {
    diag::build_info(&state).await
}

/// What the report would contain. Nothing is written until `report_write`.
#[tauri::command]
pub async fn report_preview(
    state: State<'_, AppState>,
    id: Option<i64>,
    lines: Option<usize>,
) -> AppResult<ReportPreview> {
    diag::preview(&state, id, lines.unwrap_or(diag::DEFAULT_LINES)).await
}

/// Writes the report the user just read to the path they picked.
///
/// The preview is rebuilt here rather than sent back from the UI, so what lands
/// on disk is the app's own text and not something a page could have edited.
#[tauri::command]
pub async fn report_write(
    state: State<'_, AppState>,
    target: String,
    id: Option<i64>,
    lines: Option<usize>,
) -> AppResult<String> {
    let preview = diag::preview(&state, id, lines.unwrap_or(diag::DEFAULT_LINES)).await?;
    let target = PathBuf::from(target);
    diag::write_zip(&preview, &target)?;
    Ok(target.to_string_lossy().to_string())
}

/// Whether the machine can run a server at all, for the first-run screen.
#[tauri::command]
pub async fn startup_readiness(state: State<'_, AppState>) -> AppResult<crate::diag::Readiness> {
    crate::diag::readiness(&state).await
}
