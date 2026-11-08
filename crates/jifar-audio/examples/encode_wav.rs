//! Runs a 48 kHz WAV through the real capture pipeline (FakeSource → ring →
//! worker → Ogg/Opus) and prints track stats.
//!
//!     cargo run -p jifar-audio --example encode_wav -- in.wav out.opus [--realtime]

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use jifar_audio::{FakeSource, Pacing, TrackRecorder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, rest @ ..] = args.as_slice() else {
        eprintln!("usage: encode_wav <in.wav> <out.opus> [--realtime]");
        std::process::exit(2);
    };
    let pacing = if rest.iter().any(|a| a == "--realtime") {
        Pacing::Realtime
    } else {
        Pacing::Lossless
    };

    let source = FakeSource::from_wav(&PathBuf::from(input), pacing)?;
    let recorder = TrackRecorder::start(Box::new(source), &PathBuf::from(output))?;
    while !recorder.is_source_finished() {
        thread::sleep(Duration::from_millis(20));
    }
    let stats = recorder.stop()?;
    println!(
        "{output}: {} samples, {:.2} s, {} overrun samples",
        stats.samples,
        stats.duration_ms as f64 / 1000.0,
        stats.overrun_samples
    );
    Ok(())
}
