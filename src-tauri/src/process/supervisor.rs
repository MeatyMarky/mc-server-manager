//! Owning running servers: spawn, console capture, stdin, stop, auto-restart.
//!
//! The supervisor holds no `Child`: everything after spawn is done by pid, the
//! same way an orphan adopted at startup is handled. That keeps one code path
//! for "a server we started" and "a server that outlived the app", and the
//! pid is only ever trusted together with its recorded start time.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::db::models::{Instance, InstanceStatus};
use crate::db::{now_rfc3339, record_event};
use crate::error::{AppError, AppResult};
use crate::events;
use crate::instance;
use crate::java;
use crate::logparse::{self, LogEvent, ParsedLine};
use crate::state::AppState;

use super::console::{ConsoleBuffer, ConsoleFile};
use super::{backoff, launch, port, StopStage};

/// Console lines are collected for this long before one event is emitted.
const BATCH_INTERVAL: Duration = Duration::from_millis(100);
/// …or until this many lines are queued, whichever comes first.
const BATCH_MAX_LINES: usize = 250;
/// How often the stop sequence re-checks whether the process is gone.
const EXIT_POLL: Duration = Duration::from_millis(200);
/// Grace given to SIGTERM before the hard kill.
const TERMINATE_GRACE: Duration = Duration::from_secs(10);

/// One running server.
struct Running {
    pid: u32,
    stdin: mpsc::UnboundedSender<String>,
    /// Set by the monitor task the moment the process exits.
    exited: Arc<AtomicBool>,
    /// Set before a deliberate stop, so the exit is not treated as a crash.
    stop_requested: Arc<AtomicBool>,
}

/// Live process registry plus the console history, which outlives the process
/// so the last output of a crashed server is still readable.
#[derive(Default)]
pub struct Supervisor {
    running: Mutex<HashMap<String, Running>>,
    consoles: Mutex<HashMap<String, Arc<Mutex<ConsoleBuffer>>>>,
    /// Who is currently connected, per instance, as the log reports it. This is
    /// what the player-count chart plots and what "skip the backup if nobody
    /// played" reads; a server this app does not own has no entry, which is
    /// different from an entry saying nobody is online.
    online: Mutex<HashMap<String, HashSet<String>>>,
}

impl Supervisor {
    pub fn console(&self, uuid: &str) -> Arc<Mutex<ConsoleBuffer>> {
        let mut consoles = self.consoles.lock().expect("console registry");
        consoles
            .entry(uuid.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(ConsoleBuffer::new())))
            .clone()
    }

    pub fn tail(&self, uuid: &str, count: usize) -> Vec<ParsedLine> {
        self.console(uuid)
            .lock()
            .map(|buffer| buffer.tail(count))
            .unwrap_or_default()
    }

    pub fn is_running(&self, uuid: &str) -> bool {
        self.running
            .lock()
            .map(|map| map.contains_key(uuid))
            .unwrap_or(false)
    }

    pub fn pid_of(&self, uuid: &str) -> Option<u32> {
        self.running
            .lock()
            .ok()
            .and_then(|map| map.get(uuid).map(|running| running.pid))
    }

    /// How many players the log has seen join and not leave.
    pub fn online_count(&self, uuid: &str) -> Option<usize> {
        self.online
            .lock()
            .ok()
            .and_then(|map| map.get(uuid).map(|names| names.len()))
    }

    pub fn online_players(&self, uuid: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .online
            .lock()
            .ok()
            .and_then(|map| map.get(uuid).map(|names| names.iter().cloned().collect()))
            .unwrap_or_default();
        names.sort();
        names
    }

    pub(crate) fn player_joined(&self, uuid: &str, name: &str) {
        if let Ok(mut map) = self.online.lock() {
            map.entry(uuid.to_string()).or_default().insert(name.to_string());
        }
    }

    pub(crate) fn player_left(&self, uuid: &str, name: &str) {
        if let Ok(mut map) = self.online.lock() {
            if let Some(names) = map.get_mut(uuid) {
                names.remove(name);
            }
        }
    }

    /// A server that just started has nobody on it, and one that stopped has
    /// nobody either. Both cases replace the set rather than leaving the last
    /// session's names to be counted again.
    pub(crate) fn reset_online(&self, uuid: &str) {
        if let Ok(mut map) = self.online.lock() {
            map.insert(uuid.to_string(), HashSet::new());
        }
    }

    pub(crate) fn drop_online(&self, uuid: &str) {
        if let Ok(mut map) = self.online.lock() {
            map.remove(uuid);
        }
    }

    fn insert(&self, uuid: &str, running: Running) {
        if let Ok(mut map) = self.running.lock() {
            map.insert(uuid.to_string(), running);
        }
    }

    fn remove(&self, uuid: &str) {
        if let Ok(mut map) = self.running.lock() {
            map.remove(uuid);
        }
        self.drop_online(uuid);
    }
}

/// Everything that must be true before a server can start.
async fn preflight(state: &AppState, instance: &Instance) -> AppResult<PathBuf> {
    let dir = instance.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: instance.name.clone(),
            path: dir,
        });
    }
    if state.status_of(&instance.uuid).is_live() {
        return Err(AppError::InstanceRunning(instance.name.clone()));
    }
    if instance.installed_at.is_none() && instance.launch_target.is_none() {
        return Err(AppError::NotInstalled(instance.name.clone()));
    }
    // The EULA gate is a hard stop: the server would refuse to boot anyway, and
    // silently writing eula.txt is exactly what this app never does.
    if !instance.eula_accepted {
        return Err(AppError::EulaNotAccepted(instance.name.clone()));
    }

    let check = port::check(&state.db, instance.id, &dir, &state.live_uuids()).await?;
    if let Some(err) = check.as_error() {
        return Err(err);
    }

    // Scripts bring their own Java; everything else needs one we can name.
    if instance.launch_kind == crate::db::models::LaunchKind::Script {
        return Ok(PathBuf::from("java"));
    }

    let required = instance
        .java_major
        .unwrap_or_else(|| java::required_java_for(&instance.mc_version));

    if let Some(pinned) = &instance.java_path {
        let path = PathBuf::from(pinned);
        if path.is_file() {
            return Ok(path);
        }
        return Err(AppError::JavaPinnedMissing {
            instance: instance.name.clone(),
            path: pinned.clone(),
        });
    }

    if let Some(runtime) = java::best_for(&state.db, required).await? {
        return Ok(PathBuf::from(runtime.path));
    }

    // "No Java at all" and "Java, but too old" have different fixes, and the
    // second is the one that confuses people: they installed Java, so why is
    // the app still complaining?
    let newest = java::list(&state.db)
        .await?
        .into_iter()
        .map(|runtime| runtime.major)
        .max();
    Err(match newest {
        Some(found) => AppError::JavaTooOld {
            required,
            found,
            mc_version: instance.mc_version.clone(),
        },
        None => AppError::JavaNotFound { required },
    })
}

/// Starts a server. Returns once the process is spawned; readiness arrives later
/// as a status event driven by the server's own "Done" line.
pub async fn start(app: &AppHandle, state: &AppState, id: i64) -> AppResult<()> {
    let instance = instance::get(&state.db, id).await?;
    let java = preflight(state, &instance).await?;
    let plan = launch::plan(&instance, &java)?;

    let console = state.supervisor.console(&instance.uuid);
    if let Ok(mut buffer) = console.lock() {
        buffer.push_system(&format!(
            "Starting {} with {}",
            instance.name,
            plan.program.display()
        ));
    }

    let mut command = tokio::process::Command::new(&plan.program);
    command
        .args(&plan.args)
        .current_dir(&plan.working_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(false);

    #[cfg(unix)]
    {
        // Own process group, so a stop reaches the JVM's children too.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|e| AppError::io("start the server", &plan.program, e))?;

    let pid = child.id().ok_or_else(|| {
        AppError::Other("the server process exited before it could be tracked".into())
    })?;
    let start_time = process_start_time(pid).unwrap_or(0);

    // Persist pid + start time immediately: a crash of *this app* one second
    // from now must still leave enough to reconcile against.
    sqlx::query(
        "UPDATE instances SET pid = ?, process_start_time = ?, last_status = 'starting',
            last_started_at = ?, updated_at = ? WHERE id = ?",
    )
    .bind(pid as i64)
    .bind(start_time as i64)
    .bind(now_rfc3339())
    .bind(now_rfc3339())
    .bind(id)
    .execute(&state.db)
    .await?;

    record_event(&state.db, id, "started", Some(&format!("pid {pid}"))).await?;
    state.set_status(&instance.uuid, InstanceStatus::Starting);
    events::instance_status(app, &instance.uuid, InstanceStatus::Starting, None);

    let (line_tx, line_rx) = mpsc::unbounded_channel::<(String, bool)>();
    let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();

    // stdout / stderr readers: one line at a time into the batching channel.
    if let Some(stdout) = child.stdout.take() {
        let tx = line_tx.clone();
        tauri::async_runtime::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send((line, false)).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = line_tx.clone();
        tauri::async_runtime::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send((line, true)).is_err() {
                    break;
                }
            }
        });
    }
    drop(line_tx);

    // stdin writer.
    if let Some(mut stdin) = child.stdin.take() {
        tauri::async_runtime::spawn(async move {
            while let Some(command) = stdin_rx.recv().await {
                if stdin.write_all(command.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });
    }

    let exited = Arc::new(AtomicBool::new(false));
    let stop_requested = Arc::new(AtomicBool::new(false));

    state.supervisor.insert(
        &instance.uuid,
        Running {
            pid,
            stdin: stdin_tx,
            exited: exited.clone(),
            stop_requested: stop_requested.clone(),
        },
    );

    spawn_console_pump(app.clone(), instance.clone(), console, line_rx);
    spawn_monitor(app.clone(), instance, child, exited, stop_requested);

    Ok(())
}

/// Batches console lines into one event per interval, so chunk generation cannot
/// flood the IPC bridge.
fn spawn_console_pump(
    app: AppHandle,
    instance: Instance,
    console: Arc<Mutex<ConsoleBuffer>>,
    mut lines: mpsc::UnboundedReceiver<(String, bool)>,
) {
    tauri::async_runtime::spawn(async move {
        let mut file = match ConsoleFile::open(&instance.path_buf()) {
            Ok(file) => Some(file),
            Err(err) => {
                tracing::warn!(error = %err, "console capture will stay in memory only");
                None
            }
        };

        let mut pending: Vec<ParsedLine> = Vec::with_capacity(BATCH_MAX_LINES);
        let mut ticker = tokio::time::interval(BATCH_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                received = lines.recv() => {
                    let Some((raw, stderr)) = received else {
                        break;
                    };

                    if let Some(file) = file.as_mut() {
                        file.write_line(&raw);
                    }

                    let parsed = match console.lock() {
                        Ok(mut buffer) => buffer.push(&raw, stderr),
                        Err(_) => continue,
                    };

                    if let Some(event) = logparse::detect_event(&parsed.message) {
                        handle_log_event(&app, &instance, event).await;
                    }

                    pending.push(parsed);
                    if pending.len() >= BATCH_MAX_LINES {
                        events::console_lines(&app, &instance.uuid, std::mem::take(&mut pending));
                    }
                }
                _ = ticker.tick() => {
                    if !pending.is_empty() {
                        events::console_lines(&app, &instance.uuid, std::mem::take(&mut pending));
                    }
                }
            }
        }

        if !pending.is_empty() {
            events::console_lines(&app, &instance.uuid, pending);
        }
    });
}

/// Reacts to the events the log parser recognizes.
async fn handle_log_event(app: &AppHandle, instance: &Instance, event: LogEvent) {
    use tauri::Manager;
    let state = app.state::<AppState>();

    match event {
        LogEvent::Ready { took } => {
            state.set_status(&instance.uuid, InstanceStatus::Running);
            events::instance_status(app, &instance.uuid, InstanceStatus::Running, None);
            events::instances_changed(app);
            let _ = sqlx::query("UPDATE instances SET last_status = 'running' WHERE id = ?")
                .bind(instance.id)
                .execute(&state.db)
                .await;
            state.supervisor.reset_online(&instance.uuid);
            // If the app died between save-off and save-on, this is the first
            // moment a console exists to put it right.
            crate::backup::saveguard::recover_on_start(&state, instance.id).await;
            tracing::info!(instance = %instance.name, took = ?took, "server is ready");
        }
        LogEvent::Stopping => {
            state.set_status(&instance.uuid, InstanceStatus::Stopping);
            events::instance_status(app, &instance.uuid, InstanceStatus::Stopping, None);
        }
        LogEvent::PlayerUuid { name, uuid } => {
            record_player(&state, instance.id, &uuid, &name).await;
            state.supervisor.player_joined(&instance.uuid, &name);
            events::player(app, &instance.uuid, "join", &name, Some(&uuid));
        }
        LogEvent::PlayerJoined { name, .. } => {
            state.supervisor.player_joined(&instance.uuid, &name);
            events::player(app, &instance.uuid, "join", &name, None);
        }
        LogEvent::PlayerLeft { name } => {
            state.supervisor.player_left(&instance.uuid, &name);
            events::player(app, &instance.uuid, "leave", &name, None);
        }
        LogEvent::PortInUse { detail } => {
            let _ = record_event(&state.db, instance.id, "error", Some(&detail)).await;
        }
        LogEvent::Crash { detail } => {
            let _ = record_event(&state.db, instance.id, "crashed", Some(&detail)).await;
        }
        LogEvent::Saved => {}
    }
}

pub(crate) async fn record_player(state: &AppState, instance_id: i64, uuid: &str, name: &str) {
    let now = now_rfc3339();
    let result = sqlx::query(
        "INSERT INTO players_seen (instance_id, uuid, name, first_seen, last_seen)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(instance_id, uuid) DO UPDATE SET name = excluded.name, last_seen = excluded.last_seen",
    )
    .bind(instance_id)
    .bind(uuid)
    .bind(name)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await;

    if let Err(err) = result {
        tracing::warn!(error = %err, "could not record a player sighting");
    }
}

/// Waits for the process, then decides between "stopped", "crashed" and
/// "restart after a backoff".
fn spawn_monitor(
    app: AppHandle,
    instance: Instance,
    mut child: tokio::process::Child,
    exited: Arc<AtomicBool>,
    stop_requested: Arc<AtomicBool>,
) {
    tauri::async_runtime::spawn(async move {
        use tauri::Manager;

        let status = child.wait().await;
        exited.store(true, Ordering::SeqCst);

        let code = status.as_ref().ok().and_then(|status| status.code());

        let state = app.state::<AppState>();
        state.supervisor.remove(&instance.uuid);

        let requested = stop_requested.load(Ordering::SeqCst);
        let crashed = backoff::is_crash(code, requested);

        let _ = sqlx::query(
            "UPDATE instances SET pid = NULL, process_start_time = NULL, last_exit_code = ?,
                last_status = ?, last_stopped_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(code.map(i64::from))
        .bind(if crashed { "crashed" } else { "stopped" })
        .bind(now_rfc3339())
        .bind(now_rfc3339())
        .bind(instance.id)
        .execute(&state.db)
        .await;

        if let Ok(mut buffer) = state.supervisor.console(&instance.uuid).lock() {
            buffer.push_system(&match code {
                Some(code) if crashed => format!("Server exited with code {code}"),
                Some(code) => format!("Server stopped (exit code {code})"),
                None => "Server process ended".to_string(),
            });
        }

        let status_now = if crashed {
            InstanceStatus::Crashed
        } else {
            InstanceStatus::Stopped
        };
        state.set_status(&instance.uuid, status_now);
        events::instance_status(&app, &instance.uuid, status_now, code.map(i64::from));
        events::instances_changed(&app);

        let _ = record_event(
            &state.db,
            instance.id,
            if crashed { "crashed" } else { "stopped" },
            Some(&format!("exit code {code:?}")),
        )
        .await;

        if !crashed {
            return;
        }

        // Auto-restart, backoff-limited so an instantly-crashing server cannot spin.
        let recent = recent_crashes(&state, instance.id, instance.restart_window_s).await;
        match backoff::decide(
            instance.auto_restart,
            false,
            recent.saturating_sub(1).max(0),
            instance.restart_max,
            instance.restart_window_s,
        ) {
            backoff::RestartDecision::Restart { delay, attempt } => {
                let _ = record_event(
                    &state.db,
                    instance.id,
                    "restarted",
                    Some(&format!(
                        "attempt {attempt} of {} after {}s",
                        instance.restart_max,
                        delay.as_secs()
                    )),
                )
                .await;
                if let Ok(mut buffer) = state.supervisor.console(&instance.uuid).lock() {
                    buffer.push_system(&format!(
                        "Restarting in {}s (attempt {attempt} of {})",
                        delay.as_secs(),
                        instance.restart_max
                    ));
                }

                tokio::time::sleep(delay).await;
                let state = app.state::<AppState>();
                if let Err(err) = start(&app, &state, instance.id).await {
                    tracing::warn!(error = %err, instance = %instance.name, "auto-restart failed");
                    let _ =
                        record_event(&state.db, instance.id, "error", Some(&err.to_string())).await;
                }
            }
            backoff::RestartDecision::GaveUp {
                attempts,
                window_secs,
            } => {
                let message = format!(
                    "Gave up restarting after {attempts} crashes in {}s",
                    window_secs
                );
                tracing::warn!(instance = %instance.name, "{message}");
                if let Ok(mut buffer) = state.supervisor.console(&instance.uuid).lock() {
                    buffer.push_system(&message);
                }
                let _ = record_event(&state.db, instance.id, "error", Some(&message)).await;
            }
            backoff::RestartDecision::Disabled | backoff::RestartDecision::CleanExit => {}
        }
    });
}

/// Crashes recorded for this instance inside the restart window.
async fn recent_crashes(state: &AppState, instance_id: i64, window_secs: i64) -> i64 {
    let since = chrono::Utc::now() - chrono::Duration::seconds(window_secs.max(1));
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM instance_events
         WHERE instance_id = ? AND kind = 'crashed' AND ts >= ?",
    )
    .bind(instance_id)
    .bind(since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    .fetch_one(&state.db)
    .await
    .unwrap_or(0)
}

/// Sends a command on stdin, exactly as typed in the console input.
pub async fn send_command(state: &AppState, id: i64, command: &str) -> AppResult<()> {
    let instance = instance::get(&state.db, id).await?;
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    let sender = state
        .supervisor
        .running
        .lock()
        .ok()
        .and_then(|map| map.get(&instance.uuid).map(|running| running.stdin.clone()));

    let Some(sender) = sender else {
        return Err(AppError::Other(format!(
            "\"{}\" is not running, or its console is not owned by this app",
            instance.name
        )));
    };

    sender
        .send(format!("{trimmed}\n"))
        .map_err(|_| AppError::Other("the server stopped accepting commands".into()))?;

    if let Ok(mut buffer) = state.supervisor.console(&instance.uuid).lock() {
        buffer.push_system(&format!("> {trimmed}"));
    }
    remember_command(state, id, trimmed).await?;
    Ok(())
}

/// The console keeps the last 100 commands per instance, oldest pruned.
pub async fn remember_command(state: &AppState, id: i64, command: &str) -> AppResult<()> {
    sqlx::query("INSERT INTO command_history (instance_id, command, ran_at) VALUES (?, ?, ?)")
        .bind(id)
        .bind(command)
        .bind(now_rfc3339())
        .execute(&state.db)
        .await?;

    sqlx::query(
        "DELETE FROM command_history
         WHERE instance_id = ? AND id NOT IN (
             SELECT id FROM command_history WHERE instance_id = ? ORDER BY id DESC LIMIT 100
         )",
    )
    .bind(id)
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Oldest first, which is the order an up-arrow history expects.
pub async fn command_history(state: &AppState, id: i64) -> AppResult<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT command FROM command_history WHERE instance_id = ? ORDER BY id ASC LIMIT 100",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// Graceful stop: `stop` on stdin, then SIGTERM, then SIGKILL, reporting how far
/// it had to go.
pub async fn stop(app: &AppHandle, state: &AppState, id: i64) -> AppResult<StopStage> {
    let instance = instance::get(&state.db, id).await?;
    let timeout = Duration::from_secs(instance.stop_timeout_s.clamp(5, 3_600) as u64);

    let running = state
        .supervisor
        .running
        .lock()
        .ok()
        .and_then(|map| map.get(&instance.uuid).map(|r| (r.pid, r.stdin.clone(), r.exited.clone(), r.stop_requested.clone())));

    let Some((pid, stdin, exited, stop_requested)) = running else {
        // Not ours: it may still be an orphan we adopted at startup.
        return stop_unmanaged(app, state, &instance).await;
    };

    stop_requested.store(true, Ordering::SeqCst);
    state.set_status(&instance.uuid, InstanceStatus::Stopping);
    events::instance_status(app, &instance.uuid, InstanceStatus::Stopping, None);

    let _ = stdin.send("stop\n".to_string());
    if wait_for_exit(&exited, timeout).await {
        return finish_stop(app, state, &instance, StopStage::Graceful).await;
    }

    tracing::warn!(instance = %instance.name, "stop command ignored; terminating");
    super::signal::request_terminate(pid);
    if wait_for_exit(&exited, TERMINATE_GRACE).await {
        return finish_stop(app, state, &instance, StopStage::Terminated).await;
    }

    tracing::warn!(instance = %instance.name, "terminate ignored; killing");
    super::signal::force_kill(pid);
    wait_for_exit(&exited, TERMINATE_GRACE).await;
    finish_stop(app, state, &instance, StopStage::Killed).await
}

/// Stops a server this app does not own: an orphan adopted at startup. There is
/// no stdin, so it goes straight to terminate and then kill.
async fn stop_unmanaged(
    app: &AppHandle,
    state: &AppState,
    instance: &Instance,
) -> AppResult<StopStage> {
    let (Some(pid), Some(start_time)) = (instance.pid, instance.process_start_time) else {
        return Ok(StopStage::AlreadyStopped);
    };
    let pid = u32::try_from(pid).map_err(|_| AppError::Other("invalid recorded pid".into()))?;

    // The pid is only trusted when the start time still matches.
    if process_start_time(pid) != Some(start_time as u64) {
        instance::reconcile::clear_pid(&state.db, instance.id, Some("stopped")).await?;
        state.set_status(&instance.uuid, InstanceStatus::Stopped);
        events::instance_status(app, &instance.uuid, InstanceStatus::Stopped, None);
        return Ok(StopStage::AlreadyStopped);
    }

    state.set_status(&instance.uuid, InstanceStatus::Stopping);
    events::instance_status(app, &instance.uuid, InstanceStatus::Stopping, None);

    super::signal::request_terminate(pid);
    let gone = Arc::new(AtomicBool::new(false));
    if !wait_for_pid_exit(pid, start_time as u64, &gone, TERMINATE_GRACE).await {
        super::signal::force_kill(pid);
        wait_for_pid_exit(pid, start_time as u64, &gone, TERMINATE_GRACE).await;
        instance::reconcile::clear_pid(&state.db, instance.id, Some("stopped")).await?;
        return finish_stop(app, state, instance, StopStage::Killed).await;
    }

    instance::reconcile::clear_pid(&state.db, instance.id, Some("stopped")).await?;
    finish_stop(app, state, instance, StopStage::Terminated).await
}

async fn finish_stop(
    app: &AppHandle,
    state: &AppState,
    instance: &Instance,
    stage: StopStage,
) -> AppResult<StopStage> {
    state.set_status(&instance.uuid, InstanceStatus::Stopped);
    events::instance_status(app, &instance.uuid, InstanceStatus::Stopped, None);
    events::instances_changed(app);
    record_event(&state.db, instance.id, "stopped", Some(stage.as_str())).await?;

    if let Ok(mut buffer) = state.supervisor.console(&instance.uuid).lock() {
        buffer.push_system(&stage.describe(&instance.name));
    }
    Ok(stage)
}

/// Hard kill, no grace period. Used by the "Force stop" button.
pub async fn kill(app: &AppHandle, state: &AppState, id: i64) -> AppResult<StopStage> {
    let instance = instance::get(&state.db, id).await?;

    let running = state
        .supervisor
        .running
        .lock()
        .ok()
        .and_then(|map| map.get(&instance.uuid).map(|r| (r.pid, r.exited.clone(), r.stop_requested.clone())));

    match running {
        Some((pid, exited, stop_requested)) => {
            stop_requested.store(true, Ordering::SeqCst);
            super::signal::force_kill(pid);
            wait_for_exit(&exited, TERMINATE_GRACE).await;
            finish_stop(app, state, &instance, StopStage::Killed).await
        }
        None => stop_unmanaged(app, state, &instance).await,
    }
}

/// Stop then start, keeping the caller's timeout semantics.
pub async fn restart(app: &AppHandle, state: &AppState, id: i64) -> AppResult<()> {
    if state.supervisor.is_running(&instance_uuid(state, id).await?) {
        stop(app, state, id).await?;
    }
    start(app, state, id).await
}

async fn instance_uuid(state: &AppState, id: i64) -> AppResult<String> {
    Ok(instance::get(&state.db, id).await?.uuid)
}

/// Waits for the monitor task to observe the exit.
async fn wait_for_exit(exited: &Arc<AtomicBool>, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if exited.load(Ordering::SeqCst) {
            return true;
        }
        tokio::time::sleep(EXIT_POLL).await;
    }
    exited.load(Ordering::SeqCst)
}

/// Same, for a process we do not own: the pid must disappear, or be replaced by
/// a process with a different start time.
async fn wait_for_pid_exit(
    pid: u32,
    start_time: u64,
    gone: &Arc<AtomicBool>,
    timeout: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if process_start_time(pid) != Some(start_time) {
            gone.store(true, Ordering::SeqCst);
            return true;
        }
        tokio::time::sleep(EXIT_POLL).await;
    }
    false
}

/// The OS-reported start time of a pid, which is what makes a pid trustworthy.
pub fn process_start_time(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system
        .process(Pid::from_u32(pid))
        .map(|process| process.start_time())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_reports_a_start_time() {
        let me = std::process::id();
        let start = process_start_time(me);
        assert!(start.is_some(), "the current process must be visible");
        assert!(start.unwrap() > 0);
    }

    #[test]
    fn a_dead_pid_reports_no_start_time() {
        assert_eq!(process_start_time(4_294_967_280), None);
    }

    #[tokio::test]
    async fn waiting_returns_as_soon_as_the_flag_flips() {
        let exited = Arc::new(AtomicBool::new(false));
        let flag = exited.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            flag.store(true, Ordering::SeqCst);
        });

        let started = std::time::Instant::now();
        assert!(wait_for_exit(&exited, Duration::from_secs(5)).await);
        assert!(started.elapsed() < Duration::from_secs(2), "it did not wait the full timeout");
    }

    #[tokio::test]
    async fn waiting_gives_up_at_the_timeout() {
        let exited = Arc::new(AtomicBool::new(false));
        assert!(!wait_for_exit(&exited, Duration::from_millis(300)).await);
    }

    #[tokio::test]
    async fn command_history_keeps_only_the_last_hundred() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'A', 'Z:/a', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        for index in 0..120 {
            remember_command(&state, 1, &format!("say {index}")).await.unwrap();
        }

        let history = command_history(&state, 1).await.unwrap();
        assert_eq!(history.len(), 100);
        assert_eq!(history.first().unwrap(), "say 20", "the oldest are pruned");
        assert_eq!(history.last().unwrap(), "say 119");
    }

    #[tokio::test]
    async fn sending_to_a_stopped_instance_is_a_readable_error() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'Survival', 'Z:/a', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        let err = send_command(&state, 1, "say hi").await.unwrap_err();
        assert!(err.to_string().contains("is not running"), "{err}");
    }

    #[tokio::test]
    async fn an_empty_command_is_ignored_rather_than_sent() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'A', 'Z:/a', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        assert!(send_command(&state, 1, "   ").await.is_ok());
        assert!(command_history(&state, 1).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn player_sightings_are_recorded_and_updated() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'A', 'Z:/a', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        let uuid = "069a79f4-44e9-4726-a5be-fca90e38aaf5";
        record_player(&state, 1, uuid, "Notch").await;
        record_player(&state, 1, uuid, "Notch").await;
        // A rename keeps the same UUID row, which is why the UUID is the key.
        record_player(&state, 1, uuid, "NotchRenamed").await;

        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT uuid, name FROM players_seen WHERE instance_id = 1")
                .fetch_all(&state.db)
                .await
                .unwrap();
        assert_eq!(rows.len(), 1, "one row per player, not per sighting");
        assert_eq!(rows[0].1, "NotchRenamed");
    }

    #[tokio::test]
    async fn recent_crashes_only_counts_inside_the_window() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'A', 'Z:/a', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        // Two crashes now, one an hour ago.
        record_event(&state.db, 1, "crashed", None).await.unwrap();
        record_event(&state.db, 1, "crashed", None).await.unwrap();
        sqlx::query("INSERT INTO instance_events (instance_id, ts, kind) VALUES (1, ?, 'crashed')")
            .bind(
                (chrono::Utc::now() - chrono::Duration::hours(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            )
            .execute(&state.db)
            .await
            .unwrap();

        assert_eq!(recent_crashes(&state, 1, 600).await, 2);
        assert_eq!(recent_crashes(&state, 1, 7_200).await, 3);
    }
}
