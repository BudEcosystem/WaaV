//! Yandex SpeechKit TTS API Message Types
//!
//! This module defines response and error structures for the Yandex SpeechKit TTS API.

use serde::Deserialize;
use std::fmt;

// =============================================================================
// Synthesis Response
// =============================================================================

/// Successful synthesis response
///
/// Note: The API returns raw audio bytes, not JSON.
/// This struct is used for metadata if needed.
#[derive(Debug, Clone)]
pub struct YandexSynthesisResponse {
    /// Audio data bytes
    pub audio_data: Vec<u8>,
    /// Content type of the audio
    pub content_type: String,
}

impl YandexSynthesisResponse {
    /// Create a new synthesis response
    pub fn new(audio_data: Vec<u8>, content_type: String) -> Self {
        Self {
            audio_data,
            content_type,
        }
    }

    /// Get the audio data
    pub fn audio(&self) -> &[u8] {
        &self.audio_data
    }

    /// Get the audio length in bytes
    pub fn len(&self) -> usize {
        self.audio_data.len()
    }

    /// Check if the audio data is empty
    pub fn is_empty(&self) -> bool {
        self.audio_data.is_empty()
    }
}

// =============================================================================
// Error Response
// =============================================================================

/// Yandex API error response
#[derive(Debug, Clone, Deserialize)]
pub struct YandexApiError {
    /// Error code
    #[serde(default)]
    pub code: i32,

    /// Error message
    #[serde(default)]
    pub message: String,

    /// Detailed error information
    #[serde(default)]
    pub details: Option<Vec<YandexErrorDetail>>,
}

/// Detailed error information
#[derive(Debug, Clone, Deserialize)]
pub struct YandexErrorDetail {
    /// Error type
    #[serde(rename = "@type", default)]
    pub error_type: String,

    /// Detailed message
    #[serde(default)]
    pub message: Option<String>,

    /// Error reason
    #[serde(default)]
    pub reason: Option<String>,
}

impl YandexApiError {
    /// Create an error from HTTP status and body
    pub fn from_response(status: u16, body: &str) -> Self {
        // Try to parse as JSON first
        if let Ok(error) = serde_json::from_str::<YandexApiError>(body) {
            return error;
        }

        // Fall back to generic error
        Self {
            code: status as i32,
            message: if body.is_empty() {
                format!("HTTP error {}", status)
            } else {
                body.to_string()
            },
            details: None,
        }
    }

    /// Check if this is an authentication error
    pub fn is_auth_error(&self) -> bool {
        self.code == 401 || self.code == 403 || self.message.to_lowercase().contains("auth")
    }

    /// Check if this is a rate limit error
    pub fn is_rate_limit_error(&self) -> bool {
        self.code == 429 || self.message.to_lowercase().contains("rate")
    }

    /// Check if this is a validation error
    pub fn is_validation_error(&self) -> bool {
        self.code == 400 || self.message.to_lowercase().contains("invalid")
    }

    /// Check if this is a quota/credit error
    pub fn is_quota_error(&self) -> bool {
        self.code == 402 || self.message.to_lowercase().contains("quota")
    }

    /// Get a human-readable error message
    pub fn display_message(&self) -> String {
        if let Some(ref details) = self.details
            && let Some(detail) = details.first()
        {
            if let Some(ref msg) = detail.message {
                return format!("{}: {}", self.message, msg);
            }
            if let Some(ref reason) = detail.reason {
                return format!("{}: {}", self.message, reason);
            }
        }
        self.message.clone()
    }
}

impl fmt::Display for YandexApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Yandex API error ({}): {}", self.code, self.message)
    }
}

impl std::error::Error for YandexApiError {}

// =============================================================================
// gRPC Response Types (for future v3 API support)
// =============================================================================

/// Audio chunk from streaming synthesis
#[derive(Debug, Clone)]
pub struct YandexAudioChunk {
    /// Audio data
    pub data: Vec<u8>,
    /// Chunk index
    pub chunk_index: u32,
}

/// Synthesis completion metadata
#[derive(Debug, Clone, Default)]
pub struct YandexSynthesisMetadata {
    /// Number of characters synthesized
    pub characters_count: u32,
    /// Duration of synthesized audio in milliseconds
    pub duration_ms: u32,
}

// =============================================================================
// HTTP Status Code Mapping
// =============================================================================

/// Yandex API HTTP status codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YandexStatusCode {
    /// Success
    Ok,
    /// Bad request - invalid parameters
    BadRequest,
    /// Unauthorized - invalid API key or token
    Unauthorized,
    /// Forbidden - access denied
    Forbidden,
    /// Not found
    NotFound,
    /// Request timeout
    Timeout,
    /// Payload too large
    PayloadTooLarge,
    /// Too many requests - rate limited
    TooManyRequests,
    /// Internal server error
    InternalError,
    /// Service unavailable
    ServiceUnavailable,
    /// Unknown status code
    Unknown(u16),
}

impl YandexStatusCode {
    /// Create from HTTP status code
    pub fn from_http_status(status: u16) -> Self {
        match status {
            200 | 201 => Self::Ok,
            400 => Self::BadRequest,
            401 => Self::Unauthorized,
            403 => Self::Forbidden,
            404 => Self::NotFound,
            408 => Self::Timeout,
            413 => Self::PayloadTooLarge,
            429 => Self::TooManyRequests,
            500 => Self::InternalError,
            503 => Self::ServiceUnavailable,
            _ => Self::Unknown(status),
        }
    }

    /// Check if the status indicates success
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Ok)
    }

    /// Check if the error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::TooManyRequests | Self::InternalError | Self::ServiceUnavailable
        )
    }

    /// Get a description of the status
    pub fn description(&self) -> &'static str {
        match self {
            Self::Ok => "Success",
            Self::BadRequest => "Bad request - check parameters",
            Self::Unauthorized => "Unauthorized - check API key",
            Self::Forbidden => "Forbidden - check permissions",
            Self::NotFound => "Not found",
            Self::Timeout => "Request timeout",
            Self::PayloadTooLarge => "Payload too large - reduce text length",
            Self::TooManyRequests => "Rate limited - slow down requests",
            Self::InternalError => "Internal server error",
            Self::ServiceUnavailable => "Service temporarily unavailable",
            Self::Unknown(_) => "Unknown error",
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_synthesis_response() {
        let response = YandexSynthesisResponse::new(vec![1, 2, 3, 4], "audio/mpeg".to_string());
        assert_eq!(response.len(), 4);
        assert!(!response.is_empty());
        assert_eq!(response.audio(), &[1, 2, 3, 4]);
        assert_eq!(response.content_type, "audio/mpeg");
    }

    #[test]
    fn test_api_error_from_json() {
        let json = r#"{"code": 400, "message": "Invalid parameter"}"#;
        let error: YandexApiError = serde_json::from_str(json).unwrap();
        assert_eq!(error.code, 400);
        assert_eq!(error.message, "Invalid parameter");
    }

    #[test]
    fn test_api_error_with_details() {
        let json = r#"{
            "code": 401,
            "message": "Unauthorized",
            "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "INVALID_TOKEN"}]
        }"#;
        let error: YandexApiError = serde_json::from_str(json).unwrap();
        assert!(error.is_auth_error());
        assert!(error.details.is_some());
    }

    #[test]
    fn test_api_error_from_response() {
        let error = YandexApiError::from_response(500, "Internal error");
        assert_eq!(error.code, 500);
        assert_eq!(error.message, "Internal error");
    }

    #[test]
    fn test_api_error_from_empty_response() {
        let error = YandexApiError::from_response(404, "");
        assert_eq!(error.code, 404);
        assert_eq!(error.message, "HTTP error 404");
    }

    #[test]
    fn test_api_error_display() {
        let error = YandexApiError {
            code: 400,
            message: "Bad request".to_string(),
            details: None,
        };
        let display = format!("{}", error);
        assert!(display.contains("400"));
        assert!(display.contains("Bad request"));
    }

    #[test]
    fn test_status_code_from_http() {
        assert_eq!(
            YandexStatusCode::from_http_status(200),
            YandexStatusCode::Ok
        );
        assert_eq!(
            YandexStatusCode::from_http_status(401),
            YandexStatusCode::Unauthorized
        );
        assert_eq!(
            YandexStatusCode::from_http_status(429),
            YandexStatusCode::TooManyRequests
        );
    }

    #[test]
    fn test_status_code_is_success() {
        assert!(YandexStatusCode::Ok.is_success());
        assert!(!YandexStatusCode::BadRequest.is_success());
    }

    #[test]
    fn test_status_code_is_retryable() {
        assert!(YandexStatusCode::TooManyRequests.is_retryable());
        assert!(YandexStatusCode::ServiceUnavailable.is_retryable());
        assert!(!YandexStatusCode::BadRequest.is_retryable());
        assert!(!YandexStatusCode::Unauthorized.is_retryable());
    }

    #[test]
    fn test_error_type_detection() {
        let auth_error = YandexApiError {
            code: 401,
            message: "auth failed".to_string(),
            details: None,
        };
        assert!(auth_error.is_auth_error());

        let rate_limit_error = YandexApiError {
            code: 429,
            message: "rate limit exceeded".to_string(),
            details: None,
        };
        assert!(rate_limit_error.is_rate_limit_error());

        let validation_error = YandexApiError {
            code: 400,
            message: "invalid parameter".to_string(),
            details: None,
        };
        assert!(validation_error.is_validation_error());
    }

    #[test]
    fn test_display_message_with_details() {
        let error = YandexApiError {
            code: 401,
            message: "Unauthorized".to_string(),
            details: Some(vec![YandexErrorDetail {
                error_type: "error".to_string(),
                message: Some("Token expired".to_string()),
                reason: None,
            }]),
        };
        assert_eq!(error.display_message(), "Unauthorized: Token expired");
    }
}
