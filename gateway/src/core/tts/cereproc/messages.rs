//! CereProc CereVoice Cloud API Message Types
//!
//! This module defines request and response structures for the CereVoice Cloud API v2,
//! including authentication, synthesis, and error handling.

use serde::Deserialize;
use std::fmt;

// =============================================================================
// Authentication Messages
// =============================================================================

/// Authentication response from the /auth endpoint
#[derive(Debug, Clone, Deserialize)]
pub struct AuthResponse {
    /// Bearer token for subsequent API calls
    pub token: String,
}

// =============================================================================
// Synthesis Messages
// =============================================================================

/// Response from the /speak endpoint
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeakResponse {
    /// URL to download the generated audio file
    #[serde(alias = "fileUrl")]
    pub file_url: String,

    /// Number of characters processed
    #[serde(alias = "charCount", default)]
    pub char_count: String,

    /// Result code (1 = success)
    #[serde(alias = "resultCode", default)]
    pub result_code: String,

    /// Human-readable result description
    #[serde(alias = "resultDescription", default)]
    pub result_description: String,

    /// Optional metadata (if requested)
    #[serde(default)]
    pub metadata: Option<String>,
}

impl SpeakResponse {
    /// Check if the response indicates success
    pub fn is_success(&self) -> bool {
        self.result_code == "1"
    }

    /// Get the number of characters used as an integer
    pub fn chars_used(&self) -> u64 {
        self.char_count.parse().unwrap_or(0)
    }
}

// =============================================================================
// Credit Messages
// =============================================================================

/// Response from the /credit endpoint
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditResponse {
    /// Free credits remaining
    #[serde(default)]
    pub free_credit: String,

    /// Paid credits remaining
    #[serde(default)]
    pub paid_credit: String,

    /// Total characters available
    #[serde(alias = "charsAvailable", default)]
    pub chars_available: String,
}

impl CreditResponse {
    /// Get total available credits as a number
    pub fn total_available(&self) -> u64 {
        self.chars_available.parse().unwrap_or(0)
    }
}

// =============================================================================
// Voice List Messages
// =============================================================================

/// Response from the /voices endpoint
#[derive(Debug, Clone, Deserialize)]
pub struct VoicesResponse {
    /// List of available voices
    pub voices: Vec<VoiceInfo>,
}

/// Information about a single voice
#[derive(Debug, Clone, Deserialize)]
pub struct VoiceInfo {
    /// Voice name
    pub name: String,

    /// Language code
    #[serde(default)]
    pub language: String,

    /// Gender
    #[serde(default)]
    pub gender: Option<String>,

    /// Whether emotions are supported
    #[serde(default)]
    pub has_emotions: bool,

    /// List of supported emotions
    #[serde(default)]
    pub emotions: Vec<String>,
}

// =============================================================================
// Audio Format List Messages
// =============================================================================

/// Response from the /formats endpoint
#[derive(Debug, Clone, Deserialize)]
pub struct FormatsResponse {
    /// List of available audio formats
    pub formats: Vec<FormatInfo>,
}

/// Information about a single audio format
#[derive(Debug, Clone, Deserialize)]
pub struct FormatInfo {
    /// Format name
    pub name: String,

    /// File extension
    #[serde(default)]
    pub extension: String,

    /// MIME type
    #[serde(default)]
    pub mime_type: String,
}

// =============================================================================
// Lexicon Messages
// =============================================================================

/// Response from uploading a lexicon
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LexiconUploadResponse {
    /// Upload result code
    #[serde(default)]
    pub result_code: String,

    /// Upload result description
    #[serde(default)]
    pub result_description: String,

    /// Lexicon ID if successful
    #[serde(default)]
    pub lexicon_id: Option<String>,
}

/// Response from listing lexicons
#[derive(Debug, Clone, Deserialize)]
pub struct LexiconsListResponse {
    /// List of lexicons
    pub lexicons: Vec<LexiconInfo>,
}

/// Information about a single lexicon
#[derive(Debug, Clone, Deserialize)]
pub struct LexiconInfo {
    /// Lexicon ID
    pub id: String,

    /// Language code
    #[serde(default)]
    pub language: String,

    /// Accent code
    #[serde(default)]
    pub accent: String,

    /// Created timestamp
    #[serde(default)]
    pub created: Option<String>,
}

// =============================================================================
// Error Messages
// =============================================================================

/// CereProc API error response
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CereprocApiError {
    /// Error code
    #[serde(alias = "resultCode", default)]
    pub code: String,

    /// Error message
    #[serde(
        alias = "resultDescription",
        alias = "error",
        alias = "message",
        default
    )]
    pub message: String,

    /// Additional error details
    #[serde(default)]
    pub detail: Option<String>,
}

impl CereprocApiError {
    /// Check if this is an authentication error
    pub fn is_auth_error(&self) -> bool {
        self.code == "-1" || self.message.to_lowercase().contains("auth")
    }

    /// Check if this is a credit/quota error
    pub fn is_credit_error(&self) -> bool {
        self.code == "-2" || self.message.to_lowercase().contains("credit")
    }

    /// Check if this is an invalid voice error
    pub fn is_voice_error(&self) -> bool {
        self.code == "-3" || self.message.to_lowercase().contains("voice")
    }

    /// Check if this is an invalid format error
    pub fn is_format_error(&self) -> bool {
        self.code == "-4" || self.message.to_lowercase().contains("format")
    }

    /// Check if this is a text too long error
    pub fn is_text_length_error(&self) -> bool {
        self.code == "-5" || self.message.to_lowercase().contains("too long")
    }

    /// Get a human-readable error message
    pub fn display_message(&self) -> String {
        if let Some(ref detail) = self.detail {
            format!("{}: {}", self.message, detail)
        } else {
            self.message.clone()
        }
    }
}

impl fmt::Display for CereprocApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CereProc API error ({}): {}", self.code, self.message)
    }
}

impl std::error::Error for CereprocApiError {}

// =============================================================================
// Result Code Enum
// =============================================================================

/// CereProc API result codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultCode {
    /// Success
    Success,
    /// General error
    GeneralError,
    /// Authentication failed
    AuthFailed,
    /// Insufficient credits
    InsufficientCredits,
    /// Invalid voice
    InvalidVoice,
    /// Invalid audio format
    InvalidFormat,
    /// Text too long
    TextTooLong,
    /// Unknown error code
    Unknown(i32),
}

impl ResultCode {
    /// Parse result code from string
    pub fn from_code(code: &str) -> Self {
        match code.parse::<i32>() {
            Ok(1) => Self::Success,
            Ok(0) => Self::GeneralError,
            Ok(-1) => Self::AuthFailed,
            Ok(-2) => Self::InsufficientCredits,
            Ok(-3) => Self::InvalidVoice,
            Ok(-4) => Self::InvalidFormat,
            Ok(-5) => Self::TextTooLong,
            Ok(n) => Self::Unknown(n),
            Err(_) => Self::Unknown(0),
        }
    }

    /// Check if this represents success
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    /// Get a human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::Success => "Success",
            Self::GeneralError => "General error",
            Self::AuthFailed => "Authentication failed",
            Self::InsufficientCredits => "Insufficient credits",
            Self::InvalidVoice => "Invalid voice",
            Self::InvalidFormat => "Invalid audio format",
            Self::TextTooLong => "Text too long",
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
    fn test_auth_response_deserialization() {
        let json = r#"{"token": "abc123xyz"}"#;
        let response: AuthResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.token, "abc123xyz");
    }

    #[test]
    fn test_speak_response_deserialization() {
        let json = r#"{
            "fileUrl": "https://example.com/audio.mp3",
            "charCount": "100",
            "resultCode": "1",
            "resultDescription": "Success"
        }"#;

        let response: SpeakResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.file_url, "https://example.com/audio.mp3");
        assert_eq!(response.char_count, "100");
        assert!(response.is_success());
        assert_eq!(response.chars_used(), 100);
    }

    #[test]
    fn test_speak_response_camel_case() {
        let json = r#"{
            "fileUrl": "https://example.com/audio.mp3",
            "charCount": "50",
            "resultCode": "1",
            "resultDescription": "OK"
        }"#;

        let response: SpeakResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.file_url, "https://example.com/audio.mp3");
        assert_eq!(response.chars_used(), 50);
    }

    #[test]
    fn test_credit_response_deserialization() {
        let json = r#"{
            "freeCredit": "5000",
            "paidCredit": "10000",
            "charsAvailable": "15000"
        }"#;

        let response: CreditResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.free_credit, "5000");
        assert_eq!(response.paid_credit, "10000");
        assert_eq!(response.total_available(), 15000);
    }

    #[test]
    fn test_voice_info_deserialization() {
        let json = r#"{
            "name": "Stuart",
            "language": "en-SC",
            "gender": "male",
            "has_emotions": true,
            "emotions": ["happy", "sad"]
        }"#;

        let voice: VoiceInfo = serde_json::from_str(json).unwrap();
        assert_eq!(voice.name, "Stuart");
        assert_eq!(voice.language, "en-SC");
        assert!(voice.has_emotions);
        assert_eq!(voice.emotions.len(), 2);
    }

    #[test]
    fn test_voice_info_minimal() {
        let json = r#"{"name": "Stuart"}"#;
        let voice: VoiceInfo = serde_json::from_str(json).unwrap();
        assert_eq!(voice.name, "Stuart");
        assert!(voice.language.is_empty());
        assert!(!voice.has_emotions);
    }

    #[test]
    fn test_api_error_deserialization() {
        let json = r#"{
            "resultCode": "-1",
            "resultDescription": "Authentication failed"
        }"#;

        let error: CereprocApiError = serde_json::from_str(json).unwrap();
        assert_eq!(error.code, "-1");
        assert!(error.is_auth_error());
    }

    #[test]
    fn test_api_error_with_detail() {
        let json = r#"{
            "resultCode": "-2",
            "resultDescription": "Insufficient credits",
            "detail": "0 credits remaining"
        }"#;

        let error: CereprocApiError = serde_json::from_str(json).unwrap();
        assert!(error.is_credit_error());
        assert_eq!(
            error.display_message(),
            "Insufficient credits: 0 credits remaining"
        );
    }

    #[test]
    fn test_api_error_display() {
        let error = CereprocApiError {
            code: "-3".to_string(),
            message: "Invalid voice".to_string(),
            detail: None,
        };

        let display = format!("{}", error);
        assert!(display.contains("-3"));
        assert!(display.contains("Invalid voice"));
    }

    #[test]
    fn test_result_code_parsing() {
        assert_eq!(ResultCode::from_code("1"), ResultCode::Success);
        assert_eq!(ResultCode::from_code("0"), ResultCode::GeneralError);
        assert_eq!(ResultCode::from_code("-1"), ResultCode::AuthFailed);
        assert_eq!(ResultCode::from_code("-2"), ResultCode::InsufficientCredits);
        assert_eq!(ResultCode::from_code("-3"), ResultCode::InvalidVoice);
        assert_eq!(ResultCode::from_code("-4"), ResultCode::InvalidFormat);
        assert_eq!(ResultCode::from_code("-5"), ResultCode::TextTooLong);
    }

    #[test]
    fn test_result_code_success() {
        assert!(ResultCode::Success.is_success());
        assert!(!ResultCode::GeneralError.is_success());
        assert!(!ResultCode::AuthFailed.is_success());
    }

    #[test]
    fn test_result_code_unknown() {
        let unknown = ResultCode::from_code("-99");
        assert!(matches!(unknown, ResultCode::Unknown(-99)));
        assert!(!unknown.is_success());
    }

    #[test]
    fn test_error_type_detection() {
        let auth_error = CereprocApiError {
            code: "-1".to_string(),
            message: "Auth failed".to_string(),
            detail: None,
        };
        assert!(auth_error.is_auth_error());

        let credit_error = CereprocApiError {
            code: "-2".to_string(),
            message: "No credit".to_string(),
            detail: None,
        };
        assert!(credit_error.is_credit_error());

        let voice_error = CereprocApiError {
            code: "-3".to_string(),
            message: "Invalid voice specified".to_string(),
            detail: None,
        };
        assert!(voice_error.is_voice_error());
    }

    #[test]
    fn test_format_info_deserialization() {
        let json = r#"{
            "name": "mp3",
            "extension": ".mp3",
            "mime_type": "audio/mpeg"
        }"#;

        let format: FormatInfo = serde_json::from_str(json).unwrap();
        assert_eq!(format.name, "mp3");
        assert_eq!(format.extension, ".mp3");
        assert_eq!(format.mime_type, "audio/mpeg");
    }

    #[test]
    fn test_lexicon_info_deserialization() {
        let json = r#"{
            "id": "lex123",
            "language": "en",
            "accent": "GB"
        }"#;

        let lexicon: LexiconInfo = serde_json::from_str(json).unwrap();
        assert_eq!(lexicon.id, "lex123");
        assert_eq!(lexicon.language, "en");
        assert_eq!(lexicon.accent, "GB");
    }
}
