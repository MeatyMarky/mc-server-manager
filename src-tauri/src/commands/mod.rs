//! Thin `#[tauri::command]` wrappers. These deserialize arguments, call a domain
//! function and let `AppError` serialize itself. No business logic lives here.

pub mod app;
pub mod backups;
pub mod config;
pub mod diag;
pub mod instances;
pub mod packs;
pub mod players;
pub mod runtimes;
pub mod java;
pub mod mods;
pub mod net;
pub mod process;
pub mod setup;
pub mod worlds;
