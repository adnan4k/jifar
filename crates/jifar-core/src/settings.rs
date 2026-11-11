//! User settings in `$XDG_CONFIG_HOME/jifar/settings.json` (SPEC §7.1).
//! No secrets here: API keys live in the keyring (FR-PRV-04).
//!
//! Unknown fields are ignored and missing ones take defaults, so older and
//! newer versions of the app can share the file. Writes are atomic (temp
//! file + rename), so a crash never leaves a half-written file.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Remembered input device ID (FR-REC-07); `None` = system default.
    pub mic_device: Option<String>,
    /// Remembered output device ID; `None` = system default.
    pub sink_device: Option<String>,
    /// Default language for new meetings (ISO 639-1).
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mic_device: None,
            sink_device: None,
            language: "om".to_owned(),
        }
    }
}

impl Settings {
    /// Loads `path`, or defaults if it is missing. A corrupt file is logged
    /// and replaced by defaults rather than blocking startup.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                warn!(path = %path.display(), error = %e, "settings unreadable; using defaults");
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "settings unreadable; using defaults");
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        file.write_all(&json)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            Settings::load(&tmp.path().join("settings.json")),
            Settings::default()
        );
    }

    #[test]
    fn roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg/settings.json");
        let s = Settings {
            mic_device: Some("alsa_input.usb-headset".into()),
            sink_device: None,
            language: "om".into(),
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn corrupt_or_partial_files_are_tolerated() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        // Missing fields default; unknown ones are ignored.
        std::fs::write(&path, br#"{"micDevice":"x","futureField":1}"#).unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.mic_device.as_deref(), Some("x"));
        assert_eq!(s.language, "om");
    }
}
