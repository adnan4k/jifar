// Test helpers outside #[test] fns are not covered by allow-unwrap-in-tests.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::Cursor;

use common::{assert_aligned, chirp, sine};
use jifar_audio::{FRAME_SAMPLES, OggOpusReader, OggOpusWriter, SAMPLE_RATE};

fn encode(samples: &[f32], write_in: usize) -> Vec<u8> {
    let mut w = OggOpusWriter::new(Vec::new(), 1).unwrap();
    for block in samples.chunks(write_in) {
        w.write_samples(block).unwrap();
    }
    w.finish().unwrap()
}

#[test]
fn roundtrip_preserves_length_and_alignment() {
    // Deliberately not a multiple of the 20 ms frame.
    let input = chirp(1.537);
    assert_ne!(input.len() % FRAME_SAMPLES, 0);
    let bytes = encode(&input, 333);

    let mut reader = OggOpusReader::new(Cursor::new(bytes)).unwrap();
    let decoded = reader.read_to_end().unwrap();
    assert!(!reader.is_truncated());
    assert_eq!(
        decoded.len(),
        input.len(),
        "pre-skip and end trimming must be exact"
    );

    assert_aligned(&input, &decoded);
}

#[test]
fn bitrate_is_roughly_32kbps() {
    let secs = 5.0;
    let bytes = encode(&sine(300.0, secs), 4800);
    let kbps = bytes.len() as f32 * 8.0 / secs / 1000.0;
    assert!((20.0..45.0).contains(&kbps), "{kbps:.1} kbps");
}

#[test]
fn empty_stream_is_valid() {
    let bytes = encode(&[], 1);
    let mut reader = OggOpusReader::new(Cursor::new(bytes)).unwrap();
    assert!(reader.read_to_end().unwrap().is_empty());
    assert!(!reader.is_truncated());
}

#[test]
fn truncated_file_decodes_up_to_last_complete_page() {
    // Simulate a crash: 5 s written, never finished, and the tail of the
    // last page torn off.
    let input = chirp(5.0);
    let mut w = OggOpusWriter::new(Vec::new(), 7).unwrap();
    w.write_samples(&input).unwrap();
    let mut bytes = w.get_ref().clone();
    bytes.truncate(bytes.len() - 50);

    let mut reader = OggOpusReader::new(Cursor::new(bytes)).unwrap();
    let decoded = reader.read_to_end().unwrap();
    assert!(reader.is_truncated());
    // Pages hold 1 s; losing at most the last page plus the torn one.
    let lost_secs = (input.len() - decoded.len()) as f32 / SAMPLE_RATE as f32;
    assert!(lost_secs <= 2.0, "lost {lost_secs:.2} s");
    assert_aligned(&input, &decoded);
}
