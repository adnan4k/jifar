//! Rust → UI events (SPEC §6.2), typed through tauri-specta.

use serde::Serialize;
use tauri_specta::Event;

use crate::dto::MeetingStatusDto;
use crate::error::ErrorCode;

/// ~10 Hz while recording. Levels are RMS dBFS of the last 100 ms (floored
/// at -100), `null` before a track has audio or if it isn't recorded.
#[derive(Debug, Clone, Serialize, specta::Type, Event)]
#[tauri_specta(event_name = "audio://levels")]
#[serde(rename_all = "camelCase")]
pub struct AudioLevels {
    pub meeting_id: String,
    pub mic: Option<f32>,
    pub system: Option<f32>,
    /// Recording time, pauses excluded.
    pub elapsed_ms: f64,
    pub paused: bool,
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TrackDto {
    Mic,
    System,
}

/// A track crossed the 30 s silence limit (`sinceMs` set) or has sound
/// again (`sinceMs` null). FR-REC-08.
#[derive(Debug, Clone, Serialize, specta::Type, Event)]
#[tauri_specta(event_name = "audio://silent")]
#[serde(rename_all = "camelCase")]
pub struct AudioSilent {
    pub meeting_id: String,
    pub track: TrackDto,
    pub since_ms: Option<f64>,
}

/// A meeting changed status, including recordings started or stopped
/// from the tray.
#[derive(Debug, Clone, Serialize, specta::Type, Event)]
#[tauri_specta(event_name = "meeting://status")]
#[serde(rename_all = "camelCase")]
pub struct MeetingStatusChanged {
    pub meeting_id: String,
    pub status: MeetingStatusDto,
}

/// Something the user should know about that no command returned.
#[derive(Debug, Clone, Serialize, specta::Type, Event)]
#[tauri_specta(event_name = "app://error")]
pub struct AppErrorEvent {
    pub code: ErrorCode,
    pub message: String,
    pub fatal: bool,
}
