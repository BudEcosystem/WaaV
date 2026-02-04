//! Smallest.ai TTS API Message Types
//!
//! This module defines request and response structures for the Smallest.ai Waves API.

use serde::{Deserialize, Serialize};

// =============================================================================
// REST API Request Types
// =============================================================================

/// TTS synthesis request body for Smallest.ai REST API (Lightning model).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmallestTtsRequest {
    /// Text to convert to speech.
    pub text: String,

    /// Voice identifier.
    pub voice_id: String,

    /// Audio sample rate in Hz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,

    /// Speech speed multiplier (0.5-2.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,

    /// Language code for number pronunciation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Output audio format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
}

impl SmallestTtsRequest {
    /// Creates a new TTS request with required parameters.
    pub fn new(text: impl Into<String>, voice_id: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            voice_id: voice_id.into(),
            sample_rate: None,
            speed: None,
            language: None,
            output_format: None,
        }
    }

    /// Sets the sample rate.
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = Some(sample_rate);
        self
    }

    /// Sets the speech speed.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = Some(speed);
        self
    }

    /// Sets the language.
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Sets the output format.
    pub fn with_output_format(mut self, format: impl Into<String>) -> Self {
        self.output_format = Some(format.into());
        self
    }
}

// =============================================================================
// WebSocket Request Types (Lightning-V2)
// =============================================================================

/// WebSocket TTS request message for Lightning-V2 streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmallestWsRequest {
    /// Voice identifier.
    pub voice_id: String,

    /// Text to convert to speech.
    pub text: String,

    /// Language code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Audio sample rate in Hz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,

    /// Speech speed multiplier (0.1-5.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,

    /// Voice consistency parameter (0-1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consistency: Option<f32>,

    /// Audio enhancement level (0-2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enhancement: Option<u8>,

    /// Voice similarity to reference (0-1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f32>,

    /// Max wait time before flushing output buffer (0-1000ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_buffer_flush_ms: Option<u32>,

    /// Whether to buffer and await more input.
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    pub continue_stream: Option<bool>,

    /// Force current buffer flush.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flush: Option<bool>,

    /// Wait time after last chunk before completion (0-10000ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complete_backoff_ms: Option<u32>,
}

impl SmallestWsRequest {
    /// Creates a new WebSocket TTS request.
    pub fn new(text: impl Into<String>, voice_id: impl Into<String>) -> Self {
        Self {
            voice_id: voice_id.into(),
            text: text.into(),
            language: None,
            sample_rate: None,
            speed: None,
            consistency: None,
            enhancement: None,
            similarity: None,
            max_buffer_flush_ms: None,
            continue_stream: None,
            flush: None,
            complete_backoff_ms: None,
        }
    }

    /// Sets the language.
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
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

    /// Sets the consistency.
    pub fn with_consistency(mut self, consistency: f32) -> Self {
        self.consistency = Some(consistency);
        self
    }

    /// Sets the enhancement level.
    pub fn with_enhancement(mut self, enhancement: u8) -> Self {
        self.enhancement = Some(enhancement);
        self
    }

    /// Sets the similarity.
    pub fn with_similarity(mut self, similarity: f32) -> Self {
        self.similarity = Some(similarity);
        self
    }

    /// Sets continue stream flag.
    pub fn with_continue(mut self, continue_stream: bool) -> Self {
        self.continue_stream = Some(continue_stream);
        self
    }

    /// Sets flush flag.
    pub fn with_flush(mut self, flush: bool) -> Self {
        self.flush = Some(flush);
        self
    }
}

// =============================================================================
// Response Types
// =============================================================================

/// TTS synthesis error response from Smallest.ai API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmallestTtsResponse {
    /// Error type.
    #[serde(default)]
    pub error: Option<String>,

    /// Error message.
    #[serde(default)]
    pub message: Option<String>,
}

impl SmallestTtsResponse {
    /// Check if this is an error response.
    pub fn is_error(&self) -> bool {
        self.error.is_some() || self.message.is_some()
    }

    /// Get the error message.
    pub fn error_message(&self) -> Option<String> {
        self.error.clone().or_else(|| self.message.clone())
    }
}

/// WebSocket chunk response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmallestWsChunkResponse {
    /// Request identifier.
    pub request_id: String,

    /// Response status ("chunk" or "complete").
    pub status: String,

    /// Audio data (present in chunk responses).
    #[serde(default)]
    pub data: Option<SmallestWsAudioData>,

    /// Completion message (present in complete responses).
    #[serde(default)]
    pub message: Option<String>,

    /// Done flag (present in complete responses).
    #[serde(default)]
    pub done: Option<bool>,
}

impl SmallestWsChunkResponse {
    /// Check if this is a chunk with audio data.
    pub fn is_chunk(&self) -> bool {
        self.status == "chunk"
    }

    /// Check if this is a completion message.
    pub fn is_complete(&self) -> bool {
        self.status == "complete"
    }

    /// Get the audio data if present.
    pub fn audio_data(&self) -> Option<&str> {
        self.data.as_ref().map(|d| d.audio.as_str())
    }
}

/// Audio data in WebSocket response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmallestWsAudioData {
    /// Base64-encoded audio data.
    pub audio: String,
}

/// WebSocket error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmallestWsErrorResponse {
    /// Error type.
    pub error: String,

    /// Error message.
    #[serde(default)]
    pub message: Option<String>,
}

// =============================================================================
// Voice Types
// =============================================================================

/// Voice information returned by Get Voices API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmallestVoice {
    /// Voice identifier.
    #[serde(rename = "voiceId")]
    pub voice_id: String,

    /// Display name.
    #[serde(rename = "displayName")]
    pub display_name: String,

    /// Voice tags/metadata.
    #[serde(default)]
    pub tags: SmallestVoiceTags,
}

/// Voice metadata tags.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SmallestVoiceTags {
    /// Supported languages.
    #[serde(default)]
    pub language: Vec<String>,

    /// Regional accent.
    #[serde(default)]
    pub accent: Option<String>,

    /// Voice gender.
    #[serde(default)]
    pub gender: Option<String>,
}

/// Response from Get Voices API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmallestVoicesResponse {
    /// List of available voices.
    pub voices: Vec<SmallestVoice>,
}

// =============================================================================
// Voice Cloning Types
// =============================================================================

/// Response from Add Voice (clone) API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmallestAddVoiceResponse {
    /// Status message.
    pub message: String,

    /// Voice data.
    pub data: SmallestAddVoiceData,
}

/// Voice data in add voice response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmallestAddVoiceData {
    /// Generated voice ID.
    #[serde(rename = "voiceId")]
    pub voice_id: String,

    /// Model used for voice.
    pub model: String,

    /// Voice status.
    pub status: String,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // REST Request Tests
    // =========================================================================

    #[test]
    fn test_tts_request_new() {
        let request = SmallestTtsRequest::new("Hello world", "emily");

        assert_eq!(request.text, "Hello world");
        assert_eq!(request.voice_id, "emily");
        assert!(request.sample_rate.is_none());
        assert!(request.speed.is_none());
    }

    #[test]
    fn test_tts_request_builder() {
        let request = SmallestTtsRequest::new("Hello", "emily")
            .with_sample_rate(24000)
            .with_speed(1.2)
            .with_language("en")
            .with_output_format("wav");

        assert_eq!(request.sample_rate, Some(24000));
        assert!((request.speed.unwrap() - 1.2).abs() < 0.001);
        assert_eq!(request.language, Some("en".to_string()));
        assert_eq!(request.output_format, Some("wav".to_string()));
    }

    #[test]
    fn test_tts_request_serialization() {
        let request = SmallestTtsRequest::new("नमस्ते", "emily")
            .with_sample_rate(24000)
            .with_speed(1.0);

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"text\":\"नमस्ते\""));
        assert!(json.contains("\"voice_id\":\"emily\""));
        assert!(json.contains("\"sample_rate\":24000"));
    }

    // =========================================================================
    // WebSocket Request Tests
    // =========================================================================

    #[test]
    fn test_ws_request_new() {
        let request = SmallestWsRequest::new("Hello world", "emily");

        assert_eq!(request.text, "Hello world");
        assert_eq!(request.voice_id, "emily");
        assert!(request.language.is_none());
    }

    #[test]
    fn test_ws_request_builder() {
        let request = SmallestWsRequest::new("Hello", "emily")
            .with_language("en")
            .with_sample_rate(24000)
            .with_speed(1.5)
            .with_consistency(0.7)
            .with_enhancement(2)
            .with_similarity(0.3)
            .with_continue(true)
            .with_flush(false);

        assert_eq!(request.language, Some("en".to_string()));
        assert_eq!(request.sample_rate, Some(24000));
        assert!((request.speed.unwrap() - 1.5).abs() < 0.001);
        assert!((request.consistency.unwrap() - 0.7).abs() < 0.001);
        assert_eq!(request.enhancement, Some(2));
        assert!((request.similarity.unwrap() - 0.3).abs() < 0.001);
        assert_eq!(request.continue_stream, Some(true));
        assert_eq!(request.flush, Some(false));
    }

    #[test]
    fn test_ws_request_serialization() {
        let request = SmallestWsRequest::new("Test", "emily").with_continue(true);

        let json = serde_json::to_string(&request).unwrap();

        // The "continue" field should be serialized as "continue"
        assert!(json.contains("\"continue\":true"));
    }

    // =========================================================================
    // Response Tests
    // =========================================================================

    #[test]
    fn test_tts_response_is_error() {
        let error_response = SmallestTtsResponse {
            error: Some("invalid_parameter".to_string()),
            message: Some("Voice ID is required".to_string()),
        };

        assert!(error_response.is_error());

        let success_response = SmallestTtsResponse {
            error: None,
            message: None,
        };

        assert!(!success_response.is_error());
    }

    #[test]
    fn test_ws_chunk_response_parsing() {
        let json = r#"{
            "request_id": "abc123",
            "status": "chunk",
            "data": {
                "audio": "SGVsbG8gV29ybGQ="
            }
        }"#;

        let response: SmallestWsChunkResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.request_id, "abc123");
        assert!(response.is_chunk());
        assert!(!response.is_complete());
        assert_eq!(response.audio_data(), Some("SGVsbG8gV29ybGQ="));
    }

    #[test]
    fn test_ws_complete_response_parsing() {
        let json = r#"{
            "request_id": "abc123",
            "status": "complete",
            "message": "All chunks sent",
            "done": true
        }"#;

        let response: SmallestWsChunkResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.request_id, "abc123");
        assert!(!response.is_chunk());
        assert!(response.is_complete());
        assert_eq!(response.message, Some("All chunks sent".to_string()));
        assert_eq!(response.done, Some(true));
    }

    // =========================================================================
    // Voice Tests
    // =========================================================================

    #[test]
    fn test_voice_parsing() {
        let json = r#"{
            "voiceId": "emily",
            "displayName": "Emily",
            "tags": {
                "language": ["en"],
                "accent": "american",
                "gender": "female"
            }
        }"#;

        let voice: SmallestVoice = serde_json::from_str(json).unwrap();

        assert_eq!(voice.voice_id, "emily");
        assert_eq!(voice.display_name, "Emily");
        assert_eq!(voice.tags.language, vec!["en"]);
        assert_eq!(voice.tags.accent, Some("american".to_string()));
        assert_eq!(voice.tags.gender, Some("female".to_string()));
    }

    #[test]
    fn test_voices_response_parsing() {
        let json = r#"{
            "voices": [
                {
                    "voiceId": "emily",
                    "displayName": "Emily",
                    "tags": {}
                },
                {
                    "voiceId": "nyah",
                    "displayName": "Nyah",
                    "tags": {}
                }
            ]
        }"#;

        let response: SmallestVoicesResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.voices.len(), 2);
        assert_eq!(response.voices[0].voice_id, "emily");
        assert_eq!(response.voices[1].voice_id, "nyah");
    }

    // =========================================================================
    // Voice Cloning Tests
    // =========================================================================

    #[test]
    fn test_add_voice_response_parsing() {
        let json = r#"{
            "message": "Voice created successfully",
            "data": {
                "voiceId": "custom_voice_123",
                "model": "lightning-large",
                "status": "ready"
            }
        }"#;

        let response: SmallestAddVoiceResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.message, "Voice created successfully");
        assert_eq!(response.data.voice_id, "custom_voice_123");
        assert_eq!(response.data.model, "lightning-large");
        assert_eq!(response.data.status, "ready");
    }
}
