//! Capture backend selection (SPEC §6.3): native PipeWire where the audio
//! server is PipeWire, PulseAudio otherwise (stock Ubuntu 22.04).
//!
//! Detection asks the PulseAudio-protocol server for its name. PipeWire's
//! `pipewire-pulse` reports `"PulseAudio (on PipeWire x.y.z)"`; a real
//! PulseAudio daemon reports `"pulseaudio"`. A running PipeWire daemon alone
//! is not enough: 22.04 runs one for screen sharing while PulseAudio still
//! owns the audio devices.
//!
//! `JIFAR_AUDIO_BACKEND=pipewire|pulse` overrides detection.

use std::fmt;

use tracing::info;

use crate::device::AudioDevice;
use crate::source::CaptureSource;
use crate::{AudioError, Result};

/// What a capture source records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget {
    /// The default input device (FR-REC-02).
    Microphone,
    /// The monitor of the default output device: everything the computer
    /// plays, i.e. the other meeting participants (FR-REC-03).
    SystemOutput,
}

impl CaptureTarget {
    /// Stream / node name shown in the audio server's tools.
    pub fn node_name(self) -> &'static str {
        match self {
            Self::Microphone => "jifar-mic",
            Self::SystemOutput => "jifar-system",
        }
    }
}

/// An audio server API Jifar can capture through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    PipeWire,
    PulseAudio,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PipeWire => "pipewire",
            Self::PulseAudio => "pulse",
        }
    }

    /// Backends compiled into this build, preferred first.
    pub fn compiled() -> &'static [Backend] {
        &[
            #[cfg(feature = "pipewire")]
            Backend::PipeWire,
            #[cfg(feature = "pulse")]
            Backend::PulseAudio,
        ]
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pipewire" | "pw" => Some(Self::PipeWire),
            "pulse" | "pulseaudio" => Some(Self::PulseAudio),
            _ => None,
        }
    }

    /// Picks the backend for this machine. See the module docs.
    pub fn detect() -> Result<Self> {
        let compiled = Self::compiled();
        if let Ok(value) = std::env::var("JIFAR_AUDIO_BACKEND") {
            let backend = Self::parse(&value).ok_or_else(|| {
                AudioError::Source(format!("JIFAR_AUDIO_BACKEND={value}: unknown backend"))
            })?;
            if !compiled.contains(&backend) {
                return Err(AudioError::Source(format!(
                    "JIFAR_AUDIO_BACKEND={value}: not compiled into this build"
                )));
            }
            info!(
                backend = backend.as_str(),
                "audio backend set by environment"
            );
            return Ok(backend);
        }
        let backend = choose(compiled, probe_server_name().as_deref())?;
        info!(backend = backend.as_str(), "audio backend detected");
        Ok(backend)
    }

    /// Creates a capture source for `target` on this backend. `device` is a
    /// device ID from [`Self::list_devices`] (resolve a remembered one with
    /// [`crate::resolve_device`] first); `None` records the system default.
    pub fn open(
        self,
        target: CaptureTarget,
        device: Option<&str>,
    ) -> Result<Box<dyn CaptureSource>> {
        let device = device.map(str::to_owned);
        match self {
            #[cfg(feature = "pipewire")]
            Self::PipeWire => Ok(Box::new(crate::PipeWireSource::with_device(target, device))),
            #[cfg(feature = "pulse")]
            Self::PulseAudio => Ok(Box::new(crate::PulseSource::with_device(target, device))),
            #[allow(unreachable_patterns)]
            other => {
                let _ = (target, device);
                Err(AudioError::Source(format!(
                    "{} backend not compiled into this build",
                    other.as_str()
                )))
            }
        }
    }
}

impl Backend {
    /// Inputs and outputs available for recording, with the current
    /// defaults marked (FR-REC-07). Listed through the PulseAudio protocol,
    /// which both real PulseAudio and pipewire-pulse speak, and whose names
    /// are valid device IDs for either backend.
    pub fn list_devices(self) -> Result<Vec<AudioDevice>> {
        #[cfg(feature = "pulse")]
        {
            let _ = self;
            crate::pulse_source::list_devices()
        }
        #[cfg(not(feature = "pulse"))]
        {
            Err(AudioError::Source(format!(
                "device listing needs the `pulse` feature ({} backend)",
                self.as_str()
            )))
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Decision table behind [`Backend::detect`]. `server_name` is what the
/// PulseAudio-protocol server reported, `None` if none answered.
fn choose(compiled: &[Backend], server_name: Option<&str>) -> Result<Backend> {
    let has = |b| compiled.contains(&b);
    let on_pipewire = server_name.is_some_and(|n| n.contains("PipeWire"));
    let preferred = match server_name {
        // pipewire-pulse: go native; Pulse API through it also works.
        Some(_) if on_pipewire => [Backend::PipeWire, Backend::PulseAudio],
        // Real PulseAudio owns the devices; PipeWire would see none.
        Some(_) => [Backend::PulseAudio, Backend::PulseAudio],
        // No Pulse server: PipeWire without pipewire-pulse, or nothing.
        None => [Backend::PipeWire, Backend::PipeWire],
    };
    preferred.into_iter().find(|&b| has(b)).ok_or_else(|| {
        AudioError::Source(match server_name {
            Some(name) => format!("audio server is {name:?}, but no matching backend is compiled"),
            None => "no audio server found (neither PipeWire nor PulseAudio)".to_owned(),
        })
    })
}

#[cfg(feature = "pulse")]
fn probe_server_name() -> Option<String> {
    crate::pulse_source::server_name()
}

#[cfg(not(feature = "pulse"))]
fn probe_server_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: &[Backend] = &[Backend::PipeWire, Backend::PulseAudio];

    #[test]
    fn pipewire_pulse_prefers_native_pipewire() {
        let name = Some("PulseAudio (on PipeWire 1.0.5)");
        assert_eq!(choose(BOTH, name).unwrap(), Backend::PipeWire);
        assert_eq!(
            choose(&[Backend::PulseAudio], name).unwrap(),
            Backend::PulseAudio
        );
    }

    #[test]
    fn real_pulseaudio_uses_pulse_even_if_pipewire_compiled() {
        assert_eq!(
            choose(BOTH, Some("pulseaudio")).unwrap(),
            Backend::PulseAudio
        );
        assert!(choose(&[Backend::PipeWire], Some("pulseaudio")).is_err());
    }

    #[test]
    fn no_pulse_server_falls_back_to_pipewire() {
        assert_eq!(choose(BOTH, None).unwrap(), Backend::PipeWire);
        assert!(choose(&[Backend::PulseAudio], None).is_err());
        assert!(choose(&[], None).is_err());
    }

    #[test]
    fn parses_override_values() {
        assert_eq!(Backend::parse("PipeWire"), Some(Backend::PipeWire));
        assert_eq!(Backend::parse(" pulse "), Some(Backend::PulseAudio));
        assert_eq!(Backend::parse("pulseaudio"), Some(Backend::PulseAudio));
        assert_eq!(Backend::parse("alsa"), None);
    }
}
