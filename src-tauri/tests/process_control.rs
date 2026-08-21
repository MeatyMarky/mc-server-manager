//! Process control against real OS processes.
//!
//! These run in CI: they use a long-lived system command rather than a
//! Minecraft server, which is enough to exercise the parts that actually broke
//! in practice — pid trust, orphan adoption after an app restart, and killing a
//! process this app no longer owns.

use std::path::Path;
use std::process::{Child, Command, Stdio};

use mc_server_manager_lib::db;
use mc_server_manager_lib::db::models::InstanceStatus;
use mc_server_manager_lib::instance::reconcile::{self, ObservedProcess, Reconciliation};
use mc_server_manager_lib::process::supervisor::process_start_time;
use mc_server_manager_lib::state::AppState;

/// A process that stays alive long enough to be inspected, on either platform.
///
/// Spawned directly rather than through a shell: `cmd /C ping` hands back the
/// pid of `cmd`, so the process being measured is not the process doing the
/// waiting, and killing it leaves `ping.exe` orphaned for a full minute. Four
/// tests doing that per run left the CI runner churning through pids, which is
/// the pressure that makes a freed pid get reused immediately.
fn spawn_long_lived() -> Child {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("ping");
        command.args(["-n", "60", "127.0.0.1"]);
        command
    } else {
        let mut command = Command::new("sleep");
        command.arg("60");
        command
    };

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn a long-lived process")
}

async fn state_with_instance(dir: &Path, pid: Option<u32>, start_time: Option<u64>) -> AppState {
    let pool = db::connect_in_memory().await.expect("database");
    let state = AppState::new(pool, dir.to_path_buf());
    let now = db::now_rfc3339();

    sqlx::query(
        "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
            jvm_args, server_args, pid, process_start_time, created_at, updated_at)
         VALUES ('u1', 'Survival', ?, 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?, ?, ?)",
    )
    .bind(dir.to_string_lossy().to_string())
    .bind(pid.map(|p| p as i64))
    .bind(start_time.map(|t| t as i64))
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await
    .expect("insert instance");

    state
}

/// The scenario the whole pid+start_time design exists for: the app dies while a
/// server keeps running, and the next launch has to recognise it.
#[tokio::test]
async fn a_surviving_process_is_adopted_after_an_app_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = spawn_long_lived();
    let pid = child.id();
    let start_time = process_start_time(pid).expect("the process is visible");

    // This is what the app wrote before it died.
    let state = state_with_instance(dir.path(), Some(pid), Some(start_time)).await;

    let adopted = reconcile::reconcile_all(&state).await.expect("reconcile");
    assert_eq!(adopted, 1, "the surviving process is adopted");
    assert_eq!(
        state.status_of("u1"),
        InstanceStatus::Unmanaged,
        "running, but this app does not own its console"
    );

    // The row keeps the pid, so stop-by-pid still works.
    let row: (Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT pid, process_start_time FROM instances WHERE uuid = 'u1'")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(row.0, Some(pid as i64));
    assert_eq!(row.1, Some(start_time as i64));

    // And stopping it really kills the process and clears the row.
    reconcile::force_stop_orphan(&state, 1).await.expect("force stop");
    assert_eq!(state.status_of("u1"), InstanceStatus::Stopped);

    let cleared: Option<i64> = sqlx::query_scalar("SELECT pid FROM instances WHERE uuid = 'u1'")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(cleared, None);

    // The OS agrees the process is gone.
    let exited = child.wait().expect("wait");
    assert!(!exited.success() || cfg!(windows), "the process was killed");
    assert_eq!(process_start_time(pid), None);
}

/// A pid that has been recycled by an unrelated process must never be adopted:
/// otherwise stopping the instance would kill a stranger's process.
#[tokio::test]
async fn a_recycled_pid_is_rejected_and_the_instance_is_marked_crashed() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = spawn_long_lived();
    let pid = child.id();
    let real_start = process_start_time(pid).expect("visible");

    // Same pid, but the start time recorded by the app does not match.
    let state = state_with_instance(dir.path(), Some(pid), Some(real_start + 5_000)).await;

    let adopted = reconcile::reconcile_all(&state).await.expect("reconcile");
    assert_eq!(adopted, 0, "the mismatch prevents adoption");
    assert_eq!(state.status_of("u1"), InstanceStatus::Crashed);

    let cleared: Option<i64> = sqlx::query_scalar("SELECT pid FROM instances WHERE uuid = 'u1'")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(cleared, None, "the untrusted pid is dropped");

    // The unrelated process is still running: nothing killed it.
    assert_eq!(process_start_time(pid), Some(real_start));
    let _ = child.kill();
    let _ = child.wait();
}

/// A process that ended while the app was down leaves a crashed instance.
#[tokio::test]
async fn a_process_that_ended_leaves_the_instance_crashed() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = spawn_long_lived();
    let pid = child.id();
    let start_time = process_start_time(pid).expect("visible");
    child.kill().expect("kill");
    child.wait().expect("wait");

    let state = state_with_instance(dir.path(), Some(pid), Some(start_time)).await;
    assert_eq!(reconcile::reconcile_all(&state).await.unwrap(), 0);
    assert_eq!(state.status_of("u1"), InstanceStatus::Crashed);

    let events: Vec<(String,)> =
        sqlx::query_as("SELECT kind FROM instance_events WHERE instance_id = 1")
            .fetch_all(&state.db)
            .await
            .unwrap();
    assert!(events.iter().any(|event| event.0 == "crashed"));
}

/// The start time of a live process is stable, which is what makes the whole
/// scheme work; once the process ends, its pid stops being adoptable.
#[test]
fn start_times_identify_a_process() {
    let mut child = spawn_long_lived();
    let pid = child.id();

    let first = process_start_time(pid).expect("visible");
    let second = process_start_time(pid).expect("still visible");
    assert_eq!(first, second, "the start time does not drift");
    assert_eq!(
        reconcile::decide(
            Some(pid as i64),
            Some(first as i64),
            Some(ObservedProcess { start_time: second })
        ),
        Reconciliation::StillRunning
    );

    child.kill().unwrap();
    child.wait().unwrap();

    // What happens to a pid the moment its process ends is not the same on both
    // platforms. A reaped Unix pid leaves the process table at once. Windows
    // makes no such promise: the pid can still be reported for a moment, and it
    // can be handed straight to a new process — CI has seen `Some(..)` here on a
    // Windows runner where this machine reports `None` in 700 consecutive
    // attempts, idle and under process churn alike.
    //
    // The rule that has to hold on both is the one the pid guard exists for, so
    // that is what is asserted: whatever the process table says afterwards, the
    // recorded pid must no longer be adoptable. A start time that still matches
    // would mean a dead process being recognised as ours — the exact case the
    // guard is for — so it fails loudly rather than being tolerated.
    let after = process_start_time(pid);
    assert_ne!(
        after,
        Some(first),
        "the pid of an ended process still reports the start time it had while \
         alive, so the pid guard would adopt a process that no longer exists"
    );
    if cfg!(unix) {
        assert_eq!(after, None, "a reaped pid leaves the process table");
    }
    assert_eq!(
        reconcile::decide(
            Some(pid as i64),
            Some(first as i64),
            after.map(|start_time| ObservedProcess { start_time })
        ),
        Reconciliation::Gone,
        "an ended process is never still ours, whatever the pid now holds"
    );
}
