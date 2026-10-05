//! Deepgram's prerecorded answer (`POST /v1/listen`), read once for every caller.
//!
//! Callers decide what to keep: the gateway's file transcription keeps every channel, the words,
//! speakers and runner-up hypotheses; a segmented session keeps channel 0's best transcript.

use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prerecorded {
    pub channels: Vec<Channel>,
    /// `metadata.duration`, seconds: the audio Deepgram bills.
    pub duration_secs: Option<f64>,
    /// `metadata.request_id`.
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Channel {
    /// As Deepgram spells it (`en`, `en-US`), when language detection ran.
    pub detected_language: Option<String>,
    /// Best first; the rest are runner-up hypotheses (`alternatives=N`).
    pub alternatives: Vec<Alternative>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Alternative {
    pub transcript: String,
    /// Only what Deepgram reported: absent is not "fully confident".
    pub confidence: Option<f64>,
    /// `None` when the answer carries no `words` array.
    pub words: Option<Vec<Word>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Word {
    /// `punctuated_word` (what `smart_format` produces), else the bare `word`.
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub confidence: Option<f64>,
    /// Deepgram numbers its speakers (diarization).
    pub speaker: Option<u64>,
}

/// Whether a Deepgram model is its hosted Whisper.
pub fn is_whisper(model: &str) -> bool {
    model.trim().to_ascii_lowercase().starts_with("whisper")
}

/// The query parameter a Deepgram model takes key terms in: `keyterm` on Nova-3, `keywords` on
/// Nova-2 and older (an empty model is Deepgram's legacy default), none on Whisper.
pub fn key_terms_param(model: &str) -> Option<&'static str> {
    let lower = model.trim().to_ascii_lowercase();
    if is_whisper(&lower) {
        None
    } else if lower.starts_with("nova-3") {
        Some("keyterm")
    } else {
        Some("keywords")
    }
}

/// Reads `results.channels[].alternatives[]` and `metadata`. An error only when the answer has no
/// `results.channels` list at all.
pub fn parse_prerecorded(body: &Value) -> Result<Prerecorded, String> {
    let channels = body
        .pointer("/results/channels")
        .and_then(Value::as_array)
        .ok_or("no results.channels in the response")?;
    let f64_of = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    Ok(Prerecorded {
        duration_secs: body.pointer("/metadata/duration").and_then(Value::as_f64),
        request_id: body
            .pointer("/metadata/request_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        channels: channels
            .iter()
            .map(|c| Channel {
                detected_language: c
                    .get("detected_language")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                alternatives: c
                    .get("alternatives")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|a| Alternative {
                        transcript: a
                            .get("transcript")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .trim()
                            .to_string(),
                        confidence: f64_of(a, "confidence").filter(|c| c.is_finite()),
                        words: a.get("words").and_then(Value::as_array).map(|ws| {
                            ws.iter()
                                .map(|w| Word {
                                    text: w
                                        .get("punctuated_word")
                                        .or_else(|| w.get("word"))
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                    start: f64_of(w, "start").unwrap_or(0.0),
                                    end: f64_of(w, "end").unwrap_or(0.0),
                                    confidence: f64_of(w, "confidence"),
                                    speaker: w.get("speaker").and_then(Value::as_u64),
                                })
                                .collect()
                        }),
                    })
                    .collect(),
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_terms_go_in_the_parameter_the_model_reads() {
        assert_eq!(key_terms_param("nova-3-medical"), Some("keyterm"));
        assert_eq!(key_terms_param("nova-2"), Some("keywords"));
        assert_eq!(key_terms_param(""), Some("keywords"));
        assert_eq!(key_terms_param("whisper-large"), None);
    }

    #[test]
    fn every_channel_word_and_runner_up_is_read() {
        let p = parse_prerecorded(&serde_json::json!({
            "metadata": {"request_id": " dg-1 ", "duration": 1.25},
            "results": {"channels": [
                {"detected_language": "en", "alternatives": [
                    {"transcript": " hello world ", "confidence": 0.93, "words": [
                        {"word": "hello", "punctuated_word": "Hello", "start": 0.1, "end": 0.4, "confidence": 0.9, "speaker": 0},
                        {"word": "world", "start": 0.5, "end": 0.9}
                    ]},
                    {"transcript": "yellow world"}
                ]},
                {"alternatives": []}
            ]}
        }))
        .unwrap();
        assert_eq!(p.request_id.as_deref(), Some("dg-1"));
        assert_eq!(p.duration_secs, Some(1.25));
        let best = &p.channels[0].alternatives[0];
        assert_eq!(best.transcript, "hello world");
        assert_eq!(best.confidence, Some(0.93));
        let words = best.words.as_ref().unwrap();
        assert_eq!(
            (words[0].text.as_str(), words[0].speaker),
            ("Hello", Some(0))
        );
        assert_eq!(
            (words[1].text.as_str(), words[1].confidence),
            ("world", None)
        );
        assert_eq!(p.channels[0].alternatives[1].transcript, "yellow world");
        assert_eq!(p.channels[0].alternatives[1].words, None);
        assert!(p.channels[1].alternatives.is_empty());
        assert!(parse_prerecorded(&serde_json::json!({"results": {}})).is_err());
    }
}
