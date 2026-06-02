//! Yandex SpeechKit STT API Message Types
//!
//! Request and response structures for the Yandex SpeechKit STT API.

use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Synchronous Recognition Response
// =============================================================================

/// Synchronous recognition response from Yandex STT API v1
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YandexSyncResponse {
    /// Recognition result text
    pub result: Option<String>,
    /// Error if recognition failed
    pub error: Option<YandexSTTApiError>,
}

impl YandexSyncResponse {
    /// Get the transcript if available
    pub fn transcript(&self) -> Option<&str> {
        self.result.as_deref()
    }

    /// Check if this is an error response
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }
}

// =============================================================================
// Streaming Recognition Messages (for future gRPC v3 support)
// =============================================================================

/// Streaming recognition request specification
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamingRecognitionConfig {
    /// Language code
    pub language_code: String,
    /// Recognition model
    pub model: String,
    /// Enable partial (interim) results
    pub partial_results: bool,
    /// Audio encoding
    pub audio_encoding: String,
    /// Sample rate in Hz
    pub sample_rate_hertz: u32,
    /// Number of audio channels
    pub audio_channels_count: u32,
    /// Enable automatic punctuation
    pub raw_results: bool,
}

/// Streaming recognition result
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamingRecognitionResult {
    /// Recognition alternatives
    pub alternatives: Vec<RecognitionAlternative>,
    /// Channel tag (for multi-channel audio)
    pub channel_tag: Option<u32>,
    /// Whether this is a final result
    pub is_final: Option<bool>,
    /// End of utterance detection
    pub end_of_utterance: Option<bool>,
}

impl StreamingRecognitionResult {
    /// Get the best (first) alternative transcript
    pub fn best_transcript(&self) -> Option<&str> {
        self.alternatives.first().map(|a| a.text.as_str())
    }

    /// Get the best confidence score
    pub fn best_confidence(&self) -> f32 {
        self.alternatives
            .first()
            .and_then(|a| a.confidence)
            .unwrap_or(0.0)
    }

    /// Check if this is a final result
    pub fn is_final(&self) -> bool {
        self.is_final.unwrap_or(false)
    }

    /// Check if this marks end of utterance
    pub fn is_end_of_utterance(&self) -> bool {
        self.end_of_utterance.unwrap_or(false)
    }
}

/// Recognition alternative with text and confidence
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecognitionAlternative {
    /// Recognized text
    pub text: String,
    /// Confidence score (0.0 to 1.0)
    pub confidence: Option<f32>,
    /// Word-level timestamps
    pub words: Option<Vec<WordInfo>>,
}

/// Word-level timing information
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WordInfo {
    /// The word text
    pub word: String,
    /// Start time offset in milliseconds
    pub start_time_ms: Option<u64>,
    /// End time offset in milliseconds
    pub end_time_ms: Option<u64>,
}

// =============================================================================
// API Error Response
// =============================================================================

/// Yandex STT API error response
#[derive(Debug, Clone, Deserialize)]
pub struct YandexSTTApiError {
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

impl YandexSTTApiError {
    /// Create an error from HTTP status and body
    pub fn from_response(status: u16, body: &str) -> Self {
        // Try to parse as JSON first
        if let Ok(error) = serde_json::from_str::<YandexSTTApiError>(body) {
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
            && let Some(detail) = details.first() {
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

impl fmt::Display for YandexSTTApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Yandex STT API error ({}): {}", self.code, self.message)
    }
}

impl std::error::Error for YandexSTTApiError {}

// =============================================================================
// HTTP Status Code Mapping
// =============================================================================

/// Yandex STT API HTTP status codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YandexSTTStatusCode {
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
    /// Payload too large (max 1MB for sync)
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

impl YandexSTTStatusCode {
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
            Self::PayloadTooLarge => "Audio too large - max 1MB for sync recognition",
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
    fn test_sync_response_transcript() {
        let response = YandexSyncResponse {
            result: Some("Hello world".to_string()),
            error: None,
        };
        assert_eq!(response.transcript(), Some("Hello world"));
        assert!(!response.is_error());
    }

    #[test]
    fn test_sync_response_error() {
        let response = YandexSyncResponse {
            result: None,
            error: Some(YandexSTTApiError {
                code: 400,
                message: "Bad request".to_string(),
                details: None,
            }),
        };
        assert!(response.is_error());
        assert!(response.transcript().is_none());
    }

    #[test]
    fn test_streaming_result() {
        let result = StreamingRecognitionResult {
            alternatives: vec![RecognitionAlternative {
                text: "Hello world".to_string(),
                confidence: Some(0.95),
                words: None,
            }],
            channel_tag: None,
            is_final: Some(true),
            end_of_utterance: Some(true),
        };

        assert_eq!(result.best_transcript(), Some("Hello world"));
        assert_eq!(result.best_confidence(), 0.95);
        assert!(result.is_final());
        assert!(result.is_end_of_utterance());
    }

    #[test]
    fn test_api_error_from_response() {
        let error = YandexSTTApiError::from_response(401, "Unauthorized");
        assert_eq!(error.code, 401);
        assert_eq!(error.message, "Unauthorized");
    }

    #[test]
    fn test_api_error_from_json() {
        let json = r#"{"code": 400, "message": "Invalid parameter"}"#;
        let error: YandexSTTApiError = serde_json::from_str(json).unwrap();
        assert_eq!(error.code, 400);
        assert_eq!(error.message, "Invalid parameter");
    }

    #[test]
    fn test_api_error_display() {
        let error = YandexSTTApiError {
            code: 401,
            message: "Unauthorized".to_string(),
            details: None,
        };
        let display = format!("{}", error);
        assert!(display.contains("401"));
        assert!(display.contains("Unauthorized"));
    }

    #[test]
    fn test_status_code_from_http() {
        assert_eq!(
            YandexSTTStatusCode::from_http_status(200),
            YandexSTTStatusCode::Ok
        );
        assert_eq!(
            YandexSTTStatusCode::from_http_status(401),
            YandexSTTStatusCode::Unauthorized
        );
        assert_eq!(
            YandexSTTStatusCode::from_http_status(429),
            YandexSTTStatusCode::TooManyRequests
        );
    }

    #[test]
    fn test_status_code_is_success() {
        assert!(YandexSTTStatusCode::Ok.is_success());
        assert!(!YandexSTTStatusCode::BadRequest.is_success());
    }

    #[test]
    fn test_status_code_is_retryable() {
        assert!(YandexSTTStatusCode::TooManyRequests.is_retryable());
        assert!(YandexSTTStatusCode::ServiceUnavailable.is_retryable());
        assert!(!YandexSTTStatusCode::BadRequest.is_retryable());
        assert!(!YandexSTTStatusCode::Unauthorized.is_retryable());
    }

    #[test]
    fn test_error_type_detection() {
        let auth_error = YandexSTTApiError {
            code: 401,
            message: "auth failed".to_string(),
            details: None,
        };
        assert!(auth_error.is_auth_error());

        let rate_limit_error = YandexSTTApiError {
            code: 429,
            message: "rate limit exceeded".to_string(),
            details: None,
        };
        assert!(rate_limit_error.is_rate_limit_error());

        let validation_error = YandexSTTApiError {
            code: 400,
            message: "invalid parameter".to_string(),
            details: None,
        };
        assert!(validation_error.is_validation_error());
    }
}
