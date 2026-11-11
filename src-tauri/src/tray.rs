//! System tray (FR-REC-06, NFR-05): start/stop, and an indicator that is
//! always visible and turns into a red record dot whenever audio is being
//! captured. There is no hidden recording.

use std::sync::Arc;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Wry};
use tracing::warn;

use crate::AppState;
use crate::commands::{notify, start_from_settings};
use crate::events::AppErrorEvent;
use tauri_specta::Event;

const IDLE_ICON: &[u8] = include_bytes!("../icons/64x64.png");
const RECORDING_ICON: &[u8] = include_bytes!("../icons/tray-recording.png");

/// Handles the live ticker uses to keep the tray in sync.
#[derive(Clone)]
pub struct Tray {
    icon: TrayIcon<Wry>,
    start: MenuItem<Wry>,
    stop: MenuItem<Wry>,
}

impl Tray {
    pub fn build(app: &AppHandle) -> tauri::Result<Self> {
        let start = MenuItem::with_id(app, "start", "Start recording", true, None::<&str>)?;
        let stop = MenuItem::with_id(app, "stop", "Stop recording", false, None::<&str>)?;
        let show = MenuItem::with_id(app, "show", "Show Jifar", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        let sep = PredefinedMenuItem::separator(app)?;
        let menu = Menu::with_items(app, &[&start, &stop, &sep, &show, &quit])?;
        let icon = TrayIconBuilder::with_id("jifar")
            .icon(Image::from_bytes(IDLE_ICON)?)
            .tooltip("Jifar: not recording")
            .menu(&menu)
            .show_menu_on_left_click(true)
            .on_menu_event(|app, event| on_menu(app, event.id.as_ref()))
            .build(app)?;
        Ok(Self { icon, start, stop })
    }

    /// Shows whether capture is active. Called by the live ticker.
    pub fn set_state(&self, recording: Option<(u64, bool)>) {
        let result = (|| -> tauri::Result<()> {
            match recording {
                Some((elapsed_ms, paused)) => {
                    self.icon
                        .set_icon(Some(Image::from_bytes(RECORDING_ICON)?))?;
                    let clock = clock(elapsed_ms);
                    let tip = if paused {
                        format!("Jifar: recording paused ({clock}); microphone still open")
                    } else {
                        format!("Jifar: recording {clock}")
                    };
                    self.icon.set_tooltip(Some(tip))?;
                    self.start.set_enabled(false)?;
                    self.stop.set_enabled(true)?;
                }
                None => {
                    self.icon.set_icon(Some(Image::from_bytes(IDLE_ICON)?))?;
                    self.icon.set_tooltip(Some("Jifar: not recording"))?;
                    self.start.set_enabled(true)?;
                    self.stop.set_enabled(false)?;
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            warn!(error = %e, "tray update failed");
        }
    }
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "start" | "stop" => {
            let app = app.clone();
            let id = id.to_owned();
            // Audio calls block; keep them off the UI event loop.
            std::thread::spawn(move || {
                let state = app.state::<AppState>();
                let rec = Arc::clone(&state.recording);
                let result = if id == "start" {
                    let settings = state.settings().clone();
                    start_from_settings(&rec, &settings).map(|s| s.meeting)
                } else {
                    match rec.stop_any() {
                        Some(s) => {
                            crate::transcribe::after_stop(&app, &s);
                            Ok(s.meeting)
                        }
                        None => return,
                    }
                };
                match result {
                    Ok(meeting) => notify(&app, &meeting.into()),
                    Err(e) => {
                        let _ = AppErrorEvent {
                            code: e.code,
                            message: e.message,
                            fatal: false,
                        }
                        .emit(&app);
                    }
                }
            });
        }
        "show" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}
