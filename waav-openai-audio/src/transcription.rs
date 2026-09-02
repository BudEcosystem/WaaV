//! `POST /v1/audio/transcriptions` and `/v1/audio/translations`.
//!
//! Both take multipart form data with an audio file. Translation is transcription with the
//! target language fixed to English, so they share everything except that.

use serde::Serialize;

use crate::AudioError;

/// OpenAI's upload ceiling. Matching it means a caller who sized their chunking against OpenAI
/// does not have to re-tune for a Bud endpoint.
pub const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;

/// Container formats accepted on upload.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "flac", "m4a", "mp3", "mp4", "mpeg", "mpga", "oga", "ogg", "wav", "webm",
];

/// How the transcript should be shaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionResponseFormat {
    Json,
    Text,
    VerboseJson,
    Srt,
    Vtt,
}

impl TranscriptionResponseFormat {
    pub const DEFAULT: Self = Self::Json;

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "json" => Some(Self::Json),
            "text" => Some(Self::Text),
            "verbose_json" => Some(Self::VerboseJson),
            "srt" => Some(Self::Srt),
            "vtt" => Some(Self::Vtt),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Text => "text",
            Self::VerboseJson => "verbose_json",
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Json | Self::VerboseJson => "application/json",
            Self::Text => "text/plain",
            // Subtitle formats are text, but naming them precisely lets a browser or curl
            // save them with the right extension.
            Self::Srt => "application/x-subrip",
            Self::Vtt => "text/vtt",
        }
    }

    /// Whether producing this format needs per-segment timings from the vendor.
    ///
    /// Load-bearing: a vendor that returns only a flat transcript cannot produce SRT or VTT,
    /// and emitting a single subtitle spanning the whole recording would be worse than an
    /// honest refusal.
    pub fn requires_timestamps(self) -> bool {
        matches!(self, Self::Srt | Self::Vtt | Self::VerboseJson)
    }

    fn all() -> &'static str {
        "json, text, verbose_json, srt, vtt"
    }
}

/// The parsed multipart request.
#[derive(Debug, Clone)]
pub struct TranscriptionRequest {
    /// Bud endpoint name.
    pub model: String,
    pub filename: String,
    pub file_len: usize,
    pub response_format: Option<String>,
    /// ISO-639-1. Ignored for translation, which always targets English.
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub temperature: Option<f32>,
    /// True for `/v1/audio/translations`.
    pub translate: bool,
}

/// A translated request, ready for WaaV's STT layer.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptionSettings {
    pub endpoint: String,
    pub response_format: TranscriptionResponseFormat,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub temperature: Option<f32>,
    pub translate: bool,
}

fn extension_of(filename: &str) -> Option<String> {
    filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|e| !e.is_empty())
}

pub fn translate(req: TranscriptionRequest) -> Result<TranscriptionSettings, AudioError> {
    if req.model.trim().is_empty() {
        return Err(AudioError::Missing { field: "model" });
    }
    if req.file_len == 0 {
        return Err(AudioError::Missing { field: "file" });
    }
    if req.file_len > MAX_FILE_BYTES {
        return Err(AudioError::TooLarge {
            field: "file",
            limit: format!("{} bytes", MAX_FILE_BYTES),
            actual: format!("{} bytes", req.file_len),
        });
    }

    // Checked before anything is sent upstream: a vendor rejecting an unknown container
    // produces a far less legible error, after the whole file has been uploaded twice.
    match extension_of(&req.filename) {
        Some(ext) if SUPPORTED_EXTENSIONS.contains(&ext.as_str()) => {}
        Some(ext) => {
            return Err(AudioError::Unsupported {
                field: "file",
                value: ext,
                expected: SUPPORTED_EXTENSIONS.join(", "),
            });
        }
        None => {
            return Err(AudioError::Unsupported {
                field: "file",
                value: req.filename.clone(),
                expected: SUPPORTED_EXTENSIONS.join(", "),
            });
        }
    }

    let response_format = match req.response_format.as_deref() {
        None => TranscriptionResponseFormat::DEFAULT,
        Some(f) => {
            TranscriptionResponseFormat::parse(f).ok_or_else(|| AudioError::Unsupported {
                field: "response_format",
                value: f.to_string(),
                expected: TranscriptionResponseFormat::all().to_string(),
            })?
        }
    };

    let temperature = match req.temperature {
        None => None,
        Some(t) if (0.0..=1.0).contains(&t) => Some(t),
        Some(t) => {
            return Err(AudioError::OutOfRange {
                field: "temperature",
                value: t.to_string(),
                min: "0.0".into(),
                max: "1.0".into(),
            });
        }
    };

    Ok(TranscriptionSettings {
        endpoint: req.model,
        response_format,
        // Translation always targets English, so a source `language` is meaningless and is
        // dropped rather than passed on to be misinterpreted as the target.
        language: if req.translate { None } else { req.language },
        prompt: req.prompt,
        temperature,
        translate: req.translate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> TranscriptionRequest {
        TranscriptionRequest {
            model: "deepgram-stt".into(),
            filename: "meeting.wav".into(),
            file_len: 1024,
            response_format: None,
            language: None,
            prompt: None,
            temperature: None,
            translate: false,
        }
    }

    #[test]
    fn translates_a_minimal_request() {
        let s = translate(req()).unwrap();
        assert_eq!(s.endpoint, "deepgram-stt");
        assert_eq!(s.response_format, TranscriptionResponseFormat::Json);
        assert!(!s.translate);
    }

    #[test]
    fn every_supported_container_is_accepted() {
        for ext in SUPPORTED_EXTENSIONS {
            let r = TranscriptionRequest {
                filename: format!("audio.{ext}"),
                ..req()
            };
            assert!(translate(r).is_ok(), "{ext} should be accepted");
        }
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        for name in ["AUDIO.WAV", "Audio.Mp3", "clip.FLAC"] {
            assert!(
                translate(TranscriptionRequest {
                    filename: name.into(),
                    ..req()
                })
                .is_ok(),
                "{name} should be accepted"
            );
        }
    }

    /// Rejecting here rather than upstream: a vendor's error arrives after the whole file has
    /// been uploaded twice, and says much less.
    #[test]
    fn an_unsupported_container_is_refused_before_upload() {
        let err = translate(TranscriptionRequest {
            filename: "recording.aiff".into(),
            ..req()
        })
        .unwrap_err();
        match err {
            AudioError::Unsupported { field, value, .. } => {
                assert_eq!(field, "file");
                assert_eq!(value, "aiff");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn a_file_with_no_extension_is_refused() {
        for name in ["recording", "archive."] {
            assert!(
                translate(TranscriptionRequest {
                    filename: name.into(),
                    ..req()
                })
                .is_err(),
                "{name} should be refused"
            );
        }
    }

    #[test]
    fn matches_openais_upload_ceiling() {
        assert_eq!(MAX_FILE_BYTES, 25 * 1024 * 1024);

        assert!(
            translate(TranscriptionRequest {
                file_len: MAX_FILE_BYTES,
                ..req()
            })
            .is_ok(),
            "exactly at the limit must be accepted"
        );

        let err = translate(TranscriptionRequest {
            file_len: MAX_FILE_BYTES + 1,
            ..req()
        })
        .unwrap_err();
        assert!(matches!(err, AudioError::TooLarge { field: "file", .. }));
    }

    #[test]
    fn an_empty_upload_is_a_missing_file() {
        let err = translate(TranscriptionRequest {
            file_len: 0,
            ..req()
        })
        .unwrap_err();
        assert_eq!(err, AudioError::Missing { field: "file" });
    }

    #[test]
    fn every_response_format_maps() {
        for (input, expected) in [
            ("json", TranscriptionResponseFormat::Json),
            ("text", TranscriptionResponseFormat::Text),
            ("verbose_json", TranscriptionResponseFormat::VerboseJson),
            ("srt", TranscriptionResponseFormat::Srt),
            ("vtt", TranscriptionResponseFormat::Vtt),
        ] {
            let s = translate(TranscriptionRequest {
                response_format: Some(input.into()),
                ..req()
            })
            .unwrap();
            assert_eq!(s.response_format, expected);
        }
    }

    /// A vendor returning only a flat transcript cannot make subtitles. Emitting one caption
    /// spanning the whole recording would be worse than refusing.
    #[test]
    fn subtitle_formats_declare_their_timestamp_requirement() {
        assert!(TranscriptionResponseFormat::Srt.requires_timestamps());
        assert!(TranscriptionResponseFormat::Vtt.requires_timestamps());
        assert!(TranscriptionResponseFormat::VerboseJson.requires_timestamps());
        assert!(!TranscriptionResponseFormat::Json.requires_timestamps());
        assert!(!TranscriptionResponseFormat::Text.requires_timestamps());
    }

    #[test]
    fn content_types_let_a_client_save_the_right_file() {
        assert_eq!(
            TranscriptionResponseFormat::Json.content_type(),
            "application/json"
        );
        assert_eq!(
            TranscriptionResponseFormat::Text.content_type(),
            "text/plain"
        );
        assert_eq!(TranscriptionResponseFormat::Vtt.content_type(), "text/vtt");
        assert_eq!(
            TranscriptionResponseFormat::Srt.content_type(),
            "application/x-subrip"
        );
    }

    #[test]
    fn temperature_is_bounded() {
        for t in [0.0, 0.5, 1.0] {
            assert!(
                translate(TranscriptionRequest {
                    temperature: Some(t),
                    ..req()
                })
                .is_ok()
            );
        }
        for t in [-0.1, 1.1, 5.0] {
            assert!(matches!(
                translate(TranscriptionRequest {
                    temperature: Some(t),
                    ..req()
                }),
                Err(AudioError::OutOfRange {
                    field: "temperature",
                    ..
                })
            ));
        }
    }

    #[test]
    fn transcription_keeps_the_source_language() {
        let s = translate(TranscriptionRequest {
            language: Some("es".into()),
            translate: false,
            ..req()
        })
        .unwrap();
        assert_eq!(s.language.as_deref(), Some("es"));
    }

    /// Translation always targets English, so a source `language` is meaningless here and must
    /// not be forwarded as if it were the target.
    #[test]
    fn translation_drops_the_language_hint() {
        let s = translate(TranscriptionRequest {
            language: Some("es".into()),
            translate: true,
            ..req()
        })
        .unwrap();
        assert!(s.translate);
        assert_eq!(
            s.language, None,
            "a source language was forwarded on a translation request"
        );
    }

    #[test]
    fn an_unsupported_response_format_names_the_alternatives() {
        let err = translate(TranscriptionRequest {
            response_format: Some("yaml".into()),
            ..req()
        })
        .unwrap_err();
        assert!(err.to_string().contains("verbose_json"));
    }

    #[test]
    fn a_prompt_is_forwarded_for_vendors_that_bias_on_it() {
        let s = translate(TranscriptionRequest {
            prompt: Some("Bud, WaaV, budgateway".into()),
            ..req()
        })
        .unwrap();
        assert_eq!(s.prompt.as_deref(), Some("Bud, WaaV, budgateway"));
    }
}

/// One timed span of transcript.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Segment {
    pub id: u32,
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// What a vendor returned, before it is shaped into a response format.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptionResult {
    pub text: String,
    pub language: Option<String>,
    pub duration: Option<f64>,
    /// Empty when the vendor returns only a flat transcript. Subtitle formats then cannot be
    /// produced, and the caller is told so rather than handed one caption spanning the file.
    pub segments: Vec<Segment>,
}

/// `HH:MM:SS,mmm` (SRT) or `HH:MM:SS.mmm` (WebVTT).
///
/// The separator is the only difference, and getting it wrong produces a file that players
/// silently ignore rather than reject — so it is a parameter here, never a find-and-replace.
fn format_timestamp(seconds: f64, millis_separator: char) -> String {
    let total_ms = (seconds.max(0.0) * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    format!("{h:02}:{m:02}:{s:02}{millis_separator}{ms:03}")
}

impl TranscriptionResult {
    /// SubRip. Cues are 1-indexed and separated by a blank line.
    pub fn to_srt(&self) -> String {
        let mut out = String::new();
        for (i, seg) in self.segments.iter().enumerate() {
            out.push_str(&format!(
                "{}\n{} --> {}\n{}\n\n",
                i + 1,
                format_timestamp(seg.start, ','),
                format_timestamp(seg.end, ','),
                seg.text.trim()
            ));
        }
        out
    }

    /// WebVTT. The `WEBVTT` header is mandatory — a file without it is rejected outright.
    pub fn to_vtt(&self) -> String {
        let mut out = String::from("WEBVTT\n\n");
        for seg in &self.segments {
            out.push_str(&format!(
                "{} --> {}\n{}\n\n",
                format_timestamp(seg.start, '.'),
                format_timestamp(seg.end, '.'),
                seg.text.trim()
            ));
        }
        out
    }
}

#[cfg(test)]
mod subtitle_tests {
    use super::*;

    fn result() -> TranscriptionResult {
        TranscriptionResult {
            text: "Hello there. General Kenobi.".into(),
            language: Some("en".into()),
            duration: Some(5.5),
            segments: vec![
                Segment {
                    id: 0,
                    start: 0.0,
                    end: 2.25,
                    text: " Hello there.".into(),
                },
                Segment {
                    id: 1,
                    start: 2.25,
                    end: 5.5,
                    text: " General Kenobi.".into(),
                },
            ],
        }
    }

    #[test]
    fn srt_uses_a_comma_before_milliseconds() {
        let srt = result().to_srt();
        assert!(srt.contains("00:00:00,000 --> 00:00:02,250"), "got:\n{srt}");
        // Only the CUE lines matter here — transcript text legitimately contains periods.
        for line in srt.lines().filter(|l| l.contains("-->")) {
            assert!(
                !line.contains('.'),
                "SRT timestamps must use a comma before milliseconds: {line}"
            );
        }
    }

    #[test]
    fn vtt_uses_a_dot_and_carries_the_mandatory_header() {
        let vtt = result().to_vtt();
        assert!(
            vtt.starts_with("WEBVTT\n"),
            "a VTT file without the header is rejected outright"
        );
        assert!(vtt.contains("00:00:02.250 --> 00:00:05.500"), "got:\n{vtt}");
    }

    #[test]
    fn srt_cues_are_one_indexed() {
        let srt = result().to_srt();
        assert!(
            srt.starts_with("1\n"),
            "SRT indexing starts at 1, not 0:\n{srt}"
        );
        assert!(srt.contains("\n2\n"));
    }

    #[test]
    fn vtt_cues_are_not_numbered() {
        // Numbering is optional in VTT and its absence is the common form; what matters is
        // that we do not accidentally emit SRT-style indices after the header.
        let vtt = result().to_vtt();
        assert!(
            !vtt.contains("\n1\n00:"),
            "VTT gained SRT-style cue numbers:\n{vtt}"
        );
    }

    #[test]
    fn leading_whitespace_from_the_vendor_is_trimmed() {
        // Whisper-family vendors prefix every segment with a space; leaving it produces
        // visibly indented subtitles.
        assert!(result().to_srt().contains("\nHello there.\n"));
        assert!(result().to_vtt().contains("\nGeneral Kenobi.\n"));
    }

    #[test]
    fn timestamps_carry_hours_for_long_recordings() {
        let long = TranscriptionResult {
            segments: vec![Segment {
                id: 0,
                start: 3661.5,
                end: 3665.0,
                text: "late".into(),
            }],
            ..Default::default()
        };
        assert!(
            long.to_srt().contains("01:01:01,500 --> 01:01:05,000"),
            "got:\n{}",
            long.to_srt()
        );
    }

    #[test]
    fn a_negative_timestamp_clamps_rather_than_underflowing() {
        // Some vendors emit a small negative start on the first segment. Unclamped, the
        // unsigned conversion would wrap to an enormous timestamp.
        let odd = TranscriptionResult {
            segments: vec![Segment {
                id: 0,
                start: -0.2,
                end: 1.0,
                text: "x".into(),
            }],
            ..Default::default()
        };
        assert!(
            odd.to_srt().contains("00:00:00,000 -->"),
            "got:\n{}",
            odd.to_srt()
        );
    }

    #[test]
    fn no_segments_produces_an_empty_body_not_a_panic() {
        let empty = TranscriptionResult::default();
        assert_eq!(empty.to_srt(), "");
        assert_eq!(empty.to_vtt(), "WEBVTT\n\n");
    }

    #[test]
    fn cues_are_blank_line_separated() {
        let srt = result().to_srt();
        assert!(
            srt.contains("Hello there.\n\n2\n"),
            "cues must be separated by a blank line"
        );
    }
}
