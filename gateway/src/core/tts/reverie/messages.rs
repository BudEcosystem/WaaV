//! Reverie TTS API Message Types
//!
//! This module defines request and response structures for the Reverie TTS API.

use serde::{Deserialize, Serialize};

// =============================================================================
// Request Types
// =============================================================================

/// TTS synthesis request body for Reverie API.
///
/// Either `text` or `ssml` must be provided, but not both.
/// If `ssml` is provided, it takes precedence over `text`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReverieTtsRequest {
    /// Plain text input (can be a single string or array).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<Vec<String>>,

    /// SSML input (takes precedence over text).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssml: Option<String>,

    /// Speech rate (0.5 = slow, 1.0 = normal, 1.5 = fast).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,

    /// Pitch adjustment in semitones (-3 to +3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f32>,

    /// Output sample rate in Hz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,

    /// Output audio format ("WAV" or "MP3").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

impl ReverieTtsRequest {
    /// Create a new text-based TTS request.
    pub fn with_text(text: impl Into<String>) -> Self {
        Self {
            text: Some(vec![text.into()]),
            ssml: None,
            speed: None,
            pitch: None,
            sample_rate: None,
            format: None,
        }
    }

    /// Create a new text-based TTS request with multiple strings.
    pub fn with_texts(texts: Vec<String>) -> Self {
        Self {
            text: Some(texts),
            ssml: None,
            speed: None,
            pitch: None,
            sample_rate: None,
            format: None,
        }
    }

    /// Create a new SSML-based TTS request.
    pub fn with_ssml(ssml: impl Into<String>) -> Self {
        Self {
            text: None,
            ssml: Some(ssml.into()),
            speed: None,
            pitch: None,
            sample_rate: None,
            format: None,
        }
    }

    /// Set the speech speed.
    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = Some(speed);
        self
    }

    /// Set the pitch adjustment.
    pub fn pitch(mut self, pitch: f32) -> Self {
        self.pitch = Some(pitch);
        self
    }

    /// Set the sample rate.
    pub fn sample_rate(mut self, rate: u32) -> Self {
        self.sample_rate = Some(rate);
        self
    }

    /// Set the output format.
    pub fn format(mut self, format: impl Into<String>) -> Self {
        self.format = Some(format.into());
        self
    }
}

impl Default for ReverieTtsRequest {
    fn default() -> Self {
        Self {
            text: None,
            ssml: None,
            speed: Some(1.0),
            pitch: Some(0.0),
            sample_rate: Some(22050),
            format: Some("WAV".to_string()),
        }
    }
}

// =============================================================================
// Response Types
// =============================================================================

/// TTS synthesis response from Reverie API.
///
/// Successful responses return binary audio data directly.
/// Error responses return JSON with error details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReverieTtsResponse {
    /// Error message (only present on failure).
    #[serde(default)]
    pub error: Option<String>,

    /// Error code (only present on failure).
    #[serde(default)]
    pub code: Option<String>,

    /// Error details (only present on failure).
    #[serde(default)]
    pub message: Option<String>,
}

impl ReverieTtsResponse {
    /// Check if this is an error response.
    pub fn is_error(&self) -> bool {
        self.error.is_some() || self.code.is_some()
    }

    /// Get the error message.
    pub fn error_message(&self) -> Option<String> {
        self.error
            .clone()
            .or_else(|| self.message.clone())
            .or_else(|| self.code.clone().map(|c| format!("Error code: {}", c)))
    }
}

// =============================================================================
// Error Types
// =============================================================================

/// Reverie API error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReverieApiError {
    /// Error message.
    #[serde(default)]
    pub error: String,

    /// Error code.
    #[serde(default)]
    pub code: Option<String>,

    /// Additional message.
    #[serde(default)]
    pub message: Option<String>,
}

impl ReverieApiError {
    /// Check if this is an authentication error.
    pub fn is_auth_error(&self) -> bool {
        self.code.as_deref() == Some("401")
            || self.code.as_deref() == Some("403")
            || self.error.to_lowercase().contains("auth")
            || self.error.to_lowercase().contains("unauthorized")
    }

    /// Check if this is a rate limit error.
    pub fn is_rate_limit_error(&self) -> bool {
        self.code.as_deref() == Some("429")
            || self.error.to_lowercase().contains("rate")
            || self.error.to_lowercase().contains("quota")
    }

    /// Check if this is an invalid request error.
    pub fn is_invalid_request(&self) -> bool {
        self.code.as_deref() == Some("400")
            || self.error.to_lowercase().contains("invalid")
            || self.error.to_lowercase().contains("bad request")
    }

    /// Get a human-readable error message.
    pub fn display_message(&self) -> String {
        if let Some(ref msg) = self.message {
            if !msg.is_empty() {
                return format!("{}: {}", self.error, msg);
            }
        }
        self.error.clone()
    }
}

impl std::fmt::Display for ReverieApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reverie API error: {}", self.display_message())
    }
}

impl std::error::Error for ReverieApiError {}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Request Tests
    // =========================================================================

    #[test]
    fn test_request_with_text() {
        let request = ReverieTtsRequest::with_text("Hello world");

        assert_eq!(request.text, Some(vec!["Hello world".to_string()]));
        assert!(request.ssml.is_none());
    }

    #[test]
    fn test_request_with_texts() {
        let request = ReverieTtsRequest::with_texts(vec![
            "Hello".to_string(),
            "World".to_string(),
        ]);

        assert_eq!(request.text.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_request_with_ssml() {
        let request = ReverieTtsRequest::with_ssml("<speak>Hello</speak>");

        assert!(request.text.is_none());
        assert_eq!(request.ssml, Some("<speak>Hello</speak>".to_string()));
    }

    #[test]
    fn test_request_builder() {
        let request = ReverieTtsRequest::with_text("Test")
            .speed(1.2)
            .pitch(1.0)
            .sample_rate(16000)
            .format("MP3");

        assert_eq!(request.speed, Some(1.2));
        assert_eq!(request.pitch, Some(1.0));
        assert_eq!(request.sample_rate, Some(16000));
        assert_eq!(request.format, Some("MP3".to_string()));
    }

    #[test]
    fn test_request_serialization() {
        let request = ReverieTtsRequest::with_text("नमस्ते")
            .speed(1.0)
            .sample_rate(22050)
            .format("WAV");

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"text\":[\"नमस्ते\"]"));
        assert!(json.contains("\"speed\":1.0"));
        assert!(json.contains("\"sample_rate\":22050"));
        assert!(json.contains("\"format\":\"WAV\""));
    }

    // =========================================================================
    // Response Tests
    // =========================================================================

    #[test]
    fn test_response_is_error() {
        let error_response = ReverieTtsResponse {
            error: Some("Invalid API key".to_string()),
            code: None,
            message: None,
        };

        assert!(error_response.is_error());

        let success_response = ReverieTtsResponse {
            error: None,
            code: None,
            message: None,
        };

        assert!(!success_response.is_error());
    }

    #[test]
    fn test_response_error_message() {
        let response = ReverieTtsResponse {
            error: Some("Authentication failed".to_string()),
            code: None,
            message: None,
        };

        assert_eq!(
            response.error_message(),
            Some("Authentication failed".to_string())
        );
    }

    // =========================================================================
    // API Error Tests
    // =========================================================================

    #[test]
    fn test_api_error_is_auth_error() {
        let error = ReverieApiError {
            error: "Unauthorized".to_string(),
            code: Some("401".to_string()),
            message: None,
        };

        assert!(error.is_auth_error());
    }

    #[test]
    fn test_api_error_is_rate_limit_error() {
        let error = ReverieApiError {
            error: "Rate limit exceeded".to_string(),
            code: Some("429".to_string()),
            message: None,
        };

        assert!(error.is_rate_limit_error());
    }

    #[test]
    fn test_api_error_is_invalid_request() {
        let error = ReverieApiError {
            error: "Invalid speaker code".to_string(),
            code: Some("400".to_string()),
            message: None,
        };

        assert!(error.is_invalid_request());
    }

    #[test]
    fn test_api_error_display_message() {
        let error = ReverieApiError {
            error: "Error occurred".to_string(),
            code: None,
            message: Some("Additional details".to_string()),
        };

        assert_eq!(error.display_message(), "Error occurred: Additional details");
    }

    #[test]
    fn test_api_error_display_message_simple() {
        let error = ReverieApiError {
            error: "Simple error".to_string(),
            code: None,
            message: None,
        };

        assert_eq!(error.display_message(), "Simple error");
    }
}
