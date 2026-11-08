//! Incremental Ogg/Opus writer (FR-REC-05, SPEC §6.3).
//!
//! Each Ogg page holds ~1 s of audio and is flushed to the underlying writer as
//! soon as it is complete. With [`DurableFile`] the file is additionally
//! `fdatasync`ed every few seconds, so a crash or power loss loses at most ~5 s.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use opus::{Application, Bitrate, Channels, Encoder};

use crate::{FRAME_SAMPLES, Result, SAMPLE_RATE};

/// Target bitrate for archive tracks (~32 kbps, VOIP mode).
pub const BITRATE_BPS: i32 = 32_000;
/// Opus packets (20 ms each) per Ogg page: 50 × 20 ms = 1 s.
const PACKETS_PER_PAGE: u32 = 50;
/// Largest packet we accept from the encoder (RFC 6716 recommends 4000 bytes).
const MAX_PACKET: usize = 4000;
const VENDOR: &str = concat!("jifar ", env!("CARGO_PKG_VERSION"));

/// Writes mono 48 kHz f32 samples as an Ogg/Opus stream, page by page.
pub struct OggOpusWriter<W: Write> {
    packets: PacketWriter<'static, W>,
    encoder: Encoder,
    serial: u32,
    pre_skip: u16,
    frame: Vec<f32>,
    out: Vec<u8>,
    /// Real (non-padding) samples accepted.
    samples_in: u64,
    /// Samples fed to the encoder, including padding. Equals the granule
    /// position after the last written packet.
    encoded: u64,
    packets_in_page: u32,
}

impl<W: Write> OggOpusWriter<W> {
    /// Writes the OpusHead and OpusTags header pages and flushes them.
    pub fn new(inner: W, serial: u32) -> Result<Self> {
        let mut encoder = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip)?;
        encoder.set_bitrate(Bitrate::Bits(BITRATE_BPS))?;
        let pre_skip = u16::try_from(encoder.get_lookahead()?).unwrap_or(u16::MAX);

        let mut packets = PacketWriter::new(inner);
        packets.write_packet(opus_head(pre_skip), serial, PacketWriteEndInfo::EndPage, 0)?;
        packets.write_packet(opus_tags(), serial, PacketWriteEndInfo::EndPage, 0)?;
        packets.inner_mut().flush()?;

        Ok(Self {
            packets,
            encoder,
            serial,
            pre_skip,
            frame: Vec::with_capacity(FRAME_SAMPLES),
            out: vec![0; MAX_PACKET],
            samples_in: 0,
            encoded: 0,
            packets_in_page: 0,
        })
    }

    pub fn pre_skip(&self) -> u16 {
        self.pre_skip
    }

    /// The underlying writer. Holds every completed page; the in-progress
    /// page is buffered internally until it ends.
    pub fn get_ref(&self) -> &W {
        self.packets.inner()
    }

    /// Real samples accepted so far.
    pub fn samples_written(&self) -> u64 {
        self.samples_in
    }

    pub fn duration_ms(&self) -> u64 {
        self.samples_in * 1000 / u64::from(SAMPLE_RATE)
    }

    pub fn write_samples(&mut self, mut samples: &[f32]) -> Result<()> {
        self.samples_in += samples.len() as u64;
        while !samples.is_empty() {
            let take = (FRAME_SAMPLES - self.frame.len()).min(samples.len());
            self.frame.extend_from_slice(&samples[..take]);
            samples = &samples[take..];
            if self.frame.len() == FRAME_SAMPLES {
                self.encode_frame(None)?;
            }
        }
        Ok(())
    }

    /// Pads the last frame, writes the end-of-stream page with the exact
    /// granule position (so decoders trim padding), flushes and returns the
    /// inner writer.
    pub fn finish(mut self) -> Result<W> {
        // Encode enough trailing silence that every real sample makes it out of
        // the encoder's lookahead: decoded output = encoded - pre_skip.
        let target = self.samples_in + u64::from(self.pre_skip);
        let final_granule = target;
        loop {
            self.frame.resize(FRAME_SAMPLES, 0.0);
            let last = self.encoded + FRAME_SAMPLES as u64 >= target;
            self.encode_frame(last.then_some(final_granule))?;
            if last {
                break;
            }
        }
        let mut inner = self.packets.into_inner();
        inner.flush()?;
        Ok(inner)
    }

    /// Encodes `self.frame` (exactly one frame). `eos_granule` marks the last
    /// packet of the stream.
    fn encode_frame(&mut self, eos_granule: Option<u64>) -> Result<()> {
        let len = self.encoder.encode_float(&self.frame, &mut self.out)?;
        self.frame.clear();
        self.encoded += FRAME_SAMPLES as u64;
        self.packets_in_page += 1;

        let (info, granule) = match eos_granule {
            Some(g) => (PacketWriteEndInfo::EndStream, g),
            None if self.packets_in_page >= PACKETS_PER_PAGE => {
                (PacketWriteEndInfo::EndPage, self.encoded)
            }
            None => (PacketWriteEndInfo::NormalPacket, self.encoded),
        };
        self.packets
            .write_packet(self.out[..len].to_vec(), self.serial, info, granule)?;
        if info != PacketWriteEndInfo::NormalPacket {
            self.packets_in_page = 0;
            self.packets.inner_mut().flush()?;
        }
        Ok(())
    }
}

fn opus_head(pre_skip: u16) -> Vec<u8> {
    let mut h = Vec::with_capacity(19);
    h.extend_from_slice(b"OpusHead");
    h.push(1); // version
    h.push(1); // channels
    h.extend_from_slice(&pre_skip.to_le_bytes());
    h.extend_from_slice(&SAMPLE_RATE.to_le_bytes()); // input sample rate
    h.extend_from_slice(&0_i16.to_le_bytes()); // output gain
    h.push(0); // mapping family 0: mono/stereo
    h
}

fn opus_tags() -> Vec<u8> {
    let mut t = Vec::with_capacity(16 + VENDOR.len());
    t.extend_from_slice(b"OpusTags");
    t.extend_from_slice(&(VENDOR.len() as u32).to_le_bytes());
    t.extend_from_slice(VENDOR.as_bytes());
    t.extend_from_slice(&0_u32.to_le_bytes()); // no user comments
    t
}

/// How often [`DurableFile`] forces data to disk.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(4);

/// A buffered file whose `flush` also `fdatasync`s at most every
/// [`SYNC_INTERVAL`], bounding loss on power failure. Process crashes lose
/// nothing that was flushed, since flushed bytes are already in the page cache.
pub struct DurableFile {
    inner: BufWriter<File>,
    last_sync: Instant,
}

impl DurableFile {
    pub fn create(path: &Path) -> io::Result<Self> {
        Ok(Self {
            inner: BufWriter::new(File::create(path)?),
            last_sync: Instant::now(),
        })
    }

    /// Flushes and fully syncs the file. Call once when recording ends.
    pub fn close(mut self) -> io::Result<()> {
        self.inner.flush()?;
        self.inner.get_ref().sync_all()
    }
}

impl Write for DurableFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()?;
        if self.last_sync.elapsed() >= SYNC_INTERVAL {
            self.inner.get_ref().sync_data()?;
            self.last_sync = Instant::now();
        }
        Ok(())
    }
}
