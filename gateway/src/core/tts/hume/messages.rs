//! API request and response message types for Hume AI Octave TTS.
//!
//! This module defines the JSON structures for communicating with the
//! Hume AI TTS REST API endpoints.
//!
//! # API Endpoints
//!
//! - **Streaming**: `POST /v0/tts/stream/file` - Streams raw audio bytes
//! - **Sync**: `POST /v0/tts/file` - Returns complete audio file
//!
//! # Request Structure
//!
//! The Hume TTS API uses an `utterances` array pattern where each utterance
//! contains text and voice configuration:
//!
//! ```json
//! {
//!   "utterances": [{
//!     "text": "Hello world",
//!     "voice": { "name": "Kora" },
//!     "description": "happy, energetic",
//!     "speed": 1.0,
//!     "trailing_silence": 0.5
//!   }],
//!   "format": { "type": "pcm16", "sample_rate": 24000 },
//!   "instant_mode": true,
//!   "num_generations": 1
//! }
//! ```

use serde::{Deserialize, Serialize};

// =============================================================================
// Voice Configuration
// =============================================================================

/// Voice specification for Hume TTS requests.
///
/// Hume supports two modes for voice specification:
/// - By name: Use a predefined voice like "Kora"
/// - By ID: Use a custom cloned voice by UUID
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HumeVoiceSpec {
    /// Use a predefined voice by name.
    ByName {
        /// Voice name (e.g., "Kora")
        name: String,
    },
    /// Use a custom voice by ID.
    ById {
        /// Custom voice UUID
        id: String,
    },
}

impl HumeVoiceSpec {
    /// Create a voice spec by name.
    pub fn by_name(name: impl Into<String>) -> Self {
        Self::ByName { name: name.into() }
    }

    /// Create a voice spec by ID.
    pub fn by_id(id: impl Into<String>) -> Self {
        Self::ById { id: id.into() }
    }
}

impl Default for HumeVoiceSpec {
    fn default() -> Self {
        Self::by_name("Kora")
    }
}

// =============================================================================
// Utterance
// =============================================================================

/// A single utterance in the Hume TTS request.
///
/// Each utterance contains the text to synthesize and optional voice/emotion settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumeUtterance {
    /// Text to synthesize.
    pub text: String,

    /// Voice specification (name or ID).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<HumeVoiceSpec>,

    /// Acting instructions for emotion/style (max 100 chars).
    /// Examples: "happy, energetic", "sad, melancholic", "whispered fearfully"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Speaking speed (0.5 to 2.0, default 1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,

    /// Trailing silence duration in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailing_silence: Option<f32>,
}

impl HumeUtterance {
    /// Create a new utterance with just text.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            voice: None,
            description: None,
            speed: None,
            trailing_silence: None,
        }
    }

    /// Set the voice specification.
    pub fn with_voice(mut self, voice: HumeVoiceSpec) -> Self {
        self.voice = Some(voice);
        self
    }

    /// Set the voice by name.
    pub fn with_voice_name(mut self, name: impl Into<String>) -> Self {
        self.voice = Some(HumeVoiceSpec::by_name(name));
        self
    }

    /// Set the acting instructions (emotion/style).
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the speaking speed.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = Some(speed.clamp(0.5, 2.0));
        self
    }

    /// Set the trailing silence.
    pub fn with_trailing_silence(mut self, seconds: f32) -> Self {
        self.trailing_silence = Some(seconds.max(0.0));
        self
    }
}

// =============================================================================
// Output Format
// =============================================================================

/// Output format specification for Hume TTS requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumeRequestFormat {
    /// Audio format type.
    #[serde(rename = "type")]
    pub format_type: String,

    /// Sample rate in Hz.
    pub sample_rate: u32,
}

impl HumeRequestFormat {
    /// Create a new format specification.
    pub fn new(format_type: impl Into<String>, sample_rate: u32) -> Self {
        Self {
            format_type: format_type.into(),
            sample_rate,
        }
    }

    /// Create PCM16 format.
    pub fn pcm16(sample_rate: u32) -> Self {
        Self::new("pcm16", sample_rate)
    }

    /// Create MP3 format.
    pub fn mp3(sample_rate: u32) -> Self {
        Self::new("mp3", sample_rate)
    }

    /// Create WAV format.
    pub fn wav(sample_rate: u32) -> Self {
        Self::new("wav", sample_rate)
    }
}

impl Default for HumeRequestFormat {
    fn default() -> Self {
        Self::pcm16(24000)
    }
}

// =============================================================================
// TTS Request
// =============================================================================

/// Complete Hume TTS API request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumeTTSRequest {
    /// Array of utterances to synthesize.
    pub utterances: Vec<HumeUtterance>,

    /// Output format specification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<HumeRequestFormat>,

    /// Enable instant mode for low-latency streaming.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instant_mode: Option<bool>,

    /// Number of audio variations to generate (1-3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_generations: Option<u8>,

    /// Generation ID for context continuity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_id: Option<String>,

    /// Context from previous conversation (for continuity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<HumeContext>,
}

impl HumeTTSRequest {
    /// Create a new TTS request with a single utterance.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            utterances: vec![HumeUtterance::new(text)],
            format: None,
            instant_mode: None,
            num_generations: None,
            generation_id: None,
            context: None,
        }
    }

    /// Create a request with multiple utterances.
    pub fn with_utterances(utterances: Vec<HumeUtterance>) -> Self {
        Self {
            utterances,
            format: None,
            instant_mode: None,
            num_generations: None,
            generation_id: None,
            context: None,
        }
    }

    /// Set the output format.
    pub fn with_format(mut self, format: HumeRequestFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// Enable/disable instant mode.
    pub fn with_instant_mode(mut self, enabled: bool) -> Self {
        self.instant_mode = Some(enabled);
        self
    }

    /// Set number of generations.
    pub fn with_num_generations(mut self, num: u8) -> Self {
        self.num_generations = Some(num.clamp(1, 3));
        self
    }

    /// Set generation ID for continuity.
    pub fn with_generation_id(mut self, id: impl Into<String>) -> Self {
        self.generation_id = Some(id.into());
        self
    }

    /// Set context for continuity.
    pub fn with_context(mut self, context: HumeContext) -> Self {
        self.context = Some(context);
        self
    }
}

// =============================================================================
// Context
// =============================================================================

/// Context information for conversation continuity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumeContext {
    /// Previous text for context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_text: Option<String>,

    /// Previous generation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_generation_id: Option<String>,
}

impl HumeContext {
    /// Create a new context with previous text.
    pub fn with_previous_text(text: impl Into<String>) -> Self {
        Self {
            previous_text: Some(text.into()),
            previous_generation_id: None,
        }
    }

    /// Add previous generation ID.
    pub fn with_previous_generation_id(mut self, id: impl Into<String>) -> Self {
        self.previous_generation_id = Some(id.into());
        self
    }
}

// =============================================================================
// Response Types
// =============================================================================

/// Hume TTS API error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumeErrorResponse {
    /// Error code.
    #[serde(default)]
    pub code: Option<String>,

    /// Error message.
    pub message: String,

    /// Additional error details.
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

/// Metadata returned with TTS response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumeTTSMetadata {
    /// Generation ID for this audio.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_id: Option<String>,

    /// Duration of the audio in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,

    /// Sample rate of the audio.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,

    /// Number of characters processed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub characters_processed: Option<u64>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // HumeVoiceSpec Tests
    // =========================================================================

    #[test]
    fn test_voice_spec_by_name() {
        let spec = HumeVoiceSpec::by_name("Kora");
        match spec {
            HumeVoiceSpec::ByName { name } => assert_eq!(name, "Kora"),
            _ => panic!("Expected ByName variant"),
        }
    }

    #[test]
    fn test_voice_spec_by_id() {
        let spec = HumeVoiceSpec::by_id("uuid-123");
        match spec {
            HumeVoiceSpec::ById { id } => assert_eq!(id, "uuid-123"),
            _ => panic!("Expected ById variant"),
        }
    }

    #[test]
    fn test_voice_spec_default() {
        let spec = HumeVoiceSpec::default();
        match spec {
            HumeVoiceSpec::ByName { name } => assert_eq!(name, "Kora"),
            _ => panic!("Expected ByName variant"),
        }
    }

    #[test]
    fn test_voice_spec_serialization_by_name() {
        let spec = HumeVoiceSpec::by_name("Kora");
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("\"name\":\"Kora\""));
    }

    #[test]
    fn test_voice_spec_serialization_by_id() {
        let spec = HumeVoiceSpec::by_id("uuid-123");
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("\"id\":\"uuid-123\""));
    }

    // =========================================================================
    // HumeUtterance Tests
    // =========================================================================

    #[test]
    fn test_utterance_new() {
        let utterance = HumeUtterance::new("Hello world");
        assert_eq!(utterance.text, "Hello world");
        assert!(utterance.voice.is_none());
        assert!(utterance.description.is_none());
        assert!(utterance.speed.is_none());
        assert!(utterance.trailing_silence.is_none());
    }

    #[test]
    fn test_utterance_with_voice() {
        let utterance =
            HumeUtterance::new("Hello").with_voice(HumeVoiceSpec::by_name("Kora"));
        assert!(utterance.voice.is_some());
    }

    #[test]
    fn test_utterance_with_voice_name() {
        let utterance = HumeUtterance::new("Hello").with_voice_name("Kora");
        match utterance.voice {
            Some(HumeVoiceSpec::ByName { name }) => assert_eq!(name, "Kora"),
            _ => panic!("Expected ByName variant"),
        }
    }

    #[test]
    fn test_utterance_with_description() {
        let utterance =
            HumeUtterance::new("Hello").with_description("happy, energetic");
        assert_eq!(utterance.description, Some("happy, energetic".to_string()));
    }

    #[test]
    fn test_utterance_with_speed() {
        let utterance = HumeUtterance::new("Hello").with_speed(1.5);
        assert_eq!(utterance.speed, Some(1.5));
    }

    #[test]
    fn test_utterance_with_speed_clamps() {
        let utterance = HumeUtterance::new("Hello").with_speed(5.0);
        assert_eq!(utterance.speed, Some(2.0)); // Clamped to max

        let utterance = HumeUtterance::new("Hello").with_speed(0.1);
        assert_eq!(utterance.speed, Some(0.5)); // Clamped to min
    }

    #[test]
    fn test_utterance_with_trailing_silence() {
        let utterance = HumeUtterance::new("Hello").with_trailing_silence(0.5);
        assert_eq!(utterance.trailing_silence, Some(0.5));
    }

    #[test]
    fn test_utterance_with_trailing_silence_clamps_negative() {
        let utterance = HumeUtterance::new("Hello").with_trailing_silence(-1.0);
        assert_eq!(utterance.trailing_silence, Some(0.0));
    }

    #[test]
    fn test_utterance_serialization() {
        let utterance = HumeUtterance::new("Hello")
            .with_voice_name("Kora")
            .with_description("happy")
            .with_speed(1.0);

        let json = serde_json::to_string(&utterance).unwrap();
        assert!(json.contains("\"text\":\"Hello\""));
        assert!(json.contains("\"description\":\"happy\""));
        assert!(json.contains("\"speed\":1.0"));
    }

    #[test]
    fn test_utterance_serialization_omits_none() {
        let utterance = HumeUtterance::new("Hello");
        let json = serde_json::to_string(&utterance).unwrap();

        // Should not contain optional fields
        assert!(!json.contains("description"));
        assert!(!json.contains("speed"));
        assert!(!json.contains("trailing_silence"));
    }

    // =========================================================================
    // HumeRequestFormat Tests
    // =========================================================================

    #[test]
    fn test_request_format_new() {
        let format = HumeRequestFormat::new("pcm16", 24000);
        assert_eq!(format.format_type, "pcm16");
        assert_eq!(format.sample_rate, 24000);
    }

    #[test]
    fn test_request_format_pcm16() {
        let format = HumeRequestFormat::pcm16(16000);
        assert_eq!(format.format_type, "pcm16");
        assert_eq!(format.sample_rate, 16000);
    }

    #[test]
    fn test_request_format_mp3() {
        let format = HumeRequestFormat::mp3(24000);
        assert_eq!(format.format_type, "mp3");
    }

    #[test]
    fn test_request_format_wav() {
        let format = HumeRequestFormat::wav(44100);
        assert_eq!(format.format_type, "wav");
        assert_eq!(format.sample_rate, 44100);
    }

    #[test]
    fn test_request_format_default() {
        let format = HumeRequestFormat::default();
        assert_eq!(format.format_type, "pcm16");
        assert_eq!(format.sample_rate, 24000);
    }

    #[test]
    fn test_request_format_serialization() {
        let format = HumeRequestFormat::pcm16(24000);
        let json = serde_json::to_string(&format).unwrap();
        assert!(json.contains("\"type\":\"pcm16\""));
        assert!(json.contains("\"sample_rate\":24000"));
    }

    // =========================================================================
    // HumeTTSRequest Tests
    // =========================================================================

    #[test]
    fn test_tts_request_new() {
        let request = HumeTTSRequest::new("Hello world");
        assert_eq!(request.utterances.len(), 1);
        assert_eq!(request.utterances[0].text, "Hello world");
        assert!(request.format.is_none());
        assert!(request.instant_mode.is_none());
    }

    #[test]
    fn test_tts_request_with_utterances() {
        let utterances = vec![
            HumeUtterance::new("First"),
            HumeUtterance::new("Second"),
        ];
        let request = HumeTTSRequest::with_utterances(utterances);
        assert_eq!(request.utterances.len(), 2);
    }

    #[test]
    fn test_tts_request_with_format() {
        let request = HumeTTSRequest::new("Hello")
            .with_format(HumeRequestFormat::pcm16(24000));
        assert!(request.format.is_some());
        assert_eq!(request.format.as_ref().unwrap().format_type, "pcm16");
    }

    #[test]
    fn test_tts_request_with_instant_mode() {
        let request = HumeTTSRequest::new("Hello").with_instant_mode(true);
        assert_eq!(request.instant_mode, Some(true));
    }

    #[test]
    fn test_tts_request_with_num_generations() {
        let request = HumeTTSRequest::new("Hello").with_num_generations(2);
        assert_eq!(request.num_generations, Some(2));
    }

    #[test]
    fn test_tts_request_with_num_generations_clamps() {
        let request = HumeTTSRequest::new("Hello").with_num_generations(5);
        assert_eq!(request.num_generations, Some(3)); // Clamped to max
    }

    #[test]
    fn test_tts_request_with_generation_id() {
        let request = HumeTTSRequest::new("Hello").with_generation_id("gen-123");
        assert_eq!(request.generation_id, Some("gen-123".to_string()));
    }

    #[test]
    fn test_tts_request_full_serialization() {
        let request = HumeTTSRequest::new("Hello")
            .with_format(HumeRequestFormat::pcm16(24000))
            .with_instant_mode(true)
            .with_num_generations(1);

        let json = serde_json::to_string(&request).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(parsed["utterances"].is_array());
        assert_eq!(parsed["utterances"][0]["text"], "Hello");
        assert_eq!(parsed["format"]["type"], "pcm16");
        assert_eq!(parsed["instant_mode"], true);
        assert_eq!(parsed["num_generations"], 1);
    }

    #[test]
    fn test_tts_request_serialization_omits_none() {
        let request = HumeTTSRequest::new("Hello");
        let json = serde_json::to_string(&request).unwrap();

        // Should not contain optional fields
        assert!(!json.contains("format"));
        assert!(!json.contains("instant_mode"));
        assert!(!json.contains("num_generations"));
        assert!(!json.contains("generation_id"));
    }

    // =========================================================================
    // HumeContext Tests
    // =========================================================================

    #[test]
    fn test_context_with_previous_text() {
        let context = HumeContext::with_previous_text("Previous sentence");
        assert_eq!(
            context.previous_text,
            Some("Previous sentence".to_string())
        );
        assert!(context.previous_generation_id.is_none());
    }

    #[test]
    fn test_context_with_previous_generation_id() {
        let context = HumeContext::with_previous_text("Text")
            .with_previous_generation_id("gen-456");
        assert_eq!(
            context.previous_generation_id,
            Some("gen-456".to_string())
        );
    }

    // =========================================================================
    // HumeErrorResponse Tests
    // =========================================================================

    #[test]
    fn test_error_response_deserialization() {
        let json = r#"{"code":"invalid_request","message":"Invalid API key"}"#;
        let error: HumeErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(error.code, Some("invalid_request".to_string()));
        assert_eq!(error.message, "Invalid API key");
    }

    #[test]
    fn test_error_response_deserialization_minimal() {
        let json = r#"{"message":"Something went wrong"}"#;
        let error: HumeErrorResponse = serde_json::from_str(json).unwrap();
        assert!(error.code.is_none());
        assert_eq!(error.message, "Something went wrong");
    }

    // =========================================================================
    // HumeTTSMetadata Tests
    // =========================================================================

    #[test]
    fn test_metadata_deserialization() {
        let json = r#"{
            "generation_id": "gen-789",
            "duration_seconds": 2.5,
            "sample_rate": 24000,
            "characters_processed": 100
        }"#;

        let metadata: HumeTTSMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(metadata.generation_id, Some("gen-789".to_string()));
        assert_eq!(metadata.duration_seconds, Some(2.5));
        assert_eq!(metadata.sample_rate, Some(24000));
        assert_eq!(metadata.characters_processed, Some(100));
    }

    #[test]
    fn test_metadata_deserialization_partial() {
        let json = r#"{"generation_id": "gen-abc"}"#;
        let metadata: HumeTTSMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(metadata.generation_id, Some("gen-abc".to_string()));
        assert!(metadata.duration_seconds.is_none());
    }

    // =========================================================================
    // Complex Request Tests
    // =========================================================================

    #[test]
    fn test_complex_request_serialization() {
        let utterance = HumeUtterance::new("Hello, how are you today?")
            .with_voice_name("Kora")
            .with_description("warm, friendly, inviting")
            .with_speed(1.0)
            .with_trailing_silence(0.3);

        let request = HumeTTSRequest::with_utterances(vec![utterance])
            .with_format(HumeRequestFormat::pcm16(24000))
            .with_instant_mode(true)
            .with_generation_id("conversation-123")
            .with_context(HumeContext::with_previous_text("Hi there!"));

        let json = serde_json::to_string_pretty(&request).unwrap();

        // Verify structure
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["utterances"][0]["text"],
            "Hello, how are you today?"
        );
        assert_eq!(
            parsed["utterances"][0]["description"],
            "warm, friendly, inviting"
        );
        assert_eq!(parsed["utterances"][0]["voice"]["name"], "Kora");
        assert_eq!(parsed["format"]["type"], "pcm16");
        assert_eq!(parsed["instant_mode"], true);
        assert_eq!(parsed["generation_id"], "conversation-123");
        assert_eq!(parsed["context"]["previous_text"], "Hi there!");
    }
}
