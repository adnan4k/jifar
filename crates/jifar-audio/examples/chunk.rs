//! Runs the offline chunker on one track archive and lists the chunks.
//!
//!     ORT_DYLIB_PATH=$(scripts/fetch-onnxruntime.sh) \
//!     cargo run -p jifar-audio --example chunk -- <track.opus> [out-dir] \
//!         [--offset MS] [--energy]
//!
//! The track (`mic`/`system`) comes from the file name, `mic` by default.
//! `out-dir` defaults to `chunks/` next to the archive. `--energy` uses the
//! level-threshold VAD instead of Silero (no ONNX Runtime needed).

use std::path::PathBuf;
use std::time::Instant;

use jifar_audio::{ChunkerConfig, EnergyVad, SileroVad, Track, Vad, chunk_track};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut positional = Vec::new();
    let mut offset_ms = 0_i64;
    let mut energy = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--offset" => offset_ms = args.next().ok_or("--offset needs a value")?.parse()?,
            "--energy" => energy = true,
            _ => positional.push(PathBuf::from(arg)),
        }
    }
    let [input, rest @ ..] = positional.as_slice() else {
        eprintln!("usage: chunk <track.opus> [out-dir] [--offset MS] [--energy]");
        std::process::exit(2);
    };
    let out_dir = match rest.first() {
        Some(dir) => dir.clone(),
        None => input
            .parent()
            .map_or_else(|| PathBuf::from("chunks"), |p| p.join("chunks")),
    };
    let track = match input.file_stem().and_then(|s| s.to_str()) {
        Some("system") => Track::System,
        _ => Track::Mic,
    };
    let mut vad: Box<dyn Vad> = if energy {
        Box::new(EnergyVad::default())
    } else {
        Box::new(SileroVad::new()?)
    };

    let started = Instant::now();
    let report = chunk_track(
        input,
        &out_dir,
        track,
        offset_ms,
        vad.as_mut(),
        &ChunkerConfig::default(),
    )?;
    let elapsed = started.elapsed();

    for c in &report.chunks {
        println!(
            "{:>4}  {:>9.2} s → {:>9.2} s  ({:>5.2} s, speech {:>5.2} s)  {}",
            c.index,
            c.start_ms as f64 / 1000.0,
            c.end_ms as f64 / 1000.0,
            (c.end_ms - c.start_ms) as f64 / 1000.0,
            f64::from(c.speech_ms) / 1000.0,
            c.path.display()
        );
    }
    let duration = report.duration_ms as f64 / 1000.0;
    println!(
        "{}: {:.1} s of audio, {:.1} s speech, {} chunks, {} skipped{} — chunked in {:.2} s ({:.0}× real time)",
        track.as_str(),
        duration,
        f64::from(report.speech_ms) / 1000.0,
        report.chunks.len(),
        report.skipped,
        if report.truncated {
            ", archive truncated"
        } else {
            ""
        },
        elapsed.as_secs_f64(),
        duration / elapsed.as_secs_f64().max(1e-9),
    );
    Ok(())
}
