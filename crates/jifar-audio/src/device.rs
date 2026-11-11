//! Audio devices and device choice (FR-REC-07).
//!
//! A device ID is the audio server's node / source name (for example
//! `alsa_input.usb-Logitech_Headset-00.mono-fallback`). PulseAudio and
//! PipeWire share these names (pipewire-pulse uses `node.name`), so the same
//! ID works on both backends and stays stable across reboots, which makes it
//! safe to remember in settings. For outputs the ID is the sink's name; Jifar
//! records its monitor.
//!
//! A remembered device may be gone (headset unplugged). [`resolve_device`]
//! then falls back to the system default and says so, rather than refusing
//! to record.

use crate::backend::CaptureTarget;

/// What kind of device it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    /// A capture source: microphone, headset mic, line in.
    Input,
    /// A playback sink; Jifar records what it plays (its monitor).
    Output,
}

impl DeviceKind {
    pub fn for_target(target: CaptureTarget) -> Self {
        match target {
            CaptureTarget::Microphone => Self::Input,
            CaptureTarget::SystemOutput => Self::Output,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    /// Stable ID to store and pass back to [`crate::Backend::open`].
    pub id: String,
    /// Human-readable name ("Built-in Audio Analog Stereo").
    pub description: String,
    pub kind: DeviceKind,
    /// The system's current default of its kind.
    pub is_default: bool,
}

/// Which device a track will actually record from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceChoice {
    /// `None` means the system default.
    pub id: Option<String>,
    /// The wanted device was not found; the default is used instead.
    pub fell_back_from: Option<String>,
}

/// Picks the device for `target`. `wanted` is the user's choice (`None` for
/// the system default). A wanted device that isn't in `devices` (or is of the
/// wrong kind) falls back to the default. Pure, so it is unit-tested.
pub fn resolve_device(
    devices: &[AudioDevice],
    target: CaptureTarget,
    wanted: Option<&str>,
) -> DeviceChoice {
    let kind = DeviceKind::for_target(target);
    match wanted {
        None => DeviceChoice {
            id: None,
            fell_back_from: None,
        },
        Some(id) if devices.iter().any(|d| d.kind == kind && d.id == id) => DeviceChoice {
            id: Some(id.to_owned()),
            fell_back_from: None,
        },
        Some(id) => DeviceChoice {
            id: None,
            fell_back_from: Some(id.to_owned()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, kind: DeviceKind, is_default: bool) -> AudioDevice {
        AudioDevice {
            id: id.into(),
            description: id.into(),
            kind,
            is_default,
        }
    }

    fn devices() -> Vec<AudioDevice> {
        vec![
            dev("builtin-mic", DeviceKind::Input, true),
            dev("usb-headset-mic", DeviceKind::Input, false),
            dev("builtin-speakers", DeviceKind::Output, true),
            dev("hdmi", DeviceKind::Output, false),
        ]
    }

    #[test]
    fn no_choice_means_default() {
        let c = resolve_device(&devices(), CaptureTarget::Microphone, None);
        assert_eq!(c.id, None);
        assert_eq!(c.fell_back_from, None);
    }

    #[test]
    fn present_device_is_used() {
        let c = resolve_device(
            &devices(),
            CaptureTarget::Microphone,
            Some("usb-headset-mic"),
        );
        assert_eq!(c.id.as_deref(), Some("usb-headset-mic"));
        let c = resolve_device(&devices(), CaptureTarget::SystemOutput, Some("hdmi"));
        assert_eq!(c.id.as_deref(), Some("hdmi"));
    }

    #[test]
    fn missing_device_falls_back_and_says_so() {
        let c = resolve_device(&devices(), CaptureTarget::Microphone, Some("unplugged"));
        assert_eq!(c.id, None);
        assert_eq!(c.fell_back_from.as_deref(), Some("unplugged"));
    }

    #[test]
    fn device_of_the_wrong_kind_is_not_used() {
        // An output can't be the mic track.
        let c = resolve_device(&devices(), CaptureTarget::Microphone, Some("hdmi"));
        assert_eq!(c.id, None);
        assert_eq!(c.fell_back_from.as_deref(), Some("hdmi"));
    }
}
