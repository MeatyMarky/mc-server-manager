pub mod backup;
pub mod commands;
pub mod config;
pub mod db;
pub mod diag;
pub mod error;
pub mod download;
pub mod events;
pub mod http;
pub mod instance;
pub mod java;
pub mod logparse;
pub mod mcversion;
pub mod providers;
pub mod metrics;
pub mod mods;
pub mod paths;
pub mod players;
pub mod process;
pub mod state;
pub mod tasks;
pub mod worlds;

use std::path::PathBuf;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, WindowEvent};
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

const MAIN_WINDOW: &str = "main";

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            init_tracing(&data_dir);

            let db_path = data_dir.join("msm.sqlite");
            tracing::info!(path = %db_path.display(), "opening database");
            let pool = tauri::async_runtime::block_on(db::connect(&db_path))?;

            // A damaged database does not announce itself: a broken index makes
            // COUNT(*) disagree with a table scan and makes ON CONFLICT update
            // the wrong row, which reads as the app forgetting things. Say so
            // loudly instead, and throw away the one table that is only a cache.
            match tauri::async_runtime::block_on(db::integrity_problems(&pool)) {
                Ok(problems) if problems.is_empty() => {}
                Ok(problems) => {
                    tracing::error!(
                        count = problems.len(),
                        first = %problems[0],
                        "the database reports integrity problems; \
                         rebuilding the Java cache and continuing"
                    );
                    if let Err(err) = tauri::async_runtime::block_on(java::rebuild_cache(&pool)) {
                        tracing::warn!(error = %err, "could not rebuild the Java cache");
                    }
                }
                Err(err) => tracing::warn!(error = %err, "could not check the database"),
            }

            // Before the collector, the scheduler or anything else writes: a
            // VACUUM needs a quiet moment, and this is the only one there is.
            match tauri::async_runtime::block_on(db::enable_incremental_vacuum(&pool)) {
                Ok(true) => tracing::info!("database rebuilt for incremental auto-vacuum"),
                Ok(false) => {}
                Err(err) => tracing::warn!(error = %err, "could not set incremental auto-vacuum"),
            }

            let state = AppState::new(pool.clone(), data_dir);

            // Orphan recovery has to happen before the UI paints, so the sidebar
            // shows adopted servers instead of claiming everything is stopped.
            match tauri::async_runtime::block_on(instance::reconcile::reconcile_all(&state)) {
                Ok(0) => {}
                Ok(n) => tracing::info!(count = n, "adopted orphaned server processes"),
                Err(err) => tracing::error!(error = %err, "orphan reconciliation failed"),
            }

            // An app killed mid-backup can leave a server with saving off. This
            // decides, per instance, whether that state died with the process or
            // is still live and needs a console to fix.
            match tauri::async_runtime::block_on(backup::saveguard::reconcile_on_launch(&state)) {
                Ok(0) => {}
                Ok(n) => tracing::warn!(count = n, "instances still have world saving disabled"),
                Err(err) => tracing::error!(error = %err, "could not reconcile saving markers"),
            }

            tauri::async_runtime::spawn(metrics::retention::pruner_loop(pool.clone()));
            // One sampler and one scheduler for the whole app, whatever the
            // instance count. Both are started after `manage` below so they can
            // read the state; see the spawns further down.
            // Detect in the background so the Settings tab and the install flow
            // have something to offer immediately — and redo it when the cached
            // list is a day old, because a JDK installed since the last scan is
            // otherwise invisible until somebody presses Rescan.
            tauri::async_runtime::spawn(async move {
                match java::rescan_if_stale(&pool).await {
                    Ok(true) => {}
                    Ok(false) => tracing::debug!("Java cache is current"),
                    Err(err) => tracing::warn!(error = %err, "Java detection failed"),
                }
            });
            app.manage(state);
            setup_tray(app.handle())?;

            // Resource sampling and scheduled backups: exactly one task each.
            // The scheduler's first tick is what catches up anything that was
            // due while the app was closed.
            // The self-check runs once at launch: whatever is wrong is in the
            // log before the user notices the symptom.
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    use tauri::Manager;
                    let state = handle.state::<AppState>();
                    let health = diag::health(&state).await;
                    for check in &health.checks {
                        match check.status {
                            diag::HealthStatus::Ok => {
                                tracing::info!(check = %check.name, detail = %check.detail, "self-check")
                            }
                            diag::HealthStatus::Warn => {
                                tracing::warn!(check = %check.name, detail = %check.detail, "self-check")
                            }
                            diag::HealthStatus::Fail => {
                                tracing::error!(check = %check.name, detail = %check.detail, "self-check")
                            }
                        }
                    }
                });
            }

            tauri::async_runtime::spawn(metrics::collector::run(app.handle().clone()));
            tauri::async_runtime::spawn(backup::runner::run(app.handle().clone()));
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window minimizes to tray; servers keep running.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(err) = window.hide() {
                    tracing::warn!(error = %err, "could not hide the window");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::instances::instance_list,
            commands::instances::instance_get,
            commands::instances::instance_create,
            commands::instances::instance_clone,
            commands::instances::instance_rename,
            commands::instances::instance_update,
            commands::instances::instance_delete,
            commands::instances::instance_locate,
            commands::instances::instance_suggest_path,
            commands::instances::instance_import_detect,
            commands::instances::instance_import,
            commands::instances::instance_force_stop,
            commands::instances::instance_open_folder,
            commands::app::app_info,
            commands::app::app_quit,
            commands::app::live_instances,
            commands::app::settings_get_all,
            commands::app::settings_set,
            commands::setup::provider_versions,
            commands::setup::provider_builds,
            commands::setup::install_server,
            commands::setup::task_cancel,
            commands::setup::eula_get,
            commands::setup::eula_set,
            commands::setup::read_installer_log,
            commands::java::java_list,
            commands::java::java_rescan,
            commands::java::java_add_manual,
            commands::java::java_status,
            commands::java::java_required_for,
            commands::process::instance_start,
            commands::process::instance_stop,
            commands::process::instance_kill,
            commands::process::instance_restart,
            commands::process::instance_send_command,
            commands::process::console_tail,
            commands::process::command_history,
            commands::process::port_status,
            commands::config::properties_read,
            commands::config::properties_write,
            commands::config::properties_schema,
            commands::players::players_read,
            commands::players::players_mutate,
            commands::players::players_resolve_uuid,
            commands::worlds::worlds_list,
            commands::worlds::world_measure,
            commands::worlds::world_switch,
            commands::worlds::world_delete,
            commands::worlds::world_export,
            commands::worlds::world_import,
            commands::mods::mods_list,
            commands::mods::mods_search,
            commands::mods::mods_versions,
            commands::mods::mods_plan,
            commands::mods::mods_install,
            commands::mods::mods_set_enabled,
            commands::mods::mods_set_pinned,
            commands::mods::mods_uninstall,
            commands::mods::mods_install_local,
            commands::mods::mods_check_updates,
            commands::mods::mods_loader,
            commands::mods::mrpack_plan,
            commands::mods::mrpack_import,
            commands::backups::backups_list,
            commands::backups::backup_plan,
            commands::backups::backup_estimate,
            commands::backups::backup_create,
            commands::backups::backup_delete,
            commands::backups::backup_preview,
            commands::backups::backup_restore,
            commands::backups::backups_prune,
            commands::backups::schedules_list,
            commands::backups::schedule_save,
            commands::backups::schedule_delete,
            commands::backups::schedule_run_now,
            commands::backups::metrics_range,
            commands::backups::metrics_heap_bytes,
            commands::diag::build_info,
            commands::diag::report_preview,
            commands::diag::report_write,
            commands::diag::startup_readiness,
            commands::diag::health_check,
            commands::runtimes::managed_runtimes_list,
            commands::runtimes::managed_runtimes_size,
            commands::runtimes::managed_runtime_delete,
            commands::runtimes::managed_runtime_install,
            commands::runtimes::java_plan_for,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start the application");
}

fn init_tracing(data_dir: &std::path::Path) {
    let logs = data_dir.join("logs");
    let filter = EnvFilter::try_from_env("MSM_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    // File logging is best-effort: without a writable data folder the app still
    // runs, it just logs to stderr only.
    match std::fs::create_dir_all(&logs) {
        Ok(()) => {
            let appender = tracing_appender::rolling::daily(&logs, "msm.log");
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(appender)
                .with_ansi(false)
                .try_init();
        }
        Err(err) => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
            tracing::warn!(error = %err, "could not create the log folder");
        }
    }
}

fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show window", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("Minecraft Server Manager")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "quit" => request_quit(app),
            _ => {}
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Quitting is never silent while servers are alive: the window comes back and
/// the frontend asks for confirmation before calling `app_quit`.
fn request_quit(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let live = state.live_uuids();
    if live.is_empty() {
        app.exit(0);
        return;
    }

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app_handle.state::<AppState>();
        let mut names = Vec::new();
        for uuid in state.live_uuids() {
            if let Ok(row) = instance::get_by_uuid(&state.db, &uuid).await {
                names.push(row.name);
            }
        }
        names.sort();
        show_main_window(&app_handle);
        events::quit_requested(&app_handle, names);
    });
}
