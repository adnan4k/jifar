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
use pulse::callbacks::ListResult;
use pulse::context::{Context, FlagSet as ContextFlags, State as ContextState};
use pulse::def::BufferAttr;
use pulse::mainloop::standard::{IterateResult, Mainloop};
use pulse::sample::{Format, Spec};
use pulse::stream::Direction;
use tracing::{debug, info, warn};

use crate::backend::CaptureTarget;
use crate::device::{AudioDevice, DeviceKind};
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
    /// Device ID (see [`crate::AudioDevice`]); `None` for the default.
    device: Option<String>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl PulseSource {
    pub fn new(target: CaptureTarget) -> Self {
        Self::with_device(target, None)
    }

    /// Records from a specific device (a source for the mic, a sink whose
    /// monitor is recorded for system output). `None` uses the default.
    pub fn with_device(target: CaptureTarget, device: Option<String>) -> Self {
        Self {
            name: target.node_name().to_owned(),
            target,
            device,
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
        let device = self.device.clone();
        let shared = Arc::clone(&self.shared);
        let thread = thread::Builder::new()
            .name(format!("pulse:{}", self.name))
            .spawn(move || capture_loop(target, device.as_deref(), sink, &shared, &ready_tx))?;

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
    device: Option<&str>,
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
    let device = match (target, device) {
        (CaptureTarget::Microphone, dev) => dev.map(str::to_owned),
        (CaptureTarget::SystemOutput, Some(sink)) => Some(format!("{sink}.monitor")),
        (CaptureTarget::SystemOutput, None) => Some("@DEFAULT_MONITOR@".to_owned()),
    };
    let stream = match Simple::new(
        None,
        APP_NAME,
        Direction::Record,
        device.as_deref(),
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
        let (frames, _) = bytes.as_chunks::<BYTES_PER_SAMPLE>();
        for (dst, src) in samples.iter_mut().zip(frames) {
            *dst = f32::from_le_bytes(*src);
        }
        sink.push(&samples);
    }
}

/// A connected PulseAudio-protocol context, for one-off queries.
struct Query {
    mainloop: Mainloop,
    context: Context,
    deadline: Instant,
}

impl Query {
    /// Connects to the server, or `None` if none answers within
    /// [`PROBE_TIMEOUT`].
    fn connect() -> Option<Self> {
        let mainloop = Mainloop::new()?;
        let mut context = Context::new(&mainloop, APP_NAME)?;
        // NOAUTOSPAWN: probing must not start a PulseAudio daemon on a
        // PipeWire-only system.
        context
            .connect(None, ContextFlags::NOAUTOSPAWN, None)
            .ok()?;
        let mut q = Self {
            mainloop,
            context,
            deadline: Instant::now() + PROBE_TIMEOUT,
        };
        loop {
            match q.context.get_state() {
                ContextState::Ready => return Some(q),
                ContextState::Failed | ContextState::Terminated => return None,
                _ if !q.iterate() => return None,
                _ => {}
            }
        }
    }

    /// Runs the loop once. False once the deadline passed or the loop died.
    fn iterate(&mut self) -> bool {
        match self.mainloop.iterate(false) {
            IterateResult::Success(_) => {
                thread::sleep(Duration::from_millis(5));
                Instant::now() < self.deadline
            }
            IterateResult::Quit(_) | IterateResult::Err(_) => false,
        }
    }

    /// Iterates until `done()` or the deadline.
    fn wait(&mut self, done: impl Fn() -> bool) -> bool {
        while !done() {
            if !self.iterate() {
                return false;
            }
        }
        true
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        self.context.disconnect();
    }
}

/// Name the PulseAudio-protocol server reports (`"pulseaudio"`, or
/// `"PulseAudio (on PipeWire x.y.z)"` for pipewire-pulse). `None` if no
/// server answers within [`PROBE_TIMEOUT`].
pub(crate) fn server_name() -> Option<String> {
    let mut q = Query::connect()?;
    let name: Arc<Mutex<Option<Option<String>>>> = Arc::default();
    let slot = Arc::clone(&name);
    let _op = q.context.introspect().get_server_info(move |info| {
        if let Ok(mut s) = slot.lock() {
            *s = Some(info.server_name.as_ref().map(|n| n.to_string()));
        }
    });
    let answered = q.wait(|| name.lock().map(|s| s.is_some()).unwrap_or(true));
    let found = answered
        .then(|| name.lock().ok().and_then(|s| s.clone()).flatten())
        .flatten();
    debug!(server = ?found, "pulseaudio server probed");
    found
}

#[derive(Default)]
struct Listing {
    default_source: Option<String>,
    default_sink: Option<String>,
    server_done: bool,
    sources: Vec<(String, String)>,
    sources_done: bool,
    sinks: Vec<(String, String)>,
    sinks_done: bool,
    failed: bool,
}

/// Lists inputs (real sources, not monitors) and outputs (sinks) through the
/// PulseAudio protocol. Works on real PulseAudio and on pipewire-pulse, where
/// the names are PipeWire node names, so the IDs are valid for both backends.
pub(crate) fn list_devices() -> Result<Vec<AudioDevice>> {
    let mut q = Query::connect().ok_or_else(|| {
        AudioError::Source(
            "no PulseAudio-protocol server to list devices (is pipewire-pulse running?)".into(),
        )
    })?;
    let listing: Arc<Mutex<Listing>> = Arc::default();
    let introspect = q.context.introspect();
    let l = Arc::clone(&listing);
    let _server = introspect.get_server_info(move |info| {
        if let Ok(mut l) = l.lock() {
            l.default_source = info.default_source_name.as_ref().map(|n| n.to_string());
            l.default_sink = info.default_sink_name.as_ref().map(|n| n.to_string());
            l.server_done = true;
        }
    });
    let l = Arc::clone(&listing);
    let _sources = introspect.get_source_info_list(move |item| {
        let Ok(mut l) = l.lock() else { return };
        match item {
            ListResult::Item(src) => {
                // Monitors are outputs seen as sources; outputs are listed
                // as sinks instead.
                if src.monitor_of_sink.is_none()
                    && let Some(name) = &src.name
                {
                    let desc = src.description.as_deref().unwrap_or(name).to_owned();
                    l.sources.push((name.to_string(), desc));
                }
            }
            ListResult::End => l.sources_done = true,
            ListResult::Error => l.failed = true,
        }
    });
    let l = Arc::clone(&listing);
    let _sinks = introspect.get_sink_info_list(move |item| {
        let Ok(mut l) = l.lock() else { return };
        match item {
            ListResult::Item(sink) => {
                if let Some(name) = &sink.name {
                    let desc = sink.description.as_deref().unwrap_or(name).to_owned();
                    l.sinks.push((name.to_string(), desc));
                }
            }
            ListResult::End => l.sinks_done = true,
            ListResult::Error => l.failed = true,
        }
    });
    let done = q.wait(|| {
        listing
            .lock()
            .map(|l| l.failed || (l.server_done && l.sources_done && l.sinks_done))
            .unwrap_or(true)
    });
    let l = listing
        .lock()
        .map_err(|_| AudioError::Source("device listing state poisoned".into()))?;
    if !done || l.failed {
        return Err(AudioError::Source(
            "listing audio devices failed or timed out".into(),
        ));
    }
    let to_devices = |items: &[(String, String)], kind, default: &Option<String>| {
        items
            .iter()
            .map(|(id, description)| AudioDevice {
                id: id.clone(),
                description: description.clone(),
                kind,
                is_default: default.as_deref() == Some(id),
            })
            .collect::<Vec<_>>()
    };
    let mut devices = to_devices(&l.sources, DeviceKind::Input, &l.default_source);
    devices.extend(to_devices(&l.sinks, DeviceKind::Output, &l.default_sink));
    Ok(devices)
}
