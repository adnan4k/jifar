//! Lock-free SPSC ring buffer between the real-time capture callback and the
//! encoder worker (NFR-02).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::SAMPLE_RATE;

/// Default ring capacity: 2 s of 48 kHz mono audio. The worker drains every
/// few milliseconds, so this only fills if the worker stalls badly.
pub const DEFAULT_RING_CAPACITY: usize = SAMPLE_RATE as usize * 2;

/// Sentinel for "no sample delivered yet".
const NONE: u64 = u64::MAX;

#[derive(Debug)]
struct StatsInner {
    /// Reference point for `first_sample_ns`.
    base: Instant,
    /// Capture time of the first delivered sample, in ns after `base`.
    first_sample_ns: AtomicU64,
    /// Samples offered by the source (accepted + dropped).
    delivered: AtomicU64,
    /// Samples dropped because the ring was full.
    overruns: AtomicU64,
    /// While set, pushed samples are discarded (FR-REC-09).
    paused: AtomicBool,
    /// When the current pause began, in ns after `base`.
    pause_started_ns: AtomicU64,
    /// Paused time after the first sample, from finished pauses.
    paused_total_ns: AtomicU64,
}

/// Counters shared between the capture callback and everyone else. All
/// updates are relaxed atomics, so they are safe on the real-time path.
#[derive(Debug, Clone)]
pub struct CaptureStats(Arc<StatsInner>);

impl CaptureStats {
    fn new() -> Self {
        Self(Arc::new(StatsInner {
            base: Instant::now(),
            first_sample_ns: AtomicU64::new(NONE),
            delivered: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            pause_started_ns: AtomicU64::new(0),
            paused_total_ns: AtomicU64::new(0),
        }))
    }

    pub fn is_paused(&self) -> bool {
        self.0.paused.load(Ordering::Acquire)
    }

    /// Pauses or resumes the track at `now`. While paused the source keeps
    /// running but its samples are discarded, and the time doesn't count as
    /// recording time. Only the session's control thread calls this.
    pub(crate) fn set_paused(&self, paused: bool, now: Instant) {
        let inner = &self.0;
        if paused == inner.paused.load(Ordering::Acquire) {
            return;
        }
        let now_ns = self.ns_since_base(now);
        if paused {
            inner.pause_started_ns.store(now_ns, Ordering::Relaxed);
            inner.paused.store(true, Ordering::Release);
        } else {
            inner.paused.store(false, Ordering::Release);
            let add = self.current_pause_ns(now_ns);
            inner.paused_total_ns.fetch_add(add, Ordering::Relaxed);
        }
    }

    /// Recording time since the first sample at `now`: wall time minus the
    /// paused time. `None` until audio arrives.
    pub fn active_since_first(&self, now: Instant) -> Option<Duration> {
        let first = self.first_sample_at()?;
        let mut paused = self.0.paused_total_ns.load(Ordering::Relaxed);
        if self.is_paused() {
            paused += self.current_pause_ns(self.ns_since_base(now));
        }
        Some(
            now.saturating_duration_since(first)
                .saturating_sub(Duration::from_nanos(paused)),
        )
    }

    /// Part of the current pause that falls after the first sample (a pause
    /// before any audio arrived doesn't shift the track's timeline).
    fn current_pause_ns(&self, now_ns: u64) -> u64 {
        match self.0.first_sample_ns.load(Ordering::Acquire) {
            NONE => 0,
            first => {
                let start = self.0.pause_started_ns.load(Ordering::Relaxed).max(first);
                now_ns.saturating_sub(start)
            }
        }
    }

    fn ns_since_base(&self, at: Instant) -> u64 {
        u64::try_from(at.saturating_duration_since(self.0.base).as_nanos()).unwrap_or(NONE - 1)
    }

    /// Samples dropped so far. Zero on a healthy recording (NFR-02).
    pub fn overrun_samples(&self) -> u64 {
        self.0.overruns.load(Ordering::Relaxed)
    }

    /// Samples the source has delivered so far, including dropped ones. This
    /// is the device's own sense of elapsed time, used to measure drift.
    pub fn delivered_samples(&self) -> u64 {
        self.0.delivered.load(Ordering::Relaxed)
    }

    /// Estimated capture time of the first sample: when the first block
    /// arrived, minus that block's duration. `None` until audio arrives.
    pub fn first_sample_at(&self) -> Option<Instant> {
        match self.0.first_sample_ns.load(Ordering::Acquire) {
            NONE => None,
            ns => Some(self.0.base + Duration::from_nanos(ns)),
        }
    }
}

/// Producer half, owned by the capture callback.
///
/// [`RtSink::push`] is real-time safe: it never allocates, locks, logs,
/// performs I/O or blocks. It is the only thing a capture callback may do.
pub struct RtSink {
    producer: Producer<f32>,
    stats: CaptureStats,
}

impl RtSink {
    /// Copies as many samples as fit into the ring and counts the rest as
    /// overruns. Returns the number of samples accepted. While paused,
    /// samples are discarded uncounted and reported as accepted.
    #[inline]
    pub fn push(&mut self, samples: &[f32]) -> usize {
        if samples.is_empty() {
            return 0;
        }
        let stats = &self.stats.0;
        if stats.paused.load(Ordering::Relaxed) {
            return samples.len();
        }
        if stats.first_sample_ns.load(Ordering::Relaxed) == NONE {
            // `Instant::now` is a vDSO clock read: no syscall, lock or alloc.
            let block =
                Duration::from_nanos(samples.len() as u64 * 1_000_000_000 / u64::from(SAMPLE_RATE));
            let at = stats.base.elapsed().saturating_sub(block);
            let ns = u64::try_from(at.as_nanos()).unwrap_or(NONE - 1);
            stats.first_sample_ns.store(ns, Ordering::Release);
        }
        stats
            .delivered
            .fetch_add(samples.len() as u64, Ordering::Relaxed);
        let (_, rest) = self.producer.push_partial_slice(samples);
        if !rest.is_empty() {
            stats
                .overruns
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
pub fn ring(capacity: usize) -> (RtSink, RingReader, CaptureStats) {
    let (producer, consumer) = RingBuffer::new(capacity);
    let stats = CaptureStats::new();
    (
        RtSink {
            producer,
            stats: stats.clone(),
        },
        RingReader { consumer },
        stats,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_overruns_when_full() {
        let (mut sink, mut reader, stats) = ring(8);
        assert_eq!(sink.push(&[1.0; 5]), 5);
        assert_eq!(sink.push(&[2.0; 5]), 3);
        assert_eq!(stats.overrun_samples(), 2);
        assert_eq!(stats.delivered_samples(), 10);

        let mut buf = [0.0; 16];
        assert_eq!(reader.pop_into(&mut buf), 8);
        assert_eq!(&buf[..8], &[1.0, 1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0]);
        assert!(reader.is_empty());
        assert_eq!(sink.push(&[3.0; 4]), 4);
        assert_eq!(stats.overrun_samples(), 2);
    }

    #[test]
    fn first_sample_time_is_set_once_and_backdated() {
        let (mut sink, _reader, stats) = ring(SAMPLE_RATE as usize);
        assert!(stats.first_sample_at().is_none());
        let before = Instant::now();
        sink.push(&[0.0; SAMPLE_RATE as usize / 10]); // 100 ms block
        let first = stats.first_sample_at().unwrap();
        assert!(
            first <= before,
            "first sample should be backdated by the block length"
        );
        assert!(before.duration_since(first) < Duration::from_millis(150));
        sink.push(&[0.0; 10]);
        assert_eq!(stats.first_sample_at(), Some(first));
    }

    #[test]
    fn paused_samples_are_discarded_and_not_counted() {
        let (mut sink, mut reader, stats) = ring(64);
        sink.push(&[1.0; 4]);
        let t0 = Instant::now();
        stats.set_paused(true, t0);
        assert_eq!(
            sink.push(&[9.0; 100]),
            100,
            "reported as taken, not as overrun"
        );
        stats.set_paused(false, t0 + Duration::from_millis(500));
        sink.push(&[2.0; 4]);

        assert_eq!(stats.delivered_samples(), 8);
        assert_eq!(stats.overrun_samples(), 0);
        let mut buf = [0.0; 16];
        assert_eq!(reader.pop_into(&mut buf), 8);
        assert_eq!(&buf[..8], &[1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
        // The 500 ms pause is not recording time.
        let active = stats
            .active_since_first(t0 + Duration::from_millis(600))
            .unwrap();
        let wall = (t0 + Duration::from_millis(600)) - stats.first_sample_at().unwrap();
        assert_eq!(wall - active, Duration::from_millis(500));
    }

    #[test]
    fn pause_before_first_sample_does_not_shift_the_timeline() {
        let (mut sink, _reader, stats) = ring(64);
        let t0 = Instant::now();
        stats.set_paused(true, t0);
        sink.push(&[1.0; 4]);
        assert!(stats.first_sample_at().is_none());
        stats.set_paused(false, t0 + Duration::from_millis(300));
        sink.push(&[1.0; 4]);
        let first = stats.first_sample_at().unwrap();
        let now = first + Duration::from_millis(100);
        assert_eq!(
            stats.active_since_first(now),
            Some(Duration::from_millis(100))
        );
    }

    #[test]
    fn abandonment_is_visible_to_both_sides() {
        let (sink, reader, _) = ring(4);
        drop(sink);
        assert!(reader.is_abandoned());
    }
}
