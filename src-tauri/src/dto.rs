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
    /// Waveform overview of the mixed audio, 0..=1 per bar.
    pub peaks: Vec<f32>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    /// Addis AI (transcription and notes share one key).
    AddisAi,
}

impl From<ProviderId> for jifar_core::provider_keys::Provider {
    fn from(p: ProviderId) -> Self {
        match p {
            ProviderId::AddisAi => Self::AddisAi,
        }
    }
}

/// Whether a key is saved. The key itself never crosses IPC.
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderKeyStatus {
    pub saved: bool,
    /// Last 4 characters, e.g. "3456".
    pub hint: Option<String>,
    /// Set when the system keyring can't be used.
    pub keyring_error: Option<String>,
}

impl From<jifar_core::provider_keys::KeyStatus> for ProviderKeyStatus {
    fn from(s: jifar_core::provider_keys::KeyStatus) -> Self {
        Self {
            saved: s.saved,
            hint: s.hint,
            keyring_error: s.keyring_error,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum KeyTestStatus {
    Ok,
    NoKey,
    InvalidKey,
    NoCredits,
    Unreachable,
    Failed,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ProviderKeyTest {
    pub status: KeyTestStatus,
    pub message: Option<String>,
    pub balance: Option<f64>,
    pub currency: Option<String>,
}

impl From<jifar_core::provider_keys::TestOutcome> for ProviderKeyTest {
    fn from(o: jifar_core::provider_keys::TestOutcome) -> Self {
        use jifar_core::provider_keys::TestOutcome as T;
        let (status, message, balance, currency) = match o {
            T::Ok { balance, currency } => (KeyTestStatus::Ok, None, balance, currency),
            T::NoKey => (KeyTestStatus::NoKey, None, None, None),
            T::InvalidKey(m) => (KeyTestStatus::InvalidKey, Some(m), None, None),
            T::NoCredits(m) => (KeyTestStatus::NoCredits, Some(m), None, None),
            T::Unreachable(m) => (KeyTestStatus::Unreachable, Some(m), None, None),
            T::Failed(m) => (KeyTestStatus::Failed, Some(m), None, None),
        };
        Self {
            status,
            message,
            balance,
            currency,
        }
    }
}

/// A saved meeting with its transcript (FR-HIS-03).
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MeetingDetailDto {
    pub meeting: Meeting,
    pub segments: Vec<crate::events::SegmentDto>,
    /// Waveform overview for the player (0..=1).
    pub peaks: Vec<f32>,
    /// `mixed.opus` exists and can be played.
    pub has_audio: bool,
}

impl From<jifar_core::library::MeetingDetail> for MeetingDetailDto {
    fn from(d: jifar_core::library::MeetingDetail) -> Self {
        use crate::events::{SegmentDto, TrackDto};
        Self {
            segments: d
                .segments
                .into_iter()
                .map(|s| SegmentDto {
                    id: s.id,
                    track: if s.track == "system" {
                        TrackDto::System
                    } else {
                        TrackDto::Mic
                    },
                    start_ms: s.start_ms as f64,
                    end_ms: s.end_ms as f64,
                    text: s.text,
                })
                .collect(),
            peaks: d.peaks,
            has_audio: d.has_audio,
            meeting: d.meeting.into(),
        }
    }
}
