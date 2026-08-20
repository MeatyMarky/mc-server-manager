//! Owning running servers: spawn, console capture, stdin, stop, auto-restart.
//!
//! The supervisor holds no `Child`: everything after spawn is done by pid, the
//! same way an orphan adopted at startup is handled. That keeps one code path
//! for "a server we started" and "a server that outlived the app", and the
//! pid is only ever trusted together with its recorded start time.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
    /// Set when the server printed its "Done" line. An exit before this is a
    /// start that failed, not a crash, and must not be retried.
    reached_ready: Arc<AtomicBool>,
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
    /// Records that this server finished starting.
    pub(crate) fn mark_ready(&self, uuid: &str) {
        if let Ok(map) = self.running.lock() {
            if let Some(running) = map.get(uuid) {
                running.reached_ready.store(true, Ordering::SeqCst);
            }
        }
    }

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

    // Scripts bring their own Java: they run `java` from PATH and read their
    // heap from user_jvm_args.txt, so neither the pinned runtime nor the RAM
    // fields apply. The one thing that can still be checked is whether that
    // PATH java can hold the heap the script will ask for.
    if instance.launch_kind == crate::db::models::LaunchKind::Script {
        let resolved = java::detect::java_on_path().unwrap_or_else(|| PathBuf::from("java"));
        let heap_mb = launch::script_heap_mb(&dir);
        report_choice(state, instance, &resolved, heap_mb, "PATH (start script)").await;

        if let Some(requested_mb) = heap_mb {
            check_heap(state, &resolved, requested_mb).await?;
        }
        // A start script runs `java` from PATH, which the user controls the same
        // way a pin is under their control: warned about, not refused.
        check_java_version(
            state,
            instance,
            &resolved,
            java::required_for(instance.java_major, &instance.mc_version),
            java::fit_for(instance.server_type),
            java::Origin::Pinned,
        )
        .await?;
        return Ok(resolved);
    }

    // The recorded number and the version table, whichever is higher: a stale
    // row must not be able to lower the requirement.
    let required = java::required_for(instance.java_major, &instance.mc_version);
    // Vanilla and the Bukkit family take anything newer; the mod loaders want
    // the major their Minecraft release was built against.
    let fit = java::fit_for(instance.server_type);

    // A pin that points at nothing is an error rather than a silent fallback:
    // the user chose it, so its disappearance is worth saying out loud.
    if let Some(pinned) = &instance.java_path {
        if !Path::new(pinned).is_file() {
            return Err(AppError::JavaPinnedMissing {
                instance: instance.name.clone(),
                path: pinned.clone(),
            });
        }
    }

    if let Some(selection) =
        java::select_for(state, instance.java_path.as_deref(), required, fit).await?
    {
        let how = match selection.origin {
            java::Origin::Pinned => "pinned for this server",
            java::Origin::Managed => "downloaded by this app",
            java::Origin::System => "chosen automatically",
        };
        report_choice(
            state,
            instance,
            &selection.path,
            launch::effective_heap_mb(instance),
            how,
        )
        .await;
        // Whatever it came from, it is asked what it is and refused if it
        // cannot run this server — a managed runtime included.
        check_java_version(state, instance, &selection.path, required, fit, selection.origin)
            .await?;
        check_heap_fits(state, instance, &selection.path).await?;
        return Ok(selection.path);
    }

    // "No Java at all", "Java, but too old" and "Java, but not the one this
    // loader wants" have three different fixes, and the last two are the ones
    // that confuse people: they installed Java, so why is the app complaining?
    let newest = java::list(&state.db)
        .await?
        .into_iter()
        .map(|runtime| runtime.major)
        .max();
    Err(match (newest, fit) {
        (Some(found), java::JavaFit::Exact) if found != required => AppError::JavaWrongMajor {
            required,
            found,
            mc_version: instance.mc_version.clone(),
            server_type: instance.server_type.label().to_string(),
        },
        (Some(found), _) => AppError::JavaTooOld {
            required,
            found,
            mc_version: instance.mc_version.clone(),
        },
        (None, _) => AppError::JavaNotFound { required },
    })
}

/// Asks the resolved binary what version it is, and refuses if it is too old.
///
/// The database is not trusted for this: a row can be stale, or wrong, and the
/// consequence is a server that starts and then dies with
/// `UnsupportedClassVersionError` several seconds later — a message that names
/// class file numbers rather than Java versions.
async fn check_java_version(
    state: &AppState,
    instance: &Instance,
    java: &Path,
    required: i64,
    fit: java::JavaFit,
    origin: java::Origin,
) -> AppResult<()> {
    let Some(found) = java::probe_major(java).await else {
        // Unreadable is not proof of anything; the launch carries on and the
        // console will say what happened.
        tracing::warn!(java = %java.display(), "could not read the Java version before launch");
        return Ok(());
    };

    // Too old is always a refusal, whoever chose it: the server cannot load its
    // own class files, and a pin is not permission for that.
    if !java::satisfies(found, required) {
        return Err(AppError::JavaTooOld {
            required,
            found,
            mc_version: instance.mc_version.clone(),
        });
    }

    if fit.accepts(found, required) {
        return Ok(());
    }

    // Newer than a loader wants. A pin is the user saying they know better, so
    // it runs — with the reason on the record rather than a surprise later.
    if origin == java::Origin::Pinned {
        let message = format!(
            "Running on Java {found}, though {} {} is tested on Java {required}. Mod loaders \
             rewrite code as they load it, so a crash inside a mod may be this rather than the \
             mod.",
            instance.server_type.label(),
            instance.mc_version
        );
        tracing::warn!(
            instance = %instance.name,
            instance_id = instance.id,
            java = %java.display(),
            found,
            required,
            "pinned Java is not the major this loader was tested on"
        );
        if let Ok(mut buffer) = state.supervisor.console(&instance.uuid).lock() {
            buffer.push_system(&message);
        }
        return Ok(());
    }

    Err(AppError::JavaWrongMajor {
        required,
        found,
        mc_version: instance.mc_version.clone(),
        server_type: instance.server_type.label().to_string(),
    })
}

/// Refuses a launch a 32-bit JVM would reject anyway.
///
/// The JVM's own refusal ("Invalid maximum heap size: -Xmx8192M") arrives in the
/// console without naming which Java produced it, which is what made this hard
/// to diagnose. Checking here means the error can name the binary, its width and
/// the way out.
async fn check_heap_fits(state: &AppState, instance: &Instance, java: &Path) -> AppResult<()> {
    // The heap comes from the launch plan's own resolution — RAM fields first,
    // custom -Xmx last and winning — not from the arguments string alone. An
    // instance whose 8192 MB lives in the RAM field has no -Xmx to find.
    let Some(requested_mb) = launch::effective_heap_mb(instance) else {
        return Ok(());
    };
    check_heap(state, java, requested_mb).await
}

/// The check itself, for callers that resolve the heap differently (scripts).
async fn check_heap(state: &AppState, java: &Path, requested_mb: i64) -> AppResult<()> {
    if requested_mb <= crate::java::version::MAX_HEAP_32BIT_MB {
        return Ok(());
    }

    if java::bits_of(&state.db, java).await == Some(32) {
        return Err(AppError::Java32Bit {
            path: java.to_string_lossy().to_string(),
            requested_mb,
            limit_mb: crate::java::version::MAX_HEAP_32BIT_MB,
        });
    }
    Ok(())
}

/// Records which JVM a launch resolved to, and how wide it is.
///
/// Nothing in the console says which `java` was used, so a JVM that refuses the
/// heap looks like the app misbehaving. This line, at info level and on every
/// attempt, is the first thing to look at when a start fails.
async fn report_choice(
    state: &AppState,
    instance: &Instance,
    java: &Path,
    heap_mb: Option<i64>,
    how: &str,
) {
    let bits = java::bits_of(&state.db, java).await;
    let major = java::probe_major(java).await;
    tracing::info!(
        instance = %instance.name,
        instance_id = instance.id,
        java = %java.display(),
        java_major = major.map(|m| m.to_string()).unwrap_or_else(|| "unknown".into()),
        bits = bits.map(|b| b.to_string()).unwrap_or_else(|| "unknown".into()),
        heap_mb = heap_mb.unwrap_or(0),
        selection = how,
        launch_kind = ?instance.launch_kind,
        "resolved Java for launch"
    );

    if let Ok(mut buffer) = state.supervisor.console(&instance.uuid).lock() {
        buffer.push_system(&format!(
            "Java: {} ({}, {}, {how}){}",
            java.display(),
            match major {
                Some(major) => format!("version {major}"),
                None => "version unknown".to_string(),
            },
            match bits {
                Some(bits) => format!("{bits}-bit"),
                None => "width unknown".to_string(),
            },
            match heap_mb {
                Some(mb) => format!(", heap {mb} MB"),
                None => String::new(),
            }
        ));
    }
}

/// Starts a server. Returns once the process is spawned; readiness arrives later
/// as a status event driven by the server's own "Done" line.
pub async fn start(app: &AppHandle, state: &AppState, id: i64) -> AppResult<()> {
    let instance = instance::get(&state.db, id).await?;
    let java = preflight(state, &instance).await?;
    let plan = launch::plan(&instance, &java)?;

    // The exact command line, before anything else happens with it. Every token
    // is quoted and escaped, so a stray carriage return or a doubled flag is
    // visible rather than implied — the JVM's own complaints name a flag but
    // never say what was actually passed to it.
    let command_line = launch::quoted_command(&plan.program, &plan.args);
    tracing::info!(
        instance = %instance.name,
        instance_id = instance.id,
        argv = %command_line,
        working_dir = %plan.working_dir.display(),
        "spawning server process"
    );

    // Refuse a command line the JVM would only reject after it starts.
    launch::validate_args(&plan.args)?;

    // A first boot writes its own server.properties, and complains loudly at
    // ERROR on the way there. Decided from the row as it stands now, because
    // `last_started_at` is set a few lines below, and from the folder as it is
    // before the process exists.
    let properties_path = crate::paths::server_properties_path(&instance.path_buf());
    let properties_exists = properties_path.exists();
    let first_boot = ConsoleBuffer::is_first_boot(
        instance.last_started_at.as_deref(),
        properties_exists,
    );

    // Both facts and the decision, every launch. Working out afterwards why a
    // first boot still showed a red stack trace took a database, a folder
    // listing and three file timestamps; it should take one line.
    tracing::info!(
        instance = %instance.name,
        instance_id = instance.id,
        last_started_at = instance.last_started_at.as_deref().unwrap_or("never"),
        properties_path = %properties_path.display(),
        properties_exists,
        first_boot,
        "first-boot properties grace {}",
        if first_boot { "armed" } else { "not armed" }
    );

    let console = state.supervisor.console(&instance.uuid);
    if let Ok(mut buffer) = console.lock() {
        buffer.expect_missing_properties(first_boot);
        buffer.push_system(&format!("Command: {command_line}"));
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
    let reached_ready = Arc::new(AtomicBool::new(false));

    state.supervisor.insert(
        &instance.uuid,
        Running {
            pid,
            stdin: stdin_tx,
            exited: exited.clone(),
            stop_requested: stop_requested.clone(),
            reached_ready: reached_ready.clone(),
        },
    );

    spawn_console_pump(app.clone(), instance.clone(), console, line_rx);
    spawn_monitor(
        app.clone(),
        instance,
        child,
        exited,
        stop_requested,
        reached_ready,
    );

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
            state.supervisor.mark_ready(&instance.uuid);
            // If the app died between save-off and save-on, this is the first
            // moment a console exists to put it right.
            crate::backup::saveguard::recover_on_start(&state, instance.id).await;
            tracing::info!(
                instance = %instance.name,
                instance_id = instance.id,
                took = ?took,
                "server is ready"
            );
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
        LogEvent::ClassVersion {
            needs,
            found,
            class_name,
        } => {
            // The JVM says this in class file numbers; the user gets Java
            // versions and the name of the runtime that was actually used.
            let message = format!(
                "This server needs Java {needs} or newer, but it ran on Java {found}. \
                 Install Java {needs} and pick it in this server's Settings tab."
            );
            tracing::warn!(
                instance = %instance.name,
                instance_id = instance.id,
                needs,
                found,
                class = class_name.as_deref().unwrap_or("-"),
                "server refused its own class files"
            );
            if let Ok(mut buffer) = state.supervisor.console(&instance.uuid).lock() {
                buffer.push_system(&message);
            }
            let _ = record_event(&state.db, instance.id, "error", Some(&message)).await;
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
    reached_ready: Arc<AtomicBool>,
) {
    tauri::async_runtime::spawn(async move {
        use tauri::Manager;

        let status = child.wait().await;
        exited.store(true, Ordering::SeqCst);

        let code = status.as_ref().ok().and_then(|status| status.code());

        let state = app.state::<AppState>();
        state.supervisor.remove(&instance.uuid);

        let requested = stop_requested.load(Ordering::SeqCst);
        let ready = reached_ready.load(Ordering::SeqCst);
        let exit = backoff::classify(requested, ready);
        let crashed = exit != backoff::Exit::Requested;
        let failed_start = exit == backoff::Exit::FailedStart;

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

        // The last thing the process said is usually the whole explanation —
        // "Invalid maximum heap size: -Xmx8192M", a stack trace, a port
        // complaint — so a failed start repeats it instead of burying it.
        let last_line = state
            .supervisor
            .tail(&instance.uuid, 40)
            .into_iter()
            .rev()
            .map(|line| line.message.trim().to_string())
            .find(|line| !line.is_empty());

        if let Ok(mut buffer) = state.supervisor.console(&instance.uuid).lock() {
            buffer.push_system(&match code {
                Some(code) if failed_start => format!(
                    "Server exited with code {code} before it finished starting. \
                     Not restarting: a start that fails this way fails the same way every time."
                ),
                Some(code) if crashed => format!("Server exited with code {code}"),
                Some(code) => format!("Server stopped (exit code {code})"),
                None if failed_start => {
                    "Server process ended before it finished starting. Not restarting.".to_string()
                }
                None => "Server process ended".to_string(),
            });
        }

        let status_now = backoff::status_for(exit);
        state.set_status(&instance.uuid, status_now);
        events::instance_status(&app, &instance.uuid, status_now, code.map(i64::from));
        events::instances_changed(&app);

        // A failed start is recorded under its own kind: it must not count
        // towards the crash window, or a few bad starts would exhaust the
        // restart budget of a server that later runs fine.
        let kind = match exit {
            backoff::Exit::Requested => "stopped",
            backoff::Exit::FailedStart => "failed_start",
            backoff::Exit::Crash => "crashed",
        };
        let detail = match &last_line {
            Some(line) if failed_start => format!("exit code {code:?}; last output: {line}"),
            _ => format!("exit code {code:?}"),
        };
        let _ = record_event(&state.db, instance.id, kind, Some(&detail)).await;

        if exit == backoff::Exit::Requested {
            return;
        }

        // Auto-restart, backoff-limited so an instantly-crashing server cannot spin.
        let recent = recent_crashes(&state, instance.id, instance.restart_window_s).await;
        match backoff::decide(
            instance.auto_restart,
            exit,
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
                    tracing::warn!(
                        error = %err,
                        instance = %instance.name,
                        instance_id = instance.id,
                        "auto-restart failed"
                    );
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
                tracing::warn!(
                    instance = %instance.name,
                    instance_id = instance.id,
                    "{message}"
                );
                if let Ok(mut buffer) = state.supervisor.console(&instance.uuid).lock() {
                    buffer.push_system(&message);
                }
                let _ = record_event(&state.db, instance.id, "error", Some(&message)).await;
            }
            backoff::RestartDecision::FailedStart => {
                // Surfaced, not retried. The console line above already says
                // what happened; this is the copy the Settings tab's event list
                // and the problem report will show.
                let message = match &last_line {
                    Some(line) => format!("The server did not finish starting: {line}"),
                    None => "The server did not finish starting.".to_string(),
                };
                tracing::warn!(
                    instance = %instance.name,
                    instance_id = instance.id,
                    "{message}"
                );
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

    tracing::warn!(
        instance = %instance.name,
        instance_id = instance.id,
        "stop command ignored; terminating"
    );
    super::signal::request_terminate(pid);
    if wait_for_exit(&exited, TERMINATE_GRACE).await {
        return finish_stop(app, state, &instance, StopStage::Terminated).await;
    }

    tracing::warn!(
        instance = %instance.name,
        instance_id = instance.id,
        "terminate ignored; killing"
    );
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

    /// An instance with the memory settings the bug report used.
    async fn heap_case(max_ram_mb: i64, jvm_args: &str) -> (AppState, Instance) {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, min_ram_mb, max_ram_mb, created_at, updated_at)
             VALUES ('u1', 'Survival', 'Z:/survival', 'paper', '1.21.4', 'jar', ?, '[]', 1024, ?, ?, ?)",
        )
        .bind(jvm_args)
        .bind(max_ram_mb)
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        let instance = instance::get(&state.db, 1).await.unwrap();
        (state, instance)
    }

    async fn seed_java(state: &AppState, path: &str, bits: i64) {
        sqlx::query(
            "INSERT INTO java_runtimes (path, major, bits, source, valid, detected_at)
             VALUES (?, 8, ?, 'common_dir', 1, ?)",
        )
        .bind(path)
        .bind(bits)
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_32_bit_jvm_is_refused_before_it_can_reject_the_heap_itself() {
        let (state, instance) = heap_case(8192, "[]").await;
        let java = "C:/Program Files (x86)/Java/jre1.8.0_451/bin/java.exe";
        seed_java(&state, java, 32).await;

        let err = check_heap_fits(&state, &instance, Path::new(java))
            .await
            .unwrap_err();

        assert_eq!(err.kind(), "java_32bit");
        // The JVM's own refusal never says which Java it was; this one does.
        let message = err.user_message();
        assert!(message.contains("(x86)"), "{message}");
        assert!(message.contains("32-bit"), "{message}");
        assert!(message.contains("8192"), "{message}");
        assert!(err.hint().unwrap().contains("64-bit"));
    }

    #[tokio::test]
    async fn a_32_bit_jvm_with_a_heap_it_can_hold_is_allowed() {
        // Refusing every 32-bit launch would break a small server that works.
        let (state, instance) = heap_case(1024, "[]").await;
        let java = "C:/Program Files (x86)/Java/jre1.8.0_451/bin/java.exe";
        seed_java(&state, java, 32).await;

        assert!(check_heap_fits(&state, &instance, Path::new(java)).await.is_ok());
    }

    #[tokio::test]
    async fn a_64_bit_jvm_is_never_blocked_by_the_heap_check() {
        let (state, instance) = heap_case(8192, "[]").await;
        let java = "C:/Program Files/Java/jdk-21/bin/java.exe";
        seed_java(&state, java, 64).await;

        assert!(check_heap_fits(&state, &instance, Path::new(java)).await.is_ok());
    }

    #[tokio::test]
    async fn a_custom_xmx_is_what_gets_checked_not_the_ram_field() {
        // The form says 1 GB, the JVM args say 8 GB, and the JVM reads the last
        // -Xmx it is given.
        let (state, instance) = heap_case(1024, r#"["-Xmx8G"]"#).await;
        let java = "C:/Program Files (x86)/Java/jre1.8.0_451/bin/java.exe";
        seed_java(&state, java, 32).await;

        assert_eq!(
            check_heap_fits(&state, &instance, Path::new(java))
                .await
                .unwrap_err()
                .kind(),
            "java_32bit"
        );
    }

    #[tokio::test]
    async fn a_runtime_of_unknown_width_does_not_block_a_launch() {
        // Nothing proves this one is 32-bit, and refusing to start on a guess
        // would be worse than the JVM's own error.
        let (state, instance) = heap_case(8192, "[]").await;

        assert!(check_heap_fits(&state, &instance, Path::new("/nowhere/bin/java"))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn the_ram_field_alone_is_enough_to_refuse_a_32_bit_jvm() {
        // The reported instance: 8192 MB in the RAM field, custom arguments
        // that tune the collector and say nothing about the heap. Nothing here
        // contains "-Xmx", so a check that reads only the arguments finds
        // nothing to object to and the JVM refuses at spawn instead.
        let (state, instance) = heap_case(
            8192,
            r#"["-XX:+UseG1GC","-XX:+ParallelRefProcEnabled","-XX:+UnlockExperimentalVMOptions","-XX:+DisableExplicitGC"]"#,
        )
        .await;
        assert_eq!(launch::effective_heap_mb(&instance), Some(8192));

        let java = "C:/Program Files (x86)/Java/jre1.8.0_501/bin/java.exe";
        seed_java(&state, java, 32).await;

        let err = check_heap_fits(&state, &instance, Path::new(java))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "java_32bit");
        assert!(err.user_message().contains("8192"), "{}", err.user_message());
    }

    #[tokio::test]
    async fn preflight_refuses_before_anything_is_spawned() {
        // End to end through preflight rather than the check alone: a pinned
        // 32-bit runtime plus the RAM field must not reach `launch::plan`.
        let dir = tempfile::tempdir().unwrap();
        let instance_dir = dir.path().join("survival");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("server.jar"), b"jar").unwrap();

        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, dir.path().to_path_buf());
        let now = now_rfc3339();
        let java = dir.path().join("java32.exe");
        std::fs::write(&java, b"not really java").unwrap();

        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                launch_target, installed_at, eula_accepted, java_path, jvm_args, server_args,
                min_ram_mb, max_ram_mb, created_at, updated_at)
             VALUES ('u1', 'Survival', ?, 'fabric', '26.2', 'jar', 'server.jar', ?, 1, ?, '[]', '[]',
                1024, 8192, ?, ?)",
        )
        .bind(instance_dir.to_string_lossy().to_string())
        .bind(&now)
        .bind(java.to_string_lossy().to_string())
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO java_runtimes (path, major, bits, source, valid, detected_at)
             VALUES (?, 25, 32, 'manual', 1, ?)",
        )
        .bind(java.to_string_lossy().to_string())
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        let instance = instance::get(&state.db, 1).await.unwrap();
        let err = preflight(&state, &instance).await.unwrap_err();
        assert_eq!(err.kind(), "java_32bit");
    }

    #[tokio::test]
    async fn a_script_launch_is_checked_against_the_java_it_will_really_use() {
        // Scripts ignore the app's chosen runtime and the RAM fields: they run
        // `java` from PATH with whatever user_jvm_args.txt says. That was the
        // one launch path with no check at all.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("user_jvm_args.txt"),
            b"-Xmx8G\n",
        )
        .unwrap();
        assert_eq!(launch::script_heap_mb(dir.path()), Some(8192));

        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());
        let java = "C:/Program Files (x86)/Common Files/Oracle/Java/java8path/java.exe";
        sqlx::query(
            "INSERT INTO java_runtimes (path, major, bits, source, valid, detected_at)
             VALUES (?, 8, 32, 'path', 1, ?)",
        )
        .bind(java)
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();

        let err = check_heap(&state, Path::new(java), 8192).await.unwrap_err();
        assert_eq!(err.kind(), "java_32bit");

        // A script asking for a heap the 32-bit JVM can hold still runs.
        assert!(check_heap(&state, Path::new(java), 1024).await.is_ok());
    }

    #[tokio::test]
    async fn a_java_too_old_for_the_server_is_refused_before_spawning() {
        // The real shape: a 26.2 server and the Java 17 that was chosen for it.
        let dir = tempfile::tempdir().unwrap();
        let instance_dir = dir.path().join("survival");
        std::fs::create_dir_all(&instance_dir).unwrap();

        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, dir.path().to_path_buf());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, min_ram_mb, max_ram_mb, created_at, updated_at)
             VALUES ('u1', 'idk', ?, 'fabric', '26.2', 'jar', '[]', '[]', 1024, 4096, ?, ?)",
        )
        .bind(instance_dir.to_string_lossy().to_string())
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();
        let instance = instance::get(&state.db, 1).await.unwrap();

        // A real Java on this machine, whatever it is, checked against a
        // requirement nothing can satisfy.
        let Some(java) = crate::java::detect::java_on_path() else {
            return;
        };
        let err = check_java_version(
            &state,
            &instance,
            &java,
            999,
            java::JavaFit::Floor,
            java::Origin::System,
        )
        .await
        .expect_err("no JVM is version 999");
        assert_eq!(err.kind(), "java_too_old");
        let message = err.user_message();
        assert!(message.contains("26.2"), "{message}");
        assert!(message.contains("999"), "{message}");

        // And the same binary passes a requirement it does satisfy.
        assert!(check_java_version(
            &state,
            &instance,
            &java,
            8,
            java::JavaFit::Floor,
            java::Origin::System
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn an_unreadable_java_does_not_block_the_launch_on_a_guess() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, dir.path().to_path_buf());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'idk', 'Z:/idk', 'fabric', '26.2', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();
        let instance = instance::get(&state.db, 1).await.unwrap();

        assert!(
            check_java_version(
                &state,
                &instance,
                Path::new("/nowhere/bin/java"),
                25,
                java::JavaFit::Floor,
                java::Origin::System
            )
            .await
            .is_ok(),
            "an unanswerable probe is not proof of anything"
        );
    }
}
