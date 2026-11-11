//! Addis Scribe from the command line: store the API key, transcribe a WAV,
//! or transcribe a whole meeting recorded by Jifar.
//!
//!     cargo run -p jifar-providers --example scribe -- set-key      # paste key, Enter
//!     cargo run -p jifar-providers --example scribe -- key-status
//!     cargo run -p jifar-providers --example scribe -- clear-key
//!     cargo run -p jifar-providers --example scribe -- transcribe <file.wav> [--lang om]
//!     ORT_DYLIB_PATH=$(scripts/fetch-onnxruntime.sh) \
//!     cargo run -p jifar-providers --example scribe -- meeting <meeting-dir> [--lang om]
//!
//! The key is stored in the system keyring (service `jifar`, account
//! `addis-scribe`), the same place the app reads it from. `set-key` echoes
//! what you type; to keep the key off the screen, pipe it in instead:
//! `printf %s "$KEY" | cargo run -p jifar-providers --example scribe -- set-key`.
//!
//! `meeting` chunks `mic.opus` and `system.opus` (Silero VAD, like the app
//! will) and sends every chunk. Request IDs are derived from the meeting ID
//! and chunk, so running it again replays results at no extra cost.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use jifar_audio::{ChunkerConfig, SileroVad, Track, chunk_track};
use jifar_providers::addis::PROVIDER_ID;
use jifar_providers::{
    AddisScribe, ApiKey, ApiKeyStore, AsrProvider, AudioChunk, KeyringStore, LangCode, RawSegment,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let lang = args
        .iter()
        .position(|a| a == "--lang")
        .and_then(|i| args.get(i + 1))
        .map_or("om", String::as_str);
    let lang = LangCode::new(lang).ok_or("--lang needs a code like om")?;
    let store = KeyringStore;

    match args.first().map(String::as_str) {
        Some("set-key") => {
            print!("Addis AI API key (sk_…): ");
            std::io::stdout().flush()?;
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            let key = ApiKey::new(&line).ok_or("empty key")?;
            store.set(PROVIDER_ID, &key)?;
            println!("saved in the system keyring as jifar/{PROVIDER_ID} ({key:?})");
        }
        Some("clear-key") => {
            store.delete(PROVIDER_ID)?;
            println!("removed jifar/{PROVIDER_ID} from the keyring");
        }
        Some("key-status") => match store.get(PROVIDER_ID)? {
            Some(k) => println!("key present: {k:?}"),
            None => println!("no key stored (run `set-key`)"),
        },
        Some("transcribe") => {
            let file = args.get(1).ok_or("transcribe <file.wav>")?;
            let scribe = AddisScribe::new(Box::new(store))?;
            let chunk = AudioChunk {
                request_id: format!("jifar-cli-{}", now_ms()),
                path: PathBuf::from(file),
                start_ms: 0,
                end_ms: wav_ms(Path::new(file)).unwrap_or(i64::MAX / 2),
            };
            print_segments(&scribe.transcribe(&chunk, &lang).await?, None);
        }
        Some("meeting") => {
            let dir = PathBuf::from(args.get(1).ok_or("meeting <meeting-dir>")?);
            transcribe_meeting(&dir, &lang, AddisScribe::new(Box::new(store))?).await?;
        }
        _ => {
            eprintln!(
                "usage: scribe set-key | clear-key | key-status | transcribe <file.wav> [--lang om] \
                 | meeting <meeting-dir> [--lang om]"
            );
            std::process::exit(2);
        }
    }
    Ok(())
}

async fn transcribe_meeting(
    dir: &Path,
    lang: &LangCode,
    scribe: AddisScribe,
) -> Result<(), Box<dyn std::error::Error>> {
    let meeting_id = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("bad meeting dir")?
        .to_owned();
    let mut vad = SileroVad::new()?;
    let mut all: Vec<(Track, RawSegment)> = Vec::new();
    let mut failures = 0;
    for track in [Track::Mic, Track::System] {
        let opus = dir.join(track.file_name());
        if !opus.is_file() {
            continue;
        }
        // Start offsets aren't stored yet (no database); 0 is close enough
        // for a CLI check.
        let report = chunk_track(
            &opus,
            &dir.join("chunks"),
            track,
            0,
            &mut vad,
            &ChunkerConfig::default(),
        )?;
        eprintln!(
            "{}: {} chunks with speech ({:.1} s of speech)",
            track.as_str(),
            report.chunks.len(),
            f64::from(report.speech_ms) / 1000.0
        );
        for c in &report.chunks {
            let chunk = AudioChunk {
                request_id: format!("{meeting_id}-{}-{:04}", track.as_str(), c.index),
                path: c.path.clone(),
                start_ms: c.start_ms,
                end_ms: c.end_ms,
            };
            match scribe.transcribe(&chunk, lang).await {
                Ok(segs) => all.extend(segs.into_iter().map(|s| (track, s))),
                Err(e) => {
                    failures += 1;
                    eprintln!("  chunk {} failed: {e}", c.index);
                }
            }
        }
    }
    all.sort_by_key(|(t, s)| (s.start_ms, *t == Track::System));
    for (track, seg) in &all {
        print_segments(std::slice::from_ref(seg), Some(*track));
    }
    let chars: usize = all.iter().map(|(_, s)| s.text.encode_utf16().count()).sum();
    eprintln!(
        "\n{} segments, {chars} characters (≈ {:.2} ETB at 3.5 ETB / 1,000), {failures} failed chunks",
        all.len(),
        chars as f64 * 3.5 / 1000.0
    );
    Ok(())
}

fn print_segments(segs: &[RawSegment], track: Option<Track>) {
    for s in segs {
        let who = match track {
            Some(Track::Mic) => "Me     ",
            Some(Track::System) => "Others ",
            None => "",
        };
        println!("[{}] {who}{}", clock(s.start_ms), s.text);
    }
}

fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Duration of a PCM WAV from its header, if it is one.
fn wav_ms(path: &Path) -> Option<i64> {
    let bytes = std::fs::read(path).ok()?;
    let rate = u32::from_le_bytes(bytes.get(24..28)?.try_into().ok()?);
    let bytes_per_sec = u32::from_le_bytes(bytes.get(28..32)?.try_into().ok()?);
    (rate > 0 && bytes_per_sec > 0)
        .then(|| (bytes.len().saturating_sub(44) as i64) * 1000 / i64::from(bytes_per_sec))
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}
