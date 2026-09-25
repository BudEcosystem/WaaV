//! `POST /v1/audio/speech` — synthesis.

use serde::{Deserialize, Serialize};

use crate::AudioError;

/// Maximum input length.
///
/// budgateway caps this at 4096 characters and WaaV's own `/speak` at 10 KB. The **lower** of
/// the two is the honest limit for a Bud-registered endpoint: a caller who works against
/// budgateway and then switches to a WaaV-backed voice must not discover a different ceiling.
pub const MAX_INPUT_CHARS: usize = 4096;

/// The wire request, matching OpenAI's schema.
#[derive(Debug, Clone, Deserialize)]
pub struct SpeechRequest {
    /// The Bud endpoint name. Resolved against `voice_table` to pick the vendor.
    pub model: String,
    pub input: String,
    /// OPTIONAL since FRD-018 Part III C1, where OpenAI requires it.
    ///
    /// A Bud voice deployment carries a default voice chosen by the operator, published on the
    /// `voice_table` entry. Requiring `voice` on every request would make that default
    /// unreachable — it could only ever be overridden, never used. Omitting it here means "use
    /// the deployment's voice"; the handler still refuses when neither exists, naming the field.
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub response_format: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
    /// Present in newer OpenAI models; forwarded to vendors that support a free-text style.
    #[serde(default)]
    pub instructions: Option<String>,
    /// OpenAI's `audio` (one response body) or `sse` (a stream of events). Only `audio` is
    /// served. Declared rather than left to serde's unknown-field tolerance: an ignored `sse`
    /// answered a caller waiting for events with a binary body and a 200.
    #[serde(default)]
    pub stream_format: Option<String>,
}

/// Output encodings WaaV can return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Mp3,
    Opus,
    Aac,
    Flac,
    Wav,
    Pcm,
}

impl AudioFormat {
    /// OpenAI's default when `response_format` is omitted.
    pub const DEFAULT: Self = Self::Mp3;

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mp3" => Some(Self::Mp3),
            "opus" => Some(Self::Opus),
            "aac" => Some(Self::Aac),
            "flac" => Some(Self::Flac),
            "wav" => Some(Self::Wav),
            // OpenAI names this `pcm`; WaaV's providers call the same thing `linear16`.
            "pcm" | "linear16" => Some(Self::Pcm),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Opus => "opus",
            Self::Aac => "aac",
            Self::Flac => "flac",
            Self::Wav => "wav",
            Self::Pcm => "pcm",
        }
    }

    /// What WaaV's provider layer calls this format.
    pub fn as_waav_format(self) -> &'static str {
        match self {
            Self::Pcm => "linear16",
            other => other.as_str(),
        }
    }

    /// Whether a sample rate may be sent alongside this format.
    ///
    /// Only container-less PCM takes one. A compressed format carries its rate in its own
    /// header, and vendors reject the combination outright — Deepgram answers
    /// `UNSUPPORTED_AUDIO_FORMAT: sample_rate is not applicable when encoding=mp3`. WaaV's
    /// `TTSConfig` defaults to `Some(24000)`, so this has to be cleared deliberately rather
    /// than left to the default.
    pub fn accepts_sample_rate(self) -> bool {
        matches!(self, Self::Pcm)
    }

    /// The `Content-Type` for a response carrying this format.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Opus => "audio/opus",
            Self::Aac => "audio/aac",
            Self::Flac => "audio/flac",
            Self::Wav => "audio/wav",
            // Raw samples with no container, so the sample rate must travel out of band —
            // the handler adds X-Sample-Rate, as WaaV's own /speak already does.
            Self::Pcm => "audio/pcm",
        }
    }

    fn all() -> &'static str {
        "mp3, opus, aac, flac, wav, pcm"
    }
}

/// A translated request, ready for WaaV's TTS layer.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechSettings {
    pub endpoint: String,
    pub text: String,
    /// The vendor's voice identifier as the CALLER gave it, or `None` to take the
    /// deployment's default. Resolution happens in the handler, which is the only place that
    /// can see the endpoint; this crate stays free of control-plane types.
    pub voice: Option<String>,
    pub format: AudioFormat,
    /// 0.25–4.0, OpenAI's range. `None` means the vendor default.
    pub speaking_rate: Option<f32>,
    pub instructions: Option<String>,
}

/// OpenAI's eleven canonical voice names.
///
/// A vendor that does not publish these gets the name forwarded verbatim, and will usually
/// reject it — which is the correct outcome and a legible error, rather than silently
/// substituting a voice the caller did not ask for.
pub const OPENAI_VOICES: &[&str] = &[
    "alloy", "ash", "ballad", "coral", "echo", "fable", "onyx", "nova", "sage", "shimmer", "verse",
];

pub fn is_openai_voice(voice: &str) -> bool {
    OPENAI_VOICES.contains(&voice)
}

/// Check a requested voice against the voices a vendor is known to publish.
///
/// FRD-018 M7 wants a bad voice name to say which voices exist. Forwarding an unknown name to
/// the vendor does not achieve that: the gateway turns a vendor rejection into a 502
/// `api_error` carrying the vendor's wording, so the caller learns that something upstream
/// broke and nothing about what to type instead. Rejecting here makes it a 400 the caller can
/// act on, with the full list in the message.
///
/// `known` empty means WaaV has no catalog for that vendor — self-hosted deployments and any
/// newly added vendor. Those forward verbatim, exactly as before. Validating against an empty
/// list would refuse every voice and turn a missing catalog into an outage for that endpoint.
pub fn validate_voice(voice: &str, known: &[&str]) -> Result<(), AudioError> {
    if known.is_empty() || known.contains(&voice) {
        return Ok(());
    }
    Err(AudioError::Unsupported {
        field: "voice",
        value: voice.to_string(),
        expected: known.join(", "),
    })
}

/// Validate and translate a synthesis request.
pub fn translate(req: SpeechRequest) -> Result<SpeechSettings, AudioError> {
    if req.model.trim().is_empty() {
        return Err(AudioError::Missing { field: "model" });
    }
    // Whitespace counts as missing. It reached the provider, which skips blank text without
    // queueing a request — so nothing ever completed, and the caller waited out the full 30 s
    // synthesis timeout for a 502 that blamed the vendor.
    if req.input.trim().is_empty() {
        return Err(AudioError::Missing { field: "input" });
    }
    // Nothing a voice can say: punctuation, symbols or emoji alone. Vendors answer these
    // inconsistently — ElevenLabs returns an EMPTY clip for "..." (which surfaced as a 502 that
    // blamed the vendor) and a 400 for "👍" — so the one answer is given here, before a vendor
    // call. Letters and digits in any script count; `is_alphanumeric` is Unicode-aware, so
    // Japanese, Hindi and numbers pass.
    if !req.input.chars().any(char::is_alphanumeric) {
        return Err(AudioError::InvalidField {
            field: "input",
            reason: "it contains no letters or digits, so there is nothing to speak".to_string(),
        });
    }
    match req.stream_format.as_deref() {
        None | Some("audio") => {}
        Some(other) => {
            return Err(AudioError::Unsupported {
                field: "stream_format",
                value: other.to_string(),
                expected: "audio".to_string(),
            });
        }
    }
    // An EMPTY voice is treated as absent rather than refused: a form that submits `""` for an
    // untouched field is indistinguishable from one that omitted it, and refusing the first
    // while honouring the second is a distinction no caller can see. The handler refuses when
    // neither the request nor the deployment supplies one.
    let voice = req.voice.filter(|v| !v.trim().is_empty());

    // Characters, not bytes. A byte limit would give a caller writing Japanese roughly a third
    // of the budget for the same text, which is not a limit anybody can reason about.
    let chars = req.input.chars().count();
    if chars > MAX_INPUT_CHARS {
        return Err(AudioError::TooLarge {
            field: "input",
            limit: format!("{MAX_INPUT_CHARS} characters"),
            actual: format!("{chars} characters"),
        });
    }

    let format = match req.response_format.as_deref() {
        None => AudioFormat::DEFAULT,
        Some(f) => AudioFormat::parse(f).ok_or_else(|| AudioError::Unsupported {
            field: "response_format",
            value: f.to_string(),
            expected: AudioFormat::all().to_string(),
        })?,
    };

    let speaking_rate = match req.speed {
        None => None,
        Some(s) if (0.25..=4.0).contains(&s) => Some(s),
        Some(s) => {
            return Err(AudioError::OutOfRange {
                field: "speed",
                value: s.to_string(),
                min: "0.25".into(),
                max: "4.0".into(),
            });
        }
    };

    Ok(SpeechSettings {
        endpoint: req.model,
        text: req.input,
        voice,
        format,
        speaking_rate,
        instructions: req.instructions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> SpeechRequest {
        SpeechRequest {
            model: "deepgram-tts".into(),
            input: "Hello from Bud.".into(),
            voice: Some("aura-asteria-en".into()),
            response_format: None,
            speed: None,
            instructions: None,
            stream_format: None,
        }
    }

    #[test]
    fn whitespace_only_input_is_missing_input() {
        for input in ["   ", "\n\t "] {
            let err = translate(SpeechRequest {
                input: input.into(),
                ..req()
            })
            .unwrap_err();
            assert!(
                matches!(err, AudioError::Missing { field: "input" }),
                "{input:?}: {err:?}"
            );
        }
    }

    #[test]
    fn input_with_nothing_to_speak_is_refused_but_any_script_passes() {
        for input in ["...", "👍", "?!", "—"] {
            let err = translate(SpeechRequest {
                input: input.into(),
                ..req()
            })
            .unwrap_err();
            assert!(
                matches!(err, AudioError::InvalidField { field: "input", .. }),
                "{input:?}: {err:?}"
            );
        }
        for input in ["こんにちは。", "नमस्ते।", "42", "Hi 👍", "[laughs] ok"]
        {
            assert!(
                translate(SpeechRequest {
                    input: input.into(),
                    ..req()
                })
                .is_ok(),
                "{input:?} must pass"
            );
        }
    }

    #[test]
    fn only_the_whole_body_stream_format_is_served() {
        for ok in [None, Some("audio")] {
            assert!(
                translate(SpeechRequest {
                    stream_format: ok.map(str::to_string),
                    ..req()
                })
                .is_ok()
            );
        }
        let err = translate(SpeechRequest {
            stream_format: Some("sse".into()),
            ..req()
        })
        .unwrap_err();
        assert!(
            matches!(
                err,
                AudioError::Unsupported {
                    field: "stream_format",
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn translates_a_minimal_request() {
        let s = translate(req()).unwrap();
        assert_eq!(s.endpoint, "deepgram-tts");
        assert_eq!(s.text, "Hello from Bud.");
        assert_eq!(s.voice.as_deref(), Some("aura-asteria-en"));
        assert_eq!(s.format, AudioFormat::Mp3, "OpenAI defaults to mp3");
        assert_eq!(s.speaking_rate, None);
    }

    #[test]
    fn a_vendor_voice_passes_through_untouched() {
        // Rejecting these would make every vendor's own catalog unreachable.
        for voice in ["aura-asteria-en", "Rachel", "sonic-english", "元気な女性"] {
            let s = translate(SpeechRequest {
                voice: Some(voice.into()),
                ..req()
            })
            .unwrap();
            assert_eq!(s.voice.as_deref(), Some(voice));
        }
    }

    #[test]
    fn openai_voice_names_are_recognised() {
        assert!(is_openai_voice("alloy"));
        assert!(is_openai_voice("verse"));
        assert!(!is_openai_voice("aura-asteria-en"));
        assert_eq!(OPENAI_VOICES.len(), 11);
    }

    #[test]
    fn every_openai_format_maps() {
        for (input, expected) in [
            ("mp3", AudioFormat::Mp3),
            ("opus", AudioFormat::Opus),
            ("aac", AudioFormat::Aac),
            ("flac", AudioFormat::Flac),
            ("wav", AudioFormat::Wav),
            ("pcm", AudioFormat::Pcm),
        ] {
            let s = translate(SpeechRequest {
                response_format: Some(input.into()),
                ..req()
            })
            .unwrap();
            assert_eq!(s.format, expected, "format {input}");
        }
    }

    /// WaaV's providers call raw samples `linear16`; OpenAI calls the same thing `pcm`.
    #[test]
    fn pcm_and_linear16_are_the_same_format() {
        let a = translate(SpeechRequest {
            response_format: Some("pcm".into()),
            ..req()
        })
        .unwrap();
        let b = translate(SpeechRequest {
            response_format: Some("linear16".into()),
            ..req()
        })
        .unwrap();
        assert_eq!(a.format, b.format);
        assert_eq!(a.format.as_waav_format(), "linear16");
        assert_eq!(a.format.as_str(), "pcm", "the wire name stays OpenAI's");
    }

    /// Rule 1. A caller who asked for FLAC and quietly received MP3 has no way to find out.
    #[test]
    fn an_unsupported_format_is_an_error_not_a_substitution() {
        let err = translate(SpeechRequest {
            response_format: Some("ogg".into()),
            ..req()
        })
        .unwrap_err();
        assert!(
            matches!(&err, AudioError::Unsupported { field: "response_format", value, .. } if value == "ogg"),
            "expected Unsupported for response_format, got {err:?}"
        );
        assert!(
            err.to_string().contains("mp3"),
            "the error must list what IS supported"
        );
    }

    #[test]
    fn speed_is_accepted_across_openais_range() {
        for s in [0.25, 1.0, 2.5, 4.0] {
            let out = translate(SpeechRequest {
                speed: Some(s),
                ..req()
            })
            .unwrap();
            assert_eq!(out.speaking_rate, Some(s));
        }
    }

    #[test]
    fn speed_outside_the_range_is_refused_at_both_ends() {
        for s in [0.24, 0.0, -1.0, 4.01, 100.0] {
            let err = translate(SpeechRequest {
                speed: Some(s),
                ..req()
            })
            .unwrap_err();
            assert!(
                matches!(err, AudioError::OutOfRange { field: "speed", .. }),
                "speed {s} should be refused, got {err:?}"
            );
        }
    }

    /// The limit is the LOWER of budgateway's and WaaV's, so switching a Bud endpoint from a
    /// hosted vendor to a WaaV-backed one never moves the ceiling under a working caller.
    #[test]
    fn input_is_capped_at_the_lower_of_the_two_limits() {
        assert_eq!(MAX_INPUT_CHARS, 4096);

        let ok = translate(SpeechRequest {
            input: "a".repeat(MAX_INPUT_CHARS),
            ..req()
        });
        assert!(ok.is_ok(), "exactly at the limit must be accepted");

        let err = translate(SpeechRequest {
            input: "a".repeat(MAX_INPUT_CHARS + 1),
            ..req()
        })
        .unwrap_err();
        assert!(matches!(err, AudioError::TooLarge { field: "input", .. }));
    }

    /// A byte limit would give a caller writing Japanese a third of the budget for the same
    /// text — a limit nobody can reason about from the API docs.
    #[test]
    fn the_limit_counts_characters_not_bytes() {
        // 2000 multi-byte characters: well over 4096 BYTES, well under 4096 characters.
        let text = "あ".repeat(2000);
        assert!(
            text.len() > MAX_INPUT_CHARS,
            "fixture must exceed the byte count"
        );
        assert!(
            translate(SpeechRequest {
                input: text,
                ..req()
            })
            .is_ok(),
            "a byte-based limit rejected valid input"
        );
    }

    #[test]
    fn required_fields_are_named_individually() {
        for (mutate, field) in [
            (
                Box::new(|r: &mut SpeechRequest| r.model = "  ".into())
                    as Box<dyn Fn(&mut SpeechRequest)>,
                "model",
            ),
            (
                Box::new(|r: &mut SpeechRequest| r.input = String::new()),
                "input",
            ),
        ] {
            let mut r = req();
            mutate(&mut r);
            let err = translate(r).unwrap_err();
            assert_eq!(
                err,
                AudioError::Missing { field },
                "a caller can only fix a field the error names"
            );
        }
    }

    /// FRD-018 Part III C1. `voice` moved from required to optional, so the two spellings of
    /// "I did not choose one" have to mean the same thing — otherwise a form that submits an
    /// empty string behaves differently from one that omits the key, and no caller can see why.
    #[test]
    fn an_absent_and_a_blank_voice_are_both_none() {
        for blank in [None, Some(String::new()), Some("   ".to_string())] {
            let s = translate(SpeechRequest {
                voice: blank.clone(),
                ..req()
            })
            .unwrap();
            assert_eq!(
                s.voice, None,
                "{blank:?} should mean 'use the endpoint default'"
            );
        }
    }

    #[test]
    fn a_request_without_a_voice_key_still_deserialises() {
        // The wire-level half of the same change: OpenAI's schema requires `voice`, so a
        // `#[serde(default)]` that went missing would turn every default-voice request into a
        // 400 at parse time, before any of the logic above runs.
        let req: SpeechRequest =
            serde_json::from_str(r#"{"model":"deepgram-tts","input":"hi"}"#).unwrap();
        assert_eq!(req.voice, None);
    }

    /// A vendor rejects the combination outright, so this cannot be left to a default.
    #[test]
    fn only_pcm_accepts_a_sample_rate() {
        assert!(AudioFormat::Pcm.accepts_sample_rate());
        for f in [
            AudioFormat::Mp3,
            AudioFormat::Opus,
            AudioFormat::Aac,
            AudioFormat::Flac,
            AudioFormat::Wav,
        ] {
            assert!(
                !f.accepts_sample_rate(),
                "{f:?} carries its rate in its own header; sending one is a 400 from the vendor"
            );
        }
    }

    #[test]
    fn content_types_are_correct_for_each_format() {
        assert_eq!(AudioFormat::Mp3.content_type(), "audio/mpeg");
        assert_eq!(AudioFormat::Wav.content_type(), "audio/wav");
        assert_eq!(AudioFormat::Flac.content_type(), "audio/flac");
        assert_eq!(AudioFormat::Pcm.content_type(), "audio/pcm");
    }

    #[test]
    fn instructions_are_forwarded_for_vendors_that_take_a_style() {
        let s = translate(SpeechRequest {
            instructions: Some("cheerful and brisk".into()),
            ..req()
        })
        .unwrap();
        assert_eq!(s.instructions.as_deref(), Some("cheerful and brisk"));
    }

    #[test]
    fn deserialises_the_openai_wire_shape() {
        let json =
            r#"{"model":"tts-1","input":"hi","voice":"alloy","response_format":"wav","speed":1.5}"#;
        let parsed: SpeechRequest = serde_json::from_str(json).unwrap();
        let s = translate(parsed).unwrap();
        assert_eq!(s.format, AudioFormat::Wav);
        assert_eq!(s.speaking_rate, Some(1.5));
    }

    #[test]
    fn unknown_wire_fields_are_ignored_rather_than_rejected() {
        // OpenAI adds fields over time; a 400 on an unrecognised one would break callers using
        // a newer SDK against an older WaaV.
        let json = r#"{"model":"tts-1","input":"hi","voice":"alloy","some_future_field":true}"#;
        let parsed: SpeechRequest = serde_json::from_str(json).unwrap();
        assert!(translate(parsed).is_ok());
    }
}

#[cfg(test)]
mod voice_validation_tests {
    use super::*;

    // FRD-018 M7 exit criterion 3: "a bad voice name says which voices exist".
    //
    // Before this, an unknown voice was forwarded verbatim on the assumption the vendor would
    // reject it legibly. In practice the gateway wraps a vendor rejection as a 502 `api_error`
    // carrying the vendor's own wording, so the caller learned that something upstream failed
    // and nothing about what to type instead.

    #[test]
    fn an_unknown_voice_is_rejected_and_names_the_alternatives() {
        let err = validate_voice("aloy", OPENAI_VOICES).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("aloy"),
            "must quote what the caller actually sent: {msg}"
        );
        assert!(msg.contains("alloy"), "must list the real voices: {msg}");
        assert!(
            msg.contains("shimmer"),
            "must list ALL of them, not a sample: {msg}"
        );
    }

    #[test]
    fn a_known_voice_passes() {
        for v in OPENAI_VOICES {
            assert!(
                validate_voice(v, OPENAI_VOICES).is_ok(),
                "{v} should be accepted"
            );
        }
    }

    #[test]
    fn an_empty_known_set_forwards_verbatim() {
        // A vendor whose voice list WaaV does not know must keep working. Rejecting against an
        // empty set would refuse every voice for self-hosted and any newly added vendor —
        // turning a missing catalog into a total outage for that endpoint.
        assert!(validate_voice("anything-at-all", &[]).is_ok());
    }

    #[test]
    fn the_error_is_a_client_error_shape_not_an_upstream_one() {
        // Unsupported is what `response_format` already uses, and the handler maps it to 400.
        // The whole point is that this stops being a 502: the caller can fix it themselves.
        let err = validate_voice("nope", OPENAI_VOICES).unwrap_err();
        assert!(matches!(
            err,
            AudioError::Unsupported { field: "voice", .. }
        ));
    }

    #[test]
    fn matching_is_exact_not_fuzzy() {
        // "Alloy" and " alloy" are mistakes worth naming rather than silently accepting; a
        // caller who gets a quiet pass here would be surprised by a vendor rejection later.
        assert!(validate_voice("Alloy", OPENAI_VOICES).is_err());
        assert!(validate_voice(" alloy", OPENAI_VOICES).is_err());
    }
}
