//! Tauri command handlers. Keep these thin: validate, call into the crates,
//! map errors to `AppError`. Every command must be listed in SPEC §6.2.

use serde::Serialize;
use tauri::State;

use crate::{AppState, error::AppError};

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
}

/// Health check used by the UI to detect the native core (M0).
#[tauri::command]
#[specta::specta]
pub async fn ping(state: State<'_, AppState>) -> Result<AppInfo, AppError> {
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        data_dir: state.paths.data_dir.display().to_string(),
    })
}
