//! Turning world saving off around a backup of a running server, and — the part
//! that actually matters — always turning it back on.
//!
//! A server that is still writing chunks while they are being archived produces
//! a torn backup, so saving is disabled first. But a backup that fails, is
//! cancelled, or dies with the app while saving is off leaves a server that
//! silently keeps nothing: every subsequent crash loses everything since the
//! last save. So:
//!
//!   * `save-on` runs on every exit path, success or not;
//!   * the disabled state is recorded in the database *before* `save-off` is
//!     sent, so a killed app can put it right on the next launch;
//!   * the marker is only cleared once `save-on` has actually been sent.

use std::time::Duration;

use crate::db::{now_rfc3339, record_event};
use crate::error::{AppError, AppResult};
use crate::logparse::{self, LogEvent};
use crate::process::supervisor;
use crate::state::AppState;

/// How long to wait for the server to confirm it has flushed.
pub const FLUSH_TIMEOUT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(250);

/// Marks in the database that this instance has saving disabled.
pub async fn mark_disabled(state: &AppState, id: i64) -> AppResult<()> {
    sqlx::query("UPDATE instances SET saving_disabled_at = ?, updated_at = ? WHERE id = ?")
        .bind(now_rfc3339())
        .bind(now_rfc3339())
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(())
}

pub async fn clear_marker(state: &AppState, id: i64) -> AppResult<()> {
    sqlx::query("UPDATE instances SET saving_disabled_at = NULL, updated_at = ? WHERE id = ?")
        .bind(now_rfc3339())
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(())
}

pub async fn is_marked(state: &AppState, id: i64) -> AppResult<bool> {
    let marker: Option<Option<String>> =
        sqlx::query_scalar("SELECT saving_disabled_at FROM instances WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    Ok(marker.flatten().is_some())
}

/// Instances whose last backup left saving disabled.
pub async fn marked_instances(state: &AppState) -> AppResult<Vec<(i64, String, String)>> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, name, saving_disabled_at FROM instances WHERE saving_disabled_at IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(rows)
}

/// Sends `save-off` and waits for the flush to be confirmed in the console.
///
/// The marker is written first: if the app dies between here and `resume`, the
/// next launch still knows saving needs turning back on.
pub async fn suspend(state: &AppState, id: i64, uuid: &str) -> AppResult<()> {
    mark_disabled(state, id).await?;

    let watermark = console_len(state, uuid);
    supervisor::send_command(state, id, "save-off").await?;
    supervisor::send_command(state, id, "save-all flush").await?;

    if wait_for_flush(state, uuid, watermark).await {
        return Ok(());
    }

    // The flush was not confirmed. Saving is still off, so the caller must still
    // resume; that is why this is an error the caller handles rather than a
    // reason to bail out silently.
    Err(AppError::Other(
        "the server did not confirm it had saved within two minutes".into(),
    ))
}

/// Sends `save-on` and clears the marker. Runs on every exit path.
///
/// Failures are logged rather than propagated: this is the cleanup step, and
/// masking the original error with a secondary one helps nobody.
pub async fn resume(state: &AppState, id: i64) {
    if let Err(err) = supervisor::send_command(state, id, "save-on").await {
        tracing::error!(
            error = %err,
            instance_id = id,
            "could not re-enable world saving; the marker is kept so the next start fixes it"
        );
        let _ = record_event(
            &state.db,
            id,
            "error",
            Some("world saving could not be re-enabled after a backup"),
        )
        .await;
        return;
    }

    if let Err(err) = clear_marker(state, id).await {
        tracing::warn!(error = %err, "could not clear the saving-disabled marker");
    }
}

/// Called when a server reports ready: if an interrupted backup left saving
/// disabled, put it right now that there is a console to say it on.
pub async fn recover_on_start(state: &AppState, id: i64) {
    match is_marked(state, id).await {
        Ok(true) => {
            tracing::warn!(
                instance_id = id,
                "an interrupted backup left world saving disabled; re-enabling it"
            );
            let _ = record_event(
                &state.db,
                id,
                "backup",
                Some("re-enabled world saving after an interrupted backup"),
            )
            .await;
            resume(state, id).await;
        }
        Ok(false) => {}
        Err(err) => tracing::warn!(error = %err, "could not check the saving-disabled marker"),
    }
}

/// Startup reconciliation for interrupted backups.
///
/// `save-off` is a property of the running JVM's memory, not of the folder: if
/// the process is gone, saving is back on the moment the server starts again,
/// and keeping the marker would only produce a pointless `save-on` later. So a
/// marked instance whose process did not survive has its marker cleared here,
/// while one that is still alive keeps it until a console exists to fix it.
///
/// Either way the interruption is recorded, because a backup that never
/// finished is something the user should see in the instance's history.
pub async fn reconcile_on_launch(state: &AppState) -> AppResult<usize> {
    let marked = marked_instances(state).await?;
    let mut still_disabled = 0;

    for (id, name, since) in marked {
        let live = match crate::instance::get(&state.db, id).await {
            Ok(row) => state.status_of(&row.uuid).is_live(),
            Err(_) => false,
        };

        if live {
            still_disabled += 1;
            tracing::warn!(
                instance = %name,
                instance_id = id,
                since = %since,
                "a backup was interrupted while world saving was off, and the server is still                  running outside this app; start it from here to re-enable saving"
            );
            let _ = record_event(
                &state.db,
                id,
                "error",
                Some(
                    "a backup was interrupted with world saving off;                      restart this server from the app to re-enable it",
                ),
            )
            .await;
        } else {
            tracing::info!(
                instance = %name,
                instance_id = id,
                since = %since,
                "clearing a saving-disabled marker left by an interrupted backup: the server is                  no longer running, so saving is on again"
            );
            let _ = record_event(
                &state.db,
                id,
                "backup",
                Some("a backup was interrupted; the server has since stopped, so saving is on again"),
            )
            .await;
            clear_marker(state, id).await?;
        }
    }

    Ok(still_disabled)
}

fn console_len(state: &AppState, uuid: &str) -> u64 {
    state
        .supervisor
        .console(uuid)
        .lock()
        .map(|buffer| buffer.total_seen())
        .unwrap_or(0)
}

/// Watches the console for the server's own confirmation that it has saved.
async fn wait_for_flush(state: &AppState, uuid: &str, watermark: u64) -> bool {
    let deadline = tokio::time::Instant::now() + FLUSH_TIMEOUT;

    while tokio::time::Instant::now() < deadline {
        let lines = state.supervisor.tail(uuid, 400);
        if lines
            .iter()
            .filter(|line| line.seq >= watermark)
            .any(|line| flush_confirmed(&line.message))
        {
            return true;
        }
        tokio::time::sleep(POLL).await;
    }
    false
}

/// True when a console line says the world has been written to disk.
///
/// Vanilla says "Saved the game"; Paper adds its own phrasing, and both print a
/// chunk-storage line when the save really is complete.
pub fn flush_confirmed(message: &str) -> bool {
    if matches!(logparse::detect_event(message), Some(LogEvent::Saved)) {
        return true;
    }
    let lower = message.to_ascii_lowercase();
    lower.contains("all chunks are saved")
        || lower.contains("saved the world")
        || lower.contains("saving is already turned off")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::InstanceStatus;

    async fn state_with_instance() -> (AppState, i64, String) {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'Survival', 'Z:/survival', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();
        (state, 1, "u1".to_string())
    }

    #[test]
    fn the_flush_confirmation_is_recognised_across_server_families() {
        assert!(flush_confirmed("Saved the game"));
        assert!(flush_confirmed("Saved the world"));
        assert!(flush_confirmed(
            "ThreadedAnvilChunkStorage (world): All chunks are saved"
        ));
        assert!(flush_confirmed("Saving is already turned off"));

        assert!(!flush_confirmed("Saving..."));
        assert!(!flush_confirmed("Automatic saving is now disabled"));
        assert!(!flush_confirmed("Preparing spawn area: 42%"));
    }

    #[tokio::test]
    async fn the_marker_survives_so_a_killed_app_can_put_it_right() {
        let (state, id, _uuid) = state_with_instance().await;
        assert!(!is_marked(&state, id).await.unwrap());

        mark_disabled(&state, id).await.unwrap();
        assert!(is_marked(&state, id).await.unwrap());

        // What the next launch sees.
        let marked = marked_instances(&state).await.unwrap();
        assert_eq!(marked.len(), 1);
        assert_eq!(marked[0].1, "Survival");

        clear_marker(&state, id).await.unwrap();
        assert!(!is_marked(&state, id).await.unwrap());
        assert!(marked_instances(&state).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn resume_keeps_the_marker_when_the_command_cannot_be_sent() {
        // The instance is not running, so `save-on` cannot be delivered. The
        // marker must stay, because saving is still off on whatever is running.
        let (state, id, _uuid) = state_with_instance().await;
        mark_disabled(&state, id).await.unwrap();

        resume(&state, id).await;

        assert!(
            is_marked(&state, id).await.unwrap(),
            "the marker is only cleared once save-on was actually sent"
        );
        let events: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT kind, detail FROM instance_events WHERE instance_id = ?")
                .bind(id)
                .fetch_all(&state.db)
                .await
                .unwrap();
        assert!(events.iter().any(|(kind, _)| kind == "error"));
    }

    #[tokio::test]
    async fn recovery_does_nothing_when_no_backup_was_interrupted() {
        let (state, id, _uuid) = state_with_instance().await;
        recover_on_start(&state, id).await;

        let events: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM instance_events WHERE instance_id = ?")
                .bind(id)
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(events, 0, "nothing to recover, nothing recorded");
    }

    #[tokio::test]
    async fn recovery_reports_when_it_finds_an_interrupted_backup() {
        let (state, id, uuid) = state_with_instance().await;
        state.set_status(&uuid, InstanceStatus::Running);
        mark_disabled(&state, id).await.unwrap();

        recover_on_start(&state, id).await;

        let events: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT kind, detail FROM instance_events WHERE instance_id = ?")
                .bind(id)
                .fetch_all(&state.db)
                .await
                .unwrap();
        assert!(
            events
                .iter()
                .any(|(kind, detail)| kind == "backup"
                    && detail
                        .as_deref()
                        .unwrap_or_default()
                        .contains("re-enabled world saving")),
            "{events:?}"
        );
    }

    #[tokio::test]
    async fn suspending_a_stopped_instance_reports_rather_than_hanging() {
        let (state, id, _uuid) = state_with_instance().await;
        let err = suspend(&state, id, "u1").await.unwrap_err();
        assert!(err.to_string().contains("not running"), "{err}");
        // The marker is set before the command, so the state on disk is honest.
        assert!(is_marked(&state, id).await.unwrap());
    }

    #[tokio::test]
    async fn a_marker_whose_server_died_with_the_app_is_cleared_on_launch() {
        let (state, id, _uuid) = state_with_instance().await;
        mark_disabled(&state, id).await.unwrap();

        // The JVM is gone, so `save-off` went with it: there is nothing left to
        // re-enable and the marker would only cause a pointless command later.
        let still_disabled = reconcile_on_launch(&state).await.unwrap();

        assert_eq!(still_disabled, 0);
        assert!(!is_marked(&state, id).await.unwrap());

        let events: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT kind, detail FROM instance_events WHERE instance_id = ?")
                .bind(id)
                .fetch_all(&state.db)
                .await
                .unwrap();
        assert!(
            events.iter().any(|(kind, detail)| kind == "backup"
                && detail
                    .as_deref()
                    .unwrap_or_default()
                    .contains("interrupted")),
            "the interruption is still recorded: {events:?}"
        );
    }

    #[tokio::test]
    async fn a_marker_on_a_server_that_outlived_the_app_is_kept_until_a_console_exists() {
        let (state, id, uuid) = state_with_instance().await;
        mark_disabled(&state, id).await.unwrap();
        // Adopted at startup: alive, but with no console this app can write to.
        state.set_status(&uuid, InstanceStatus::Unmanaged);

        let still_disabled = reconcile_on_launch(&state).await.unwrap();

        assert_eq!(still_disabled, 1);
        assert!(
            is_marked(&state, id).await.unwrap(),
            "the marker survives so the next managed start re-enables saving"
        );

        let events: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT kind, detail FROM instance_events WHERE instance_id = ?")
                .bind(id)
                .fetch_all(&state.db)
                .await
                .unwrap();
        assert!(events.iter().any(|(kind, _)| kind == "error"));
    }

    #[tokio::test]
    async fn launch_reconciliation_is_a_no_op_without_markers() {
        let (state, id, _uuid) = state_with_instance().await;
        assert_eq!(reconcile_on_launch(&state).await.unwrap(), 0);

        let events: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM instance_events WHERE instance_id = ?")
                .bind(id)
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(events, 0, "a clean start says nothing");
    }
}
