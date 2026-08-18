pub mod commands;
pub mod db;
pub mod error;
pub mod events;
pub mod instance;
pub mod metrics;
pub mod paths;
pub mod state;

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

            let state = AppState::new(pool.clone(), data_dir);

            // Orphan recovery has to happen before the UI paints, so the sidebar
            // shows adopted servers instead of claiming everything is stopped.
            match tauri::async_runtime::block_on(instance::reconcile::reconcile_all(&state)) {
                Ok(0) => {}
                Ok(n) => tracing::info!(count = n, "adopted orphaned server processes"),
                Err(err) => tracing::error!(error = %err, "orphan reconciliation failed"),
            }

            tauri::async_runtime::spawn(metrics::retention::pruner_loop(pool));
            app.manage(state);
            setup_tray(app.handle())?;
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
