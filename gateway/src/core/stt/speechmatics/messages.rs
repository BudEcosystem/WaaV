//! WebSocket message types for Speechmatics Realtime STT API.
//!
//! This module contains all message types for communication with the
//! Speechmatics WebSocket API, including:
//!
//! - **Outgoing messages** (Client to Server):
//!   - [`StartRecognitionMessage`]: Initialize session with config
//!   - [`EndOfStreamMessage`]: Signal end of audio stream
//!   - Binary audio data (sent directly)
//!
//! - **Incoming messages** (Server to Client):
//!   - [`RecognitionStartedMessage`]: Session confirmation
//!   - [`AudioAddedMessage`]: Audio chunk acknowledgment
//!   - [`AddPartialTranscriptMessage`]: Interim transcription results
//!   - [`AddTranscriptMessage`]: Final transcription results
//!   - [`EndOfTranscriptMessage`]: Session end confirmation
//!   - [`ErrorMessage`]: Error responses

use serde::{Deserialize, Serialize};

use super::config::{SpeechmaticsEncoding, SpeechmaticsLanguage, SpeechmaticsOperatingPoint};

// =============================================================================
// Outgoing Messages (Client to Server)
// =============================================================================

/// Audio format configuration for the StartRecognition message.
#[derive(Debug, Clone, Serialize)]
pub struct AudioFormat {
    /// Audio type: "raw" for PCM or "file" for encoded formats
    #[serde(rename = "type")]
    pub format_type: String,
    /// Audio encoding (only for raw type)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    /// Sample rate in Hz (only for raw type)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

impl AudioFormat {
    /// Create a raw audio format configuration
    pub fn raw(encoding: SpeechmaticsEncoding, sample_rate: u32) -> Self {
        Self {
            format_type: "raw".to_string(),
            encoding: Some(encoding.to_string()),
            sample_rate: Some(sample_rate),
        }
    }

    /// Create a file-based audio format (auto-detected)
    pub fn file() -> Self {
        Self {
            format_type: "file".to_string(),
            encoding: None,
            sample_rate: None,
        }
    }
}

/// Speaker diarization configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SpeakerDiarizationConfig {
    /// Maximum number of speakers to detect
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_speakers: Option<u8>,
    /// Sensitivity of speaker detection (0.0-1.0). Higher detects more distinct speakers.
    /// Speechmatics `speaker_diarization_config.speaker_sensitivity`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_sensitivity: Option<f32>,
    /// Bias the diarizer toward attributing ambiguous words to the current speaker.
    /// Speechmatics `speaker_diarization_config.prefer_current_speaker`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefer_current_speaker: Option<bool>,
}

/// Conversation/turn-detection configuration.
#[derive(Debug, Clone, Serialize)]
pub struct ConversationConfig {
    /// Seconds of silence after speech before an end-of-turn is triggered.
    /// Speechmatics `conversation_config.end_of_utterance_silence_trigger`.
    pub end_of_utterance_silence_trigger: f32,
}

/// Punctuation behaviour overrides.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PunctuationOverrides {
    /// The set of punctuation marks the recognizer is permitted to emit
    /// (Speechmatics `punctuation_overrides.permitted_marks`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permitted_marks: Option<Vec<String>>,
    /// Punctuation sensitivity (0.0-1.0). Higher inserts more punctuation.
    /// Speechmatics `punctuation_overrides.sensitivity`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensitivity: Option<f32>,
}

/// A single find-and-replace rule for the transcript.
#[derive(Debug, Clone, Serialize)]
pub struct Replacement {
    /// The literal/pattern to match (Speechmatics `from`).
    pub from: String,
    /// The replacement text (Speechmatics `to`).
    pub to: String,
}

/// Transcript post-filtering configuration (disfluency removal + word replacements).
#[derive(Debug, Clone, Default, Serialize)]
pub struct TranscriptFilteringConfig {
    /// Remove disfluencies ("uh", "um", false starts) from the transcript.
    /// Speechmatics `transcript_filtering_config.remove_disfluencies`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove_disfluencies: Option<bool>,
    /// Word/phrase find-and-replace rules.
    /// Speechmatics `transcript_filtering_config.replacements`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacements: Option<Vec<Replacement>>,
}

/// Custom vocabulary word with optional phonetic hints.
#[derive(Debug, Clone, Serialize)]
pub struct AdditionalVocabWord {
    /// The word or phrase to add
    pub content: String,
    /// Optional phonetic hints
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sounds_like: Option<Vec<String>>,
}

impl AdditionalVocabWord {
    /// Create a simple vocabulary word
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            sounds_like: None,
        }
    }

    /// Create a vocabulary word with phonetic hints
    pub fn with_sounds_like(content: impl Into<String>, hints: Vec<String>) -> Self {
        Self {
            content: content.into(),
            sounds_like: Some(hints),
        }
    }
}

/// Transcription configuration for StartRecognition.
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptionConfig {
    /// Language code (e.g., "en", "fr", "auto")
    pub language: String,
    /// Operating point: "standard" or "enhanced"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operating_point: Option<String>,
    /// Enable partial (interim) transcripts
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_partials: Option<bool>,
    /// Maximum delay in seconds (0.0-10.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_delay: Option<f32>,
    /// Enable entity recognition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_entities: Option<bool>,
    /// Diarization mode: "none", "speaker", "channel", "channel_and_speaker"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diarization: Option<String>,
    /// Speaker diarization settings
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_diarization_config: Option<SpeakerDiarizationConfig>,
    /// Custom vocabulary
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_vocab: Option<Vec<AdditionalVocabWord>>,
    /// Conversation/turn-detection configuration (end-of-utterance silence trigger)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_config: Option<ConversationConfig>,
    /// Transcript post-filtering (disfluency removal + find-and-replace)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_filtering_config: Option<TranscriptFilteringConfig>,
    /// Punctuation overrides (permitted marks + sensitivity)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub punctuation_overrides: Option<PunctuationOverrides>,
    /// Output locale (regional spelling/formatting variant, e.g. "en-US")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_locale: Option<String>,
    /// Domain language pack (e.g. "finance", "bilingual-en")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Max-delay mode: "flexible" or "fixed"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_delay_mode: Option<String>,
}

impl TranscriptionConfig {
    /// Create a new transcription configuration with required fields
    pub fn new(language: SpeechmaticsLanguage) -> Self {
        Self {
            language: language.to_string(),
            operating_point: None,
            enable_partials: None,
            max_delay: None,
            enable_entities: None,
            diarization: None,
            speaker_diarization_config: None,
            additional_vocab: None,
            conversation_config: None,
            transcript_filtering_config: None,
            punctuation_overrides: None,
            output_locale: None,
            domain: None,
            max_delay_mode: None,
        }
    }

    /// Set the operating point
    pub fn with_operating_point(mut self, op: SpeechmaticsOperatingPoint) -> Self {
        self.operating_point = Some(op.to_string());
        self
    }

    /// Enable or disable partial transcripts
    pub fn with_partials(mut self, enable: bool) -> Self {
        self.enable_partials = Some(enable);
        self
    }

    /// Set the maximum delay
    pub fn with_max_delay(mut self, delay: f32) -> Self {
        self.max_delay = Some(delay.clamp(0.0, 10.0));
        self
    }

    /// Enable speaker diarization
    pub fn with_diarization(mut self, max_speakers: Option<u8>) -> Self {
        self.diarization = Some("speaker".to_string());
        if let Some(max) = max_speakers {
            self.speaker_diarization_config = Some(SpeakerDiarizationConfig {
                max_speakers: Some(max.clamp(1, 20)),
                ..Default::default()
            });
        }
        self
    }

    /// Add custom vocabulary
    pub fn with_vocab(mut self, words: Vec<String>) -> Self {
        self.additional_vocab = Some(words.into_iter().map(AdditionalVocabWord::new).collect());
        self
    }

    /// Add custom vocabulary as already-structured words (carrying optional `sounds_like` hints).
    pub fn with_vocab_words(mut self, words: Vec<AdditionalVocabWord>) -> Self {
        if !words.is_empty() {
            self.additional_vocab = Some(words);
        }
        self
    }

    /// Set the diarization mode verbatim ("speaker", "channel", "channel_and_speaker", "none").
    pub fn with_diarization_mode(mut self, mode: impl Into<String>) -> Self {
        self.diarization = Some(mode.into());
        self
    }

    /// Set the speaker diarization sub-config (sensitivity / prefer-current-speaker / max-speakers).
    pub fn with_speaker_diarization_config(mut self, cfg: SpeakerDiarizationConfig) -> Self {
        self.speaker_diarization_config = Some(cfg);
        self
    }

    /// Set the end-of-utterance silence trigger (seconds) — turn detection.
    pub fn with_end_of_utterance_silence_trigger(mut self, seconds: f32) -> Self {
        self.conversation_config = Some(ConversationConfig {
            end_of_utterance_silence_trigger: seconds,
        });
        self
    }

    /// Set the transcript filtering config (disfluency removal + replacements).
    pub fn with_transcript_filtering_config(mut self, cfg: TranscriptFilteringConfig) -> Self {
        self.transcript_filtering_config = Some(cfg);
        self
    }

    /// Set the punctuation overrides (permitted marks + sensitivity).
    pub fn with_punctuation_overrides(mut self, overrides: PunctuationOverrides) -> Self {
        self.punctuation_overrides = Some(overrides);
        self
    }

    /// Set the output locale (regional variant).
    pub fn with_output_locale(mut self, locale: impl Into<String>) -> Self {
        self.output_locale = Some(locale.into());
        self
    }

    /// Set the domain language pack.
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Set the max-delay mode ("flexible" | "fixed").
    pub fn with_max_delay_mode(mut self, mode: impl Into<String>) -> Self {
        self.max_delay_mode = Some(mode.into());
        self
    }
}

/// P5 translation config — the `translation_config` PEER object (a sibling of
/// `transcription_config` on `StartRecognition`, NOT nested inside it). Speechmatics emits
/// `AddTranslation`/`AddPartialTranslation` messages for these targets.
#[derive(Debug, Clone, Serialize)]
pub struct SpeechmaticsTranslationConfig {
    /// Target languages (ISO-639-1). Speechmatics accepts at most 5.
    pub target_languages: Vec<String>,
    /// Emit partial (interim) translations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_partials: Option<bool>,
}

/// StartRecognition message to initialize a transcription session.
#[derive(Debug, Clone, Serialize)]
pub struct StartRecognitionMessage {
    /// Message type identifier
    pub message: String,
    /// Audio format configuration
    pub audio_format: AudioFormat,
    /// Transcription configuration
    pub transcription_config: TranscriptionConfig,
    /// P5 translation: the `translation_config` PEER object (sibling of `transcription_config`).
    /// Omitted when no translation was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translation_config: Option<SpeechmaticsTranslationConfig>,
}

impl StartRecognitionMessage {
    /// Create a new StartRecognition message
    pub fn new(
        encoding: SpeechmaticsEncoding,
        sample_rate: u32,
        language: SpeechmaticsLanguage,
    ) -> Self {
        Self {
            message: "StartRecognition".to_string(),
            audio_format: AudioFormat::raw(encoding, sample_rate),
            transcription_config: TranscriptionConfig::new(language),
            translation_config: None,
        }
    }

    /// Create with full configuration
    pub fn with_config(
        audio_format: AudioFormat,
        transcription_config: TranscriptionConfig,
    ) -> Self {
        Self {
            message: "StartRecognition".to_string(),
            audio_format,
            transcription_config,
            translation_config: None,
        }
    }

    /// P5: attach the `translation_config` peer object (no-op when `target_languages` is empty).
    pub fn with_translation(
        mut self,
        target_languages: Vec<String>,
        enable_partials: Option<bool>,
    ) -> Self {
        if !target_languages.is_empty() {
            self.translation_config = Some(SpeechmaticsTranslationConfig {
                target_languages,
                enable_partials,
            });
        }
        self
    }
}

/// EndOfStream message to signal the end of audio input.
#[derive(Debug, Clone, Serialize)]
pub struct EndOfStreamMessage {
    /// Message type identifier
    pub message: String,
    /// Last sequence number sent
    pub last_seq_no: u64,
}

impl EndOfStreamMessage {
    /// Create a new EndOfStream message
    pub fn new(last_seq_no: u64) -> Self {
        Self {
            message: "EndOfStream".to_string(),
            last_seq_no,
        }
    }
}

// =============================================================================
// Incoming Messages (Server to Client)
// =============================================================================

/// RecognitionStarted message confirming session initialization.
#[derive(Debug, Clone, Deserialize)]
pub struct RecognitionStartedMessage {
    /// Message type identifier ("RecognitionStarted")
    pub message: String,
    /// Session ID
    #[serde(default)]
    pub id: Option<String>,
}

/// AudioAdded message acknowledging received audio.
#[derive(Debug, Clone, Deserialize)]
pub struct AudioAddedMessage {
    /// Message type identifier ("AudioAdded")
    pub message: String,
    /// Sequence number of the acknowledged audio chunk
    pub seq_no: u64,
}

/// Word alternative with confidence score.
#[derive(Debug, Clone, Deserialize)]
pub struct WordAlternative {
    /// The word text
    pub content: String,
    /// Confidence score (0.0-1.0)
    pub confidence: f64,
    /// Language code (for multilingual)
    #[serde(default)]
    pub language: Option<String>,
    /// Speaker label (for diarization)
    #[serde(default)]
    pub speaker: Option<String>,
}

/// Transcript result element (word, punctuation, or entity).
#[derive(Debug, Clone, Deserialize)]
pub struct TranscriptResult {
    /// Result type: "word", "punctuation", or "entity"
    #[serde(rename = "type")]
    pub result_type: String,
    /// Start time in seconds from audio start
    pub start_time: f64,
    /// End time in seconds from audio start
    pub end_time: f64,
    /// Alternatives with confidence scores
    #[serde(default)]
    pub alternatives: Vec<WordAlternative>,
    /// Whether this is an end-of-sentence marker
    #[serde(default)]
    pub is_eos: Option<bool>,
    /// Attached tokens (for punctuation)
    #[serde(default)]
    pub attaches_to: Option<String>,
}

impl TranscriptResult {
    /// Get the best (highest confidence) content
    pub fn content(&self) -> Option<&str> {
        self.alternatives.first().map(|a| a.content.as_str())
    }

    /// Get the confidence score of the best alternative
    pub fn confidence(&self) -> f64 {
        self.alternatives
            .first()
            .map(|a| a.confidence)
            .unwrap_or(0.0)
    }

    /// Check if this is a word result
    pub fn is_word(&self) -> bool {
        self.result_type == "word"
    }

    /// Check if this is a punctuation result
    pub fn is_punctuation(&self) -> bool {
        self.result_type == "punctuation"
    }
}

/// Transcript metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct TranscriptMetadata {
    /// Start time of the transcript segment
    pub start_time: f64,
    /// End time of the transcript segment
    pub end_time: f64,
    /// Full transcript text
    pub transcript: String,
}

/// AddPartialTranscript message with interim results.
#[derive(Debug, Clone, Deserialize)]
pub struct AddPartialTranscriptMessage {
    /// Message type identifier ("AddPartialTranscript")
    pub message: String,
    /// API format version
    #[serde(default)]
    pub format: Option<String>,
    /// Transcript metadata
    pub metadata: TranscriptMetadata,
    /// Detailed results with word timing
    #[serde(default)]
    pub results: Vec<TranscriptResult>,
}

impl AddPartialTranscriptMessage {
    /// Get the transcript text
    pub fn transcript(&self) -> &str {
        &self.metadata.transcript
    }

    /// Check if this is a partial (interim) result
    pub fn is_partial(&self) -> bool {
        true
    }
}

/// AddTranscript message with final results.
#[derive(Debug, Clone, Deserialize)]
pub struct AddTranscriptMessage {
    /// Message type identifier ("AddTranscript")
    pub message: String,
    /// API format version
    #[serde(default)]
    pub format: Option<String>,
    /// Transcript metadata
    pub metadata: TranscriptMetadata,
    /// Detailed results with word timing
    #[serde(default)]
    pub results: Vec<TranscriptResult>,
}

impl AddTranscriptMessage {
    /// Get the transcript text
    pub fn transcript(&self) -> &str {
        &self.metadata.transcript
    }

    /// Check if this is a final result
    pub fn is_final(&self) -> bool {
        true
    }

    /// Get words only (excluding punctuation)
    pub fn words(&self) -> impl Iterator<Item = &TranscriptResult> {
        self.results.iter().filter(|r| r.is_word())
    }
}

/// One translated segment within an `AddTranslation`/`AddPartialTranslation` frame.
///
/// Speechmatics emits a `results` array per target language; each element carries
/// the translated `content` plus timing. The gateway concatenates the `content`
/// values (in order) into the single translated string for that language.
#[derive(Debug, Clone, Deserialize)]
pub struct TranslationResult {
    /// Start time in seconds from audio start.
    #[serde(default)]
    pub start_time: f64,
    /// End time in seconds from audio start.
    #[serde(default)]
    pub end_time: f64,
    /// The translated text fragment for this segment.
    #[serde(default)]
    pub content: String,
    /// Speaker label (for diarization), when present.
    #[serde(default)]
    pub speaker: Option<String>,
}

/// `AddTranslation` (final) / `AddPartialTranslation` (interim) message.
///
/// The `language` field is the ISO-639-1 target code Speechmatics translated to
/// (matching what the gateway requested via `translation_config.target_languages`).
/// The translated string is `results[].content` joined in order. P5.
#[derive(Debug, Clone, Deserialize)]
pub struct AddTranslationMessage {
    /// Message type identifier ("AddTranslation" or "AddPartialTranslation").
    pub message: String,
    /// API format version.
    #[serde(default)]
    pub format: Option<String>,
    /// The target language this translation is in (ISO-639-1, e.g. "de", "es").
    pub language: String,
    /// Translated result segments.
    #[serde(default)]
    pub results: Vec<TranslationResult>,
}

impl AddTranslationMessage {
    /// The full translated text for this frame (`results[].content` joined).
    pub fn text(&self) -> String {
        self.results
            .iter()
            .map(|r| r.content.as_str())
            .collect::<Vec<_>>()
            .join("")
    }

    /// `true` for `AddPartialTranslation` (interim), `false` for `AddTranslation` (final).
    pub fn is_partial(&self) -> bool {
        self.message == "AddPartialTranslation"
    }
}

/// EndOfTranscript message signaling session completion.
#[derive(Debug, Clone, Deserialize)]
pub struct EndOfTranscriptMessage {
    /// Message type identifier ("EndOfTranscript")
    pub message: String,
}

/// Error message from Speechmatics.
#[derive(Debug, Clone, Deserialize)]
pub struct ErrorMessage {
    /// Message type identifier ("Error")
    pub message: String,
    /// Error type/code
    #[serde(rename = "type")]
    pub error_type: String,
    /// Human-readable error reason
    pub reason: String,
}

impl std::fmt::Display for ErrorMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Speechmatics error [{}]: {}",
            self.error_type, self.reason
        )
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_format_raw() {
        let format = AudioFormat::raw(SpeechmaticsEncoding::PcmS16le, 16000);
        assert_eq!(format.format_type, "raw");
        assert_eq!(format.encoding, Some("pcm_s16le".to_string()));
        assert_eq!(format.sample_rate, Some(16000));
    }

    #[test]
    fn test_audio_format_file() {
        let format = AudioFormat::file();
        assert_eq!(format.format_type, "file");
        assert!(format.encoding.is_none());
        assert!(format.sample_rate.is_none());
    }

    #[test]
    fn test_transcription_config_new() {
        let config = TranscriptionConfig::new(SpeechmaticsLanguage::English);
        assert_eq!(config.language, "en");
        assert!(config.operating_point.is_none());
    }

    #[test]
    fn test_transcription_config_builder() {
        let config = TranscriptionConfig::new(SpeechmaticsLanguage::French)
            .with_operating_point(SpeechmaticsOperatingPoint::Enhanced)
            .with_partials(true)
            .with_max_delay(1.5)
            .with_diarization(Some(4))
            .with_vocab(vec!["custom".to_string(), "words".to_string()]);

        assert_eq!(config.language, "fr");
        assert_eq!(config.operating_point, Some("enhanced".to_string()));
        assert_eq!(config.enable_partials, Some(true));
        assert_eq!(config.max_delay, Some(1.5));
        assert_eq!(config.diarization, Some("speaker".to_string()));
        assert!(config.speaker_diarization_config.is_some());
        assert!(config.additional_vocab.is_some());
    }

    #[test]
    fn test_start_recognition_message() {
        let msg = StartRecognitionMessage::new(
            SpeechmaticsEncoding::PcmS16le,
            16000,
            SpeechmaticsLanguage::English,
        );
        assert_eq!(msg.message, "StartRecognition");
        assert_eq!(msg.audio_format.format_type, "raw");
        assert_eq!(msg.transcription_config.language, "en");
    }

    #[test]
    fn test_start_recognition_serialization() {
        let msg = StartRecognitionMessage::new(
            SpeechmaticsEncoding::PcmS16le,
            16000,
            SpeechmaticsLanguage::English,
        );
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"message\":\"StartRecognition\""));
        assert!(json.contains("\"type\":\"raw\""));
        assert!(json.contains("\"encoding\":\"pcm_s16le\""));
        assert!(json.contains("\"sample_rate\":16000"));
        assert!(json.contains("\"language\":\"en\""));
    }

    #[test]
    fn test_end_of_stream_message() {
        let msg = EndOfStreamMessage::new(100);
        assert_eq!(msg.message, "EndOfStream");
        assert_eq!(msg.last_seq_no, 100);

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"message\":\"EndOfStream\""));
        assert!(json.contains("\"last_seq_no\":100"));
    }

    #[test]
    fn test_recognition_started_deserialization() {
        let json = r#"{"message": "RecognitionStarted", "id": "session-123"}"#;
        let msg: RecognitionStartedMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message, "RecognitionStarted");
        assert_eq!(msg.id, Some("session-123".to_string()));
    }

    #[test]
    fn test_audio_added_deserialization() {
        let json = r#"{"message": "AudioAdded", "seq_no": 42}"#;
        let msg: AudioAddedMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message, "AudioAdded");
        assert_eq!(msg.seq_no, 42);
    }

    #[test]
    fn test_add_transcript_deserialization() {
        let json = r#"{
            "message": "AddTranscript",
            "format": "2.9",
            "metadata": {
                "start_time": 0.0,
                "end_time": 1.5,
                "transcript": "Hello world."
            },
            "results": [
                {
                    "type": "word",
                    "start_time": 0.0,
                    "end_time": 0.5,
                    "alternatives": [
                        {"content": "Hello", "confidence": 0.98}
                    ]
                },
                {
                    "type": "word",
                    "start_time": 0.6,
                    "end_time": 1.2,
                    "alternatives": [
                        {"content": "world", "confidence": 0.95}
                    ]
                },
                {
                    "type": "punctuation",
                    "start_time": 1.2,
                    "end_time": 1.2,
                    "alternatives": [
                        {"content": ".", "confidence": 1.0}
                    ]
                }
            ]
        }"#;

        let msg: AddTranscriptMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message, "AddTranscript");
        assert_eq!(msg.transcript(), "Hello world.");
        assert_eq!(msg.results.len(), 3);

        let words: Vec<_> = msg.words().collect();
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].content(), Some("Hello"));
        assert_eq!(words[1].content(), Some("world"));
    }

    #[test]
    fn test_add_partial_transcript_deserialization() {
        let json = r#"{
            "message": "AddPartialTranscript",
            "metadata": {
                "start_time": 0.0,
                "end_time": 0.8,
                "transcript": "Hello"
            },
            "results": []
        }"#;

        let msg: AddPartialTranscriptMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message, "AddPartialTranscript");
        assert_eq!(msg.transcript(), "Hello");
        assert!(msg.is_partial());
    }

    #[test]
    fn test_error_message_deserialization() {
        let json = r#"{
            "message": "Error",
            "type": "invalid_config",
            "reason": "Invalid language code"
        }"#;

        let msg: ErrorMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message, "Error");
        assert_eq!(msg.error_type, "invalid_config");
        assert_eq!(msg.reason, "Invalid language code");
    }

    #[test]
    fn test_additional_vocab_word() {
        let word = AdditionalVocabWord::new("Speechmatics");
        assert_eq!(word.content, "Speechmatics");
        assert!(word.sounds_like.is_none());

        let word_with_hints = AdditionalVocabWord::with_sounds_like(
            "Speechmatics",
            vec!["speech matics".to_string()],
        );
        assert!(word_with_hints.sounds_like.is_some());
    }

    #[test]
    fn test_transcript_result_helpers() {
        let result = TranscriptResult {
            result_type: "word".to_string(),
            start_time: 0.0,
            end_time: 0.5,
            alternatives: vec![WordAlternative {
                content: "Hello".to_string(),
                confidence: 0.98,
                language: None,
                speaker: None,
            }],
            is_eos: None,
            attaches_to: None,
        };

        assert!(result.is_word());
        assert!(!result.is_punctuation());
        assert_eq!(result.content(), Some("Hello"));
        assert_eq!(result.confidence(), 0.98);
    }
}
