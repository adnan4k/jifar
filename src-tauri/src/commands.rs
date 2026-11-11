//! Tauri command handlers. Keep these thin: validate, call into the crates,
//! map errors to `AppError`. Every command must be listed in SPEC §6.2.
//!
//! Audio calls can block (talking to the audio server, starting streams), so
//! they run on the blocking pool, never on the async runtime.

use std::sync::Arc;

use jifar_core::recording::{RecordingService, StartOptions};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::AppState;
use crate::dto::{
    DeviceList, ProviderId, ProviderKeyStatus, ProviderKeyTest, RecordingStartArgs,
    RecordingStarted, RecordingStopped, Settings,
};
use crate::error::{AppError, ErrorCode};
use crate::events::MeetingStatusChanged;

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

/// Inputs and outputs available for recording (FR-REC-07).
#[tauri::command]
#[specta::specta]
pub async fn devices_list(state: State<'_, AppState>) -> Result<DeviceList, AppError> {
    let rec = Arc::clone(&state.recording);
    let devices = blocking(move || rec.devices().map_err(AppError::from)).await?;
    Ok(DeviceList::from_devices(devices))
}

/// Starts recording mic + system audio (FR-REC-01). Nothing is transcribed
/// until stop (SPEC D1).
#[tauri::command]
#[specta::specta]
pub async fn recording_start(
    app: AppHandle,
    state: State<'_, AppState>,
    args: RecordingStartArgs,
) -> Result<RecordingStarted, AppError> {
    let rec = Arc::clone(&state.recording);
    let started = blocking(move || {
        rec.start(StartOptions {
            title: args.title,
            language: args.language,
            mic_device: args.mic_device,
            sink_device: args.sink_device,
        })
        .map_err(AppError::from)
    })
    .await?;
    let meeting: crate::dto::Meeting = started.meeting.into();
    notify(&app, &meeting);
    Ok(RecordingStarted {
        meeting,
        notices: started.notices,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn recording_pause(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<(), AppError> {
    Ok(state.recording.pause(&meeting_id)?)
}

#[tauri::command]
#[specta::specta]
pub async fn recording_resume(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<(), AppError> {
    Ok(state.recording.resume(&meeting_id)?)
}

/// Stops and finalizes the recording.
#[tauri::command]
#[specta::specta]
pub async fn recording_stop(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<RecordingStopped, AppError> {
    let rec = Arc::clone(&state.recording);
    let stopped = blocking(move || rec.stop(&meeting_id).map_err(AppError::from)).await?;
    crate::transcribe::after_stop(&app, &stopped);
    let meeting: crate::dto::Meeting = stopped.meeting.into();
    notify(&app, &meeting);
    Ok(RecordingStopped {
        meeting,
        problems: stopped.problems,
        peaks: stopped.peaks,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn settings_get(state: State<'_, AppState>) -> Result<Settings, AppError> {
    Ok(state.settings().clone().into())
}

/// Saves settings (e.g. the chosen devices, FR-REC-07) and returns them.
#[tauri::command]
#[specta::specta]
pub async fn settings_set(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Settings, AppError> {
    let new: jifar_core::settings::Settings = settings.into();
    new.save(&state.paths.settings_path())
        .map_err(|e| AppError::new(ErrorCode::StorageFailed, format!("saving settings: {e}")))?;
    *state.settings() = new.clone();
    Ok(new.into())
}

/// Whether an API key is saved for `provider` (never the key itself).
#[tauri::command]
#[specta::specta]
pub async fn provider_key_status(
    state: State<'_, AppState>,
    provider: ProviderId,
) -> Result<ProviderKeyStatus, AppError> {
    let keys = Arc::clone(&state.keys);
    blocking(move || Ok(keys.status(provider.into()).into())).await
}

/// Saves an API key in the OS keyring (FR-PRV-04).
#[tauri::command]
#[specta::specta]
pub async fn provider_key_set(
    state: State<'_, AppState>,
    provider: ProviderId,
    key: String,
) -> Result<ProviderKeyStatus, AppError> {
    let keys = Arc::clone(&state.keys);
    blocking(move || Ok(keys.set(provider.into(), &key)?.into())).await
}

#[tauri::command]
#[specta::specta]
pub async fn provider_key_delete(
    state: State<'_, AppState>,
    provider: ProviderId,
) -> Result<ProviderKeyStatus, AppError> {
    let keys = Arc::clone(&state.keys);
    blocking(move || Ok(keys.delete(provider.into())?.into())).await
}

/// Checks the saved key with the provider (free; shows the balance).
#[tauri::command]
#[specta::specta]
pub async fn provider_key_test(
    state: State<'_, AppState>,
    provider: ProviderId,
) -> Result<ProviderKeyTest, AppError> {
    Ok(state.keys.test(provider.into()).await.into())
}

/// Re-runs a meeting's unfinished transcription (after fixing the key or
/// balance, or back online). Finished chunks are not sent again.
#[tauri::command]
#[specta::specta]
pub async fn queue_retry(
    transcription: State<'_, jifar_core::transcription::TranscriptionService>,
    meeting_id: String,
) -> Result<(), AppError> {
    use jifar_core::transcription::RetryError;
    transcription.retry(&meeting_id).map_err(|e| match e {
        RetryError::Unknown => AppError::new(ErrorCode::MeetingNotFound, e.to_string()),
        RetryError::Running => AppError::new(ErrorCode::InvalidInput, e.to_string()),
    })
}

pub(crate) fn notify(app: &AppHandle, meeting: &crate::dto::Meeting) {
    let _ = MeetingStatusChanged {
        meeting_id: meeting.id.clone(),
        status: meeting.status,
    }
    .emit(app);
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::new(ErrorCode::Internal, format!("background task failed: {e}")))?
}

/// Starts a recording with the remembered settings (tray "Start").
pub(crate) fn start_from_settings(
    rec: &RecordingService,
    settings: &jifar_core::settings::Settings,
) -> Result<jifar_core::recording::Started, AppError> {
    Ok(rec.start(StartOptions {
        title: None,
        language: settings.language.clone(),
        mic_device: settings.mic_device.clone(),
        sink_device: settings.sink_device.clone(),
    })?)
}
