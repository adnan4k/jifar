//! Streaming Ogg/Opus decoder for files written by [`crate::OggOpusWriter`].
//!
//! Tolerates truncated files (crash recovery): decoding stops at the last
//! complete page and the result is flagged as truncated.

use std::io::{Read, Seek};

use ogg::reading::{OggReadError, PacketReader};
use opus::{Channels, Decoder};
use tracing::warn;

use crate::{AudioError, Result, SAMPLE_RATE};

/// Max samples per Opus packet at 48 kHz (120 ms).
const MAX_PACKET_SAMPLES: usize = 5760;

/// Decodes a mono 48 kHz Ogg/Opus stream block by block, applying pre-skip
/// and end trimming. Memory use is constant regardless of file length.
pub struct OggOpusReader<R: Read + Seek> {
    packets: PacketReader<R>,
    decoder: Decoder,
    pre_skip: u16,
    skip_left: usize,
    /// Decoder output samples so far (comparable to granule positions).
    decoded: u64,
    buf: Vec<f32>,
    truncated: bool,
    done: bool,
}

impl<R: Read + Seek> OggOpusReader<R> {
    /// Reads and validates the OpusHead and OpusTags headers.
    pub fn new(inner: R) -> Result<Self> {
        let mut packets = PacketReader::new(inner);
        let head = packets
            .read_packet()
            .map_err(invalid)?
            .ok_or_else(|| AudioError::InvalidStream("empty stream".into()))?;
        let pre_skip = parse_head(&head.data)?;
        let tags = packets
            .read_packet()
            .map_err(invalid)?
            .ok_or_else(|| AudioError::InvalidStream("missing OpusTags".into()))?;
        if !tags.data.starts_with(b"OpusTags") {
            return Err(AudioError::InvalidStream("missing OpusTags".into()));
        }
        Ok(Self {
            packets,
            decoder: Decoder::new(SAMPLE_RATE, Channels::Mono)?,
            pre_skip,
            skip_left: usize::from(pre_skip),
            decoded: 0,
            buf: vec![0.0; MAX_PACKET_SAMPLES],
            truncated: false,
            done: false,
        })
    }

    pub fn pre_skip(&self) -> u16 {
        self.pre_skip
    }

    /// True if the stream ended without an end-of-stream page (e.g. the
    /// recorder crashed). Only meaningful once decoding has finished.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Returns the next block of decoded samples, or `None` at the end.
    pub fn next_block(&mut self) -> Result<Option<&[f32]>> {
        loop {
            if self.done {
                return Ok(None);
            }
            let packet = match self.packets.read_packet() {
                Ok(Some(p)) => p,
                Ok(None) => {
                    self.done = true;
                    self.truncated = true;
                    return Ok(None);
                }
                Err(e) => {
                    warn!(error = %e, "ogg/opus stream ends in a damaged page; treating as truncated");
                    self.done = true;
                    self.truncated = true;
                    return Ok(None);
                }
            };
            let n = self
                .decoder
                .decode_float(&packet.data, &mut self.buf, false)?;
            let mut end = n;
            self.decoded += n as u64;
            if packet.last_in_stream() {
                self.done = true;
                // The final granule position marks where real audio ends.
                let excess = self.decoded.saturating_sub(packet.absgp_page());
                end = n.saturating_sub(usize::try_from(excess).unwrap_or(usize::MAX));
            }
            let start = self.skip_left.min(end);
            self.skip_left -= self.skip_left.min(n);
            if start < end {
                return Ok(Some(&self.buf[start..end]));
            }
        }
    }

    /// Decodes everything into memory. Only for short files and tests; long
    /// recordings should be streamed with [`Self::next_block`].
    pub fn read_to_end(&mut self) -> Result<Vec<f32>> {
        let mut all = Vec::new();
        while let Some(block) = self.next_block()? {
            all.extend_from_slice(block);
        }
        Ok(all)
    }
}

fn parse_head(data: &[u8]) -> Result<u16> {
    if data.len() < 19 || !data.starts_with(b"OpusHead") {
        return Err(AudioError::InvalidStream("missing OpusHead".into()));
    }
    let channels = data[9];
    if channels != 1 {
        return Err(AudioError::UnsupportedFormat(format!(
            "{channels} channels, expected mono"
        )));
    }
    Ok(u16::from_le_bytes([data[10], data[11]]))
}

fn invalid(e: OggReadError) -> AudioError {
    AudioError::InvalidStream(e.to_string())
}
