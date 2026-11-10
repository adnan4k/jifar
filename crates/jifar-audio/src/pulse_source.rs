//! [`PulseSource`]: capture through the PulseAudio API, for systems where
//! PulseAudio (not PipeWire) owns the audio devices, e.g. stock Ubuntu 22.04
//! (FR-REC-02, FR-REC-03, SPEC §6.3).
//!
//! Each source owns one capture thread with a blocking `pa_simple` record
//! stream asking for F32LE, 48 kHz, mono, so the server resamples and
//! downmixes. The microphone is the default source; system audio is the
//! default sink's monitor (`@DEFAULT_MONITOR@`).
//!
//! Real-time rules: the capture thread is our own reader, not a server
//! callback, but it follows the same rules as one on its hot path. It reads
//! into preallocated buffers, converts and calls [`RtSink::push`], with no
//! allocation, locks or logging per block. Errors end the loop and are stored
//! once, off the hot path.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use libpulse_binding as pulse;
use libpulse_simple_binding::Simple;
use pulse::context::{Context, FlagSet as ContextFlags, State as ContextState};
use pulse::def::BufferAttr;
use pulse::mainloop::standard::{IterateResult, Mainloop};
use pulse::sample::{Format, Spec};
use pulse::stream::Direction;
use tracing::{debug, info, warn};

use crate::backend::CaptureTarget;
use crate::ring::RtSink;
use crate::source::CaptureSource;
use crate::{AudioError, FRAME_SAMPLES, Result, SAMPLE_RATE};

/// How long [`PulseSource::start`] waits for the stream to connect.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// How long [`server_name`] waits for the server to answer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Samples per blocking read: one Opus frame (20 ms). Also the fragment size
/// we ask the server for, so reads return at that cadence and `stop` is
/// noticed within ~20 ms.
const READ_SAMPLES: usize = FRAME_SAMPLES;
const BYTES_PER_SAMPLE: usize = 4;
const APP_NAME: &str = "Jifar";

/// State shared between the caller and the capture thread.
#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    /// Last fault. Written once, when the capture loop ends with an error.
    error: Mutex<Option<String>>,
}

impl Shared {
    fn fail(&self, msg: String) {
        warn!(error = %msg, "pulseaudio capture fault");
        if let Ok(mut slot) = self.error.lock() {
            slot.get_or_insert(msg);
        }
    }
}

/// A live PulseAudio record stream. See the module docs.
pub struct PulseSource {
    name: String,
    target: CaptureTarget,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl PulseSource {
    pub fn new(target: CaptureTarget) -> Self {
        Self {
            name: target.node_name().to_owned(),
            target,
            shared: Arc::default(),
            thread: None,
        }
    }

    pub fn microphone() -> Self {
        Self::new(CaptureTarget::Microphone)
    }

    pub fn system_output() -> Self {
        Self::new(CaptureTarget::SystemOutput)
    }

    pub fn target(&self) -> CaptureTarget {
        self.target
    }
}

impl CaptureSource for PulseSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self, sink: RtSink) -> Result<()> {
        if self.thread.is_some() {
            return Err(AudioError::AlreadyRunning);
        }
        self.shared.stop.store(false, Ordering::Release);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<std::result::Result<(), String>>(1);
        let target = self.target;
        let shared = Arc::clone(&self.shared);
        let thread = thread::Builder::new()
            .name(format!("pulse:{}", self.name))
            .spawn(move || capture_loop(target, sink, &shared, &ready_tx))?;

        match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => {
                self.thread = Some(thread);
                info!(source = %self.name, "pulseaudio stream connected");
                Ok(())
            }
            Ok(Err(msg)) => {
                let _ = thread.join();
                Err(AudioError::Source(format!("{}: {msg}", self.name)))
            }
            Err(_) => {
                // Still stuck connecting; it exits on its own once it sees stop.
                self.shared.stop.store(true, Ordering::Release);
                Err(AudioError::Source(format!(
                    "{}: timed out connecting to PulseAudio",
                    self.name
                )))
            }
        }
    }

    fn stop(&mut self) -> Result<()> {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| {
                AudioError::Source(format!("{}: capture thread panicked", self.name))
            })?;
            debug!(source = %self.name, "pulseaudio stream stopped");
        }
        Ok(())
    }

    fn error(&self) -> Option<String> {
        self.shared.error.lock().ok().and_then(|e| e.clone())
    }
}

impl Drop for PulseSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Body of the capture thread: connect, report through `ready`, then read
/// until stopped or the stream fails.
fn capture_loop(
    target: CaptureTarget,
    mut sink: RtSink,
    shared: &Shared,
    ready: &mpsc::SyncSender<std::result::Result<(), String>>,
) {
    let spec = Spec {
        format: Format::F32le,
        channels: 1,
        rate: SAMPLE_RATE,
    };
    let fragment = u32::try_from(READ_SAMPLES * BYTES_PER_SAMPLE).unwrap_or(u32::MAX);
    let attr = BufferAttr {
        maxlength: u32::MAX,
        tlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: fragment,
    };
    let device = match target {
        CaptureTarget::Microphone => None,
        CaptureTarget::SystemOutput => Some("@DEFAULT_MONITOR@"),
    };
    let stream = match Simple::new(
        None,
        APP_NAME,
        Direction::Record,
        device,
        target.node_name(),
        &spec,
        None,
        Some(&attr),
    ) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(format!("connect record stream: {e}")));
            return;
        }
    };
    if ready.send(Ok(())).is_err() || shared.stop.load(Ordering::Acquire) {
        // start() gave up waiting; nobody will consume this stream.
        return;
    }

    let mut bytes = vec![0u8; READ_SAMPLES * BYTES_PER_SAMPLE].into_boxed_slice();
    let mut samples = vec![0.0f32; READ_SAMPLES].into_boxed_slice();
    while !shared.stop.load(Ordering::Acquire) {
        if let Err(e) = stream.read(&mut bytes) {
            shared.fail(format!("read: {e}"));
            return;
        }
        for (dst, src) in samples.iter_mut().zip(bytes.chunks_exact(BYTES_PER_SAMPLE)) {
            *dst = f32::from_le_bytes([src[0], src[1], src[2], src[3]]);
        }
        sink.push(&samples);
    }
}

/// Name the PulseAudio-protocol server reports (`"pulseaudio"`, or
/// `"PulseAudio (on PipeWire x.y.z)"` for pipewire-pulse). `None` if no
/// server answers within [`PROBE_TIMEOUT`].
pub(crate) fn server_name() -> Option<String> {
    let mut mainloop = Mainloop::new()?;
    let mut context = Context::new(&mainloop, APP_NAME)?;
    // NOAUTOSPAWN: probing must not start a PulseAudio daemon on a
    // PipeWire-only system.
    context
        .connect(None, ContextFlags::NOAUTOSPAWN, None)
        .ok()?;

    let deadline = Instant::now() + PROBE_TIMEOUT;
    let iterate = |mainloop: &mut Mainloop| match mainloop.iterate(false) {
        IterateResult::Success(_) => {
            thread::sleep(Duration::from_millis(5));
            Instant::now() < deadline
        }
        IterateResult::Quit(_) | IterateResult::Err(_) => false,
    };

    let result = loop {
        match context.get_state() {
            ContextState::Ready => break true,
            ContextState::Failed | ContextState::Terminated => break false,
            _ if !iterate(&mut mainloop) => break false,
            _ => {}
        }
    };
    if !result {
        context.disconnect();
        return None;
    }

    let name: Arc<Mutex<Option<Option<String>>>> = Arc::default();
    let slot = Arc::clone(&name);
    let _op = context.introspect().get_server_info(move |info| {
        if let Ok(mut s) = slot.lock() {
            *s = Some(info.server_name.as_ref().map(|n| n.to_string()));
        }
    });
    let found = loop {
        if let Some(answer) = name.lock().ok().and_then(|s| s.clone()) {
            break answer;
        }
        if !iterate(&mut mainloop) {
            break None;
        }
    };
    context.disconnect();
    debug!(server = ?found, "pulseaudio server probed");
    found
}
