//! Meeting notes from the Addis AI LLM (`POST /api/v1/chat_generate`) as a
//! [`NotesProvider`] (FR-AI-01..05). An OQ-3 candidate; see
//! `docs/providers/addis-scribe.md` §8.
//!
//! The transcript goes in as numbered lines with short IDs (`s1`, `s2`, …),
//! which cost fewer tokens and are easier for the model to cite correctly
//! than real segment IDs; they are mapped back afterwards. The API has no
//! JSON mode, so the answer is validated strictly ([`MeetingNotes::parse`])
//! and, if invalid, asked for once more with the exact error. If that fails
//! too, plain prose is kept as a summary-only result rather than nothing.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tracing::{debug, warn};

use crate::addis::{
    CONNECT_TIMEOUT, DEFAULT_BASE_URL, KeySource, classify, network_error, retry_after, unexpected,
};
use crate::keys::ApiKeyStore;
use crate::notes::NotesSegment;
use crate::{ApiKey, LangCode, MeetingNotes, NotesInput, NotesProvider, ProviderError};

pub const PROVIDER_ID: &str = "addis-llm";
/// Notes take longer than transcription: long input, long output.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_OUTPUT_TOKENS: u32 = 4096;
/// Factual extraction: low temperature (Addis docs suggest 0.1–0.3).
const TEMPERATURE: f64 = 0.2;
/// Rough input budget, well inside the 128k-token context. Afaan Oromo uses
/// about 1.5–1.8 tokens per word; 3 characters per token is a safe estimate.
const MAX_INPUT_TOKENS: usize = 100_000;

pub struct AddisNotes {
    http: reqwest::Client,
    base_url: String,
    key: KeySource,
}

impl AddisNotes {
    /// Uses the Addis AI key from `store` (the same key as Scribe).
    pub fn new(store: Box<dyn ApiKeyStore>) -> Result<Self, ProviderError> {
        Self::build(KeySource::Store(store))
    }

    pub fn with_key(key: ApiKey) -> Result<Self, ProviderError> {
        Self::build(KeySource::Fixed(key))
    }

    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_owned();
        self
    }

    fn build(key: KeySource) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("jifar/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ProviderError::permanent(format!("HTTP client setup failed: {e}")))?;
        Ok(Self {
            http,
            base_url: DEFAULT_BASE_URL.to_owned(),
            key,
        })
    }

    async fn ask(
        &self,
        key: &ApiKey,
        prompt: &str,
        language: &LangCode,
    ) -> Result<String, ProviderError> {
        let body = json!({
            "prompt": prompt,
            "target_language": language.as_str(),
            "generation_config": {
                "temperature": TEMPERATURE,
                "maxOutputTokens": MAX_OUTPUT_TOKENS,
            },
        });
        let res = self
            .http
            .post(format!("{}/api/v1/chat_generate", self.base_url))
            .header("x-api-key", key.expose())
            .json(&body)
            .send()
            .await
            .map_err(network_error)?;
        let status = res.status();
        let retry = retry_after(res.headers());
        let text = res.text().await.map_err(network_error)?;
        if status != StatusCode::OK {
            return Err(classify(status, &text, retry)
                .err()
                .unwrap_or_else(|| unexpected(status)));
        }
        answer_text(&text)
    }
}

#[async_trait]
impl NotesProvider for AddisNotes {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    async fn generate(&self, input: &NotesInput) -> Result<MeetingNotes, ProviderError> {
        if input.segments.is_empty() {
            return Err(ProviderError::permanent(
                "there is no transcript to summarize",
            ));
        }
        let (transcript, ids) = numbered_transcript(&input.segments);
        if transcript.len() / 3 > MAX_INPUT_TOKENS {
            return Err(ProviderError::permanent(
                "the transcript is too long for one notes request",
            ));
        }
        let key = self.key.resolve()?;
        let prompt = build_prompt(&transcript, &input.output_language);
        let short_ids: Vec<&str> = ids.keys().map(String::as_str).collect();

        let first = self.ask(&key, &prompt, &input.output_language).await?;
        let reply = match MeetingNotes::parse(extract_json(&first), &short_ids) {
            Ok(notes) => return Ok(map_ids(notes, &ids)),
            Err(problem) => {
                debug!(%problem, "notes reply invalid; asking again");
                let retry_prompt = format!(
                    "{prompt}\n\nYour previous answer was not valid: {problem}\n\
                     Reply again with only the JSON object, exactly in the format described."
                );
                self.ask(&key, &retry_prompt, &input.output_language)
                    .await?
            }
        };
        match MeetingNotes::parse(extract_json(&reply), &short_ids) {
            Ok(notes) => Ok(map_ids(notes, &ids)),
            Err(problem) => degrade(&reply, &problem),
        }
    }
}

/// `[s1] Me 00:03: …` lines, and a map from short ID to the real segment ID.
fn numbered_transcript(segments: &[NotesSegment]) -> (String, HashMap<String, String>) {
    let mut lines = String::new();
    let mut ids = HashMap::new();
    for (i, s) in segments.iter().enumerate() {
        let short = format!("s{}", i + 1);
        let secs = s.start_ms.max(0) / 1000;
        lines.push_str(&format!(
            "[{short}] {} {:02}:{:02}: {}\n",
            s.speaker,
            secs / 60,
            secs % 60,
            s.text.trim()
        ));
        ids.insert(short, s.id.clone());
    }
    (lines, ids)
}

fn language_name(lang: &LangCode) -> &str {
    match lang.as_str() {
        "om" => "Afaan Oromo",
        "am" => "Amharic",
        "en" => "English",
        "ti" => "Tigrinya",
        other => other,
    }
}

fn build_prompt(transcript: &str, output: &LangCode) -> String {
    let lang = language_name(output);
    format!(
        "You write meeting notes from a transcript. \"Me\" is the person who recorded the \
meeting; \"Others\" are the other participants.\n\
\n\
Rules:\n\
- Use only what is in the transcript. Do not invent facts, names, owners or dates.\n\
- Write every text value in {lang}. Keep names and quoted terms as spoken.\n\
- Cite the transcript lines each decision and action item comes from, by their IDs (e.g. \"s12\").\n\
- Give an owner or a due date only if someone actually said one; otherwise use null.\n\
- If there were no decisions or no action items, use an empty list.\n\
\n\
Reply with only one JSON object, no other text, in exactly this format:\n\
{{\"summary\": \"2-4 sentences\", \
\"keyPoints\": [\"…\"], \
\"decisions\": [{{\"text\": \"…\", \"segmentIds\": [\"s3\"]}}], \
\"actionItems\": [{{\"text\": \"…\", \"owner\": null, \"due\": null, \"segmentIds\": [\"s7\"]}}]}}\n\
\n\
Transcript:\n{transcript}"
    )
}

/// The answer text, wherever this endpoint puts it. The docs show both a
/// REST field (`response_text`) and an SDK shape (`choices[0].message`).
fn answer_text(body: &str) -> Result<String, ProviderError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| ProviderError::permanent(format!("unexpected notes response: {e}")))?;
    let scopes = [v.get("data").unwrap_or(&Value::Null), &v];
    let finish = scopes.iter().find_map(|o| {
        o.get("finish_reason")
            .or_else(|| o.pointer("/choices/0/finish_reason"))
            .and_then(Value::as_str)
    });
    if finish == Some("content_filter") {
        return Err(ProviderError::permanent(
            "the notes model declined this transcript (content filter)",
        ));
    }
    scopes
        .iter()
        .find_map(|o| {
            o.get("response_text")
                .or_else(|| o.pointer("/choices/0/message/content"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned)
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| ProviderError::permanent("the notes model returned an empty answer"))
}

/// The JSON object inside a reply that may wrap it in prose or ``` fences.
fn extract_json(reply: &str) -> &str {
    match (reply.find('{'), reply.rfind('}')) {
        (Some(a), Some(b)) if a < b => &reply[a..=b],
        _ => reply.trim(),
    }
}

fn map_ids(mut notes: MeetingNotes, ids: &HashMap<String, String>) -> MeetingNotes {
    let map = |v: &mut Vec<String>| {
        *v = v.iter().filter_map(|s| ids.get(s).cloned()).collect();
    };
    for d in &mut notes.decisions {
        map(&mut d.segment_ids);
    }
    for a in &mut notes.action_items {
        map(&mut a.segment_ids);
    }
    notes
}

/// Two invalid answers: keep prose as a summary-only result (flagged by
/// having no structure), or fail if there is nothing usable.
fn degrade(reply: &str, problem: &str) -> Result<MeetingNotes, ProviderError> {
    let text = reply.trim();
    let looks_like_json = text.starts_with('{') || text.starts_with("```");
    if text.is_empty() || looks_like_json {
        return Err(ProviderError::permanent(format!(
            "the notes model did not return valid notes: {problem}"
        )));
    }
    warn!(%problem, "notes reply still invalid; keeping it as a summary only");
    Ok(MeetingNotes {
        summary: text.to_owned(),
        key_points: Vec::new(),
        decisions: Vec::new(),
        action_items: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(id: &str, speaker: &str, start_ms: i64, text: &str) -> NotesSegment {
        NotesSegment {
            id: id.into(),
            speaker: speaker.into(),
            start_ms,
            text: text.into(),
        }
    }

    #[test]
    fn transcript_lines_use_short_ids() {
        let (t, ids) = numbered_transcript(&[
            seg("mic-0000-0", "Me", 3_400, " Akkam bultan "),
            seg(
                "system-0001-0",
                "Others",
                75_000,
                "Har'a karoora irratti mari'anna.",
            ),
        ]);
        assert_eq!(
            t,
            "[s1] Me 00:03: Akkam bultan\n[s2] Others 01:15: Har'a karoora irratti mari'anna.\n"
        );
        assert_eq!(ids["s2"], "system-0001-0");
    }

    #[test]
    fn prompt_names_the_output_language() {
        let p = build_prompt("[s1] Me 00:00: x\n", &LangCode::new("om").unwrap());
        assert!(p.contains("Write every text value in Afaan Oromo"));
        assert!(p.ends_with("[s1] Me 00:00: x\n"));
    }

    #[test]
    fn finds_the_answer_in_either_response_shape() {
        assert_eq!(answer_text(r#"{"response_text":"hi"}"#).unwrap(), "hi");
        assert_eq!(
            answer_text(r#"{"choices":[{"message":{"content":"yo"},"finish_reason":"stop"}]}"#)
                .unwrap(),
            "yo"
        );
        assert!(matches!(
            answer_text(
                r#"{"choices":[{"message":{"content":""},"finish_reason":"content_filter"}]}"#
            ),
            Err(ProviderError::Permanent { .. })
        ));
        assert!(answer_text(r#"{"response_text":"  "}"#).is_err());
    }

    #[test]
    fn json_is_extracted_from_fences_and_prose() {
        assert_eq!(extract_json("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(extract_json("Here you go: {\"a\":1} done"), "{\"a\":1}");
    }

    #[test]
    fn prose_degrades_to_summary_only_but_broken_json_fails() {
        let n = degrade("Walgahiin kun baajata irratti xiyyeeffate.", "x").unwrap();
        assert!(n.decisions.is_empty() && n.summary.starts_with("Walgahiin"));
        assert!(degrade("{\"summary\": ", "x").is_err());
        assert!(degrade("  ", "x").is_err());
    }
}
