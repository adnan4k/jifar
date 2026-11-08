//! Lock-free SPSC ring buffer between the real-time capture callback and the
//! encoder worker (NFR-02).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::SAMPLE_RATE;

/// Default ring capacity: 2 s of 48 kHz mono audio. The worker drains every
/// few milliseconds, so this only fills if the worker stalls badly.
pub const DEFAULT_RING_CAPACITY: usize = SAMPLE_RATE as usize * 2;

/// Shared counter of samples dropped because the ring was full.
#[derive(Debug, Clone, Default)]
pub struct Overruns(Arc<AtomicU64>);

impl Overruns {
    /// Number of samples dropped so far. Zero on a healthy recording.
    pub fn samples(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Producer half, owned by the capture callback.
///
/// [`RtSink::push`] is real-time safe: it never allocates, locks, logs,
/// performs I/O or blocks. It is the only thing a capture callback may do.
pub struct RtSink {
    producer: Producer<f32>,
    overruns: Overruns,
}

impl RtSink {
    /// Copies as many samples as fit into the ring and counts the rest as
    /// overruns. Returns the number of samples accepted.
    #[inline]
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let (_, rest) = self.producer.push_partial_slice(samples);
        if !rest.is_empty() {
            self.overruns
                .0
                .fetch_add(rest.len() as u64, Ordering::Relaxed);
        }
        samples.len() - rest.len()
    }

    /// Free space in samples.
    #[inline]
    pub fn free_slots(&self) -> usize {
        self.producer.slots()
    }

    /// True once the worker side has been dropped.
    pub fn is_abandoned(&self) -> bool {
        self.producer.is_abandoned()
    }
}

/// Consumer half, owned by the worker thread.
pub struct RingReader {
    consumer: Consumer<f32>,
}

impl RingReader {
    /// Moves up to `buf.len()` samples out of the ring. Returns how many.
    pub fn pop_into(&mut self, buf: &mut [f32]) -> usize {
        let (filled, _) = self.consumer.pop_partial_slice(buf);
        filled.len()
    }

    pub fn is_empty(&self) -> bool {
        self.consumer.is_empty()
    }

    /// True once the capture side has been dropped.
    pub fn is_abandoned(&self) -> bool {
        self.consumer.is_abandoned()
    }
}

/// Creates a ring of `capacity` samples.
pub fn ring(capacity: usize) -> (RtSink, RingReader, Overruns) {
    let (producer, consumer) = RingBuffer::new(capacity);
    let overruns = Overruns::default();
    (
        RtSink {
            producer,
            overruns: overruns.clone(),
        },
        RingReader { consumer },
        overruns,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_overruns_when_full() {
        let (mut sink, mut reader, overruns) = ring(8);
        assert_eq!(sink.push(&[1.0; 5]), 5);
        assert_eq!(sink.push(&[2.0; 5]), 3);
        assert_eq!(overruns.samples(), 2);

        let mut buf = [0.0; 16];
        assert_eq!(reader.pop_into(&mut buf), 8);
        assert_eq!(&buf[..8], &[1.0, 1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0]);
        assert!(reader.is_empty());
        assert_eq!(sink.push(&[3.0; 4]), 4);
        assert_eq!(overruns.samples(), 2);
    }

    #[test]
    fn abandonment_is_visible_to_both_sides() {
        let (sink, reader, _) = ring(4);
        drop(sink);
        assert!(reader.is_abandoned());
    }
}
