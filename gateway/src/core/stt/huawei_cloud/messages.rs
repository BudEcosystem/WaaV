//! Huawei Cloud SIS WebSocket Message Types
//!
//! This module defines the message structures for Huawei Cloud Speech Interaction
//! Service (SIS) real-time ASR WebSocket API.

use serde::{Deserialize, Serialize};

// =============================================================================
// WebSocket Commands
// =============================================================================

/// WebSocket command types for Huawei Cloud RASR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HuaweiWsCommand {
    /// Start recognition session.
    Start,
    /// End recognition session.
    End,
    /// Cancel recognition session.
    Cancel,
}

// =============================================================================
// Start Frame
// =============================================================================

/// WebSocket START frame for initiating recognition.
#[derive(Debug, Clone, Serialize)]
pub struct HuaweiStartFrame {
    /// Command type (always "START").
    pub command: HuaweiWsCommand,

    /// Recognition configuration.
    pub config: HuaweiStartConfig,
}

/// Configuration for START frame.
#[derive(Debug, Clone, Serialize)]
pub struct HuaweiStartConfig {
    /// Audio format (e.g., "pcm16k16bit").
    pub audio_format: String,

    /// Recognition property/model (e.g., "chinese_16k_general").
    pub property: String,

    /// Add punctuation to results ("yes" or "no").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add_punc: Option<String>,

    /// Normalize digits ("yes" or "no").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digit_norm: Option<String>,

    /// Custom vocabulary table ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vocabulary_id: Option<String>,

    /// Include word-level timing ("yes" or "no").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub need_word_info: Option<String>,

    /// Interim results control ("yes" or "no").
    /// Default is "yes" for streaming mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interim_results: Option<String>,

    /// Maximum audio silence before auto-ending (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_seconds_of_silence: Option<u32>,
}

impl HuaweiStartFrame {
    /// Create a new START frame.
    pub fn new(
        audio_format: &str,
        property: &str,
        add_punc: bool,
        digit_norm: bool,
        vocabulary_id: Option<&str>,
        need_word_info: bool,
    ) -> Self {
        Self {
            command: HuaweiWsCommand::Start,
            config: HuaweiStartConfig {
                audio_format: audio_format.to_string(),
                property: property.to_string(),
                add_punc: Some(if add_punc { "yes" } else { "no" }.to_string()),
                digit_norm: Some(if digit_norm { "yes" } else { "no" }.to_string()),
                vocabulary_id: vocabulary_id.map(|s| s.to_string()),
                need_word_info: Some(if need_word_info { "yes" } else { "no" }.to_string()),
                interim_results: Some("yes".to_string()),
                max_seconds_of_silence: None,
            },
        }
    }

    /// Set maximum silence duration.
    pub fn with_max_silence(mut self, seconds: u32) -> Self {
        self.config.max_seconds_of_silence = Some(seconds);
        self
    }

    /// Disable interim results.
    pub fn without_interim_results(mut self) -> Self {
        self.config.interim_results = Some("no".to_string());
        self
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// =============================================================================
// End Frame
// =============================================================================

/// WebSocket END frame for ending recognition.
#[derive(Debug, Clone, Serialize)]
pub struct HuaweiEndFrame {
    /// Command type (always "END").
    pub command: HuaweiWsCommand,
}

impl HuaweiEndFrame {
    /// Create a new END frame.
    pub fn new() -> Self {
        Self {
            command: HuaweiWsCommand::End,
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Default for HuaweiEndFrame {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Cancel Frame
// =============================================================================

/// WebSocket CANCEL frame for cancelling recognition.
#[derive(Debug, Clone, Serialize)]
pub struct HuaweiCancelFrame {
    /// Command type (always "CANCEL").
    pub command: HuaweiWsCommand,
}

impl HuaweiCancelFrame {
    /// Create a new CANCEL frame.
    pub fn new() -> Self {
        Self {
            command: HuaweiWsCommand::Cancel,
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Default for HuaweiCancelFrame {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Response Types
// =============================================================================

/// Response type indicators from Huawei Cloud ASR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HuaweiResponseType {
    /// Interim/partial result.
    Result,
    /// Final result for utterance.
    #[serde(alias = "END")]
    End,
    /// Error response.
    Error,
    /// Session started confirmation.
    Started,
    /// Session ended confirmation.
    Ended,
    /// Unknown response type.
    #[serde(other)]
    Unknown,
}

// =============================================================================
// Real-time ASR Response
// =============================================================================

/// Real-time ASR WebSocket response.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiRealtimeResponse {
    /// Response type.
    pub resp_type: HuaweiResponseType,

    /// Trace ID for debugging.
    pub trace_id: Option<String>,

    /// Recognition result.
    pub result: Option<HuaweiAsrResult>,

    /// Error code (0 = success).
    #[serde(default)]
    pub error_code: i32,

    /// Error message.
    pub error_msg: Option<String>,
}

/// ASR recognition result.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiAsrResult {
    /// Recognized text.
    pub text: String,

    /// Confidence score (0.0 - 1.0).
    pub score: Option<f32>,

    /// Whether this is the final result.
    #[serde(default)]
    pub is_final: bool,

    /// Word-level timing information.
    pub word_info: Option<Vec<HuaweiWordInfo>>,
}

/// Word-level timing information.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiWordInfo {
    /// Word text.
    pub word: String,

    /// Start time in milliseconds.
    pub start_time: u32,

    /// End time in milliseconds.
    pub end_time: u32,

    /// Confidence score for this word.
    pub score: Option<f32>,
}

impl HuaweiRealtimeResponse {
    /// Parse response from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if response indicates success.
    pub fn is_success(&self) -> bool {
        self.error_code == 0
    }

    /// Check if response is an error.
    pub fn is_error(&self) -> bool {
        self.resp_type == HuaweiResponseType::Error || self.error_code != 0
    }

    /// Check if this is an interim/partial result.
    pub fn is_interim(&self) -> bool {
        self.resp_type == HuaweiResponseType::Result
            && self.result.as_ref().is_some_and(|r| !r.is_final)
    }

    /// Check if this is a final result.
    pub fn is_final(&self) -> bool {
        self.resp_type == HuaweiResponseType::End
            || self.result.as_ref().is_some_and(|r| r.is_final)
    }

    /// Check if session has started.
    pub fn is_started(&self) -> bool {
        self.resp_type == HuaweiResponseType::Started
    }

    /// Check if session has ended.
    pub fn is_ended(&self) -> bool {
        self.resp_type == HuaweiResponseType::Ended
    }

    /// Get the transcript text.
    pub fn get_transcript(&self) -> Option<&str> {
        self.result.as_ref().map(|r| r.text.as_str())
    }

    /// Get the confidence score.
    pub fn get_confidence(&self) -> Option<f32> {
        self.result.as_ref().and_then(|r| r.score)
    }

    /// Get word-level timing information.
    pub fn get_word_info(&self) -> Option<&[HuaweiWordInfo]> {
        self.result.as_ref().and_then(|r| r.word_info.as_deref())
    }

    /// Get error message.
    pub fn get_error(&self) -> Option<String> {
        if self.is_error() {
            Some(format!(
                "Error {}: {}",
                self.error_code,
                self.error_msg.as_deref().unwrap_or("Unknown error")
            ))
        } else {
            None
        }
    }
}

// =============================================================================
// Short Audio ASR Request
// =============================================================================

/// Short sentence recognition HTTP request body.
#[derive(Debug, Clone, Serialize)]
pub struct HuaweiShortAsrRequest {
    /// Base64-encoded audio data.
    pub data: String,

    /// Recognition configuration.
    pub config: HuaweiShortAsrConfig,
}

/// Configuration for short sentence recognition.
#[derive(Debug, Clone, Serialize)]
pub struct HuaweiShortAsrConfig {
    /// Audio format (e.g., "pcm16k16bit").
    pub audio_format: String,

    /// Recognition property/model.
    pub property: String,

    /// Add punctuation ("yes" or "no").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add_punc: Option<String>,

    /// Normalize digits ("yes" or "no").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digit_norm: Option<String>,

    /// Custom vocabulary table ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vocabulary_id: Option<String>,

    /// Include word-level timing ("yes" or "no").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub need_word_info: Option<String>,
}

impl HuaweiShortAsrRequest {
    /// Create a new short ASR request.
    pub fn new(
        audio_data: &[u8],
        audio_format: &str,
        property: &str,
        add_punc: bool,
        digit_norm: bool,
        vocabulary_id: Option<&str>,
        need_word_info: bool,
    ) -> Self {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        Self {
            data: STANDARD.encode(audio_data),
            config: HuaweiShortAsrConfig {
                audio_format: audio_format.to_string(),
                property: property.to_string(),
                add_punc: Some(if add_punc { "yes" } else { "no" }.to_string()),
                digit_norm: Some(if digit_norm { "yes" } else { "no" }.to_string()),
                vocabulary_id: vocabulary_id.map(|s| s.to_string()),
                need_word_info: Some(if need_word_info { "yes" } else { "no" }.to_string()),
            },
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// =============================================================================
// Short Audio ASR Response
// =============================================================================

/// Short sentence recognition HTTP response.
#[derive(Debug, Clone, Deserialize)]
pub struct HuaweiShortAsrResponse {
    /// Trace ID for debugging.
    pub trace_id: Option<String>,

    /// Recognition result.
    pub result: Option<HuaweiAsrResult>,

    /// Error code (0 = success).
    #[serde(default)]
    pub error_code: i32,

    /// Error message.
    pub error_msg: Option<String>,
}

impl HuaweiShortAsrResponse {
    /// Parse response from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if response indicates success.
    pub fn is_success(&self) -> bool {
        self.error_code == 0 && self.result.is_some()
    }

    /// Get the transcript text.
    pub fn get_transcript(&self) -> Option<&str> {
        self.result.as_ref().map(|r| r.text.as_str())
    }

    /// Get the confidence score.
    pub fn get_confidence(&self) -> Option<f32> {
        self.result.as_ref().and_then(|r| r.score)
    }

    /// Get error description.
    pub fn get_error(&self) -> Option<String> {
        if self.error_code != 0 {
            Some(format!(
                "Error {}: {}",
                self.error_code,
                self.error_msg.as_deref().unwrap_or("Unknown error")
            ))
        } else {
            None
        }
    }
}

// =============================================================================
// Error Codes
// =============================================================================

/// Huawei Cloud SIS error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HuaweiSisErrorCode {
    /// Success.
    Success,
    /// Authentication failed.
    AuthenticationFailed,
    /// Token expired.
    TokenExpired,
    /// Invalid parameter.
    InvalidParameter,
    /// Audio format mismatch.
    AudioFormatMismatch,
    /// Service unavailable.
    ServiceUnavailable,
    /// Rate limit exceeded.
    RateLimitExceeded,
    /// Quota exceeded.
    QuotaExceeded,
    /// Audio too long.
    AudioTooLong,
    /// Audio too short.
    AudioTooShort,
    /// No speech detected.
    NoSpeechDetected,
    /// Internal server error.
    InternalError,
    /// Unknown error.
    Unknown,
}

impl HuaweiSisErrorCode {
    /// Parse error code from integer.
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Success,
            // SIS.0001 - Authentication failed
            1 => Self::AuthenticationFailed,
            // SIS.0002 - Token expired
            2 => Self::TokenExpired,
            // SIS.0003 - Invalid parameter
            3 => Self::InvalidParameter,
            // SIS.0004 - Audio format mismatch
            4 => Self::AudioFormatMismatch,
            // SIS.0005 - Service unavailable
            5 => Self::ServiceUnavailable,
            // SIS.0006 - Rate limit exceeded
            6 => Self::RateLimitExceeded,
            // SIS.0007 - Quota exceeded
            7 => Self::QuotaExceeded,
            // Common HTTP-like codes
            400 => Self::InvalidParameter,
            401 | 403 => Self::AuthenticationFailed,
            429 => Self::RateLimitExceeded,
            500 | 502 | 503 => Self::ServiceUnavailable,
            _ => Self::Unknown,
        }
    }

    /// Check if this error is retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::ServiceUnavailable | Self::RateLimitExceeded | Self::InternalError
        )
    }

    /// Get error description.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Success => "Success",
            Self::AuthenticationFailed => "Authentication failed",
            Self::TokenExpired => "Token expired",
            Self::InvalidParameter => "Invalid parameter",
            Self::AudioFormatMismatch => "Audio format mismatch",
            Self::ServiceUnavailable => "Service unavailable",
            Self::RateLimitExceeded => "Rate limit exceeded",
            Self::QuotaExceeded => "Quota exceeded",
            Self::AudioTooLong => "Audio too long",
            Self::AudioTooShort => "Audio too short",
            Self::NoSpeechDetected => "No speech detected",
            Self::InternalError => "Internal server error",
            Self::Unknown => "Unknown error",
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
    fn test_start_frame_serialization() {
        let frame = HuaweiStartFrame::new(
            "pcm16k16bit",
            "chinese_16k_general",
            true,
            true,
            None,
            false,
        );

        let json = frame.to_json().unwrap();
        assert!(json.contains("\"command\":\"START\""));
        assert!(json.contains("\"audio_format\":\"pcm16k16bit\""));
        assert!(json.contains("\"property\":\"chinese_16k_general\""));
        assert!(json.contains("\"add_punc\":\"yes\""));
    }

    #[test]
    fn test_start_frame_with_vocabulary() {
        let frame = HuaweiStartFrame::new(
            "pcm16k16bit",
            "chinese_16k_general",
            true,
            true,
            Some("vocab123"),
            true,
        );

        let json = frame.to_json().unwrap();
        assert!(json.contains("\"vocabulary_id\":\"vocab123\""));
        assert!(json.contains("\"need_word_info\":\"yes\""));
    }

    #[test]
    fn test_start_frame_with_max_silence() {
        let frame = HuaweiStartFrame::new(
            "pcm16k16bit",
            "chinese_16k_general",
            true,
            true,
            None,
            false,
        )
        .with_max_silence(5);

        let json = frame.to_json().unwrap();
        assert!(json.contains("\"max_seconds_of_silence\":5"));
    }

    #[test]
    fn test_end_frame_serialization() {
        let frame = HuaweiEndFrame::new();
        let json = frame.to_json().unwrap();
        assert!(json.contains("\"command\":\"END\""));
    }

    #[test]
    fn test_cancel_frame_serialization() {
        let frame = HuaweiCancelFrame::new();
        let json = frame.to_json().unwrap();
        assert!(json.contains("\"command\":\"CANCEL\""));
    }

    #[test]
    fn test_realtime_response_parsing_success() {
        let json = r#"{
            "resp_type": "END",
            "trace_id": "trace123",
            "error_code": 0,
            "result": {
                "text": "你好世界",
                "score": 0.95,
                "is_final": true
            }
        }"#;

        let response = HuaweiRealtimeResponse::from_json(json).unwrap();
        assert!(response.is_success());
        assert!(response.is_final());
        assert!(!response.is_interim());
        assert!(!response.is_error());
        assert_eq!(response.get_transcript(), Some("你好世界"));
        assert_eq!(response.get_confidence(), Some(0.95));
    }

    #[test]
    fn test_realtime_response_parsing_interim() {
        let json = r#"{
            "resp_type": "RESULT",
            "error_code": 0,
            "result": {
                "text": "你好",
                "is_final": false
            }
        }"#;

        let response = HuaweiRealtimeResponse::from_json(json).unwrap();
        assert!(response.is_success());
        assert!(response.is_interim());
        assert!(!response.is_final());
        assert_eq!(response.get_transcript(), Some("你好"));
    }

    #[test]
    fn test_realtime_response_parsing_error() {
        let json = r#"{
            "resp_type": "ERROR",
            "error_code": 3,
            "error_msg": "Invalid parameter"
        }"#;

        let response = HuaweiRealtimeResponse::from_json(json).unwrap();
        assert!(!response.is_success());
        assert!(response.is_error());
        assert!(response.get_error().is_some());
        assert!(response.get_error().unwrap().contains("Invalid parameter"));
    }

    #[test]
    fn test_realtime_response_with_word_info() {
        let json = r#"{
            "resp_type": "END",
            "error_code": 0,
            "result": {
                "text": "你好世界",
                "is_final": true,
                "word_info": [
                    {"word": "你好", "start_time": 0, "end_time": 500},
                    {"word": "世界", "start_time": 500, "end_time": 1000}
                ]
            }
        }"#;

        let response = HuaweiRealtimeResponse::from_json(json).unwrap();
        let word_info = response.get_word_info().unwrap();
        assert_eq!(word_info.len(), 2);
        assert_eq!(word_info[0].word, "你好");
        assert_eq!(word_info[0].start_time, 0);
        assert_eq!(word_info[1].word, "世界");
    }

    #[test]
    fn test_realtime_response_started() {
        let json = r#"{
            "resp_type": "STARTED",
            "trace_id": "trace123",
            "error_code": 0
        }"#;

        let response = HuaweiRealtimeResponse::from_json(json).unwrap();
        assert!(response.is_started());
        assert!(!response.is_ended());
    }

    #[test]
    fn test_realtime_response_ended() {
        let json = r#"{
            "resp_type": "ENDED",
            "trace_id": "trace123",
            "error_code": 0
        }"#;

        let response = HuaweiRealtimeResponse::from_json(json).unwrap();
        assert!(response.is_ended());
        assert!(!response.is_started());
    }

    #[test]
    fn test_short_asr_request() {
        let audio_data = vec![0u8; 100];
        let request = HuaweiShortAsrRequest::new(
            &audio_data,
            "pcm16k16bit",
            "chinese_16k_general",
            true,
            true,
            None,
            false,
        );

        let json = request.to_json().unwrap();
        assert!(json.contains("\"audio_format\":\"pcm16k16bit\""));
        assert!(json.contains("\"property\":\"chinese_16k_general\""));
        assert!(json.contains("\"data\":"));
    }

    #[test]
    fn test_short_asr_response_success() {
        let json = r#"{
            "trace_id": "trace123",
            "error_code": 0,
            "result": {
                "text": "北京天气",
                "score": 0.98,
                "is_final": true
            }
        }"#;

        let response = HuaweiShortAsrResponse::from_json(json).unwrap();
        assert!(response.is_success());
        assert_eq!(response.get_transcript(), Some("北京天气"));
        assert_eq!(response.get_confidence(), Some(0.98));
    }

    #[test]
    fn test_short_asr_response_error() {
        let json = r#"{
            "trace_id": "trace123",
            "error_code": 4,
            "error_msg": "Audio format mismatch"
        }"#;

        let response = HuaweiShortAsrResponse::from_json(json).unwrap();
        assert!(!response.is_success());
        assert!(response.get_error().is_some());
    }

    #[test]
    fn test_error_code_parsing() {
        assert_eq!(
            HuaweiSisErrorCode::from_code(0),
            HuaweiSisErrorCode::Success
        );
        assert_eq!(
            HuaweiSisErrorCode::from_code(1),
            HuaweiSisErrorCode::AuthenticationFailed
        );
        assert_eq!(
            HuaweiSisErrorCode::from_code(2),
            HuaweiSisErrorCode::TokenExpired
        );
        assert_eq!(
            HuaweiSisErrorCode::from_code(6),
            HuaweiSisErrorCode::RateLimitExceeded
        );
        assert_eq!(
            HuaweiSisErrorCode::from_code(9999),
            HuaweiSisErrorCode::Unknown
        );
    }

    #[test]
    fn test_error_code_retryable() {
        assert!(HuaweiSisErrorCode::ServiceUnavailable.is_retryable());
        assert!(HuaweiSisErrorCode::RateLimitExceeded.is_retryable());
        assert!(!HuaweiSisErrorCode::AuthenticationFailed.is_retryable());
        assert!(!HuaweiSisErrorCode::InvalidParameter.is_retryable());
    }

    #[test]
    fn test_error_code_description() {
        assert_eq!(HuaweiSisErrorCode::Success.description(), "Success");
        assert_eq!(
            HuaweiSisErrorCode::AuthenticationFailed.description(),
            "Authentication failed"
        );
    }
}
