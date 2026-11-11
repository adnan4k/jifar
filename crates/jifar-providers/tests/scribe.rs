//! Contract tests for the Addis Scribe client against a local mock server,
//! using response shapes from the Addis AI docs
//! (`docs/providers/addis-scribe.md`). Replace the fixtures with recorded
//! live responses once a key is available.
//!
//! The live test at the bottom only runs with `JIFAR_LIVE_TESTS=1`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use jifar_providers::{
    ActionKind, AddisScribe, ApiKey, ApiKeyStore, AsrProvider, AudioChunk, KeyringStore, LangCode,
    MemoryKeyStore, ProviderError,
};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SUCCESS: &str = r#"{
  "data": {
    "text": "Akkam bultan hundi keessan. Har'a karoora irratti mari'anna.",
    "words": [
      { "text": "Akkam", "start": 0.62, "end": 0.98 },
      { "text": "bultan", "start": 1.02, "end": 1.4 }
    ],
    "segments": [
      { "text": "Akkam bultan hundi keessan.", "start": 0.6, "end": 2.3 },
      { "text": "Har'a karoora irratti mari'anna.", "start": 2.9, "end": 5.1 }
    ],
    "seconds": 5.4,
    "compute_ms": 412,
    "usage": { "credits_used": 0.21, "currency": "ETB", "settled": true }
  }
}"#;

fn wav(dir: &tempfile::TempDir) -> std::path::PathBuf {
    // The mock server doesn't decode audio; any bytes will do.
    let p = dir.path().join("mic-0000.wav");
    std::fs::write(&p, b"RIFF\0\0\0\0WAVEfmt fake").unwrap();
    p
}

fn chunk(dir: &tempfile::TempDir) -> AudioChunk {
    AudioChunk {
        request_id: "01a1cba5-b95a-7536-8b7c-f11611596675".into(),
        path: wav(dir),
        start_ms: 120_000,
        end_ms: 126_000,
    }
}

fn om() -> LangCode {
    LangCode::new("om").unwrap()
}

fn client(server: &MockServer) -> AddisScribe {
    AddisScribe::with_key(ApiKey::new("sk_test_123").unwrap())
        .unwrap()
        .with_base_url(&server.uri())
}

#[tokio::test]
async fn sends_the_documented_request_and_maps_segments() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/scribe/transcribe"))
        .and(query_param("backend", "turbo"))
        .and(query_param("language", "om"))
        .and(query_param("timestamps", "word"))
        .and(query_param(
            "request_id",
            "01a1cba5-b95a-7536-8b7c-f11611596675",
        ))
        .and(header("x-api-key", "sk_test_123"))
        .and(header(
            "x-request-id",
            "01a1cba5-b95a-7536-8b7c-f11611596675",
        ))
        .and(body_string_contains("name=\"audio\""))
        .respond_with(ResponseTemplate::new(200).set_body_string(SUCCESS))
        .expect(1)
        .mount(&server)
        .await;

    let segs = client(&server)
        .transcribe(&chunk(&dir), &om())
        .await
        .unwrap();
    assert_eq!(segs.len(), 2);
    // Chunk-relative seconds → recording-clock ms.
    assert_eq!((segs[0].start_ms, segs[0].end_ms), (120_600, 122_300));
    assert_eq!((segs[1].start_ms, segs[1].end_ms), (122_900, 125_100));
    // Qubee apostrophes survive untouched.
    assert_eq!(segs[1].text, "Har'a karoora irratti mari'anna.");
}

#[tokio::test]
async fn slow_requests_go_back_to_the_queue_and_a_retry_gets_the_result() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let id = "01a1cba5-b95a-7536-8b7c-f11611596675";
    // First attempt: Scribe is past ~75 s and answers 202 with no transcript.
    Mock::given(method("POST"))
        .and(path("/api/v1/scribe/transcribe"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Retry-After", "20")
                .set_body_string(format!(
                    r#"{{"request_id":"{id}","status":"processing","poll_url":"/api/v1/scribe/requests/{id}"}}"#
                )),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Second attempt (same request_id): still running.
    Mock::given(method("POST"))
        .and(path("/api/v1/scribe/transcribe"))
        .respond_with(ResponseTemplate::new(409).set_body_string(
            r#"{"status":"error","error":{"code":"IN_PROGRESS","message":"not ready"}}"#,
        ))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Third attempt: Scribe replays the finished result (not billed again).
    Mock::given(method("POST"))
        .and(path("/api/v1/scribe/transcribe"))
        .and(query_param("request_id", id))
        .respond_with(ResponseTemplate::new(200).set_body_string(SUCCESS))
        .mount(&server)
        .await;

    let c = client(&server);
    let e = c.transcribe(&chunk(&dir), &om()).await.unwrap_err();
    assert!(
        matches!(e, ProviderError::Retryable { retry_after: Some(d), .. } if d == Duration::from_secs(20)),
        "{e:?}"
    );
    assert!(
        c.transcribe(&chunk(&dir), &om())
            .await
            .unwrap_err()
            .is_retryable()
    );
    assert_eq!(c.transcribe(&chunk(&dir), &om()).await.unwrap().len(), 2);
    // Nothing polls: only the three POSTs reached the server.
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == wiremock::http::Method::POST)
    );
}

#[tokio::test]
async fn rejected_key_and_empty_wallet_need_the_user() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string(
            r#"{"status":"error","error":{"code":"invalid_api_key","message":"Invalid or expired API key."}}"#,
        ))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(402).set_body_string(
            r#"{"status":"error","error":{"code":"INSUFFICIENT_CREDITS","message":"top up"}}"#,
        ))
        .mount(&server)
        .await;

    let c = client(&server);
    let e = c.transcribe(&chunk(&dir), &om()).await.unwrap_err();
    assert!(
        matches!(
            e,
            ProviderError::ActionRequired {
                kind: ActionKind::Auth,
                ..
            }
        ),
        "{e}"
    );
    let e = c.transcribe(&chunk(&dir), &om()).await.unwrap_err();
    assert!(
        matches!(
            e,
            ProviderError::ActionRequired {
                kind: ActionKind::Credits,
                ..
            }
        ),
        "{e}"
    );
}

#[tokio::test]
async fn busy_service_is_retryable_with_its_retry_after() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "30")
                .set_body_string(
                    r#"{"status":"error","error":{"code":"BUSY","message":"at capacity"}}"#,
                ),
        )
        .mount(&server)
        .await;
    let e = client(&server)
        .transcribe(&chunk(&dir), &om())
        .await
        .unwrap_err();
    assert_eq!(
        e,
        ProviderError::Retryable {
            message: "Addis Scribe is busy or unavailable (HTTP 429, BUSY: at capacity)".into(),
            retry_after: Some(Duration::from_secs(30)),
        }
    );
}

#[tokio::test]
async fn bad_audio_is_permanent() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(422).set_body_string(
            r#"{"status":"error","error":{"code":"INVALID_AUDIO","message":"empty"}}"#,
        ))
        .mount(&server)
        .await;
    let e = client(&server)
        .transcribe(&chunk(&dir), &om())
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::Permanent { .. }), "{e}");
}

#[tokio::test]
async fn nothing_is_sent_without_a_key_or_for_an_unsupported_language() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SUCCESS))
        .expect(0)
        .mount(&server)
        .await;

    let no_key = AddisScribe::new(Box::new(MemoryKeyStore::default()))
        .unwrap()
        .with_base_url(&server.uri());
    let e = no_key.transcribe(&chunk(&dir), &om()).await.unwrap_err();
    assert!(
        matches!(
            e,
            ProviderError::ActionRequired {
                kind: ActionKind::Auth,
                ..
            }
        ),
        "{e}"
    );

    let e = client(&server)
        .transcribe(&chunk(&dir), &LangCode::new("fr").unwrap())
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::Permanent { .. }), "{e}");
}

#[tokio::test]
async fn a_key_added_later_is_picked_up() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .and(header("x-api-key", "sk_added_later"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SUCCESS))
        .mount(&server)
        .await;

    struct Shared(std::sync::Arc<MemoryKeyStore>);
    impl ApiKeyStore for Shared {
        fn get(&self, a: &str) -> Result<Option<ApiKey>, jifar_providers::KeyStoreError> {
            self.0.get(a)
        }
        fn set(&self, a: &str, k: &ApiKey) -> Result<(), jifar_providers::KeyStoreError> {
            self.0.set(a, k)
        }
        fn delete(&self, a: &str) -> Result<(), jifar_providers::KeyStoreError> {
            self.0.delete(a)
        }
    }
    let store = std::sync::Arc::new(MemoryKeyStore::default());
    let c = AddisScribe::new(Box::new(Shared(store.clone())))
        .unwrap()
        .with_base_url(&server.uri());
    assert!(c.transcribe(&chunk(&dir), &om()).await.is_err());
    store
        .set("addis-scribe", &ApiKey::new("sk_added_later").unwrap())
        .unwrap();
    assert_eq!(c.transcribe(&chunk(&dir), &om()).await.unwrap().len(), 2);
}

#[tokio::test]
async fn network_failure_is_retryable() {
    let dir = tempfile::tempdir().unwrap();
    // Nothing listens on port 9 (discard) here.
    let c = AddisScribe::with_key(ApiKey::new("sk_x").unwrap())
        .unwrap()
        .with_base_url("http://127.0.0.1:9");
    let e = c.transcribe(&chunk(&dir), &om()).await.unwrap_err();
    assert!(e.is_retryable(), "{e}");
}

/// Real API. Needs `JIFAR_LIVE_TESTS=1`, a key (`ADDIS_API_KEY` or the
/// keyring entry `jifar/addis-scribe`) and `JIFAR_LIVE_WAV=<16 kHz WAV of
/// Afaan Oromo speech>`. Costs a few ETB cents.
#[tokio::test]
async fn live_transcribes_afaan_oromo() {
    if std::env::var_os("JIFAR_LIVE_TESTS").is_none() {
        eprintln!("skipping live test (set JIFAR_LIVE_TESTS=1)");
        return;
    }
    let key = std::env::var("ADDIS_API_KEY")
        .ok()
        .and_then(|k| ApiKey::new(&k))
        .or_else(|| KeyringStore.get("addis-scribe").ok().flatten())
        .expect("ADDIS_API_KEY or keyring entry jifar/addis-scribe");
    let wav = std::env::var("JIFAR_LIVE_WAV").expect("JIFAR_LIVE_WAV");
    let c = AddisScribe::with_key(key).unwrap();
    let chunk = AudioChunk {
        // A fresh ID per run, so this is never an idempotent replay.
        request_id: format!(
            "jifar-live-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ),
        path: wav.into(),
        start_ms: 0,
        end_ms: 30_000,
    };
    let segs = c.transcribe(&chunk, &om()).await.unwrap();
    for s in &segs {
        println!("[{:>6} – {:>6}] {}", s.start_ms, s.end_ms, s.text);
    }
    assert!(!segs.is_empty(), "no speech recognized");
}
