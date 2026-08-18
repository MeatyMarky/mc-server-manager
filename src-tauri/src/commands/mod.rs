//! Thin `#[tauri::command]` wrappers. These deserialize arguments, call a domain
//! function and let `AppError` serialize itself. No business logic lives here.

pub mod app;
pub mod config;
pub mod instances;
pub mod players;
pub mod java;
pub mod mods;
pub mod process;
pub mod setup;
pub mod worlds;
