//! Meeting notes contract (FR-AI-01..05). The provider itself comes with
//! OQ-3 (M4); this fixes the shapes so the store and UI can be built now.
//!
//! LLM output is untrusted: [`MeetingNotes::parse`] is strict (unknown
//! fields rejected, types exact) and checks that every cited segment exists.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{LangCode, ProviderError};

/// Transcript given to the notes model.
#[derive(Debug, Clone)]
pub struct NotesInput {
    pub meeting_language: LangCode,
    /// Language to write the notes in (FR-AI-03): the meeting's by default.
    pub output_language: LangCode,
    pub segments: Vec<NotesSegment>,
}

#[derive(Debug, Clone)]
pub struct NotesSegment {
    pub id: String,
    /// `"Me"` or `"Others"` (or a renamed speaker).
    pub speaker: String,
    pub start_ms: i64,
    pub text: String,
}

/// `notes.body_json` (SPEC §7.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MeetingNotes {
    pub summary: String,
    pub key_points: Vec<String>,
    pub decisions: Vec<Decision>,
    pub action_items: Vec<ActionItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Decision {
    pub text: String,
    /// Transcript segments it came from (FR-AI-05).
    pub segment_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ActionItem {
    pub text: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub due: Option<String>,
    pub segment_ids: Vec<String>,
}

impl MeetingNotes {
    /// Parses model output strictly and drops citations of segments that
    /// don't exist (models invent IDs). Errors describe what was wrong so the
    /// caller can retry with that feedback.
    pub fn parse(json: &str, known_segments: &[&str]) -> Result<Self, String> {
        let mut notes: Self =
            serde_json::from_str(json).map_err(|e| format!("notes JSON invalid: {e}"))?;
        if notes.summary.trim().is_empty() {
            return Err("notes summary is empty".into());
        }
        let known = |id: &String| known_segments.contains(&id.as_str());
        for d in &mut notes.decisions {
            d.segment_ids.retain(known);
        }
        for a in &mut notes.action_items {
            a.segment_ids.retain(known);
        }
        Ok(notes)
    }
}

#[async_trait]
pub trait NotesProvider: Send + Sync {
    fn id(&self) -> &'static str;
    async fn generate(&self, input: &NotesInput) -> Result<MeetingNotes, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
        "summary": "Walgahiin kun karoora irratti xiyyeeffate.",
        "keyPoints": ["Gabaasni baajataa Jimaata dura xumuramuu qaba."],
        "decisions": [{"text": "Hojiin ji'a itti aanu jalqaba.", "segmentIds": ["s1", "ghost"]}],
        "actionItems": [{"text": "Gabaasa erguu", "owner": "Caalaa", "segmentIds": ["s2"]}]
    }"#;

    #[test]
    fn parses_strictly_and_drops_invented_citations() {
        let n = MeetingNotes::parse(GOOD, &["s1", "s2"]).unwrap();
        assert_eq!(n.decisions[0].segment_ids, vec!["s1"]);
        assert_eq!(n.action_items[0].due, None);
        // Qubee apostrophes survive.
        assert!(n.decisions[0].text.contains("ji'a"));
    }

    #[test]
    fn rejects_unknown_fields_wrong_types_and_empty_summary() {
        assert!(
            MeetingNotes::parse(
                &GOOD.replace("\"summary\"", "\"extra\": 1, \"summary\""),
                &[]
            )
            .is_err()
        );
        assert!(
            MeetingNotes::parse(
                r#"{"summary": 3, "keyPoints": [], "decisions": [], "actionItems": []}"#,
                &[]
            )
            .is_err()
        );
        assert!(
            MeetingNotes::parse(
                r#"{"summary": " ", "keyPoints": [], "decisions": [], "actionItems": []}"#,
                &[]
            )
            .is_err()
        );
    }
}
