//! Contract tests for the Addis AI notes provider against a local mock
//! server, using the documented `chat_generate` REST shape
//! (`response_text`). Swap in recorded live responses once a key exists.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use jifar_providers::notes::NotesSegment;
use jifar_providers::{
    ActionKind, AddisNotes, ApiKey, LangCode, MemoryKeyStore, NotesInput, NotesProvider,
    ProviderError,
};
use serde_json::json;
use wiremock::matchers::{body_partial_json, body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn input() -> NotesInput {
    let om = LangCode::new("om").unwrap();
    NotesInput {
        meeting_language: om.clone(),
        output_language: om,
        segments: vec![
            NotesSegment {
                id: "mic-0000-0".into(),
                speaker: "Me".into(),
                start_ms: 2_000,
                text: "Har'a karoora kurmaana sadaffaa irratti mari'anna.".into(),
            },
            NotesSegment {
                id: "system-0001-0".into(),
                speaker: "Others".into(),
                start_ms: 9_000,
                text: "Caalaan gabaasa baajataa Kamisa dura ni qopheessa.".into(),
            },
        ],
    }
}

fn answer(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "response_text": text }))
}

const NOTES: &str = r#"```json
{"summary": "Walgahiin karoora kurmaana sadaffaa irratti mari'ate.",
 "keyPoints": ["Gabaasni baajataa Kamisa dura qophaa'a."],
 "decisions": [],
 "actionItems": [{"text": "Gabaasa baajataa qopheessuu", "owner": "Caalaa", "due": "Kamisa", "segmentIds": ["s2", "s99"]}]}
```"#;

fn client(server: &MockServer) -> AddisNotes {
    AddisNotes::with_key(ApiKey::new("sk_test_123").unwrap())
        .unwrap()
        .with_base_url(&server.uri())
}

#[tokio::test]
async fn sends_the_transcript_and_maps_citations_back() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/chat_generate"))
        .and(header("x-api-key", "sk_test_123"))
        .and(body_partial_json(json!({
            "target_language": "om",
            "generation_config": { "temperature": 0.2, "maxOutputTokens": 4096 }
        })))
        .and(body_string_contains(
            "[s2] Others 00:09: Caalaan gabaasa baajataa",
        ))
        .and(body_string_contains(
            "Write every text value in Afaan Oromo",
        ))
        .respond_with(answer(NOTES))
        .expect(1)
        .mount(&server)
        .await;

    let notes = client(&server).generate(&input()).await.unwrap();
    assert!(notes.summary.contains("mari'ate"), "apostrophes survive");
    let item = &notes.action_items[0];
    assert_eq!(item.owner.as_deref(), Some("Caalaa"));
    // s2 → the real segment ID; the invented s99 is dropped.
    assert_eq!(item.segment_ids, vec!["system-0001-0"]);
}

#[tokio::test]
async fn invalid_json_is_asked_for_again_with_the_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("Your previous answer was not valid"))
        .respond_with(answer(NOTES))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(answer(r#"{"summary": "x", "keyPoints": "not a list"}"#))
        .expect(1)
        .mount(&server)
        .await;

    let notes = client(&server).generate(&input()).await.unwrap();
    assert_eq!(notes.action_items.len(), 1);
}

#[tokio::test]
async fn persistent_prose_becomes_a_summary_only_result() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(answer(
            "Walgahiin kun karoora fi baajata irratti xiyyeeffate.",
        ))
        .expect(2)
        .mount(&server)
        .await;
    let notes = client(&server).generate(&input()).await.unwrap();
    assert!(notes.summary.starts_with("Walgahiin"));
    assert!(notes.key_points.is_empty() && notes.action_items.is_empty());
}

#[tokio::test]
async fn refusal_bad_key_and_empty_wallet_are_classified() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "" }, "finish_reason": "content_filter" }]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "status": "error", "error": { "code": "invalid_api_key", "message": "Invalid or expired API key." }
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(402).set_body_string("{}"))
        .mount(&server)
        .await;

    let c = client(&server);
    let e = c.generate(&input()).await.unwrap_err();
    assert!(
        matches!(&e, ProviderError::Permanent { message } if message.contains("content filter")),
        "{e}"
    );
    let e = c.generate(&input()).await.unwrap_err();
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
    let e = c.generate(&input()).await.unwrap_err();
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
async fn nothing_is_sent_without_a_transcript_or_a_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(answer(NOTES))
        .expect(0)
        .mount(&server)
        .await;
    let empty = NotesInput {
        segments: vec![],
        ..input()
    };
    assert!(matches!(
        client(&server).generate(&empty).await,
        Err(ProviderError::Permanent { .. })
    ));
    let no_key = AddisNotes::new(Box::new(MemoryKeyStore::default()))
        .unwrap()
        .with_base_url(&server.uri());
    assert!(matches!(
        no_key.generate(&input()).await,
        Err(ProviderError::ActionRequired {
            kind: ActionKind::Auth,
            ..
        })
    ));
}
