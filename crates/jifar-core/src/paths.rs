//! Data and config locations (SPEC §7.1, FR-PRV-02).
//!
//! ```text
//! $XDG_DATA_HOME/jifar/    (default ~/.local/share/jifar)  jifar.db, meetings/
//! $XDG_CONFIG_HOME/jifar/  (default ~/.config/jifar)       settings.json
//! ```

use std::path::PathBuf;

const APP_DIR: &str = "jifar";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathsError {
    #[error("cannot resolve home directory: HOME is unset or not absolute")]
    NoHome,
}

impl AppPaths {
    /// Resolve from the process environment.
    pub fn from_env() -> Result<Self, PathsError> {
        Self::resolve(|key| std::env::var_os(key).map(PathBuf::from))
    }

    /// Resolve using `lookup` for environment variables (injectable for tests).
    ///
    /// Per the XDG Base Directory spec, relative `XDG_*` values are ignored.
    pub fn resolve(lookup: impl Fn(&str) -> Option<PathBuf>) -> Result<Self, PathsError> {
        let absolute = |key: &str| lookup(key).filter(|p| p.is_absolute());
        let home = || absolute("HOME").ok_or(PathsError::NoHome);

        let data_base = match absolute("XDG_DATA_HOME") {
            Some(p) => p,
            None => home()?.join(".local/share"),
        };
        let config_base = match absolute("XDG_CONFIG_HOME") {
            Some(p) => p,
            None => home()?.join(".config"),
        };

        Ok(Self {
            data_dir: data_base.join(APP_DIR),
            config_dir: config_base.join(APP_DIR),
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("jifar.db")
    }

    pub fn meetings_dir(&self) -> PathBuf {
        self.data_dir.join("meetings")
    }

    pub fn meeting_dir(&self, meeting_id: &str) -> PathBuf {
        self.meetings_dir().join(meeting_id)
    }

    pub fn settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    /// Create the data and config directories if missing.
    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.meetings_dir())?;
        std::fs::create_dir_all(&self.config_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<PathBuf> {
        let map: HashMap<String, PathBuf> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), PathBuf::from(v)))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn defaults_to_home() {
        let p = AppPaths::resolve(env(&[("HOME", "/home/u")])).unwrap();
        assert_eq!(p.data_dir, PathBuf::from("/home/u/.local/share/jifar"));
        assert_eq!(p.config_dir, PathBuf::from("/home/u/.config/jifar"));
        assert_eq!(
            p.db_path(),
            PathBuf::from("/home/u/.local/share/jifar/jifar.db")
        );
        assert_eq!(
            p.settings_path(),
            PathBuf::from("/home/u/.config/jifar/settings.json")
        );
    }

    #[test]
    fn honours_xdg_vars() {
        let p = AppPaths::resolve(env(&[
            ("HOME", "/home/u"),
            ("XDG_DATA_HOME", "/data"),
            ("XDG_CONFIG_HOME", "/cfg"),
        ]))
        .unwrap();
        assert_eq!(p.data_dir, PathBuf::from("/data/jifar"));
        assert_eq!(p.config_dir, PathBuf::from("/cfg/jifar"));
        assert_eq!(
            p.meeting_dir("abc"),
            PathBuf::from("/data/jifar/meetings/abc")
        );
    }

    #[test]
    fn ignores_relative_xdg_vars() {
        let p = AppPaths::resolve(env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "rel")])).unwrap();
        assert_eq!(p.data_dir, PathBuf::from("/home/u/.local/share/jifar"));
    }

    #[test]
    fn errors_without_home() {
        assert_eq!(AppPaths::resolve(env(&[])), Err(PathsError::NoHome));
    }

    #[test]
    fn xdg_vars_suffice_without_home() {
        let p = AppPaths::resolve(env(&[("XDG_DATA_HOME", "/d"), ("XDG_CONFIG_HOME", "/c")]));
        assert!(p.is_ok());
    }

    #[test]
    fn ensure_creates_dirs() {
        let tmp = std::env::temp_dir().join(format!("jifar-paths-{}", std::process::id()));
        let p = AppPaths {
            data_dir: tmp.join("data"),
            config_dir: tmp.join("config"),
        };
        p.ensure().unwrap();
        assert!(p.meetings_dir().is_dir());
        assert!(p.config_dir.is_dir());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
