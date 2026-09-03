//! Gladia WebSocket Message Types
//!
//! Message structures for Gladia's WebSocket protocol.

use serde::{Deserialize, Serialize};

use super::config::{
    GladiaLanguageConfig, GladiaMessagesConfig, GladiaPostProcessing, GladiaPreProcessing,
    GladiaRealtimeProcessing,
};

// =============================================================================
// Session Initialization (REST API)
// =============================================================================

/// Request body for POST /v2/live to initialize a session
#[derive(Debug, Clone, Serialize)]
pub struct InitSessionRequest {
    /// Audio encoding format (wav/pcm, wav/alaw, wav/ulaw)
    pub encoding: String,
    /// Audio bit depth (8, 16, 24, 32)
    pub bit_depth: u8,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Number of audio channels (1-8)
    pub channels: u8,
    /// Speech recognition model
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Endpointing duration in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpointing: Option<f32>,
    /// Maximum duration without endpointing in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_duration_without_endpointing: Option<f32>,
    /// Language configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_config: Option<GladiaLanguageConfig>,
    /// Pre-processing configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_processing: Option<GladiaPreProcessing>,
    /// Realtime processing configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realtime_processing: Option<GladiaRealtimeProcessing>,
    /// Post-processing configuration (summarization, chapterization)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_processing: Option<GladiaPostProcessing>,
    /// Messages configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages_config: Option<GladiaMessagesConfig>,
    /// Custom metadata for session tracking
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_metadata: Option<serde_json::Value>,
}

impl Default for InitSessionRequest {
    fn default() -> Self {
        Self {
            encoding: "wav/pcm".to_string(),
            bit_depth: 16,
            sample_rate: 16000,
            channels: 1,
            model: None,
            endpointing: None,
            maximum_duration_without_endpointing: None,
            language_config: None,
            pre_processing: None,
            realtime_processing: None,
            post_processing: None,
            messages_config: None,
            custom_metadata: None,
        }
    }
}

/// Response from POST /v2/live session initialization
#[derive(Debug, Clone, Deserialize)]
pub struct InitSessionResponse {
    /// Session ID
    pub id: String,
    /// Creation timestamp (ISO 8601)
    pub created_at: String,
    /// WebSocket URL to connect to (includes token)
    pub url: String,
}

// =============================================================================
// WebSocket Messages (Client -> Server)
// =============================================================================

/// Audio chunk message sent to Gladia WebSocket
#[derive(Debug, Clone, Serialize)]
pub struct AudioChunkMessage {
    /// Message type identifier
    #[serde(rename = "type")]
    pub message_type: String,
    /// Audio data container
    pub data: AudioChunkData,
}

/// Audio chunk data container
#[derive(Debug, Clone, Serialize)]
pub struct AudioChunkData {
    /// Base64-encoded audio data
    pub chunk: String,
}

impl AudioChunkMessage {
    /// Create a new audio chunk message from raw bytes
    pub fn new(audio_data: &[u8]) -> Self {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        Self {
            message_type: "audio_chunk".to_string(),
            data: AudioChunkData {
                chunk: STANDARD.encode(audio_data),
            },
        }
    }

    /// Serialize to JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Stop recording message to end the transcription session
#[derive(Debug, Clone, Serialize)]
pub struct StopRecordingMessage {
    /// Message type identifier
    #[serde(rename = "type")]
    pub message_type: String,
}

impl Default for StopRecordingMessage {
    fn default() -> Self {
        Self {
            message_type: "stop_recording".to_string(),
        }
    }
}

impl StopRecordingMessage {
    /// Create a new stop recording message
    pub fn new() -> Self {
        Self::default()
    }

    /// Serialize to JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// =============================================================================
// WebSocket Messages (Server -> Client)
// =============================================================================

/// Transcript message received from Gladia WebSocket
#[derive(Debug, Clone, Deserialize)]
pub struct TranscriptMessage {
    /// Message type (always "transcript")
    #[serde(rename = "type")]
    pub message_type: String,
    /// Session ID
    pub session_id: String,
    /// Creation timestamp (ISO 8601)
    pub created_at: String,
    /// Transcript data
    pub data: TranscriptData,
}

/// Transcript data container
#[derive(Debug, Clone, Deserialize)]
pub struct TranscriptData {
    /// Utterance identifier (e.g., "00-00000011")
    pub id: String,
    /// Whether this is a final transcript
    pub is_final: bool,
    /// Utterance details
    pub utterance: UtteranceData,
}

/// Utterance data with transcription details
#[derive(Debug, Clone, Deserialize)]
pub struct UtteranceData {
    /// Full transcribed text
    pub text: String,
    /// Detected language (ISO 639-1 code)
    pub language: String,
    /// Start timestamp in seconds
    pub start: f64,
    /// End timestamp in seconds
    pub end: f64,
    /// Confidence score (0.0-1.0)
    #[serde(default)]
    pub confidence: f64,
    /// Audio channel (0-indexed)
    #[serde(default)]
    pub channel: u8,
    /// Speaker ID (when diarization enabled)
    #[serde(default)]
    pub speaker: Option<u32>,
    /// Word-level details
    #[serde(default)]
    pub words: Vec<WordData>,
}

/// Word-level transcription data
#[derive(Debug, Clone, Deserialize)]
pub struct WordData {
    /// Individual word
    pub word: String,
    /// Word start time in seconds
    pub start: f64,
    /// Word end time in seconds
    pub end: f64,
    /// Word confidence score (0.0-1.0)
    #[serde(default)]
    pub confidence: f64,
}

// =============================================================================
// Translation Message (Server -> Client, P5)
// =============================================================================

/// Real-time translation message received from Gladia (`type:"translation"`).
///
/// Emitted for each target language when `realtime_processing.translation` is on.
/// The translated text is `data.translated_utterance.text`; the target language is
/// `data.target_language` (ISO-639-1). The gateway folds this into the uniform
/// `translations:[{lang,text}]` array. P5.
#[derive(Debug, Clone, Deserialize)]
pub struct TranslationMessage {
    /// Message type (always "translation").
    #[serde(rename = "type")]
    pub message_type: String,
    /// Session ID.
    #[serde(default)]
    pub session_id: String,
    /// Translation payload.
    pub data: TranslationDataPayload,
}

/// Payload of a Gladia `type:"translation"` message.
#[derive(Debug, Clone, Deserialize)]
pub struct TranslationDataPayload {
    /// Whether the translated utterance is final.
    #[serde(default)]
    pub is_final: bool,
    /// The source language the utterance was translated FROM (ISO-639-1).
    #[serde(default)]
    pub original_language: Option<String>,
    /// The target language this translation is IN (ISO-639-1, e.g. "es", "de").
    pub target_language: String,
    /// The translated utterance (reuses the transcript utterance shape: `text` etc.).
    pub translated_utterance: UtteranceData,
}

// =============================================================================
// Error Response
// =============================================================================

/// Error response from Gladia API
#[derive(Debug, Clone, Deserialize)]
pub struct GladiaError {
    /// Error code or status
    #[serde(default)]
    pub code: Option<String>,
    /// Error message
    pub message: String,
    /// Additional error details
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

impl std::fmt::Display for GladiaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(code) = &self.code {
            write!(f, "[{}] {}", code, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for GladiaError {}

// =============================================================================
// Generic WebSocket Message Parsing
// =============================================================================

/// Generic message type for initial parsing
#[derive(Debug, Clone, Deserialize)]
pub struct GenericMessage {
    /// Message type identifier
    #[serde(rename = "type")]
    pub message_type: String,
}

/// Enum for all possible server messages
#[derive(Debug, Clone)]
pub enum ServerMessage {
    /// Transcript message (partial or final)
    Transcript(TranscriptMessage),
    /// Real-time translation message (P5; `type:"translation"`).
    Translation(TranslationMessage),
    /// Error message
    Error(GladiaError),
    /// Unknown message type
    Unknown(String),
}

impl ServerMessage {
    /// Parse a JSON string into a server message
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        // First, parse the type field
        let generic: GenericMessage = serde_json::from_str(json)?;

        match generic.message_type.as_str() {
            "transcript" => {
                let msg: TranscriptMessage = serde_json::from_str(json)?;
                Ok(ServerMessage::Transcript(msg))
            }
            "translation" => {
                let msg: TranslationMessage = serde_json::from_str(json)?;
                Ok(ServerMessage::Translation(msg))
            }
            "error" => {
                let err: GladiaError = serde_json::from_str(json)?;
                Ok(ServerMessage::Error(err))
            }
            other => Ok(ServerMessage::Unknown(other.to_string())),
        }
    }

    /// Check if this is a final transcript
    pub fn is_final_transcript(&self) -> bool {
        matches!(self, ServerMessage::Transcript(t) if t.data.is_final)
    }

    /// Check if this is a partial transcript
    pub fn is_partial_transcript(&self) -> bool {
        matches!(self, ServerMessage::Transcript(t) if !t.data.is_final)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_session_request_default() {
        let req = InitSessionRequest::default();
        assert_eq!(req.encoding, "wav/pcm");
        assert_eq!(req.bit_depth, 16);
        assert_eq!(req.sample_rate, 16000);
        assert_eq!(req.channels, 1);
    }

    #[test]
    fn test_init_session_request_serialization() {
        let req = InitSessionRequest {
            encoding: "wav/pcm".to_string(),
            bit_depth: 16,
            sample_rate: 16000,
            channels: 1,
            model: Some("solaria-1".to_string()),
            endpointing: Some(0.05),
            ..Default::default()
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("wav/pcm"));
        assert!(json.contains("solaria-1"));
        assert!(json.contains("0.05"));
    }

    #[test]
    fn test_init_session_response_deserialization() {
        let json = r#"{
            "id": "test-session-id",
            "created_at": "2025-01-13T10:00:00.000Z",
            "url": "wss://api.gladia.io/v2/live?token=abc123"
        }"#;

        let resp: InitSessionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "test-session-id");
        assert_eq!(resp.url, "wss://api.gladia.io/v2/live?token=abc123");
    }

    #[test]
    fn test_audio_chunk_message_new() {
        let audio_data = vec![0u8, 1, 2, 3, 4, 5];
        let msg = AudioChunkMessage::new(&audio_data);

        assert_eq!(msg.message_type, "audio_chunk");
        // Verify base64 encoding
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        assert_eq!(msg.data.chunk, STANDARD.encode(&audio_data));
    }

    #[test]
    fn test_audio_chunk_message_serialization() {
        let msg = AudioChunkMessage::new(&[0, 1, 2]);
        let json = msg.to_json().unwrap();

        assert!(json.contains(r#""type":"audio_chunk""#));
        assert!(json.contains(r#""chunk":"#));
    }

    #[test]
    fn test_stop_recording_message() {
        let msg = StopRecordingMessage::new();
        assert_eq!(msg.message_type, "stop_recording");

        let json = msg.to_json().unwrap();
        assert_eq!(json, r#"{"type":"stop_recording"}"#);
    }

    #[test]
    fn test_transcript_message_deserialization() {
        let json = r#"{
            "type": "transcript",
            "session_id": "test-session",
            "created_at": "2025-01-13T10:00:00.000Z",
            "data": {
                "id": "00-00000001",
                "is_final": true,
                "utterance": {
                    "text": "Hello world",
                    "language": "en",
                    "start": 0.0,
                    "end": 1.5,
                    "confidence": 0.95,
                    "channel": 0,
                    "words": [
                        {"word": "Hello", "start": 0.0, "end": 0.5, "confidence": 0.98},
                        {"word": "world", "start": 0.6, "end": 1.5, "confidence": 0.92}
                    ]
                }
            }
        }"#;

        let msg: TranscriptMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message_type, "transcript");
        assert_eq!(msg.session_id, "test-session");
        assert!(msg.data.is_final);
        assert_eq!(msg.data.utterance.text, "Hello world");
        assert_eq!(msg.data.utterance.language, "en");
        assert_eq!(msg.data.utterance.words.len(), 2);
        assert_eq!(msg.data.utterance.words[0].word, "Hello");
    }

    #[test]
    fn test_transcript_message_partial() {
        let json = r#"{
            "type": "transcript",
            "session_id": "test-session",
            "created_at": "2025-01-13T10:00:00.000Z",
            "data": {
                "id": "00-00000001",
                "is_final": false,
                "utterance": {
                    "text": "Hello",
                    "language": "en",
                    "start": 0.0,
                    "end": 0.5
                }
            }
        }"#;

        let msg: TranscriptMessage = serde_json::from_str(json).unwrap();
        assert!(!msg.data.is_final);
    }

    #[test]
    fn test_transcript_with_speaker() {
        let json = r#"{
            "type": "transcript",
            "session_id": "test-session",
            "created_at": "2025-01-13T10:00:00.000Z",
            "data": {
                "id": "00-00000001",
                "is_final": true,
                "utterance": {
                    "text": "Hello",
                    "language": "en",
                    "start": 0.0,
                    "end": 0.5,
                    "speaker": 1
                }
            }
        }"#;

        let msg: TranscriptMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.data.utterance.speaker, Some(1));
    }

    #[test]
    fn test_gladia_error_display() {
        let err = GladiaError {
            code: Some("INVALID_CONFIG".to_string()),
            message: "Invalid configuration".to_string(),
            details: None,
        };
        assert_eq!(format!("{}", err), "[INVALID_CONFIG] Invalid configuration");

        let err_no_code = GladiaError {
            code: None,
            message: "Unknown error".to_string(),
            details: None,
        };
        assert_eq!(format!("{}", err_no_code), "Unknown error");
    }

    #[test]
    fn test_server_message_from_json_transcript() {
        let json = r#"{
            "type": "transcript",
            "session_id": "test",
            "created_at": "2025-01-13T10:00:00.000Z",
            "data": {
                "id": "00-00000001",
                "is_final": true,
                "utterance": {
                    "text": "Test",
                    "language": "en",
                    "start": 0.0,
                    "end": 0.5
                }
            }
        }"#;

        let msg = ServerMessage::from_json(json).unwrap();
        assert!(matches!(msg, ServerMessage::Transcript(_)));
        assert!(msg.is_final_transcript());
        assert!(!msg.is_partial_transcript());
    }

    #[test]
    fn test_server_message_from_json_translation() {
        // P5: a Gladia `type:"translation"` frame must parse into ServerMessage::Translation
        // with the target language + translated utterance text.
        let json = r#"{
            "type": "translation",
            "session_id": "test",
            "created_at": "2025-01-13T10:00:00.000Z",
            "data": {
                "is_final": true,
                "original_language": "en",
                "target_language": "es",
                "translated_utterance": {
                    "text": "hola mundo",
                    "language": "es",
                    "start": 0.0,
                    "end": 0.5
                }
            }
        }"#;

        let msg = ServerMessage::from_json(json).unwrap();
        match msg {
            ServerMessage::Translation(t) => {
                assert_eq!(t.data.target_language, "es");
                assert_eq!(t.data.translated_utterance.text, "hola mundo");
                assert!(t.data.is_final);
            }
            _ => panic!("expected ServerMessage::Translation"),
        }
    }

    #[test]
    fn test_server_message_from_json_partial() {
        let json = r#"{
            "type": "transcript",
            "session_id": "test",
            "created_at": "2025-01-13T10:00:00.000Z",
            "data": {
                "id": "00-00000001",
                "is_final": false,
                "utterance": {
                    "text": "Test",
                    "language": "en",
                    "start": 0.0,
                    "end": 0.5
                }
            }
        }"#;

        let msg = ServerMessage::from_json(json).unwrap();
        assert!(msg.is_partial_transcript());
        assert!(!msg.is_final_transcript());
    }

    #[test]
    fn test_server_message_from_json_unknown() {
        let json = r#"{"type": "some_unknown_type"}"#;
        let msg = ServerMessage::from_json(json).unwrap();
        assert!(matches!(msg, ServerMessage::Unknown(t) if t == "some_unknown_type"));
    }

    #[test]
    fn test_utterance_defaults() {
        let json = r#"{
            "text": "Test",
            "language": "en",
            "start": 0.0,
            "end": 0.5
        }"#;

        let utterance: UtteranceData = serde_json::from_str(json).unwrap();
        assert_eq!(utterance.confidence, 0.0); // Default
        assert_eq!(utterance.channel, 0); // Default
        assert!(utterance.speaker.is_none());
        assert!(utterance.words.is_empty());
    }

    #[test]
    fn test_word_data_defaults() {
        let json = r#"{
            "word": "hello",
            "start": 0.0,
            "end": 0.5
        }"#;

        let word: WordData = serde_json::from_str(json).unwrap();
        assert_eq!(word.word, "hello");
        assert_eq!(word.confidence, 0.0); // Default
    }
}
