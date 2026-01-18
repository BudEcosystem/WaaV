//! SberDevices SaluteSpeech STT Message Types
//!
//! Request and response types for SberDevices SaluteSpeech API.

use serde::{Deserialize, Serialize};

// =============================================================================
// OAuth Token Response
// =============================================================================

/// OAuth token response from SberDevices
#[derive(Debug, Clone, Deserialize)]
pub struct OAuthTokenResponse {
    /// JWT access token
    pub access_token: String,
    /// Token expiration timestamp in milliseconds
    pub expires_at: u64,
}

impl OAuthTokenResponse {
    /// Check if the token is expired or about to expire
    pub fn is_expired(&self, threshold_secs: u64) -> bool {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let threshold_ms = threshold_secs * 1000;
        self.expires_at <= now_ms + threshold_ms
    }

    /// Get remaining validity in seconds
    pub fn remaining_secs(&self) -> u64 {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        if self.expires_at > now_ms {
            (self.expires_at - now_ms) / 1000
        } else {
            0
        }
    }
}

// =============================================================================
// Recognition Response
// =============================================================================

/// Recognition response from SberDevices STT
#[derive(Debug, Clone, Deserialize)]
pub struct SberRecognitionResponse {
    /// Recognition results (array of transcriptions)
    pub result: Option<Vec<String>>,
    /// Error status
    pub status: Option<i32>,
    /// Error message
    pub message: Option<String>,
}

impl SberRecognitionResponse {
    /// Check if response contains an error
    pub fn is_error(&self) -> bool {
        self.status.map(|s| s != 200).unwrap_or(false)
    }

    /// Get the first transcription result
    pub fn get_transcript(&self) -> Option<String> {
        self.result.as_ref().and_then(|r| r.first().cloned())
    }

    /// Get all transcriptions joined
    pub fn get_all_transcripts(&self) -> String {
        self.result
            .as_ref()
            .map(|r| r.join(" "))
            .unwrap_or_default()
    }

    /// Get error message
    pub fn error_message(&self) -> String {
        self.message.clone().unwrap_or_else(|| {
            format!(
                "SberDevices STT error: status={}",
                self.status.unwrap_or(-1)
            )
        })
    }
}

// =============================================================================
// API Error
// =============================================================================

/// SberDevices API error
#[derive(Debug, Clone)]
pub struct SberApiError {
    /// HTTP status code
    pub http_status: u16,
    /// Error code from API
    pub error_code: Option<String>,
    /// Error message
    pub message: String,
}

impl SberApiError {
    /// Create from HTTP response
    pub fn from_response(http_status: u16, body: &str) -> Self {
        // Try to parse JSON error response
        #[derive(Deserialize)]
        struct ErrorResponse {
            error: Option<String>,
            error_description: Option<String>,
            message: Option<String>,
            code: Option<String>,
        }

        if let Ok(err) = serde_json::from_str::<ErrorResponse>(body) {
            Self {
                http_status,
                error_code: err.error.or(err.code),
                message: err.error_description.or(err.message).unwrap_or_else(|| {
                    format!("HTTP {} error", http_status)
                }),
            }
        } else {
            Self {
                http_status,
                error_code: None,
                message: if body.is_empty() {
                    format!("HTTP {} error", http_status)
                } else {
                    body.to_string()
                },
            }
        }
    }

    /// Check if this is an authentication error
    pub fn is_auth_error(&self) -> bool {
        self.http_status == 401
            || self.http_status == 403
            || self.error_code.as_deref() == Some("invalid_grant")
            || self.error_code.as_deref() == Some("invalid_client")
    }

    /// Check if this is a rate limit error
    pub fn is_rate_limit_error(&self) -> bool {
        self.http_status == 429
    }

    /// Check if this error is retriable
    pub fn is_retriable(&self) -> bool {
        self.http_status == 429
            || self.http_status == 500
            || self.http_status == 502
            || self.http_status == 503
            || self.http_status == 504
    }

    /// Get display message for logging/errors
    pub fn display_message(&self) -> String {
        if let Some(ref code) = self.error_code {
            format!("[{}] {}: {}", self.http_status, code, self.message)
        } else {
            format!("[{}] {}", self.http_status, self.message)
        }
    }
}

// =============================================================================
// Status Code Helpers
// =============================================================================

/// HTTP status code classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SberStatusCode {
    /// Success (200-299)
    Success,
    /// Bad Request (400)
    BadRequest,
    /// Unauthorized (401)
    Unauthorized,
    /// Forbidden (403)
    Forbidden,
    /// Not Found (404)
    NotFound,
    /// Unprocessable Entity (422)
    UnprocessableEntity,
    /// Rate Limited (429)
    RateLimited,
    /// Server Error (500+)
    ServerError,
    /// Unknown status
    Unknown(u16),
}

impl SberStatusCode {
    /// Create from HTTP status code
    pub fn from_http_status(status: u16) -> Self {
        match status {
            200..=299 => Self::Success,
            400 => Self::BadRequest,
            401 => Self::Unauthorized,
            403 => Self::Forbidden,
            404 => Self::NotFound,
            422 => Self::UnprocessableEntity,
            429 => Self::RateLimited,
            500..=599 => Self::ServerError,
            _ => Self::Unknown(status),
        }
    }

    /// Check if status indicates success
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    /// Check if status indicates a retriable error
    pub fn is_retriable(&self) -> bool {
        matches!(self, Self::RateLimited | Self::ServerError)
    }
}

// =============================================================================
// OAuth Request
// =============================================================================

/// OAuth token request form data
#[derive(Debug, Clone, Serialize)]
pub struct OAuthTokenRequest {
    /// OAuth scope
    pub scope: String,
}

impl OAuthTokenRequest {
    /// Create a new token request
    pub fn new(scope: &str) -> Self {
        Self {
            scope: scope.to_string(),
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
    fn test_oauth_token_response_is_expired() {
        // Create a token that expires in 5 minutes
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let token = OAuthTokenResponse {
            access_token: "test_token".to_string(),
            expires_at: now_ms + 300_000, // 5 minutes from now
        };

        // Should not be expired with 60 second threshold
        assert!(!token.is_expired(60));

        // Should be expired with 6 minute threshold
        assert!(token.is_expired(360));
    }

    #[test]
    fn test_oauth_token_response_remaining_secs() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let token = OAuthTokenResponse {
            access_token: "test_token".to_string(),
            expires_at: now_ms + 300_000, // 5 minutes from now
        };

        let remaining = token.remaining_secs();
        assert!(remaining >= 299 && remaining <= 301);
    }

    #[test]
    fn test_oauth_token_response_expired_token() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let token = OAuthTokenResponse {
            access_token: "test_token".to_string(),
            expires_at: now_ms - 1000, // Already expired
        };

        assert!(token.is_expired(0));
        assert_eq!(token.remaining_secs(), 0);
    }

    #[test]
    fn test_recognition_response_success() {
        let response = SberRecognitionResponse {
            result: Some(vec!["Hello world".to_string()]),
            status: Some(200),
            message: None,
        };

        assert!(!response.is_error());
        assert_eq!(response.get_transcript(), Some("Hello world".to_string()));
    }

    #[test]
    fn test_recognition_response_multiple_results() {
        let response = SberRecognitionResponse {
            result: Some(vec!["Hello".to_string(), "World".to_string()]),
            status: Some(200),
            message: None,
        };

        assert_eq!(response.get_all_transcripts(), "Hello World");
    }

    #[test]
    fn test_recognition_response_error() {
        let response = SberRecognitionResponse {
            result: None,
            status: Some(400),
            message: Some("Invalid audio format".to_string()),
        };

        assert!(response.is_error());
        assert_eq!(response.error_message(), "Invalid audio format");
    }

    #[test]
    fn test_recognition_response_empty() {
        let response = SberRecognitionResponse {
            result: Some(vec![]),
            status: Some(200),
            message: None,
        };

        assert!(!response.is_error());
        assert_eq!(response.get_transcript(), None);
    }

    #[test]
    fn test_sber_api_error_from_json() {
        let body = r#"{"error":"invalid_grant","error_description":"Invalid credentials"}"#;
        let error = SberApiError::from_response(401, body);

        assert_eq!(error.http_status, 401);
        assert_eq!(error.error_code, Some("invalid_grant".to_string()));
        assert_eq!(error.message, "Invalid credentials");
        assert!(error.is_auth_error());
    }

    #[test]
    fn test_sber_api_error_from_plain_text() {
        let error = SberApiError::from_response(500, "Internal Server Error");

        assert_eq!(error.http_status, 500);
        assert_eq!(error.error_code, None);
        assert_eq!(error.message, "Internal Server Error");
        assert!(error.is_retriable());
    }

    #[test]
    fn test_sber_api_error_rate_limit() {
        let error = SberApiError::from_response(429, "Too Many Requests");

        assert!(error.is_rate_limit_error());
        assert!(error.is_retriable());
        assert!(!error.is_auth_error());
    }

    #[test]
    fn test_status_code_classification() {
        assert!(SberStatusCode::from_http_status(200).is_success());
        assert!(SberStatusCode::from_http_status(201).is_success());
        assert!(!SberStatusCode::from_http_status(400).is_success());
        assert!(SberStatusCode::from_http_status(429).is_retriable());
        assert!(SberStatusCode::from_http_status(503).is_retriable());
        assert!(!SberStatusCode::from_http_status(401).is_retriable());
    }

    #[test]
    fn test_oauth_token_request() {
        let request = OAuthTokenRequest::new("SALUTE_SPEECH_PERS");
        assert_eq!(request.scope, "SALUTE_SPEECH_PERS");
    }

    #[test]
    fn test_sber_api_error_display_message() {
        let error = SberApiError {
            http_status: 401,
            error_code: Some("invalid_client".to_string()),
            message: "Invalid credentials".to_string(),
        };

        assert_eq!(
            error.display_message(),
            "[401] invalid_client: Invalid credentials"
        );

        let error_no_code = SberApiError {
            http_status: 500,
            error_code: None,
            message: "Server error".to_string(),
        };

        assert_eq!(error_no_code.display_message(), "[500] Server error");
    }
}
