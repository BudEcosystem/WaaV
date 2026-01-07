//! LMNT API message types.
//!
//! This module defines the request and response structures for the LMNT TTS API,
//! including voice objects, synthesis requests, and voice cloning.

use serde::{Deserialize, Serialize};

// =============================================================================
// Voice Types
// =============================================================================

/// Voice owner type.
///
/// Indicates who created/owns the voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LmntVoiceOwner {
    /// System-provided voice (LMNT's built-in voices)
    #[default]
    System,
    /// User-created voice (your custom clones)
    Me,
    /// Voice created by another user (shared voices)
    Other,
}

impl std::fmt::Display for LmntVoiceOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => write!(f, "system"),
            Self::Me => write!(f, "me"),
            Self::Other => write!(f, "other"),
        }
    }
}

/// Voice creation type.
///
/// Indicates how the voice was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LmntVoiceType {
    /// Instant voice clone (quick creation)
    #[default]
    Instant,
    /// Professional voice clone (higher quality, requires review)
    Professional,
}

impl std::fmt::Display for LmntVoiceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Instant => write!(f, "instant"),
            Self::Professional => write!(f, "professional"),
        }
    }
}

/// Voice state.
///
/// Indicates the current processing state of a voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LmntVoiceState {
    /// Voice is ready for use
    #[default]
    Ready,
    /// Voice is still being trained/processed
    Training,
    /// Voice processing failed
    Failed,
}

impl std::fmt::Display for LmntVoiceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready => write!(f, "ready"),
            Self::Training => write!(f, "training"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// Voice object from LMNT API.
///
/// Represents a voice that can be used for synthesis.
///
/// # Fields
///
/// - `id`: Unique identifier for the voice
/// - `name`: Display name
/// - `owner`: Who owns the voice (system, me, other)
/// - `state`: Current state (ready, training)
/// - `description`: Optional text description
/// - `gender`: Gender tag (male, female, nonbinary)
/// - `starred`: Whether the voice is starred/favorited
/// - `voice_type`: Creation method (instant, professional)
/// - `preview_url`: URL to a preview audio sample
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LmntVoice {
    /// Unique identifier for the voice
    pub id: String,

    /// Display name of the voice
    pub name: String,

    /// Who owns this voice
    #[serde(default)]
    pub owner: LmntVoiceOwner,

    /// Current state of the voice
    #[serde(default)]
    pub state: LmntVoiceState,

    /// Optional description of the voice
    #[serde(default)]
    pub description: Option<String>,

    /// Gender tag (male, female, nonbinary)
    #[serde(default)]
    pub gender: Option<String>,

    /// Whether the voice is starred/favorited
    #[serde(default)]
    pub starred: bool,

    /// How the voice was created
    #[serde(rename = "type", default)]
    pub voice_type: LmntVoiceType,

    /// URL to a preview audio sample
    #[serde(default)]
    pub preview_url: Option<String>,
}

impl LmntVoice {
    /// Returns whether this voice is ready for use.
    #[inline]
    pub fn is_ready(&self) -> bool {
        self.state == LmntVoiceState::Ready
    }

    /// Returns whether this is a system voice.
    #[inline]
    pub fn is_system_voice(&self) -> bool {
        self.owner == LmntVoiceOwner::System
    }

    /// Returns whether this is a user-created voice.
    #[inline]
    pub fn is_custom_voice(&self) -> bool {
        self.owner == LmntVoiceOwner::Me
    }
}

/// Response from the voice list endpoint.
pub type LmntVoiceListResponse = Vec<LmntVoice>;

// =============================================================================
// Voice Cloning
// =============================================================================

/// Request to create a new voice clone.
///
/// This structure is serialized as multipart/form-data for the voice clone API.
///
/// # Fields
///
/// - `name`: Display name for the new voice
/// - `enhance`: Whether to apply audio enhancement for noisy samples
///
/// Note: Audio files are sent as separate form fields, not in this struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LmntVoiceCloneRequest {
    /// Display name for the new voice
    pub name: String,

    /// Apply audio enhancement for noisy samples
    #[serde(default)]
    pub enhance: bool,
}

impl LmntVoiceCloneRequest {
    /// Creates a new voice clone request.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            enhance: false,
        }
    }

    /// Enables audio enhancement for noisy samples.
    pub fn with_enhance(mut self, enhance: bool) -> Self {
        self.enhance = enhance;
        self
    }
}

// =============================================================================
// TTS Request
// =============================================================================

/// TTS synthesis request body.
///
/// This structure is sent as JSON to the speech synthesis endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct LmntTtsRequest {
    /// Voice ID to use for synthesis
    pub voice: String,

    /// Text to synthesize (max 5000 characters)
    pub text: String,

    /// Model to use (default: "blizzard")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Language code (ISO 639-1) or "auto"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Output audio format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// Sample rate in Hz (8000, 16000, 24000)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,

    /// Speech stability control (0-1)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    /// Expressiveness control (≥0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Playback speed multiplier (0.25-2.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,

    /// Random seed for deterministic output
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,

    /// Save output to LMNT clip library
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
}

impl LmntTtsRequest {
    /// Creates a new TTS request with required fields.
    pub fn new(voice: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            voice: voice.into(),
            text: text.into(),
            model: None,
            language: None,
            format: None,
            sample_rate: None,
            top_p: None,
            temperature: None,
            speed: None,
            seed: None,
            debug: None,
        }
    }

    /// Sets the model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Sets the language.
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Sets the output format.
    pub fn with_format(mut self, format: impl Into<String>) -> Self {
        self.format = Some(format.into());
        self
    }

    /// Sets the sample rate.
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = Some(sample_rate);
        self
    }

    /// Sets the top_p value.
    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// Sets the temperature value.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Sets the speed multiplier.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = Some(speed);
        self
    }

    /// Sets the random seed.
    pub fn with_seed(mut self, seed: i64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Enables debug mode.
    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = Some(debug);
        self
    }
}

// =============================================================================
// WebSocket Messages
// =============================================================================

/// WebSocket handshake message (first message to send).
///
/// Establishes the session parameters for WebSocket streaming.
#[derive(Debug, Clone, Serialize)]
pub struct LmntWsHandshake {
    /// API key for authentication
    #[serde(rename = "X-API-Key")]
    pub api_key: String,

    /// Voice ID to use
    pub voice: String,

    /// Output format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// Language code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Sample rate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,

    /// Return timing metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_extras: Option<bool>,
}

impl LmntWsHandshake {
    /// Creates a new WebSocket handshake message.
    pub fn new(api_key: impl Into<String>, voice: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            voice: voice.into(),
            format: None,
            language: None,
            sample_rate: None,
            return_extras: None,
        }
    }
}

/// WebSocket text message (send text for synthesis).
#[derive(Debug, Clone, Serialize)]
pub struct LmntWsTextMessage {
    /// Text to synthesize
    pub text: String,
}

impl LmntWsTextMessage {
    /// Creates a new text message.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// WebSocket flush command.
///
/// Forces synthesis of buffered text without closing the connection.
#[derive(Debug, Clone, Serialize)]
pub struct LmntWsFlushCommand {
    /// Set to true to flush
    pub flush: bool,
}

impl Default for LmntWsFlushCommand {
    fn default() -> Self {
        Self { flush: true }
    }
}

/// WebSocket EOF command.
///
/// Signals end of transmission; server will synthesize remaining text and close.
#[derive(Debug, Clone, Serialize)]
pub struct LmntWsEofCommand {
    /// Set to true to end session
    pub eof: bool,
}

impl Default for LmntWsEofCommand {
    fn default() -> Self {
        Self { eof: true }
    }
}

/// WebSocket extras metadata (returned when `return_extras` is true).
#[derive(Debug, Clone, Deserialize)]
pub struct LmntWsExtras {
    /// Token duration information
    #[serde(default)]
    pub durations: Option<Vec<LmntTokenDuration>>,

    /// Whether the synthesis buffer is empty
    #[serde(default)]
    pub buffer_empty: Option<bool>,

    /// Warning message (e.g., character limit exceeded)
    #[serde(default)]
    pub warning: Option<String>,
}

/// Token duration information.
#[derive(Debug, Clone, Deserialize)]
pub struct LmntTokenDuration {
    /// Text token
    pub text: String,

    /// Start time in seconds
    pub start: f32,

    /// Duration in seconds
    pub duration: f32,
}

/// WebSocket error message.
#[derive(Debug, Clone, Deserialize)]
pub struct LmntWsError {
    /// Error description
    pub error: String,
}

// =============================================================================
// API Error Response
// =============================================================================

/// LMNT API error response.
#[derive(Debug, Clone, Deserialize)]
pub struct LmntApiError {
    /// Error message
    pub message: Option<String>,

    /// Error code
    pub code: Option<String>,

    /// HTTP status code
    pub status: Option<u16>,

    /// Additional error details
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

impl std::fmt::Display for LmntApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(msg) = &self.message {
            write!(f, "{}", msg)
        } else if let Some(code) = &self.code {
            write!(f, "Error code: {}", code)
        } else {
            write!(f, "Unknown LMNT API error")
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Voice Owner Tests
    // =========================================================================

    #[test]
    fn test_voice_owner_display() {
        assert_eq!(format!("{}", LmntVoiceOwner::System), "system");
        assert_eq!(format!("{}", LmntVoiceOwner::Me), "me");
        assert_eq!(format!("{}", LmntVoiceOwner::Other), "other");
    }

    #[test]
    fn test_voice_owner_default() {
        assert_eq!(LmntVoiceOwner::default(), LmntVoiceOwner::System);
    }

    #[test]
    fn test_voice_owner_serialization() {
        let owner = LmntVoiceOwner::Me;
        let json = serde_json::to_string(&owner).unwrap();
        assert_eq!(json, "\"me\"");
    }

    #[test]
    fn test_voice_owner_deserialization() {
        let owner: LmntVoiceOwner = serde_json::from_str("\"system\"").unwrap();
        assert_eq!(owner, LmntVoiceOwner::System);
    }

    // =========================================================================
    // Voice Type Tests
    // =========================================================================

    #[test]
    fn test_voice_type_display() {
        assert_eq!(format!("{}", LmntVoiceType::Instant), "instant");
        assert_eq!(format!("{}", LmntVoiceType::Professional), "professional");
    }

    #[test]
    fn test_voice_type_default() {
        assert_eq!(LmntVoiceType::default(), LmntVoiceType::Instant);
    }

    // =========================================================================
    // Voice State Tests
    // =========================================================================

    #[test]
    fn test_voice_state_display() {
        assert_eq!(format!("{}", LmntVoiceState::Ready), "ready");
        assert_eq!(format!("{}", LmntVoiceState::Training), "training");
        assert_eq!(format!("{}", LmntVoiceState::Failed), "failed");
    }

    #[test]
    fn test_voice_state_default() {
        assert_eq!(LmntVoiceState::default(), LmntVoiceState::Ready);
    }

    // =========================================================================
    // Voice Tests
    // =========================================================================

    #[test]
    fn test_voice_deserialization() {
        let json = r#"{
            "id": "lily",
            "name": "Lily",
            "owner": "system",
            "state": "ready",
            "gender": "female",
            "starred": false,
            "type": "instant",
            "preview_url": "https://example.com/preview.mp3"
        }"#;

        let voice: LmntVoice = serde_json::from_str(json).unwrap();

        assert_eq!(voice.id, "lily");
        assert_eq!(voice.name, "Lily");
        assert_eq!(voice.owner, LmntVoiceOwner::System);
        assert_eq!(voice.state, LmntVoiceState::Ready);
        assert_eq!(voice.gender, Some("female".to_string()));
        assert!(!voice.starred);
        assert_eq!(voice.voice_type, LmntVoiceType::Instant);
        assert!(voice.preview_url.is_some());
    }

    #[test]
    fn test_voice_deserialization_minimal() {
        let json = r#"{
            "id": "test-voice",
            "name": "Test Voice"
        }"#;

        let voice: LmntVoice = serde_json::from_str(json).unwrap();

        assert_eq!(voice.id, "test-voice");
        assert_eq!(voice.name, "Test Voice");
        assert_eq!(voice.owner, LmntVoiceOwner::default());
        assert_eq!(voice.state, LmntVoiceState::default());
        assert!(voice.description.is_none());
        assert!(voice.gender.is_none());
        assert!(!voice.starred);
    }

    #[test]
    fn test_voice_is_ready() {
        let mut voice = LmntVoice {
            id: "test".to_string(),
            name: "Test".to_string(),
            owner: LmntVoiceOwner::System,
            state: LmntVoiceState::Ready,
            description: None,
            gender: None,
            starred: false,
            voice_type: LmntVoiceType::Instant,
            preview_url: None,
        };

        assert!(voice.is_ready());

        voice.state = LmntVoiceState::Training;
        assert!(!voice.is_ready());
    }

    #[test]
    fn test_voice_is_system_voice() {
        let mut voice = LmntVoice {
            id: "test".to_string(),
            name: "Test".to_string(),
            owner: LmntVoiceOwner::System,
            state: LmntVoiceState::Ready,
            description: None,
            gender: None,
            starred: false,
            voice_type: LmntVoiceType::Instant,
            preview_url: None,
        };

        assert!(voice.is_system_voice());
        assert!(!voice.is_custom_voice());

        voice.owner = LmntVoiceOwner::Me;
        assert!(!voice.is_system_voice());
        assert!(voice.is_custom_voice());
    }

    // =========================================================================
    // Voice Clone Request Tests
    // =========================================================================

    #[test]
    fn test_voice_clone_request_new() {
        let request = LmntVoiceCloneRequest::new("My Voice");

        assert_eq!(request.name, "My Voice");
        assert!(!request.enhance);
    }

    #[test]
    fn test_voice_clone_request_with_enhance() {
        let request = LmntVoiceCloneRequest::new("My Voice").with_enhance(true);

        assert_eq!(request.name, "My Voice");
        assert!(request.enhance);
    }

    #[test]
    fn test_voice_clone_request_serialization() {
        let request = LmntVoiceCloneRequest::new("Test").with_enhance(true);
        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["name"], "Test");
        assert_eq!(json["enhance"], true);
    }

    // =========================================================================
    // TTS Request Tests
    // =========================================================================

    #[test]
    fn test_tts_request_new() {
        let request = LmntTtsRequest::new("lily", "Hello, world!");

        assert_eq!(request.voice, "lily");
        assert_eq!(request.text, "Hello, world!");
        assert!(request.model.is_none());
        assert!(request.language.is_none());
    }

    #[test]
    fn test_tts_request_with_all_options() {
        let request = LmntTtsRequest::new("lily", "Hello")
            .with_model("blizzard")
            .with_language("en")
            .with_format("pcm_s16le")
            .with_sample_rate(24000)
            .with_top_p(0.9)
            .with_temperature(1.2)
            .with_speed(1.5)
            .with_seed(12345)
            .with_debug(true);

        assert_eq!(request.model, Some("blizzard".to_string()));
        assert_eq!(request.language, Some("en".to_string()));
        assert_eq!(request.format, Some("pcm_s16le".to_string()));
        assert_eq!(request.sample_rate, Some(24000));
        assert_eq!(request.top_p, Some(0.9));
        assert_eq!(request.temperature, Some(1.2));
        assert_eq!(request.speed, Some(1.5));
        assert_eq!(request.seed, Some(12345));
        assert_eq!(request.debug, Some(true));
    }

    #[test]
    fn test_tts_request_serialization_minimal() {
        let request = LmntTtsRequest::new("lily", "Hello");
        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["voice"], "lily");
        assert_eq!(json["text"], "Hello");
        // Optional fields should not be present
        assert!(json.get("model").is_none());
        assert!(json.get("language").is_none());
    }

    #[test]
    fn test_tts_request_serialization_with_options() {
        let request = LmntTtsRequest::new("lily", "Hello")
            .with_language("en")
            .with_top_p(0.9);
        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["voice"], "lily");
        assert_eq!(json["text"], "Hello");
        assert_eq!(json["language"], "en");
        // Use approximate comparison for f32 values serialized to JSON
        let top_p = json["top_p"].as_f64().unwrap();
        assert!((top_p - 0.9).abs() < 0.001, "top_p: {}", top_p);
    }

    // =========================================================================
    // WebSocket Message Tests
    // =========================================================================

    #[test]
    fn test_ws_handshake_serialization() {
        let handshake = LmntWsHandshake::new("test-key", "lily");
        let json = serde_json::to_value(&handshake).unwrap();

        assert_eq!(json["X-API-Key"], "test-key");
        assert_eq!(json["voice"], "lily");
    }

    #[test]
    fn test_ws_text_message_serialization() {
        let msg = LmntWsTextMessage::new("Hello, world!");
        let json = serde_json::to_value(&msg).unwrap();

        assert_eq!(json["text"], "Hello, world!");
    }

    #[test]
    fn test_ws_flush_command_default() {
        let cmd = LmntWsFlushCommand::default();
        assert!(cmd.flush);

        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["flush"], true);
    }

    #[test]
    fn test_ws_eof_command_default() {
        let cmd = LmntWsEofCommand::default();
        assert!(cmd.eof);

        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["eof"], true);
    }

    #[test]
    fn test_ws_extras_deserialization() {
        let json = r#"{
            "durations": [
                {"text": "Hello", "start": 0.0, "duration": 0.5},
                {"text": "world", "start": 0.5, "duration": 0.4}
            ],
            "buffer_empty": true
        }"#;

        let extras: LmntWsExtras = serde_json::from_str(json).unwrap();

        assert!(extras.durations.is_some());
        let durations = extras.durations.unwrap();
        assert_eq!(durations.len(), 2);
        assert_eq!(durations[0].text, "Hello");
        assert!((durations[0].start - 0.0).abs() < 0.001);
        assert!((durations[0].duration - 0.5).abs() < 0.001);
        assert_eq!(extras.buffer_empty, Some(true));
        assert!(extras.warning.is_none());
    }

    #[test]
    fn test_ws_error_deserialization() {
        let json = r#"{"error": "Invalid API key"}"#;
        let error: LmntWsError = serde_json::from_str(json).unwrap();

        assert_eq!(error.error, "Invalid API key");
    }

    // =========================================================================
    // API Error Tests
    // =========================================================================

    #[test]
    fn test_api_error_display_with_message() {
        let error = LmntApiError {
            message: Some("Rate limit exceeded".to_string()),
            code: None,
            status: Some(429),
            details: None,
        };

        assert_eq!(format!("{}", error), "Rate limit exceeded");
    }

    #[test]
    fn test_api_error_display_with_code() {
        let error = LmntApiError {
            message: None,
            code: Some("RATE_LIMIT".to_string()),
            status: Some(429),
            details: None,
        };

        assert_eq!(format!("{}", error), "Error code: RATE_LIMIT");
    }

    #[test]
    fn test_api_error_display_unknown() {
        let error = LmntApiError {
            message: None,
            code: None,
            status: None,
            details: None,
        };

        assert_eq!(format!("{}", error), "Unknown LMNT API error");
    }

    #[test]
    fn test_api_error_deserialization() {
        let json = r#"{
            "message": "Invalid request",
            "code": "INVALID_REQUEST",
            "status": 400
        }"#;

        let error: LmntApiError = serde_json::from_str(json).unwrap();

        assert_eq!(error.message, Some("Invalid request".to_string()));
        assert_eq!(error.code, Some("INVALID_REQUEST".to_string()));
        assert_eq!(error.status, Some(400));
    }
}
