//! WebSocket message types for ElevenLabs STT Real-Time API.
//!
//! This module contains all message types for communication with the
//! ElevenLabs WebSocket API, including:
//!
//! - **Outgoing messages**: Messages sent from client to server
//!   - [`InputAudioChunk`]: Audio data with optional commit flag
//!   - [`EndOfStream`]: Signal end of audio stream
//!
//! - **Incoming messages**: Messages received from server
//!   - [`SessionStarted`]: Connection confirmation with session ID
//!   - [`PartialTranscript`]: Interim transcription results
//!   - [`CommittedTranscript`]: Final transcription results
//!   - [`CommittedTranscriptWithTimestamps`]: Final results with word timing
//!   - [`ElevenLabsSTTError`]: Error responses

use serde::{Deserialize, Serialize};

// =============================================================================
// Outgoing Messages (Client to Server)
// =============================================================================

/// Input message to send audio data to ElevenLabs.
///
/// Audio chunks are the primary message type for streaming audio data.
#[derive(Debug, Serialize)]
pub struct InputAudioChunk {
    /// Message type identifier (always "input_audio_chunk")
    pub message_type: &'static str,
    /// Base64-encoded audio data
    pub audio_base_64: String,
    /// Trigger manual transcription finalization.
    ///
    /// Only effective when using `CommitStrategy::Manual`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<bool>,
    /// Sample rate of the audio (optional, can be omitted if set in connection URL)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

impl InputAudioChunk {
    /// Create a new input audio chunk message.
    #[inline]
    pub fn new(audio_base_64: String) -> Self {
        Self {
            message_type: "input_audio_chunk",
            audio_base_64,
            commit: None,
            sample_rate: None,
        }
    }

    /// Set the commit flag to finalize transcription (manual commit mode).
    #[inline]
    pub fn with_commit(mut self, commit: bool) -> Self {
        self.commit = Some(commit);
        self
    }

    /// Set the sample rate for this audio chunk.
    #[inline]
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = Some(sample_rate);
        self
    }
}

/// End of stream message to signal no more audio will be sent.
#[derive(Debug, Serialize)]
pub struct EndOfStream {
    /// Message type identifier (always "eos")
    pub message_type: &'static str,
}

impl Default for EndOfStream {
    fn default() -> Self {
        Self {
            message_type: "eos",
        }
    }
}

// =============================================================================
// Incoming Messages (Server to Client)
// =============================================================================

/// Session started response from ElevenLabs.
///
/// Received after successful WebSocket connection and authentication.
#[derive(Debug, Deserialize)]
pub struct SessionStarted {
    /// Message type identifier ("session_started")
    pub message_type: String,
    /// Unique session identifier
    pub session_id: String,
    /// Configuration echoed back from server
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

/// Partial transcript (interim result) from ElevenLabs.
///
/// Sent during speech recognition before finalization.
/// The text may change as more audio is processed.
#[derive(Debug, Deserialize)]
pub struct PartialTranscript {
    /// Message type identifier ("partial_transcript")
    pub message_type: String,
    /// Current partial transcription text
    pub text: String,
}

/// Committed transcript (final result) from ElevenLabs.
///
/// Sent when a speech segment is finalized.
/// Returned when `include_timestamps` is false.
#[derive(Debug, Deserialize)]
pub struct CommittedTranscript {
    /// Message type identifier ("committed_transcript")
    pub message_type: String,
    /// Final transcription text
    pub text: String,
}

/// Word timing information for a transcribed word.
#[derive(Debug, Deserialize)]
pub struct WordTiming {
    /// The transcribed word text
    pub text: String,
    /// Start time in seconds from beginning of audio
    pub start: f64,
    /// End time in seconds from beginning of audio
    pub end: f64,
    /// Type of the word element (e.g., "word", "punctuation")
    #[serde(rename = "type")]
    pub word_type: String,
    /// Speaker identifier (if diarization is enabled)
    #[serde(default)]
    pub speaker_id: Option<String>,
    /// Log probability of the word (confidence metric)
    #[serde(default)]
    pub logprob: Option<f64>,
    /// Individual character information (if available)
    #[serde(default)]
    pub characters: Option<Vec<String>>,
}

/// Committed transcript with timestamps from ElevenLabs.
///
/// Returned when `include_timestamps` is true.
/// Contains word-level timing information.
#[derive(Debug, Deserialize)]
pub struct CommittedTranscriptWithTimestamps {
    /// Message type identifier ("committed_transcript_with_timestamps")
    pub message_type: String,
    /// Final transcription text
    pub text: String,
    /// Detected language code (if language detection is enabled)
    #[serde(default)]
    pub language_code: Option<String>,
    /// Word-level timing information
    #[serde(default)]
    pub words: Vec<WordTiming>,
}

/// Error response from ElevenLabs.
///
/// Contains error details when something goes wrong.
#[derive(Debug, Deserialize)]
pub struct ElevenLabsSTTError {
    /// Message type identifier ("error", "auth_error", or "quota_exceeded_error")
    pub message_type: String,
    /// Error description
    pub error: String,
}

// =============================================================================
// Entity Detection Messages
// =============================================================================

/// Detected entity from entity detection.
///
/// Represents a named entity recognized in the transcription.
/// ElevenLabs supports 56 entity categories including:
/// - person_name, organization, location, date, time, money, percent
/// - product, event, artwork, law, language, medical_condition
/// - And many more domain-specific categories
#[derive(Debug, Clone, Deserialize)]
pub struct DetectedEntity {
    /// The entity text as it appears in the transcript.
    pub text: String,
    /// Entity category/type (e.g., "person_name", "organization").
    pub category: String,
    /// Start time in seconds from beginning of audio.
    #[serde(default)]
    pub start: Option<f64>,
    /// End time in seconds from beginning of audio.
    #[serde(default)]
    pub end: Option<f64>,
    /// Start character offset in the transcript (0-indexed).
    #[serde(default)]
    pub start_offset: Option<usize>,
    /// End character offset in the transcript (exclusive, 0-indexed).
    #[serde(default)]
    pub end_offset: Option<usize>,
    /// Confidence score for entity detection (0.0 to 1.0).
    #[serde(default)]
    pub confidence: Option<f64>,
}

/// Entities detected message from ElevenLabs.
///
/// Sent when entity detection is enabled and entities are found.
#[derive(Debug, Deserialize)]
pub struct EntitiesDetected {
    /// Message type identifier ("entities_detected")
    pub message_type: String,
    /// List of detected entities in the current transcript segment.
    pub entities: Vec<DetectedEntity>,
}

// =============================================================================
// Speaker Diarization Messages
// =============================================================================

/// Speaker turn/segment from diarization.
///
/// Represents a continuous speech segment from a single speaker.
#[derive(Debug, Clone, Deserialize)]
pub struct SpeakerTurn {
    /// Speaker identifier (e.g., "speaker_0", "speaker_1").
    pub speaker_id: String,
    /// Start time of speaker turn in seconds.
    pub start: f64,
    /// End time of speaker turn in seconds.
    pub end: f64,
    /// Confidence score for speaker identification (0.0 to 1.0).
    #[serde(default)]
    pub confidence: Option<f64>,
    /// Text spoken during this turn.
    #[serde(default)]
    pub text: Option<String>,
}

/// Speaker turns message from ElevenLabs.
///
/// Sent when diarization is enabled and speaker turns are identified.
#[derive(Debug, Deserialize)]
pub struct SpeakerTurnsMessage {
    /// Message type identifier ("speaker_turns")
    pub message_type: String,
    /// List of speaker turns in chronological order.
    pub turns: Vec<SpeakerTurn>,
}

// =============================================================================
// Sensitive Data Detection Messages
// =============================================================================

/// Sensitive data item detected (PII or PHI).
///
/// Represents a piece of sensitive information detected in the transcript.
#[derive(Debug, Clone, Deserialize)]
pub struct SensitiveDataItem {
    /// Type of sensitive data (e.g., "ssn", "credit_card", "medical_condition").
    pub data_type: String,
    /// The original sensitive text (may be partially redacted).
    #[serde(default)]
    pub text: Option<String>,
    /// Redacted placeholder text if redaction is enabled.
    #[serde(default)]
    pub redacted: Option<String>,
    /// Start character offset in the transcript.
    #[serde(default)]
    pub start_offset: Option<usize>,
    /// End character offset in the transcript.
    #[serde(default)]
    pub end_offset: Option<usize>,
    /// Start time in seconds from beginning of audio.
    #[serde(default)]
    pub start: Option<f64>,
    /// End time in seconds from beginning of audio.
    #[serde(default)]
    pub end: Option<f64>,
    /// Confidence score for detection (0.0 to 1.0).
    #[serde(default)]
    pub confidence: Option<f64>,
    /// Category of sensitive data ("pii" or "phi").
    #[serde(default)]
    pub category: Option<String>,
}

/// Sensitive data detected message from ElevenLabs.
///
/// Sent when PII or PHI detection is enabled and sensitive data is found.
#[derive(Debug, Deserialize)]
pub struct SensitiveDataDetected {
    /// Message type identifier ("sensitive_data_detected")
    pub message_type: String,
    /// List of detected sensitive data items.
    pub items: Vec<SensitiveDataItem>,
    /// Whether the transcript was redacted.
    #[serde(default)]
    pub redacted: bool,
    /// The redacted version of the transcript (if redaction enabled).
    #[serde(default)]
    pub redacted_transcript: Option<String>,
}

// =============================================================================
// Message Enum and Parsing
// =============================================================================

/// Enum for all possible WebSocket messages from ElevenLabs.
///
/// Use `ElevenLabsMessage::parse()` to deserialize incoming WebSocket messages.
#[derive(Debug)]
pub enum ElevenLabsMessage {
    /// Session initialization complete
    SessionStarted(SessionStarted),
    /// Interim transcription result (may change)
    PartialTranscript(PartialTranscript),
    /// Final transcription result (without timestamps)
    CommittedTranscript(CommittedTranscript),
    /// Final transcription result (with word-level timestamps)
    CommittedTranscriptWithTimestamps(CommittedTranscriptWithTimestamps),
    /// Detected entities from entity detection
    EntitiesDetected(EntitiesDetected),
    /// Speaker turns from diarization
    SpeakerTurns(SpeakerTurnsMessage),
    /// Detected sensitive data (PII/PHI)
    SensitiveDataDetected(SensitiveDataDetected),
    /// Error from the server
    Error(ElevenLabsSTTError),
    /// Unknown message type (for forward compatibility)
    Unknown(String),
}

impl ElevenLabsMessage {
    /// Parse a WebSocket text message into the appropriate type.
    ///
    /// # Arguments
    /// * `text` - Raw JSON text from WebSocket message
    ///
    /// # Returns
    /// * `Result<Self, serde_json::Error>` - Parsed message or parse error
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        // First, peek at the message_type field
        #[derive(Deserialize)]
        struct MessageTypePeek {
            message_type: String,
        }

        let peek: MessageTypePeek = serde_json::from_str(text)?;

        match peek.message_type.as_str() {
            "session_started" => {
                let msg: SessionStarted = serde_json::from_str(text)?;
                Ok(ElevenLabsMessage::SessionStarted(msg))
            }
            "partial_transcript" => {
                let msg: PartialTranscript = serde_json::from_str(text)?;
                Ok(ElevenLabsMessage::PartialTranscript(msg))
            }
            "committed_transcript" => {
                let msg: CommittedTranscript = serde_json::from_str(text)?;
                Ok(ElevenLabsMessage::CommittedTranscript(msg))
            }
            "committed_transcript_with_timestamps" => {
                let msg: CommittedTranscriptWithTimestamps = serde_json::from_str(text)?;
                Ok(ElevenLabsMessage::CommittedTranscriptWithTimestamps(msg))
            }
            "entities_detected" => {
                let msg: EntitiesDetected = serde_json::from_str(text)?;
                Ok(ElevenLabsMessage::EntitiesDetected(msg))
            }
            "speaker_turns" => {
                let msg: SpeakerTurnsMessage = serde_json::from_str(text)?;
                Ok(ElevenLabsMessage::SpeakerTurns(msg))
            }
            "sensitive_data_detected" => {
                let msg: SensitiveDataDetected = serde_json::from_str(text)?;
                Ok(ElevenLabsMessage::SensitiveDataDetected(msg))
            }
            "error" | "auth_error" | "quota_exceeded_error" => {
                let msg: ElevenLabsSTTError = serde_json::from_str(text)?;
                Ok(ElevenLabsMessage::Error(msg))
            }
            _ => Ok(ElevenLabsMessage::Unknown(text.to_string())),
        }
    }

    /// Check if this message represents an error.
    #[inline]
    pub fn is_error(&self) -> bool {
        matches!(self, ElevenLabsMessage::Error(_))
    }

    /// Check if this message contains a final transcript.
    #[inline]
    pub fn is_final_transcript(&self) -> bool {
        matches!(
            self,
            ElevenLabsMessage::CommittedTranscript(_)
                | ElevenLabsMessage::CommittedTranscriptWithTimestamps(_)
        )
    }

    /// Check if this message contains entity detection results.
    #[inline]
    pub fn has_entities(&self) -> bool {
        matches!(self, ElevenLabsMessage::EntitiesDetected(_))
    }

    /// Check if this message contains speaker diarization results.
    #[inline]
    pub fn has_speaker_turns(&self) -> bool {
        matches!(self, ElevenLabsMessage::SpeakerTurns(_))
    }

    /// Check if this message contains sensitive data detection results.
    #[inline]
    pub fn has_sensitive_data(&self) -> bool {
        matches!(self, ElevenLabsMessage::SensitiveDataDetected(_))
    }
}
