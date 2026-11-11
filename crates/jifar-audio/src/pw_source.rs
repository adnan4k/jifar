//! [`PipeWireSource`]: native PipeWire capture of the default microphone or
//! the default sink's monitor (FR-REC-02, FR-REC-03, SPEC §6.3).
//!
//! Each source owns one thread running its own PipeWire main loop with one
//! capture stream. The stream is negotiated as F32LE, 48 kHz, mono, so
//! PipeWire's own converter does any resampling and downmixing.
//!
//! Real-time rules: the `process` callback runs on PipeWire's data thread
//! (`RT_PROCESS`). It only converts bytes into a preallocated scratch buffer
//! and calls [`RtSink::push`]; it reads atomics, nothing else. Format checks,
//! state changes and error reporting run on the main-loop thread and share
//! state with the callback through atomics only.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::param::audio::{AudioFormat, AudioInfoRaw};
use pw::spa::param::format::{MediaSubtype, MediaType};
use pw::spa::param::format_utils;
use pw::spa::pod::Pod;
use pw::stream::{StreamFlags, StreamState};
use tracing::{debug, info, warn};

use crate::backend::CaptureTarget;
use crate::ring::RtSink;
use crate::source::CaptureSource;
use crate::{AudioError, FRAME_SAMPLES, Result, SAMPLE_RATE};

/// How long [`PipeWireSource::start`] waits for the stream to be set up.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// Scratch buffer for byte → f32 conversion in the callback (~85 ms).
const SCRATCH_SAMPLES: usize = 4096;

/// State shared between the caller, the main-loop thread and the RT callback.
#[derive(Default)]
struct Shared {
    /// Set once the negotiated format is 48 kHz mono f32. The RT callback
    /// drops audio until then rather than recording garbage.
    format_ok: AtomicBool,
    /// Last fault. Written only from the main-loop thread, never from RT.
    error: Mutex<Option<String>>,
}

impl Shared {
    fn fail(&self, msg: String) {
        warn!(error = %msg, "pipewire capture fault");
        self.format_ok.store(false, Ordering::Release);
        if let Ok(mut slot) = self.error.lock() {
            slot.get_or_insert(msg);
        }
    }
}

/// Data owned by the RT `process` callback. Nothing else touches it.
struct RtState {
    sink: RtSink,
    scratch: Box<[f32]>,
    shared: Arc<Shared>,
}

/// A live PipeWire capture stream. See the module docs.
pub struct PipeWireSource {
    name: String,
    target: CaptureTarget,
    /// Node name to link to (see [`crate::AudioDevice`]); `None` lets the
    /// session manager pick the default.
    device: Option<String>,
    shared: Arc<Shared>,
    quit: Option<pw::channel::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl PipeWireSource {
    pub fn new(target: CaptureTarget) -> Self {
        Self::with_device(target, None)
    }

    /// Records from a specific node (a source for the mic, a sink whose
    /// monitor is recorded for system output). `None` uses the default.
    pub fn with_device(target: CaptureTarget, device: Option<String>) -> Self {
        Self {
            name: target.node_name().to_owned(),
            target,
            device,
            shared: Arc::default(),
            quit: None,
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

impl CaptureSource for PipeWireSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self, sink: RtSink) -> Result<()> {
        if self.thread.is_some() {
            return Err(AudioError::AlreadyRunning);
        }
        let (quit_tx, quit_rx) = pw::channel::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<std::result::Result<(), String>>(1);
        let target = self.target;
        let device = self.device.clone();
        let shared = Arc::clone(&self.shared);
        let thread = thread::Builder::new()
            .name(format!("pw-loop:{}", self.name))
            .spawn(move || {
                let result = run_loop(target, device, sink, shared, quit_rx, &ready_tx);
                if let Err(msg) = result {
                    // Fails only if start() already timed out and left.
                    let _ = ready_tx.send(Err(msg));
                }
            })?;

        match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => {
                self.quit = Some(quit_tx);
                self.thread = Some(thread);
                info!(source = %self.name, "pipewire stream connected");
                Ok(())
            }
            Ok(Err(msg)) => {
                let _ = thread.join();
                Err(AudioError::Source(format!("{}: {msg}", self.name)))
            }
            Err(_) => {
                // Still stuck connecting; ask the loop to quit if it ever runs.
                let _ = quit_tx.send(());
                Err(AudioError::Source(format!(
                    "{}: timed out connecting to PipeWire",
                    self.name
                )))
            }
        }
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(quit) = self.quit.take() {
            // Fails only if the loop already exited, which is fine.
            let _ = quit.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| AudioError::Source(format!("{}: loop thread panicked", self.name)))?;
            debug!(source = %self.name, "pipewire loop stopped");
        }
        Ok(())
    }

    fn error(&self) -> Option<String> {
        self.shared.error.lock().ok().and_then(|e| e.clone())
    }
}

impl Drop for PipeWireSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Body of the main-loop thread. Returns `Err` only for setup failures; it
/// reports success through `ready` and then runs until told to quit.
fn run_loop(
    target: CaptureTarget,
    device: Option<String>,
    sink: RtSink,
    shared: Arc<Shared>,
    quit: pw::channel::Receiver<()>,
    ready: &mpsc::SyncSender<std::result::Result<(), String>>,
) -> std::result::Result<(), String> {
    pw::init();
    let err = |what: &str, e: pw::Error| format!("{what}: {e}");

    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| err("main loop", e))?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| err("context", e))?;
    let core = context
        .connect_rc(None)
        .map_err(|e| err("connect to PipeWire daemon", e))?;

    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::APP_NAME => "Jifar",
        *pw::keys::NODE_NAME => target.node_name(),
        // One Opus frame per graph cycle where the graph allows it.
        *pw::keys::NODE_LATENCY => format!("{FRAME_SAMPLES}/{SAMPLE_RATE}"),
    };
    if target == CaptureTarget::SystemOutput {
        props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
    }
    if let Some(node) = device {
        // Link to this node instead of the default. Callers validate it with
        // `resolve_device` first, since a missing target silently falls
        // back to the default here.
        props.insert("target.object", node);
    }
    let stream = pw::stream::StreamBox::new(&core, target.node_name(), props)
        .map_err(|e| err("create stream", e))?;

    let rt = RtState {
        sink,
        scratch: vec![0.0; SCRATCH_SAMPLES].into_boxed_slice(),
        shared: Arc::clone(&shared),
    };
    let on_param = Arc::clone(&shared);
    let on_state = Arc::clone(&shared);
    let _listener = stream
        .add_local_listener_with_user_data(rt)
        // Main-loop thread. Must not touch the RT user data.
        .param_changed(move |_, _, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            check_format(&on_param, param);
        })
        .state_changed(move |_, _, old, new| {
            debug!(?old, ?new, "pipewire stream state");
            if let StreamState::Error(msg) = new {
                on_state.fail(format!("stream error: {msg}"));
            }
        })
        // PipeWire data thread: real-time safe code only.
        .process(|stream, rt| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            if !rt.shared.format_ok.load(Ordering::Acquire) {
                return;
            }
            let (offset, size) = (data.chunk().offset() as usize, data.chunk().size() as usize);
            let Some(bytes) = data.data() else { return };
            let end = offset.saturating_add(size).min(bytes.len());
            let bytes = &bytes[offset.min(end)..end];
            for block in bytes.chunks(rt.scratch.len() * 4) {
                let n = block.len() / 4;
                let (frames, _) = block.as_chunks::<4>();
                for (dst, src) in rt.scratch[..n].iter_mut().zip(frames) {
                    *dst = f32::from_le_bytes(*src);
                }
                rt.sink.push(&rt.scratch[..n]);
            }
        })
        .register()
        .map_err(|e| err("register listener", e))?;

    let format = enum_format_pod().map_err(|e| format!("build format: {e}"))?;
    let pod = Pod::from_bytes(&format).ok_or("build format: invalid pod")?;
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut [pod],
        )
        .map_err(|e| err("connect stream", e))?;

    let weak = mainloop.downgrade();
    let _quit = quit.attach(mainloop.loop_(), move |()| {
        if let Some(mainloop) = weak.upgrade() {
            mainloop.quit();
        }
    });

    if ready.send(Ok(())).is_err() {
        // start() gave up waiting; don't run a loop nobody can stop.
        return Ok(());
    }
    mainloop.run();
    let _ = stream.disconnect();
    Ok(())
}

/// Validates the negotiated format and enables the RT callback if it is
/// 48 kHz mono f32. Anything else is a fault: we never record audio we would
/// misinterpret.
fn check_format(shared: &Shared, param: &Pod) {
    let Ok((media_type, media_subtype)) = format_utils::parse_format(param) else {
        return;
    };
    if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
        return;
    }
    let mut info = AudioInfoRaw::new();
    if info.parse(param).is_err() {
        shared.fail("could not parse negotiated audio format".into());
        return;
    }
    let ok =
        info.format() == AudioFormat::F32LE && info.rate() == SAMPLE_RATE && info.channels() == 1;
    if ok {
        info!(
            rate = info.rate(),
            channels = info.channels(),
            "pipewire format negotiated"
        );
        shared.format_ok.store(true, Ordering::Release);
    } else {
        shared.fail(format!(
            "unexpected format {:?} {} Hz {} ch (need F32LE {SAMPLE_RATE} Hz mono)",
            info.format(),
            info.rate(),
            info.channels()
        ));
    }
}

/// EnumFormat pod asking for F32LE, 48 kHz, mono.
fn enum_format_pod() -> std::result::Result<Vec<u8>, String> {
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32LE);
    info.set_rate(SAMPLE_RATE);
    info.set_channels(1);
    let mut position = [0; spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    info.set_position(position);

    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|e| format!("{e:?}"))
}
