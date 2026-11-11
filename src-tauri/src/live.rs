//! Live recording feed: every 100 ms, sends `audio://levels`, reports
//! silence changes and track faults, and keeps the tray indicator in sync.
//! Runs on its own thread and only reads `RecordingService::live()`.

use std::time::Duration;

use jifar_core::recording::{LiveStatus, TrackLive};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::AppState;
use crate::error::ErrorCode;
use crate::events::{AppErrorEvent, AudioLevels, AudioSilent, TrackDto};
use crate::tray::Tray;

const TICK: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Seen {
    meeting: Option<String>,
    silent: [bool; 2],
    faulted: [bool; 2],
    tray_second: Option<(u64, bool)>,
}

pub fn spawn(app: AppHandle, tray: Option<Tray>) {
    let _ = std::thread::Builder::new()
        .name("live-feed".into())
        .spawn(move || {
            let mut seen = Seen::default();
            loop {
                std::thread::sleep(TICK);
                let live = app.state::<AppState>().recording.live();
                tick(&app, tray.as_ref(), live, &mut seen);
            }
        });
}

fn tick(app: &AppHandle, tray: Option<&Tray>, live: Option<LiveStatus>, seen: &mut Seen) {
    let Some(live) = live else {
        if seen.meeting.take().is_some()
            && let Some(t) = tray
        {
            t.set_state(None);
        }
        seen.tray_second = None;
        return;
    };
    if seen.meeting.as_deref() != Some(&live.meeting_id) {
        *seen = Seen {
            meeting: Some(live.meeting_id.clone()),
            ..Seen::default()
        };
    }

    let _ = AudioLevels {
        meeting_id: live.meeting_id.clone(),
        mic: live.mic.as_ref().and_then(|t| t.level_dbfs),
        system: live.system.as_ref().and_then(|t| t.level_dbfs),
        elapsed_ms: live.elapsed_ms as f64,
        paused: live.paused,
    }
    .emit(app);

    for (i, (track, state)) in [(TrackDto::Mic, &live.mic), (TrackDto::System, &live.system)]
        .into_iter()
        .enumerate()
    {
        let Some(state) = state else { continue };
        report_track(
            app,
            &live.meeting_id,
            track,
            state,
            &mut seen.silent[i],
            &mut seen.faulted[i],
        );
    }

    // The tooltip shows whole seconds; update it only when that changes.
    let second = (live.elapsed_ms / 1000, live.paused);
    if seen.tray_second != Some(second)
        && let Some(t) = tray
    {
        t.set_state(Some((live.elapsed_ms, live.paused)));
        seen.tray_second = Some(second);
    }
}

fn report_track(
    app: &AppHandle,
    meeting_id: &str,
    track: TrackDto,
    state: &TrackLive,
    was_silent: &mut bool,
    was_faulted: &mut bool,
) {
    if state.silence_warning != *was_silent {
        *was_silent = state.silence_warning;
        let _ = AudioSilent {
            meeting_id: meeting_id.to_owned(),
            track,
            since_ms: state.silence_warning.then_some(state.silent_ms as f64),
        }
        .emit(app);
    }
    if let Some(fault) = &state.fault
        && !*was_faulted
    {
        *was_faulted = true;
        let _ = AppErrorEvent {
            code: ErrorCode::AudioUnavailable,
            message: format!("{track:?} track stopped recording: {fault}"),
            fatal: false,
        }
        .emit(app);
    }
}
