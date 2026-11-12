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
    /// Latest notes version, if generated.
    pub notes: Option<NotesDto>,
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
            notes: d.notes.map(Into::into),
            meeting: d.meeting.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DecisionDto {
    pub text: String,
    /// Transcript segments it came from (FR-AI-05).
    pub segment_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ActionItemDto {
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub segment_ids: Vec<String>,
}

/// Meeting notes (FR-AI-01..05), one saved version.
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NotesDto {
    pub version: u32,
    /// Language the notes are written in.
    pub language: String,
    pub provider: Option<String>,
    pub summary: String,
    pub key_points: Vec<String>,
    pub decisions: Vec<DecisionDto>,
    pub action_items: Vec<ActionItemDto>,
}

impl From<jifar_core::notes::SavedNotes> for NotesDto {
    fn from(s: jifar_core::notes::SavedNotes) -> Self {
        let n = s.notes;
        Self {
            version: u32::try_from(s.version).unwrap_or(u32::MAX),
            language: s.language,
            provider: s.provider,
            summary: n.summary,
            key_points: n.key_points,
            decisions: n
                .decisions
                .into_iter()
                .map(|d| DecisionDto {
                    text: d.text,
                    segment_ids: d.segment_ids,
                })
                .collect(),
            action_items: n
                .action_items
                .into_iter()
                .map(|a| ActionItemDto {
                    text: a.text,
                    owner: a.owner,
                    due: a.due,
                    segment_ids: a.segment_ids,
                })
                .collect(),
        }
    }
}

// ---- Tasks (FR-AI-08) -------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TaskSourceKind {
    ActionItem,
    Decision,
    KeyPoint,
    /// Text selected in the summary.
    Summary,
    /// Text selected in the transcript.
    Transcript,
    /// Typed by hand.
    Manual,
}

impl From<TaskSourceKind> for jifar_core::tasks::SourceKind {
    fn from(k: TaskSourceKind) -> Self {
        match k {
            TaskSourceKind::ActionItem => Self::ActionItem,
            TaskSourceKind::Decision => Self::Decision,
            TaskSourceKind::KeyPoint => Self::KeyPoint,
            TaskSourceKind::Summary => Self::Summary,
            TaskSourceKind::Transcript => Self::Transcript,
            TaskSourceKind::Manual => Self::Manual,
        }
    }
}

impl From<jifar_core::tasks::SourceKind> for TaskSourceKind {
    fn from(k: jifar_core::tasks::SourceKind) -> Self {
        use jifar_core::tasks::SourceKind as K;
        match k {
            K::ActionItem => Self::ActionItem,
            K::Decision => Self::Decision,
            K::KeyPoint => Self::KeyPoint,
            K::Summary => Self::Summary,
            K::Transcript => Self::Transcript,
            K::Manual => Self::Manual,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TaskFilterDto {
    Open,
    Done,
    All,
}

impl From<TaskFilterDto> for jifar_store::TaskFilter {
    fn from(f: TaskFilterDto) -> Self {
        match f {
            TaskFilterDto::Open => Self::Open,
            TaskFilterDto::Done => Self::Done,
            TaskFilterDto::All => Self::All,
        }
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TaskDto {
    pub id: String,
    /// `None` for a task typed by hand without a meeting.
    pub meeting_id: Option<String>,
    pub meeting_title: Option<String>,
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub done: bool,
    /// Epoch ms.
    pub done_at: Option<f64>,
    pub source_kind: TaskSourceKind,
    pub source_version: Option<u32>,
    pub segment_ids: Vec<String>,
    /// Where "jump to source" seeks to, if the segment still exists.
    pub source_start_ms: Option<f64>,
    /// Source text, trimmed and whitespace-collapsed: how notes items are
    /// recognised as already added.
    pub match_key: String,
    /// Epoch ms.
    pub created_at: f64,
}

impl From<jifar_core::tasks::Task> for TaskDto {
    fn from(t: jifar_core::tasks::Task) -> Self {
        Self {
            id: t.id,
            meeting_id: t.meeting_id,
            meeting_title: t.meeting_title,
            text: t.text,
            owner: t.owner,
            due: t.due,
            done: t.done,
            done_at: t.done_at_ms.map(|v| v as f64),
            source_kind: t.source_kind.into(),
            source_version: t.source_version.and_then(|v| u32::try_from(v).ok()),
            segment_ids: t.segment_ids,
            source_start_ms: t.source_start_ms.map(|v| v as f64),
            match_key: t.match_key,
            created_at: t.created_at_ms as f64,
        }
    }
}

#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NewTaskDto {
    pub meeting_id: Option<String>,
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub source_kind: TaskSourceKind,
    pub source_version: Option<u32>,
    pub segment_ids: Vec<String>,
}

impl From<NewTaskDto> for jifar_core::tasks::NewTask {
    fn from(t: NewTaskDto) -> Self {
        Self {
            meeting_id: t.meeting_id,
            text: t.text,
            owner: t.owner,
            due: t.due,
            source_kind: t.source_kind.into(),
            source_version: t.source_version.map(i64::from),
            segment_ids: t.segment_ids,
        }
    }
}

/// Fields to change. Omitted fields stay; an empty `owner`/`due` clears it.
#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TaskPatchDto {
    pub text: Option<String>,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub done: Option<bool>,
}

impl From<TaskPatchDto> for jifar_core::tasks::TaskChanges {
    fn from(p: TaskPatchDto) -> Self {
        Self {
            text: p.text,
            owner: p.owner,
            due: p.due,
            done: p.done,
        }
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct TaskCounts {
    pub open: u32,
    pub done: u32,
}
