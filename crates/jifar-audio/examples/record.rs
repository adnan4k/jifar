//! Records the default microphone and the system output (default sink
//! monitor) to `<dir>/mic.opus` and `<dir>/system.opus` through PipeWire or
//! PulseAudio (detected at runtime, or forced with `--backend`).
//! This is the manual device check for M1: long runs, `kill -9` recovery,
//! overruns, drift, levels/silence and pause/resume.
//!
//!     cargo run -p jifar-audio --example record -- <dir> [--duration SECS]
//!         [--mic-only | --system-only] [--backend pipewire|pulse]
//!         [--pause AT:LEN]...
//!
//! Stops on Ctrl-C or after `--duration` (wall time). `--pause 10:5` pauses
//! 10 s after start for 5 s; it can be repeated. Prints a status line every
//! 5 s.
//! `RUST_LOG=debug` shows stream state changes.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use jifar_audio::{
    Backend, CaptureSource, CaptureTarget, RecordingSession, SessionStatus, Track, TrackStatus,
};

const STATUS_EVERY: Duration = Duration::from_secs(5);

struct Args {
    dir: PathBuf,
    duration: Option<Duration>,
    tracks: Vec<Track>,
    backend: Option<Backend>,
    /// (start, length) of each pause, in wall time since start.
    pauses: Vec<(Duration, Duration)>,
}

fn parse_args() -> Result<Args, String> {
    let mut dir = None;
    let mut duration = None;
    let mut tracks = vec![Track::Mic, Track::System];
    let mut backend = None;
    let mut pauses = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--duration" => {
                let secs: u64 = args
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or("--duration needs a number of seconds")?;
                duration = Some(Duration::from_secs(secs));
            }
            "--backend" => {
                backend = Some(match args.next().as_deref() {
                    Some("pipewire") => Backend::PipeWire,
                    Some("pulse") => Backend::PulseAudio,
                    _ => return Err("--backend needs pipewire or pulse".into()),
                });
            }
            "--pause" => {
                let spec = args.next().unwrap_or_default();
                let (at, len) = spec
                    .split_once(':')
                    .and_then(|(a, l)| Some((a.parse().ok()?, l.parse().ok()?)))
                    .ok_or("--pause needs AT:LEN in seconds, e.g. 10:5")?;
                pauses.push((Duration::from_secs(at), Duration::from_secs(len)));
            }
            "--mic-only" => tracks = vec![Track::Mic],
            "--system-only" => tracks = vec![Track::System],
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => dir = Some(PathBuf::from(s)),
        }
    }
    Ok(Args {
        dir: dir.ok_or("missing output directory")?,
        duration,
        tracks,
        backend,
        pauses,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("error: {msg}");
            }
            eprintln!(
                "usage: record <dir> [--duration SECS] [--mic-only | --system-only] \
                 [--backend pipewire|pulse] [--pause AT:LEN]..."
            );
            std::process::exit(2);
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed))?;
    }

    let backend = match args.backend {
        Some(b) => b,
        None => Backend::detect()?,
    };
    let mut sources: Vec<(Track, Box<dyn CaptureSource>)> = Vec::new();
    for &t in &args.tracks {
        let target = match t {
            Track::Mic => CaptureTarget::Microphone,
            Track::System => CaptureTarget::SystemOutput,
        };
        sources.push((t, backend.open(target)?));
    }

    let mut session = RecordingSession::start(&args.dir, sources)?;
    println!(
        "recording to {} via {backend} (Ctrl-C to stop{})",
        args.dir.display(),
        args.duration
            .map(|d| format!(", or after {} s", d.as_secs()))
            .unwrap_or_default()
    );
    for (track, err) in &session.status().failed_to_start {
        println!("  {}: FAILED TO START: {err}", track.as_str());
    }

    let started = Instant::now();
    let mut next_status = started + STATUS_EVERY;
    while !stop.load(Ordering::Relaxed) && args.duration.is_none_or(|d| started.elapsed() < d) {
        let t = started.elapsed();
        let in_pause = args.pauses.iter().any(|&(at, len)| t >= at && t < at + len);
        if in_pause && !session.is_paused() {
            session.pause();
            println!("[paused]");
        } else if !in_pause && session.is_paused() {
            session.resume();
            println!("[resumed]");
        }
        if Instant::now() >= next_status {
            print_status(&session.status());
            next_status += STATUS_EVERY;
        }
        thread::sleep(Duration::from_millis(50));
    }

    print_status(&session.status());
    let summary = session.stop();
    println!(
        "\nstopped after {:.1} s of recording",
        summary.duration_ms as f64 / 1000.0
    );
    for p in &summary.pauses {
        println!(
            "  paused at {:.1} s for {:.1} s",
            p.at_ms as f64 / 1000.0,
            p.len_ms as f64 / 1000.0
        );
    }
    for t in &summary.tracks {
        let offset = t.start_offset_ms.map_or("no audio".to_owned(), |ms| {
            format!("first sample at +{ms} ms")
        });
        match &t.outcome {
            Ok(stats) => println!(
                "  {:<6} {}: {:.1} s, {} overrun samples, drift correction {:+} ms \
                 ({} ms stall silence), {offset}",
                t.track.as_str(),
                t.path.display(),
                stats.duration_ms as f64 / 1000.0,
                stats.overrun_samples,
                stats.drift_correction_ms,
                stats.silence_inserted_ms
            ),
            Err(e) => println!("  {:<6} {}: ERROR {e}", t.track.as_str(), t.path.display()),
        }
    }
    if !summary.is_clean() {
        std::process::exit(1);
    }
    Ok(())
}

fn print_status(status: &SessionStatus) {
    let mut line = format!(
        "[{:>7.1} s{}]",
        status.elapsed_ms as f64 / 1000.0,
        if status.paused { " paused" } else { "" }
    );
    for t in &status.tracks {
        line.push_str(&format!("  {}", track_status(t)));
    }
    if let (Some(d), Some(r)) = (
        status.inter_track_drift_ms(),
        status.inter_track_residual_ms(),
    ) {
        line.push_str(&format!(
            "  | mic-system drift {d:+.1} ms, after correction {r:+.1} ms"
        ));
    }
    println!("{line}");
    for t in &status.tracks {
        if let Some(err) = &t.source_error {
            println!("    {}: source error: {err}", t.track.as_str());
        }
        if !t.healthy {
            println!("    {}: encoder worker stopped", t.track.as_str());
        }
    }
}

fn track_status(t: &TrackStatus) -> String {
    let secs = t.samples_written as f64 / f64::from(jifar_audio::SAMPLE_RATE);
    match t.drift_ms {
        Some(drift) => {
            let level = t.level.map_or("-".to_owned(), |l| {
                format!("{:.0}/{:.0}dBFS", l.rms_dbfs, l.peak_dbfs)
            });
            let silence = if t.silence_warning() {
                format!(" SILENT {}s", t.silent_ms / 1000)
            } else {
                String::new()
            };
            format!(
                "{} {secs:.1}s lvl={level} ovr={} drift={drift:+.1}ms corr={:+.1}ms{silence}",
                t.track.as_str(),
                t.overrun_samples,
                t.drift_correction_ms
            )
        }
        None => format!("{} waiting for audio", t.track.as_str()),
    }
}
