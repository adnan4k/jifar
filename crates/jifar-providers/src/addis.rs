//! Addis Scribe (Addis AI speech-to-text) as an [`AsrProvider`].
//! API notes and decisions: `docs/providers/addis-scribe.md`.
//!
//! One `POST /api/v1/scribe/transcribe` per VAD chunk, with
//! `backend=turbo&language=<meeting language>&timestamps=word` and the job ID
//! as `request_id`. Scribe treats `request_id` as an idempotency key: a
//! retry of the same job returns the original result and is not billed
//! again.
//!
//! Slow requests (past ~75 s on Scribe's side) answer 202 without a
//! transcript. That is returned as `Retryable` and the queue retries later
//! with the same `request_id`: Scribe then returns the finished result (not
//! billed again) or 409 `IN_PROGRESS` (retryable again). Nothing here waits or
//! polls, so a slow request never holds a transcription slot.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use tracing::{debug, warn};

use crate::keys::{ApiKeyStore, KeyStoreError};
use crate::{ActionKind, ApiKey, AsrProvider, AudioChunk, LangCode, ProviderError, RawSegment};

pub const PROVIDER_ID: &str = "addis-scribe";
pub const DEFAULT_BASE_URL: &str = "https://api.addisassistant.com";
/// Languages Scribe accepts (`language` query parameter).
pub const LANGUAGES: [&str; 7] = ["am", "om", "ti", "ha", "sid", "wal", "sw"];

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Scribe answers 202 after ~75 s; leave room for the upload.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Retry delay for a 202 that comes without `Retry-After`.
const PROCESSING_RETRY: Duration = Duration::from_secs(10);
/// Segments may end slightly past the chunk (decoder padding).
const END_SLACK_MS: i64 = 500;

/// Where the provider gets its key. Read on every request, so a key added
/// or fixed in Settings takes effect without a restart.
enum KeySource {
    Store(Box<dyn ApiKeyStore>),
    Fixed(ApiKey),
}

pub struct AddisScribe {
    http: reqwest::Client,
    base_url: String,
    key: KeySource,
}

impl AddisScribe {
    /// Uses the key stored under [`PROVIDER_ID`] in `store`.
    pub fn new(store: Box<dyn ApiKeyStore>) -> Result<Self, ProviderError> {
        Self::build(KeySource::Store(store), DEFAULT_BASE_URL)
    }

    /// Uses `key` directly (tests, CLI).
    pub fn with_key(key: ApiKey) -> Result<Self, ProviderError> {
        Self::build(KeySource::Fixed(key), DEFAULT_BASE_URL)
    }

    /// Points at another server (tests use a local mock).
    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_owned();
        self
    }

    fn build(key: KeySource, base_url: &str) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("jifar/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ProviderError::permanent(format!("HTTP client setup failed: {e}")))?;
        Ok(Self {
            http,
            base_url: base_url.to_owned(),
            key,
        })
    }

    fn api_key(&self) -> Result<ApiKey, ProviderError> {
        match &self.key {
            KeySource::Fixed(k) => Ok(k.clone()),
            KeySource::Store(store) => match store.get(PROVIDER_ID) {
                Ok(Some(k)) => Ok(k),
                Ok(None) => Err(ProviderError::ActionRequired {
                    kind: ActionKind::Auth,
                    message: "no Addis Scribe API key is set".into(),
                }),
                // Locked or missing keyring: the user has to unlock or fix it.
                Err(e @ (KeyStoreError::Unavailable(_) | KeyStoreError::Backend(_))) => {
                    Err(ProviderError::ActionRequired {
                        kind: ActionKind::Auth,
                        message: format!("could not read the API key: {e}"),
                    })
                }
            },
        }
    }
}

#[async_trait]
impl AsrProvider for AddisScribe {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    async fn transcribe(
        &self,
        chunk: &AudioChunk,
        language: &LangCode,
    ) -> Result<Vec<RawSegment>, ProviderError> {
        if !LANGUAGES.contains(&language.as_str()) {
            return Err(ProviderError::permanent(format!(
                "Addis Scribe does not support language {language:?}"
            )));
        }
        let key = self.api_key()?;
        let audio = tokio::fs::read(&chunk.path).await.map_err(|e| {
            ProviderError::permanent(format!("cannot read {}: {e}", chunk.path.display()))
        })?;
        let part = Part::bytes(audio)
            .file_name("chunk.wav")
            .mime_str("audio/wav")
            .map_err(|e| ProviderError::permanent(e.to_string()))?;
        let res = self
            .http
            .post(format!("{}/api/v1/scribe/transcribe", self.base_url))
            .query(&[
                ("backend", "turbo"),
                ("language", language.as_str()),
                ("timestamps", "word"),
                ("request_id", chunk.request_id.as_str()),
            ])
            .header("x-api-key", key.expose())
            .header("x-request-id", request_header(&chunk.request_id))
            .multipart(Form::new().part("audio", part))
            .send()
            .await
            .map_err(network_error)?;

        let status = res.status();
        let retry = retry_after(res.headers());
        let body = res.text().await.map_err(network_error)?;
        let data = match status {
            StatusCode::OK => parse_success(&body)?,
            StatusCode::ACCEPTED => {
                debug!(request_id = %chunk.request_id, "scribe still processing; handing back to the queue");
                return Err(ProviderError::Retryable {
                    message: "Addis Scribe is still processing this chunk".into(),
                    retry_after: Some(retry.unwrap_or(PROCESSING_RETRY)),
                });
            }
            _ => {
                return Err(classify(status, &body, retry)
                    .err()
                    .unwrap_or_else(|| unexpected(status)));
            }
        };
        Ok(to_segments(data, chunk))
    }
}

// ---- Response parsing -----------------------------------------------------

#[derive(Debug, Deserialize)]
struct Envelope {
    data: ScribeData,
}

#[derive(Debug, Deserialize)]
struct ScribeData {
    #[serde(default)]
    text: String,
    #[serde(default)]
    segments: Option<Vec<ScribeSegment>>,
}

#[derive(Debug, Deserialize)]
struct ScribeSegment {
    #[serde(default)]
    text: String,
    start: f64,
    end: f64,
}

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: Option<ErrorBody>,
    /// Some Scribe responses put the code at the top level.
    code: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    code: Option<String>,
    message: Option<String>,
}

fn parse_success(body: &str) -> Result<ScribeData, ProviderError> {
    serde_json::from_str::<Envelope>(body)
        .map(|e| e.data)
        .map_err(|e| {
            // An unexpected shape is likely an API change: retrying won't fix
            // it, but the same request can be recovered later by request_id.
            warn!(error = %e, "unexpected Scribe response");
            ProviderError::permanent(format!("unexpected Scribe response: {e}"))
        })
}

/// Maps Scribe's segments (seconds from the chunk start) onto the recording
/// clock, dropping empty or nonsensical ones and clamping to the chunk.
fn to_segments(data: ScribeData, chunk: &AudioChunk) -> Vec<RawSegment> {
    let len = (chunk.end_ms - chunk.start_ms).max(0);
    let segments = match data.segments {
        Some(s) if !s.is_empty() => s,
        // No timestamps: one segment for the whole chunk (SPEC §6.5).
        _ => {
            let text = data.text.trim();
            return if text.is_empty() {
                Vec::new()
            } else {
                vec![RawSegment {
                    start_ms: chunk.start_ms,
                    end_ms: chunk.end_ms,
                    text: text.to_owned(),
                    confidence: None,
                }]
            };
        }
    };
    segments
        .into_iter()
        .filter_map(|s| {
            let text = s.text.trim();
            if text.is_empty() || !s.start.is_finite() || !s.end.is_finite() {
                return None;
            }
            let start = ((s.start * 1000.0).round() as i64).clamp(0, len);
            let end = ((s.end * 1000.0).round() as i64)
                .clamp(start, len + END_SLACK_MS)
                .min(len);
            Some(RawSegment {
                start_ms: chunk.start_ms + start,
                end_ms: chunk.start_ms + end.max(start),
                text: text.to_owned(),
                confidence: None,
            })
        })
        .collect()
}

// ---- Error classification (docs/providers/addis-scribe.md §5) --------------

fn error_code(body: &str) -> (Option<String>, String) {
    match serde_json::from_str::<ErrorEnvelope>(body) {
        Ok(e) => {
            let code = e.error.as_ref().and_then(|b| b.code.clone()).or(e.code);
            let msg = e
                .error
                .and_then(|b| b.message)
                .or(e.message)
                .unwrap_or_default();
            (code, msg)
        }
        Err(_) => (None, body.chars().take(200).collect()),
    }
}

/// Classifies a non-success response. Returns `Ok(())` only for statuses
/// that aren't errors (callers treat that as unexpected).
fn classify(
    status: StatusCode,
    body: &str,
    retry_after: Option<Duration>,
) -> Result<(), ProviderError> {
    if status.is_success() {
        return Ok(());
    }
    let (code, msg) = error_code(body);
    let code_up = code.as_deref().unwrap_or("").to_ascii_uppercase();
    let describe = |what: &str| {
        let detail = match (&code, msg.is_empty()) {
            (Some(c), false) => format!("{c}: {msg}"),
            (Some(c), true) => c.clone(),
            (None, false) => msg.clone(),
            (None, true) => String::new(),
        };
        if detail.is_empty() {
            format!("{what} (HTTP {})", status.as_u16())
        } else {
            format!("{what} (HTTP {}, {detail})", status.as_u16())
        }
    };
    Err(match status.as_u16() {
        401 | 403 => ProviderError::ActionRequired {
            kind: ActionKind::Auth,
            message: describe("the Addis Scribe API key was rejected"),
        },
        402 => ProviderError::ActionRequired {
            kind: ActionKind::Credits,
            message: describe("the Addis AI balance is too low"),
        },
        409 => match code_up.as_str() {
            "IN_PROGRESS" | "REQUEST_INTERRUPTED" | "BILLING_PENDING" => ProviderError::Retryable {
                message: describe("transcription not finished"),
                retry_after,
            },
            // Same job ID with different audio: a bug on our side.
            _ => {
                warn!(code = %code_up, "scribe request ID conflict");
                ProviderError::permanent(describe("request conflict"))
            }
        },
        408 | 425 | 429 | 500..=599 => ProviderError::Retryable {
            message: describe("Addis Scribe is busy or unavailable"),
            retry_after,
        },
        _ => ProviderError::permanent(describe("request rejected")),
    })
}

fn network_error(e: reqwest::Error) -> ProviderError {
    // Timeouts, DNS, refused connections, dropped uploads: all transient.
    ProviderError::retryable(format!("network error: {e}"))
}

fn unexpected(status: StatusCode) -> ProviderError {
    ProviderError::retryable(format!("unexpected HTTP {}", status.as_u16()))
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|s| Duration::from_secs(s.min(3600)))
}

/// `X-Request-Id` allows ≤ 128 of `[A-Za-z0-9._:-]`.
fn request_header(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
        .take(128)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(start_ms: i64, end_ms: i64) -> AudioChunk {
        AudioChunk {
            request_id: "job-1".into(),
            path: "x.wav".into(),
            start_ms,
            end_ms,
        }
    }

    fn class(status: u16, body: &str) -> ProviderError {
        classify(
            StatusCode::from_u16(status).unwrap(),
            body,
            Some(Duration::from_secs(7)),
        )
        .unwrap_err()
    }

    #[test]
    fn classifies_errors_per_the_provider_notes() {
        let auth = r#"{"status":"error","error":{"code":"invalid_api_key","message":"Invalid or expired API key."}}"#;
        assert!(matches!(
            class(401, auth),
            ProviderError::ActionRequired {
                kind: ActionKind::Auth,
                ..
            }
        ));
        assert!(matches!(
            class(403, "{}"),
            ProviderError::ActionRequired {
                kind: ActionKind::Auth,
                ..
            }
        ));
        assert!(matches!(
            class(402, r#"{"error":{"code":"INSUFFICIENT_CREDITS"}}"#),
            ProviderError::ActionRequired {
                kind: ActionKind::Credits,
                ..
            }
        ));
        for s in [429, 500, 502, 503] {
            assert!(
                matches!(
                    class(s, ""),
                    ProviderError::Retryable {
                        retry_after: Some(_),
                        ..
                    }
                ),
                "{s}"
            );
        }
        assert!(class(409, r#"{"error":{"code":"IN_PROGRESS"}}"#).is_retryable());
        assert!(class(409, r#"{"error":{"code":"REQUEST_INTERRUPTED"}}"#).is_retryable());
        assert!(matches!(
            class(409, r#"{"error":{"code":"IDEMPOTENCY_CONFLICT"}}"#),
            ProviderError::Permanent { .. }
        ));
        for s in [400, 404, 413, 422] {
            assert!(
                matches!(class(s, "{}"), ProviderError::Permanent { .. }),
                "{s}"
            );
        }
        // Messages keep the code for the UI and logs.
        let ProviderError::Permanent { message } = class(
            422,
            r#"{"error":{"code":"UNSUPPORTED_LANGUAGE","message":"bad"}}"#,
        ) else {
            panic!()
        };
        assert!(message.contains("UNSUPPORTED_LANGUAGE"), "{message}");
    }

    #[test]
    fn segments_map_onto_the_recording_clock_and_are_sanitized() {
        let data = ScribeData {
            text: String::new(),
            segments: Some(vec![
                ScribeSegment {
                    text: " Akkam bultan ".into(),
                    start: 0.6,
                    end: 3.1,
                },
                ScribeSegment {
                    text: "".into(),
                    start: 3.2,
                    end: 4.0,
                },
                ScribeSegment {
                    text: "ba'e".into(),
                    start: 9.0,
                    end: 8.0,
                },
                ScribeSegment {
                    text: "dhuma".into(),
                    start: 29.5,
                    end: 31.9,
                },
                ScribeSegment {
                    text: "nan".into(),
                    start: f64::NAN,
                    end: 1.0,
                },
            ]),
        };
        let segs = to_segments(data, &chunk(60_000, 90_000));
        assert_eq!(segs.len(), 3, "{segs:?}");
        assert_eq!((segs[0].start_ms, segs[0].end_ms), (60_600, 63_100));
        assert_eq!(segs[0].text, "Akkam bultan");
        // end < start is clamped, apostrophe kept.
        assert_eq!(
            (segs[1].start_ms, segs[1].end_ms, segs[1].text.as_str()),
            (69_000, 69_000, "ba'e")
        );
        // Never past the chunk.
        assert_eq!(segs[2].end_ms, 90_000);
    }

    #[test]
    fn missing_timestamps_fall_back_to_the_chunk_span() {
        let data = ScribeData {
            text: "Galatoomaa".into(),
            segments: None,
        };
        let segs = to_segments(data, &chunk(1_000, 21_000));
        assert_eq!(segs.len(), 1);
        assert_eq!((segs[0].start_ms, segs[0].end_ms), (1_000, 21_000));
        let empty = ScribeData {
            text: "  ".into(),
            segments: Some(vec![]),
        };
        assert!(to_segments(empty, &chunk(0, 1)).is_empty());
    }

    #[test]
    fn header_values_are_sanitized() {
        assert_eq!(request_header("01a1-b2_c.d:e f/g"), "01a1-b2_c.d:efg");
        assert_eq!(request_header(&"x".repeat(300)).len(), 128);
    }
}
