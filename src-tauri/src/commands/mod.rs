//! Thin `#[tauri::command]` wrappers. These deserialize arguments, call a domain
//! function and let `AppError` serialize itself. No business logic lives here.

pub mod app;
pub mod instances;
pub mod java;
pub mod setup;
