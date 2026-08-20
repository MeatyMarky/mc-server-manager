//! Orphan recovery.
//!
//! Closing the window minimizes to tray and leaves servers running, so a crash
//! or a reboot can strand a JVM that still holds port 25565. On launch every
//! instance carrying a pid is checked against the live process table. A pid on
//! its own proves nothing — pids get recycled — so the recorded process start
//! time has to match as well.

use sqlx::SqlitePool;
use sysinfo::{Pid, ProcessesToUpdate, System};

use crate::db::models::InstanceStatus;
use crate::db::{now_rfc3339, record_event};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// What the live process table says about a recorded pid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedProcess {
    /// Seconds since the epoch, as reported by the OS.
    pub start_time: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconciliation {
    /// The process is genuinely ours and still alive.
    StillRunning,
    /// Nothing alive, or the pid now belongs to something else.
    Gone,
}

/// The whole trust rule in one pure function.
///
/// A recorded pid without a recorded start time is not trusted: an old database
/// row could otherwise adopt (and later kill) an unrelated process.
pub fn decide(
    recorded_pid: Option<i64>,
    recorded_start_time: Option<i64>,
    observed: Option<ObservedProcess>,
) -> Reconciliation {
    let (Some(pid), Some(start)) = (recorded_pid, recorded_start_time) else {
        return Reconciliation::Gone;
    };
    if pid <= 0 {
        return Reconciliation::Gone;
    }
    match observed {
        Some(process) if process.start_time == start as u64 => Reconciliation::StillRunning,
        _ => Reconciliation::Gone,
    }
}

fn observe(system: &System, pid: i64) -> Option<ObservedProcess> {
    let pid = u32::try_from(pid).ok()?;
    let process = system.process(Pid::from_u32(pid))?;
    Some(ObservedProcess {
        start_time: process.start_time(),
    })
}

/// The columns reconciliation needs; a pid is meaningless without its start time.
#[derive(Debug, sqlx::FromRow)]
struct PidRow {
    id: i64,
    uuid: String,
    name: String,
    pid: Option<i64>,
    process_start_time: Option<i64>,
}

/// Runs at startup, before the UI paints. Returns the number of adopted orphans.
pub async fn reconcile_all(state: &AppState) -> AppResult<usize> {
    let candidates = sqlx::query_as::<_, PidRow>(
        "SELECT id, uuid, name, pid, process_start_time FROM instances WHERE pid IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await?;

    if candidates.is_empty() {
        return Ok(0);
    }

    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);

    let mut adopted = 0usize;
    for PidRow {
        id,
        uuid,
        name,
        pid,
        process_start_time,
    } in candidates
    {
        let observed = pid.and_then(|p| observe(&system, p));
        match decide(pid, process_start_time, observed) {
            Reconciliation::StillRunning => {
                state.set_status(&uuid, InstanceStatus::Unmanaged);
                adopted += 1;
                tracing::info!(
                    instance = %name,
                    instance_id = id,
                    pid = ?pid,
                    "adopted an orphaned server process"
                );
                record_event(
                    &state.db,
                    id,
                    "started",
                    Some("orphan adopted at startup (console unavailable)"),
                )
                .await?;
            }
            Reconciliation::Gone => {
                state.set_status(&uuid, InstanceStatus::Crashed);
                clear_pid(&state.db, id, Some("crashed")).await?;
                tracing::warn!(
                    instance = %name,
                    instance_id = id,
                    pid = ?pid,
                    "server process is gone; marking crashed"
                );
                record_event(
                    &state.db,
                    id,
                    "crashed",
                    Some("process was not running at startup"),
                )
                .await?;
            }
        }
    }
    Ok(adopted)
}

pub async fn clear_pid(pool: &SqlitePool, id: i64, last_status: Option<&str>) -> AppResult<()> {
    sqlx::query(
        "UPDATE instances
         SET pid = NULL, process_start_time = NULL, last_status = ?, last_stopped_at = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(last_status)
    .bind(now_rfc3339())
    .bind(now_rfc3339())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Kills an adopted orphan. There is no stdin to send `stop` to, so this is a
/// hard kill; the UI labels it as such.
pub async fn force_stop_orphan(state: &AppState, id: i64) -> AppResult<()> {
    let instance = super::get(&state.db, id).await?;
    let (Some(pid), Some(start_time)) = (instance.pid, instance.process_start_time) else {
        return Err(AppError::Other(format!(
            "\"{}\" has no recorded process to stop",
            instance.name
        )));
    };

    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);

    let observed = observe(&system, pid);
    if decide(Some(pid), Some(start_time), observed) == Reconciliation::Gone {
        // Already dead; just tidy the row rather than reporting a failure.
        state.set_status(&instance.uuid, InstanceStatus::Stopped);
        clear_pid(&state.db, id, Some("stopped")).await?;
        return Ok(());
    }

    let killed = u32::try_from(pid)
        .ok()
        .and_then(|p| system.process(Pid::from_u32(p)).map(|proc| proc.kill()))
        .unwrap_or(false);

    if !killed {
        return Err(AppError::Other(format!(
            "could not stop the process for \"{}\" (pid {pid})",
            instance.name
        )));
    }

    state.set_status(&instance.uuid, InstanceStatus::Stopped);
    clear_pid(&state.db, id, Some("stopped")).await?;
    record_event(&state.db, id, "stopped", Some("force stopped (orphan)")).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_start_time_means_the_process_is_ours() {
        assert_eq!(
            decide(Some(4321), Some(1_700_000_000), Some(ObservedProcess { start_time: 1_700_000_000 })),
            Reconciliation::StillRunning
        );
    }

    #[test]
    fn a_recycled_pid_is_not_adopted() {
        // Same pid, different process: the start time gives it away.
        assert_eq!(
            decide(Some(4321), Some(1_700_000_000), Some(ObservedProcess { start_time: 1_700_009_999 })),
            Reconciliation::Gone
        );
    }

    #[test]
    fn a_dead_process_is_gone() {
        assert_eq!(decide(Some(4321), Some(1_700_000_000), None), Reconciliation::Gone);
    }

    #[test]
    fn a_pid_without_a_start_time_is_never_trusted() {
        assert_eq!(
            decide(Some(4321), None, Some(ObservedProcess { start_time: 1_700_000_000 })),
            Reconciliation::Gone
        );
        assert_eq!(decide(None, None, None), Reconciliation::Gone);
        assert_eq!(
            decide(Some(0), Some(1), Some(ObservedProcess { start_time: 1 })),
            Reconciliation::Gone
        );
    }

    #[tokio::test]
    async fn reconcile_marks_stale_rows_as_crashed() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());
        let now = now_rfc3339();
        // pid 1 with an impossible start time: never adoptable on any platform.
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, pid, process_start_time, created_at, updated_at)
             VALUES ('u1', 'Ghost', 'Z:/ghost', 'paper', '1.21.4', 'jar', '[]', '[]', 999999, 1, ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        let adopted = reconcile_all(&state).await.unwrap();
        assert_eq!(adopted, 0);
        assert_eq!(state.status_of("u1"), InstanceStatus::Crashed);

        let row: (Option<i64>, Option<String>) =
            sqlx::query_as("SELECT pid, last_status FROM instances WHERE uuid = 'u1'")
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(row.0, None, "stale pid is cleared");
        assert_eq!(row.1.as_deref(), Some("crashed"));

        let events: Vec<(String,)> =
            sqlx::query_as("SELECT kind FROM instance_events WHERE instance_id = 1")
                .fetch_all(&state.db)
                .await
                .unwrap();
        assert!(events.iter().any(|e| e.0 == "crashed"));
    }

    #[tokio::test]
    async fn reconcile_adopts_a_live_process() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());

        // Use this test process: it is guaranteed alive with a known start time.
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);
        let me = std::process::id();
        let start = system
            .process(Pid::from_u32(me))
            .expect("this process is in the table")
            .start_time();

        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, pid, process_start_time, created_at, updated_at)
             VALUES ('u2', 'Live', 'Z:/live', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?, ?, ?)",
        )
        .bind(me as i64)
        .bind(start as i64)
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        assert_eq!(reconcile_all(&state).await.unwrap(), 1);
        assert_eq!(state.status_of("u2"), InstanceStatus::Unmanaged);
    }
}
