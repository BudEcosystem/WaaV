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
    pub voice: String,
    #[serde(default)]
    pub response_format: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
    /// Present in newer OpenAI models; forwarded to vendors that support a free-text style.
    #[serde(default)]
    pub instructions: Option<String>,
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
    /// The vendor's voice identifier. OpenAI names are mapped where an equivalent exists;
    /// everything else passes through unchanged.
    pub voice: String,
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

/// Validate and translate a synthesis request.
pub fn translate(req: SpeechRequest) -> Result<SpeechSettings, AudioError> {
    if req.model.trim().is_empty() {
        return Err(AudioError::Missing { field: "model" });
    }
    if req.input.is_empty() {
        return Err(AudioError::Missing { field: "input" });
    }
    if req.voice.trim().is_empty() {
        return Err(AudioError::Missing { field: "voice" });
    }

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
        voice: req.voice,
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
            voice: "aura-asteria-en".into(),
            response_format: None,
            speed: None,
            instructions: None,
        }
    }

    #[test]
    fn translates_a_minimal_request() {
        let s = translate(req()).unwrap();
        assert_eq!(s.endpoint, "deepgram-tts");
        assert_eq!(s.text, "Hello from Bud.");
        assert_eq!(s.voice, "aura-asteria-en");
        assert_eq!(s.format, AudioFormat::Mp3, "OpenAI defaults to mp3");
        assert_eq!(s.speaking_rate, None);
    }

    #[test]
    fn a_vendor_voice_passes_through_untouched() {
        // Rejecting these would make every vendor's own catalog unreachable.
        for voice in ["aura-asteria-en", "Rachel", "sonic-english", "元気な女性"] {
            let s = translate(SpeechRequest {
                voice: voice.into(),
                ..req()
            })
            .unwrap();
            assert_eq!(s.voice, voice);
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
            (
                Box::new(|r: &mut SpeechRequest| r.voice = "".into()),
                "voice",
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
