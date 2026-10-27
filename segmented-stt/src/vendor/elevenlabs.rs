//! ElevenLabs Scribe's answer (`POST /v1/speech-to-text`), read once for every caller.
//!
//! One channel at the top level, or one per entry of `transcripts` when multi-channel was asked
//! for. Callers decide what to keep: the gateway's file transcription returns the vendor's `text`
//! and every word; a segmented session rebuilds the text from word and spacing tokens, so audio
//! events (`(laughter)`) never reach a turn's text, and derives a confidence from the logprobs.

use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcription {
    pub channels: Vec<Channel>,
    /// `transcription_id`, the request id ElevenLabs puts in the body.
    pub transcription_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Channel {
    /// The vendor's own `text`, trimmed; audio events tagged in it when they were asked for.
    pub text: Option<String>,
    /// `words`; `None` when the answer carries no such list.
    pub tokens: Option<Vec<Token>>,
    /// ISO 639-3 as ElevenLabs reports it (`eng`).
    pub language_code: Option<String>,
    pub audio_duration_secs: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Token {
    pub text: String,
    /// `word`, `spacing` or `audio_event`; absent on older answers.
    pub kind: Option<String>,
    pub start: f64,
    pub end: f64,
    pub speaker_id: Option<String>,
    pub logprob: Option<f64>,
}

impl Token {
    /// A word, or a token whose answer does not say what it is.
    pub fn is_word_or_untyped(&self) -> bool {
        self.kind.as_deref().unwrap_or("word") == "word"
    }
}

impl Channel {
    /// The spoken text rebuilt from `word` and `spacing` tokens, whitespace collapsed (removing an
    /// audio event can leave two spacing tokens side by side). `None` without tokens.
    pub fn spoken_text(&self) -> Option<String> {
        let tokens = self.tokens.as_deref().filter(|t| !t.is_empty())?;
        let mut text = String::new();
        for t in tokens {
            if matches!(t.kind.as_deref(), Some("word" | "spacing")) {
                text.push_str(&t.text);
            }
        }
        Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
    }

    /// The mean logprob of the tokens marked `word` that carry one.
    pub fn mean_word_logprob(&self) -> Option<f64> {
        let values: Vec<f64> = self
            .tokens
            .iter()
            .flatten()
            .filter(|t| t.kind.as_deref() == Some("word"))
            .filter_map(|t| t.logprob)
            .collect();
        (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
    }
}

pub fn parse(body: &Value) -> Transcription {
    let channels: Vec<&Value> = match body.get("transcripts").and_then(Value::as_array) {
        Some(list) => list.iter().collect(),
        None => vec![body],
    };
    let f64_of = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    let str_of = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Transcription {
        transcription_id: str_of(body, "transcription_id"),
        channels: channels
            .into_iter()
            .map(|c| Channel {
                text: c
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|t| t.trim().to_string()),
                tokens: c.get("words").and_then(Value::as_array).map(|ws| {
                    ws.iter()
                        .map(|w| Token {
                            text: str_of(w, "text").unwrap_or_default(),
                            kind: str_of(w, "type"),
                            start: f64_of(w, "start").unwrap_or(0.0),
                            end: f64_of(w, "end").unwrap_or(0.0),
                            speaker_id: str_of(w, "speaker_id"),
                            logprob: f64_of(w, "logprob"),
                        })
                        .collect()
                }),
                language_code: str_of(c, "language_code"),
                audio_duration_secs: f64_of(c, "audio_duration_secs"),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_text_rules_and_the_word_logprob_come_from_one_read() {
        let t = parse(&serde_json::json!({
            "transcription_id": "tr-1",
            "language_code": "eng",
            "audio_duration_secs": 2.0,
            "text": " Hello (laughter) world ",
            "words": [
                {"text": "Hello", "type": "word", "start": 0.0, "end": 0.4, "logprob": -0.2, "speaker_id": "speaker_0"},
                {"text": " ", "type": "spacing"},
                {"text": "(laughter)", "type": "audio_event"},
                {"text": " ", "type": "spacing"},
                {"text": "world", "type": "word", "logprob": -0.4}
            ]
        }));
        assert_eq!(t.transcription_id.as_deref(), Some("tr-1"));
        let c = &t.channels[0];
        assert_eq!(c.text.as_deref(), Some("Hello (laughter) world"));
        assert_eq!(c.spoken_text().as_deref(), Some("Hello world"));
        assert!((c.mean_word_logprob().unwrap() + 0.3).abs() < 1e-9);
        assert_eq!(
            c.tokens
                .iter()
                .flatten()
                .filter(|t| t.is_word_or_untyped())
                .count(),
            2
        );
        assert_eq!(c.language_code.as_deref(), Some("eng"));
        assert_eq!(c.audio_duration_secs, Some(2.0));
    }

    #[test]
    fn a_multichannel_answer_is_one_channel_per_transcript() {
        let t =
            parse(&serde_json::json!({"transcripts": [{"text": "a"}, {"text": "b", "words": []}]}));
        assert_eq!(t.channels.len(), 2);
        assert_eq!(t.channels[1].text.as_deref(), Some("b"));
        assert_eq!(t.channels[1].spoken_text(), None);
        assert_eq!(
            t.channels[1].tokens,
            Some(vec![]),
            "an empty list is kept apart from none"
        );
        assert_eq!(t.channels[0].tokens, None);
        assert_eq!(parse(&serde_json::json!({})).channels[0].text, None);
    }
}
