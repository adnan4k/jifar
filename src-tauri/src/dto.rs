//! IPC data types (SPEC §6.2). Thin, serializable mirrors of `jifar-core`
//! types, kept here so the core stays free of Tauri/specta.

use jifar_core::recording::{Device, DeviceKind, MeetingInfo, MeetingStatus};
use jifar_core::settings::Settings as CoreSettings;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MeetingStatusDto {
    Recording,
    Processing,
    Ready,
    NeedsAttention,
}

impl From<MeetingStatus> for MeetingStatusDto {
    fn from(s: MeetingStatus) -> Self {
        match s {
            MeetingStatus::Recording => Self::Recording,
            MeetingStatus::Processing => Self::Processing,
            MeetingStatus::Ready => Self::Ready,
            MeetingStatus::NeedsAttention => Self::NeedsAttention,
        }
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub language: String,
    pub status: MeetingStatusDto,
    /// Epoch ms.
    pub started_at: f64,
    /// Recording time, pauses excluded.
    pub duration_ms: f64,
    /// Where the audio files are.
    pub dir: String,
}

impl From<MeetingInfo> for Meeting {
    fn from(m: MeetingInfo) -> Self {
        Self {
            id: m.id,
            title: m.title,
            language: m.language,
            status: m.status.into(),
            // JS numbers: f64 is exact for these ranges.
            started_at: m.started_at_ms as f64,
            duration_ms: m.duration_ms as f64,
            dir: m.dir.display().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceDto {
    /// Stable ID to remember and pass back as `micDevice` / `sinkDevice`.
    pub id: String,
    pub label: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct DeviceList {
    pub inputs: Vec<AudioDeviceDto>,
    pub outputs: Vec<AudioDeviceDto>,
}

impl DeviceList {
    pub fn from_devices(devices: Vec<Device>) -> Self {
        let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
        for d in devices {
            let dto = AudioDeviceDto {
                id: d.id,
                label: d.description,
                is_default: d.is_default,
            };
            match d.kind {
                DeviceKind::Input => inputs.push(dto),
                DeviceKind::Output => outputs.push(dto),
            }
        }
        Self { inputs, outputs }
    }
}

#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStartArgs {
    pub title: Option<String>,
    pub language: String,
    pub mic_device: Option<String>,
    pub sink_device: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct RecordingStarted {
    pub meeting: Meeting,
    /// E.g. "the selected microphone is not available; using the default".
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct RecordingStopped {
    pub meeting: Meeting,
    /// Why the meeting needs attention; empty when clean.
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub mic_device: Option<String>,
    pub sink_device: Option<String>,
    pub language: String,
}

impl From<CoreSettings> for Settings {
    fn from(s: CoreSettings) -> Self {
        Self {
            mic_device: s.mic_device,
            sink_device: s.sink_device,
            language: s.language,
        }
    }
}

impl From<Settings> for CoreSettings {
    fn from(s: Settings) -> Self {
        Self {
            mic_device: s.mic_device.filter(|d| !d.is_empty()),
            sink_device: s.sink_device.filter(|d| !d.is_empty()),
            language: s.language,
        }
    }
}
