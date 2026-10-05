//! The `/audio/transcriptions` response as OpenAI, Groq, Azure OpenAI and self-hosted
//! OpenAI-compatible servers send it, in one set of types.
//!
//! Lenient by design: only `text` is required. Self-hosted servers send subsets (a segment without
//! an id, a word without timings), Groq adds `x_groq`, `gpt-transcribe` adds `logprobs`,
//! `languages` and `usage`. Each caller derives what it needs, a confidence included, from these.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `response_format: json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionResponse {
    pub text: String,
    /// Groq's metadata; its `id` is the request id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_groq: Option<GroqMetadata>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GroqMetadata {
    #[serde(default)]
    pub id: String,
}

/// `response_format: verbose_json`, and the superset every JSON answer parses into.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VerboseTranscriptionResponse {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<Segment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<Word>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_groq: Option<GroqMetadata>,
    /// Per-token log probabilities (`include[]=logprobs`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logprobs: Vec<TokenLogprob>,
    /// `gpt-transcribe`'s detected languages: objects with a `code`, or bare codes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<Value>,
    /// Billing: `{"type": "duration", "seconds": ...}` on duration-billed models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    #[serde(default)]
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seek: Option<i64>,
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub end: f64,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tokens: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_logprob: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_speech_prob: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub word: String,
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub end: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenLogprob {
    #[serde(default)]
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprob: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
}

impl VerboseTranscriptionResponse {
    /// The first detected language `gpt-transcribe` names (`languages[0].code`, or a bare code).
    pub fn first_listed_language(&self) -> Option<&str> {
        self.languages
            .first()
            .and_then(|l| l.get("code").or(Some(l)))
            .and_then(Value::as_str)
    }

    /// The billed seconds, when the answer is duration-billed.
    pub fn billed_seconds(&self) -> Option<f64> {
        self.usage
            .as_ref()
            .filter(|u| u.kind.as_deref() == Some("duration"))
            .and_then(|u| u.seconds)
    }

    /// The mean token log probability, over the tokens that carry one.
    pub fn mean_token_logprob(&self) -> Option<f64> {
        let values: Vec<f64> = self
            .logprobs
            .iter()
            .filter_map(|l| l.logprob.filter(|v| v.is_finite()))
            .collect();
        (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
    }
}

/// `response_format: diarized_json` (OpenAI's diarizing models).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiarizedTranscriptionResponse {
    pub text: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub speakers: Vec<DiarizedSpeaker>,
    #[serde(default)]
    pub segments: Vec<DiarizedSegment>,
    #[serde(default)]
    pub words: Vec<DiarizedWord>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiarizedSpeaker {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiarizedSegment {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub end: f64,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub speaker: Option<String>,
    #[serde(default)]
    pub avg_logprob: Option<f64>,
    #[serde(default)]
    pub no_speech_prob: Option<f64>,
    #[serde(default)]
    pub tokens: Vec<i64>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub compression_ratio: Option<f64>,
    #[serde(default)]
    pub seek: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiarizedWord {
    pub word: String,
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub end: f64,
    #[serde(default)]
    pub speaker: Option<String>,
    #[serde(default)]
    pub logprob: Option<f64>,
}

/// `{"error": {...}}`: OpenAI's error envelope, which Groq and most compatible servers copy.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub message: String,
    #[serde(default, rename = "type")]
    pub error_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl std::fmt::Display for ErrorBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.error_type)
    }
}

impl std::error::Error for ErrorBody {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_answer_from_a_self_hosted_server_still_parses() {
        let v: VerboseTranscriptionResponse = serde_json::from_str(
            r#"{"text":" hi ","segments":[{"text":"hi","avg_logprob":-0.2}],"words":[{"word":"hi"}]}"#,
        )
        .unwrap();
        assert_eq!(v.segments[0].id, 0);
        assert_eq!(v.segments[0].start, 0.0);
        assert_eq!(v.words[0].end, 0.0);
        assert!(
            serde_json::from_str::<VerboseTranscriptionResponse>(r#"{"segments":[]}"#).is_err()
        );
    }

    #[test]
    fn groq_and_gpt_transcribe_extras_are_read() {
        let v: VerboseTranscriptionResponse = serde_json::from_str(
            r#"{"text":"x","x_groq":{"id":"req_1"},"logprobs":[{"token":"x","logprob":-0.5},{"token":"y","logprob":-1.5},{"token":"z"}],
                "languages":[{"code":"de"}],"usage":{"type":"duration","seconds":2.5}}"#,
        )
        .unwrap();
        assert_eq!(v.x_groq.as_ref().unwrap().id, "req_1");
        assert_eq!(v.mean_token_logprob(), Some(-1.0));
        assert_eq!(v.billed_seconds(), Some(2.5));
        let bare: VerboseTranscriptionResponse =
            serde_json::from_str(r#"{"text":"x","languages":["fr"],"usage":{"type":"tokens"}}"#)
                .unwrap();
        assert_eq!(bare.first_listed_language(), Some("fr"));
        assert_eq!(bare.billed_seconds(), None, "only duration billing counts");
    }

    #[test]
    fn an_error_envelope_reads_with_or_without_its_type() {
        let e: ErrorResponse = serde_json::from_str(
            r#"{"error":{"message":"bad","type":"invalid_request_error","param":"model"}}"#,
        )
        .unwrap();
        assert_eq!(e.error.to_string(), "bad (invalid_request_error)");
        let bare: ErrorResponse = serde_json::from_str(r#"{"error":{"message":"bad"}}"#).unwrap();
        assert_eq!(bare.error.error_type, "");
    }
}
