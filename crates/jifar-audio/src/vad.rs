//! Voice activity detection for the offline chunker (SPEC §6.3).
//!
//! [`SileroVad`] runs the vendored Silero VAD model on ONNX Runtime. ONNX
//! Runtime is loaded dynamically (`libonnxruntime.so`, or the file named by
//! `ORT_DYLIB_PATH`) the first time a VAD is created. Only the chunker needs
//! it, after recording stops, so a missing library can never affect capture.
//!
//! [`EnergyVad`] is a deterministic level threshold for tests and fixtures.

use std::sync::OnceLock;

use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use crate::resample::CHUNK_SAMPLE_RATE;
use crate::{AudioError, Result};

/// Samples per VAD window at 16 kHz (32 ms). Silero only accepts 512.
pub const VAD_WINDOW: usize = 512;
/// Duration of one VAD window in ms.
pub const VAD_WINDOW_MS: u32 = (VAD_WINDOW as u32 * 1000) / CHUNK_SAMPLE_RATE;

/// Scores 16 kHz mono windows of [`VAD_WINDOW`] samples, in order.
pub trait Vad {
    /// Speech probability (0..=1) of the next window.
    fn speech_prob(&mut self, window: &[f32; VAD_WINDOW]) -> Result<f32>;
    /// Clears internal state before a new, unrelated stream.
    fn reset(&mut self);
}

/// Silero VAD (MIT, snakers4/silero-vad), embedded in the binary.
static SILERO_MODEL: &[u8] = include_bytes!("../models/silero_vad.onnx");
/// Samples of the previous window Silero expects in front of each window.
const CONTEXT: usize = 64;
const STATE_LEN: usize = 2 * 128;

pub struct SileroVad {
    session: Session,
    state: Vec<f32>,
    input: Vec<f32>,
}

impl SileroVad {
    pub fn new() -> Result<Self> {
        let env = environment()?;
        // Small model, run offline: one thread keeps CPU use modest.
        let session = Session::builder(env)
            .map_err(vad_err)?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(vad_err)?
            .with_intra_threads(1)
            .map_err(vad_err)?
            .with_inter_threads(1)
            .map_err(vad_err)?
            .commit_from_memory(SILERO_MODEL)
            .map_err(vad_err)?;
        Ok(Self {
            session,
            state: vec![0.0; STATE_LEN],
            input: vec![0.0; CONTEXT + VAD_WINDOW],
        })
    }
}

impl Vad for SileroVad {
    fn speech_prob(&mut self, window: &[f32; VAD_WINDOW]) -> Result<f32> {
        self.input[CONTEXT..].copy_from_slice(window);
        let input = Tensor::from_array(([1usize, CONTEXT + VAD_WINDOW], self.input.clone()))
            .map_err(vad_err)?;
        let state = Tensor::from_array(([2usize, 1, 128], self.state.clone())).map_err(vad_err)?;
        let sr = Tensor::from_array(((), vec![i64::from(CHUNK_SAMPLE_RATE)])).map_err(vad_err)?;
        let outputs = self
            .session
            .run(ort::inputs!["input" => input, "state" => state, "sr" => sr])
            .map_err(vad_err)?;

        let prob = outputs
            .get("output")
            .ok_or_else(|| AudioError::Vad("model has no `output`".into()))?
            .try_extract_tensor::<f32>()
            .map_err(vad_err)?
            .1
            .first()
            .copied()
            .ok_or_else(|| AudioError::Vad("empty `output`".into()))?;
        let (_, new_state) = outputs
            .get("stateN")
            .ok_or_else(|| AudioError::Vad("model has no `stateN`".into()))?
            .try_extract_tensor::<f32>()
            .map_err(vad_err)?;
        if new_state.len() != STATE_LEN {
            return Err(AudioError::Vad(format!(
                "unexpected state size {}",
                new_state.len()
            )));
        }
        self.state.copy_from_slice(new_state);
        // The tail of this window is the context for the next one.
        self.input.copy_within(VAD_WINDOW.., 0);
        Ok(prob)
    }

    fn reset(&mut self) {
        self.state.fill(0.0);
        self.input.fill(0.0);
    }
}

/// Speech when a window's RMS is at or above `threshold`. Deterministic, for
/// tests; it can't tell speech from music or noise.
pub struct EnergyVad {
    pub threshold: f32,
}

impl Default for EnergyVad {
    fn default() -> Self {
        // About -30 dBFS.
        Self { threshold: 0.03 }
    }
}

impl Vad for EnergyVad {
    fn speech_prob(&mut self, window: &[f32; VAD_WINDOW]) -> Result<f32> {
        let rms = (window.iter().map(|s| s * s).sum::<f32>() / VAD_WINDOW as f32).sqrt();
        Ok(if rms >= self.threshold { 1.0 } else { 0.0 })
    }

    fn reset(&mut self) {}
}

/// The process-wide ONNX Runtime environment. ort allows only one per process,
/// so it is created once and kept; a load failure is cached too.
fn environment() -> Result<&'static Environment> {
    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        let path = std::env::var_os("ORT_DYLIB_PATH")
            .unwrap_or_else(|| "libonnxruntime.so".into());
        ort::init_from(&path)
            .map_err(|e| {
                format!(
                    "cannot load ONNX Runtime from {}: {e} (set ORT_DYLIB_PATH; see scripts/fetch-onnxruntime.sh)",
                    path.to_string_lossy()
                )
            })?
            .with_name("jifar")
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| AudioError::Vad(e.clone()))
}

fn vad_err(e: impl std::fmt::Display) -> AudioError {
    AudioError::Vad(e.to_string())
}
