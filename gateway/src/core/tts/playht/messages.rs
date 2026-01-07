//! Play.ht API message types.
//!
//! This module defines the request and response structures for the Play.ht TTS API,
//! including voice objects, synthesis requests, and WebSocket messages.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Voice Types
// =============================================================================

/// Voice object from Play.ht API.
///
/// Represents a voice that can be used for synthesis.
///
/// # Fields
///
/// - `id`: Unique identifier for the voice (typically S3 manifest URL)
/// - `name`: Display name
/// - `voice_engine`: Compatible voice engine (Play3.0-mini, PlayDialog, etc.)
/// - `language_code`: ISO language code
/// - `gender`: Gender tag (male, female)
/// - `age`: Age tag (adult, child, etc.)
/// - `is_cloned`: Whether this is a user-cloned voice
/// - `preview_url`: URL to a preview audio sample
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayHtVoice {
    /// Unique identifier for the voice (typically S3 manifest URL)
    pub id: String,

    /// Display name of the voice
    pub name: String,

    /// Compatible voice engine
    #[serde(default)]
    pub voice_engine: String,

    /// ISO language code
    #[serde(default)]
    pub language_code: Option<String>,

    /// Gender tag
    #[serde(default)]
    pub gender: Option<String>,

    /// Age tag
    #[serde(default)]
    pub age: Option<String>,

    /// Whether this is a user-cloned voice
    #[serde(default)]
    pub is_cloned: bool,

    /// URL to a preview audio sample
    #[serde(default)]
    pub preview_url: Option<String>,

    /// Voice style (if applicable)
    #[serde(default)]
    pub style: Option<String>,

    /// Voice texture description
    #[serde(default)]
    pub texture: Option<String>,

    /// Whether this is a publicly available voice
    #[serde(default)]
    pub is_public: Option<bool>,
}

impl PlayHtVoice {
    /// Returns whether this voice is compatible with the given engine.
    pub fn is_compatible_with(&self, engine: &str) -> bool {
        self.voice_engine.eq_ignore_ascii_case(engine)
    }

    /// Returns whether this is a user-cloned voice.
    #[inline]
    pub fn is_custom_voice(&self) -> bool {
        self.is_cloned
    }

    /// Returns the language code if available.
    #[inline]
    pub fn language(&self) -> Option<&str> {
        self.language_code.as_deref()
    }
}

/// Response from the voice list endpoint.
pub type PlayHtVoiceListResponse = Vec<PlayHtVoice>;

// =============================================================================
// Voice Cloning
// =============================================================================

/// Request to create a new instant voice clone.
///
/// This structure is used for the instant voice cloning API.
///
/// # Fields
///
/// - `voice_name`: Display name for the new voice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayHtVoiceCloneRequest {
    /// Display name for the new voice
    pub voice_name: String,
}

impl PlayHtVoiceCloneRequest {
    /// Creates a new voice clone request.
    pub fn new(voice_name: impl Into<String>) -> Self {
        Self {
            voice_name: voice_name.into(),
        }
    }
}

/// Response from the voice clone endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct PlayHtVoiceCloneResponse {
    /// The ID of the created voice
    pub id: String,

    /// The name of the created voice
    pub name: String,

    /// The voice engine
    pub voice_engine: Option<String>,

    /// Status of the clone operation
    pub status: Option<String>,
}

// =============================================================================
// TTS Request
// =============================================================================

/// TTS synthesis request body.
///
/// This structure is sent as JSON to the speech synthesis endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct PlayHtTtsRequest {
    /// Voice ID to use for synthesis (typically S3 manifest URL)
    pub voice: String,

    /// Text to synthesize (max 20,000 characters)
    pub text: String,

    /// Voice engine/model to use
    pub voice_engine: String,

    /// Output audio format
    pub output_format: String,

    /// Sample rate in Hz
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,

    /// Playback speed (0.5-2.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,

    /// Audio quality tier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,

    /// Randomness control (0.0-1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Random seed for deterministic output
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,

    /// Language code (Play3.0-mini only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Text guidance (Play3.0, PlayHT2.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_guidance: Option<f32>,

    /// Voice guidance (Play3.0, PlayHT2.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_guidance: Option<f32>,

    /// Style guidance (Play3.0 only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style_guidance: Option<f32>,

    /// Repetition penalty
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repetition_penalty: Option<f32>,

    /// Second speaker voice URL (PlayDialog)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_2: Option<String>,

    /// First speaker identifier (PlayDialog)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_prefix: Option<String>,

    /// Second speaker identifier (PlayDialog)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_prefix_2: Option<String>,

    /// Voice conditioning seconds (PlayDialog)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_conditioning_seconds: Option<f32>,

    /// Number of candidates for ranking (PlayDialog)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_candidates: Option<u32>,
}

impl PlayHtTtsRequest {
    /// Creates a new TTS request with required fields.
    pub fn new(
        voice: impl Into<String>,
        text: impl Into<String>,
        voice_engine: impl Into<String>,
        output_format: impl Into<String>,
    ) -> Self {
        Self {
            voice: voice.into(),
            text: text.into(),
            voice_engine: voice_engine.into(),
            output_format: output_format.into(),
            sample_rate: None,
            speed: None,
            quality: None,
            temperature: None,
            seed: None,
            language: None,
            text_guidance: None,
            voice_guidance: None,
            style_guidance: None,
            repetition_penalty: None,
            voice_2: None,
            turn_prefix: None,
            turn_prefix_2: None,
            voice_conditioning_seconds: None,
            num_candidates: None,
        }
    }

    /// Sets the sample rate.
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = Some(sample_rate);
        self
    }

    /// Sets the speed.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = Some(speed);
        self
    }

    /// Sets the quality tier.
    pub fn with_quality(mut self, quality: impl Into<String>) -> Self {
        self.quality = Some(quality.into());
        self
    }

    /// Sets the temperature.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Sets the seed.
    pub fn with_seed(mut self, seed: i64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Sets the language.
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Sets the second speaker voice for dialogue.
    pub fn with_voice_2(mut self, voice: impl Into<String>) -> Self {
        self.voice_2 = Some(voice.into());
        self
    }

    /// Sets the turn prefixes for dialogue.
    pub fn with_turn_prefixes(
        mut self,
        prefix1: impl Into<String>,
        prefix2: impl Into<String>,
    ) -> Self {
        self.turn_prefix = Some(prefix1.into());
        self.turn_prefix_2 = Some(prefix2.into());
        self
    }
}

// =============================================================================
// WebSocket Messages
// =============================================================================

/// WebSocket authentication response.
///
/// Returned from POST /api/v4/websocket-auth.
#[derive(Debug, Clone, Deserialize)]
pub struct PlayHtWsAuthResponse {
    /// Map of voice engine to WebSocket URL
    pub websocket_urls: HashMap<String, String>,

    /// Expiration time for the auth token
    pub expires_at: String,
}

impl PlayHtWsAuthResponse {
    /// Gets the WebSocket URL for a specific voice engine.
    pub fn url_for_engine(&self, engine: &str) -> Option<&str> {
        self.websocket_urls.get(engine).map(|s| s.as_str())
    }

    /// Returns all available engine URLs.
    pub fn engines(&self) -> impl Iterator<Item = &str> {
        self.websocket_urls.keys().map(|s| s.as_str())
    }
}

/// WebSocket TTS command.
///
/// Sent to the WebSocket to request speech synthesis.
#[derive(Debug, Clone, Serialize)]
pub struct PlayHtWsCommand {
    /// Text to synthesize
    pub text: String,

    /// Voice manifest URL
    pub voice: String,

    /// Output format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,

    /// Playback speed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,

    /// Sample rate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

impl PlayHtWsCommand {
    /// Creates a new WebSocket TTS command.
    pub fn new(text: impl Into<String>, voice: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            voice: voice.into(),
            output_format: None,
            speed: None,
            sample_rate: None,
        }
    }

    /// Sets the output format.
    pub fn with_format(mut self, format: impl Into<String>) -> Self {
        self.output_format = Some(format.into());
        self
    }

    /// Sets the speed.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = Some(speed);
        self
    }

    /// Sets the sample rate.
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = Some(sample_rate);
        self
    }
}

/// WebSocket event message.
///
/// Received from the WebSocket during synthesis.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum PlayHtWsMessage {
    /// Synthesis started
    #[serde(rename = "start")]
    Start {
        /// HTTP status code
        status: u16,
        /// Request identifier
        request_id: String,
    },
    /// Synthesis ended
    #[serde(rename = "end")]
    End {
        /// HTTP status code
        status: u16,
        /// Request identifier
        request_id: String,
    },
    /// Error occurred
    #[serde(rename = "error")]
    Error {
        /// Error message
        message: String,
        /// Error code (optional)
        code: Option<String>,
    },
}

impl PlayHtWsMessage {
    /// Returns whether this is a start message.
    pub fn is_start(&self) -> bool {
        matches!(self, Self::Start { .. })
    }

    /// Returns whether this is an end message.
    pub fn is_end(&self) -> bool {
        matches!(self, Self::End { .. })
    }

    /// Returns whether this is an error message.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }

    /// Returns the request ID if available.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Start { request_id, .. } | Self::End { request_id, .. } => Some(request_id),
            Self::Error { .. } => None,
        }
    }
}

// =============================================================================
// API Error Response
// =============================================================================

/// Play.ht API error response.
#[derive(Debug, Clone, Deserialize)]
pub struct PlayHtApiError {
    /// Error message
    #[serde(alias = "error_message")]
    pub message: Option<String>,

    /// Error code
    #[serde(alias = "error_code")]
    pub code: Option<String>,

    /// HTTP status code
    pub status: Option<u16>,

    /// Additional error details
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

impl std::fmt::Display for PlayHtApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(msg) = &self.message {
            write!(f, "{}", msg)
        } else if let Some(code) = &self.code {
            write!(f, "Error code: {}", code)
        } else {
            write!(f, "Unknown Play.ht API error")
        }
    }
}

impl std::error::Error for PlayHtApiError {}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Voice Tests
    // =========================================================================

    #[test]
    fn test_voice_deserialization() {
        let json = r#"{
            "id": "s3://voice-cloning-zero-shot/test/manifest.json",
            "name": "Test Voice",
            "voice_engine": "Play3.0-mini",
            "language_code": "en-US",
            "gender": "female",
            "age": "adult",
            "is_cloned": false,
            "preview_url": "https://example.com/preview.mp3"
        }"#;

        let voice: PlayHtVoice = serde_json::from_str(json).unwrap();

        assert_eq!(voice.id, "s3://voice-cloning-zero-shot/test/manifest.json");
        assert_eq!(voice.name, "Test Voice");
        assert_eq!(voice.voice_engine, "Play3.0-mini");
        assert_eq!(voice.language_code, Some("en-US".to_string()));
        assert_eq!(voice.gender, Some("female".to_string()));
        assert_eq!(voice.age, Some("adult".to_string()));
        assert!(!voice.is_cloned);
        assert!(voice.preview_url.is_some());
    }

    #[test]
    fn test_voice_deserialization_minimal() {
        let json = r#"{
            "id": "test-voice",
            "name": "Test Voice"
        }"#;

        let voice: PlayHtVoice = serde_json::from_str(json).unwrap();

        assert_eq!(voice.id, "test-voice");
        assert_eq!(voice.name, "Test Voice");
        assert!(voice.voice_engine.is_empty());
        assert!(voice.language_code.is_none());
        assert!(voice.gender.is_none());
        assert!(!voice.is_cloned);
    }

    #[test]
    fn test_voice_is_compatible_with() {
        let voice = PlayHtVoice {
            id: "test".to_string(),
            name: "Test".to_string(),
            voice_engine: "Play3.0-mini".to_string(),
            language_code: None,
            gender: None,
            age: None,
            is_cloned: false,
            preview_url: None,
            style: None,
            texture: None,
            is_public: None,
        };

        assert!(voice.is_compatible_with("Play3.0-mini"));
        assert!(voice.is_compatible_with("play3.0-mini")); // case insensitive
        assert!(!voice.is_compatible_with("PlayDialog"));
    }

    #[test]
    fn test_voice_is_custom_voice() {
        let mut voice = PlayHtVoice {
            id: "test".to_string(),
            name: "Test".to_string(),
            voice_engine: "Play3.0-mini".to_string(),
            language_code: None,
            gender: None,
            age: None,
            is_cloned: false,
            preview_url: None,
            style: None,
            texture: None,
            is_public: None,
        };

        assert!(!voice.is_custom_voice());

        voice.is_cloned = true;
        assert!(voice.is_custom_voice());
    }

    // =========================================================================
    // Voice Clone Request Tests
    // =========================================================================

    #[test]
    fn test_voice_clone_request_new() {
        let request = PlayHtVoiceCloneRequest::new("My Voice");

        assert_eq!(request.voice_name, "My Voice");
    }

    #[test]
    fn test_voice_clone_request_serialization() {
        let request = PlayHtVoiceCloneRequest::new("Test");
        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["voice_name"], "Test");
    }

    // =========================================================================
    // TTS Request Tests
    // =========================================================================

    #[test]
    fn test_tts_request_new() {
        let request = PlayHtTtsRequest::new(
            "s3://test/manifest.json",
            "Hello, world!",
            "Play3.0-mini",
            "mp3",
        );

        assert_eq!(request.voice, "s3://test/manifest.json");
        assert_eq!(request.text, "Hello, world!");
        assert_eq!(request.voice_engine, "Play3.0-mini");
        assert_eq!(request.output_format, "mp3");
        assert!(request.sample_rate.is_none());
        assert!(request.speed.is_none());
    }

    #[test]
    fn test_tts_request_with_all_options() {
        let request = PlayHtTtsRequest::new("voice", "text", "engine", "format")
            .with_sample_rate(48000)
            .with_speed(1.5)
            .with_quality("premium")
            .with_temperature(0.8)
            .with_seed(12345)
            .with_language("en");

        assert_eq!(request.sample_rate, Some(48000));
        assert_eq!(request.speed, Some(1.5));
        assert_eq!(request.quality, Some("premium".to_string()));
        assert_eq!(request.temperature, Some(0.8));
        assert_eq!(request.seed, Some(12345));
        assert_eq!(request.language, Some("en".to_string()));
    }

    #[test]
    fn test_tts_request_with_dialogue_params() {
        let request = PlayHtTtsRequest::new("voice", "text", "PlayDialog", "mp3")
            .with_voice_2("voice2")
            .with_turn_prefixes("S1:", "S2:");

        assert_eq!(request.voice_2, Some("voice2".to_string()));
        assert_eq!(request.turn_prefix, Some("S1:".to_string()));
        assert_eq!(request.turn_prefix_2, Some("S2:".to_string()));
    }

    #[test]
    fn test_tts_request_serialization_minimal() {
        let request = PlayHtTtsRequest::new("voice", "Hello", "Play3.0-mini", "mp3");
        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["voice"], "voice");
        assert_eq!(json["text"], "Hello");
        assert_eq!(json["voice_engine"], "Play3.0-mini");
        assert_eq!(json["output_format"], "mp3");
        // Optional fields should not be present
        assert!(json.get("sample_rate").is_none());
        assert!(json.get("speed").is_none());
    }

    #[test]
    fn test_tts_request_serialization_with_options() {
        let request = PlayHtTtsRequest::new("voice", "Hello", "Play3.0-mini", "mp3")
            .with_speed(1.5)
            .with_sample_rate(48000);
        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["voice"], "voice");
        assert_eq!(json["text"], "Hello");
        assert_eq!(json["speed"], 1.5);
        assert_eq!(json["sample_rate"], 48000);
    }

    // =========================================================================
    // WebSocket Auth Response Tests
    // =========================================================================

    #[test]
    fn test_ws_auth_response_deserialization() {
        let json = r#"{
            "websocket_urls": {
                "Play3.0-mini": "wss://example.com/ws/play3",
                "PlayDialog": "wss://example.com/ws/dialog"
            },
            "expires_at": "2024-01-01T00:00:00Z"
        }"#;

        let response: PlayHtWsAuthResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.websocket_urls.len(), 2);
        assert_eq!(
            response.url_for_engine("Play3.0-mini"),
            Some("wss://example.com/ws/play3")
        );
        assert_eq!(
            response.url_for_engine("PlayDialog"),
            Some("wss://example.com/ws/dialog")
        );
        assert!(response.url_for_engine("Unknown").is_none());
        assert_eq!(response.expires_at, "2024-01-01T00:00:00Z");
    }

    #[test]
    fn test_ws_auth_response_engines() {
        let json = r#"{
            "websocket_urls": {
                "Play3.0-mini": "wss://example.com/ws/play3",
                "PlayDialog": "wss://example.com/ws/dialog"
            },
            "expires_at": "2024-01-01T00:00:00Z"
        }"#;

        let response: PlayHtWsAuthResponse = serde_json::from_str(json).unwrap();
        let engines: Vec<&str> = response.engines().collect();

        assert_eq!(engines.len(), 2);
    }

    // =========================================================================
    // WebSocket Command Tests
    // =========================================================================

    #[test]
    fn test_ws_command_new() {
        let cmd = PlayHtWsCommand::new("Hello", "voice-id");

        assert_eq!(cmd.text, "Hello");
        assert_eq!(cmd.voice, "voice-id");
        assert!(cmd.output_format.is_none());
    }

    #[test]
    fn test_ws_command_with_options() {
        let cmd = PlayHtWsCommand::new("Hello", "voice-id")
            .with_format("mp3")
            .with_speed(1.5)
            .with_sample_rate(48000);

        assert_eq!(cmd.output_format, Some("mp3".to_string()));
        assert_eq!(cmd.speed, Some(1.5));
        assert_eq!(cmd.sample_rate, Some(48000));
    }

    #[test]
    fn test_ws_command_serialization() {
        let cmd = PlayHtWsCommand::new("Hello", "voice-id").with_format("mp3");
        let json = serde_json::to_value(&cmd).unwrap();

        assert_eq!(json["text"], "Hello");
        assert_eq!(json["voice"], "voice-id");
        assert_eq!(json["output_format"], "mp3");
    }

    // =========================================================================
    // WebSocket Message Tests
    // =========================================================================

    #[test]
    fn test_ws_message_start_deserialization() {
        let json = r#"{"type": "start", "status": 200, "request_id": "abc123"}"#;
        let msg: PlayHtWsMessage = serde_json::from_str(json).unwrap();

        assert!(msg.is_start());
        assert!(!msg.is_end());
        assert!(!msg.is_error());
        assert_eq!(msg.request_id(), Some("abc123"));
    }

    #[test]
    fn test_ws_message_end_deserialization() {
        let json = r#"{"type": "end", "status": 200, "request_id": "abc123"}"#;
        let msg: PlayHtWsMessage = serde_json::from_str(json).unwrap();

        assert!(!msg.is_start());
        assert!(msg.is_end());
        assert!(!msg.is_error());
        assert_eq!(msg.request_id(), Some("abc123"));
    }

    #[test]
    fn test_ws_message_error_deserialization() {
        let json = r#"{"type": "error", "message": "Something went wrong", "code": "ERR001"}"#;
        let msg: PlayHtWsMessage = serde_json::from_str(json).unwrap();

        assert!(!msg.is_start());
        assert!(!msg.is_end());
        assert!(msg.is_error());
        assert!(msg.request_id().is_none());
    }

    // =========================================================================
    // API Error Tests
    // =========================================================================

    #[test]
    fn test_api_error_display_with_message() {
        let error = PlayHtApiError {
            message: Some("Rate limit exceeded".to_string()),
            code: None,
            status: Some(429),
            details: None,
        };

        assert_eq!(format!("{}", error), "Rate limit exceeded");
    }

    #[test]
    fn test_api_error_display_with_code() {
        let error = PlayHtApiError {
            message: None,
            code: Some("RATE_LIMIT".to_string()),
            status: Some(429),
            details: None,
        };

        assert_eq!(format!("{}", error), "Error code: RATE_LIMIT");
    }

    #[test]
    fn test_api_error_display_unknown() {
        let error = PlayHtApiError {
            message: None,
            code: None,
            status: None,
            details: None,
        };

        assert_eq!(format!("{}", error), "Unknown Play.ht API error");
    }

    #[test]
    fn test_api_error_deserialization() {
        let json = r#"{
            "message": "Invalid request",
            "code": "INVALID_REQUEST",
            "status": 400
        }"#;

        let error: PlayHtApiError = serde_json::from_str(json).unwrap();

        assert_eq!(error.message, Some("Invalid request".to_string()));
        assert_eq!(error.code, Some("INVALID_REQUEST".to_string()));
        assert_eq!(error.status, Some(400));
    }

    #[test]
    fn test_api_error_deserialization_with_aliases() {
        let json = r#"{
            "error_message": "Bad request",
            "error_code": "BAD_REQUEST"
        }"#;

        let error: PlayHtApiError = serde_json::from_str(json).unwrap();

        assert_eq!(error.message, Some("Bad request".to_string()));
        assert_eq!(error.code, Some("BAD_REQUEST".to_string()));
    }
}
