//! Tencent Cloud ASR WebSocket Message Types
//!
//! This module defines the message structures for Tencent Cloud's
//! real-time speech recognition WebSocket API.
//!
//! # Message Flow
//!
//! ```text
//! Client                              Server
//!   |                                    |
//!   |------- Binary audio chunks ------->|
//!   |<------ JSON response --------------|
//!   |         (interim/final results)    |
//!   |                                    |
//!   |------- Binary audio chunks ------->|
//!   |<------ JSON response --------------|
//!   |                                    |
//!   |------- End of stream signal ------>|
//!   |<------ Final response -------------|
//! ```
//!
//! # Response Types
//!
//! - `slice_type = 0`: Interim result (will be updated)
//! - `slice_type = 1`: One sentence/segment complete
//! - `slice_type = 2`: Recognition complete (final)
//! - `final = 1`: Overall recognition complete

use serde::{Deserialize, Serialize};

// =============================================================================
// Response Types
// =============================================================================

/// Main response structure from Tencent Cloud ASR.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TencentAsrResponse {
    /// Response code. 0 = success, non-zero = error.
    pub code: i32,

    /// Status message.
    pub message: String,

    /// Unique voice/audio session identifier.
    pub voice_id: String,

    /// Unique message identifier (optional).
    #[serde(default)]
    pub message_id: Option<String>,

    /// Recognition result (present on success).
    #[serde(default)]
    pub result: Option<TencentAsrResult>,

    /// 1 = recognition complete for entire session.
    #[serde(rename = "final", default)]
    pub is_final: Option<i32>,
}

impl TencentAsrResponse {
    /// Parse from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Convert to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Check if this response indicates an error.
    pub fn is_error(&self) -> bool {
        self.code != 0
    }

    /// Check if this response is the final result for the session.
    pub fn is_session_final(&self) -> bool {
        self.is_final == Some(1)
    }

    /// Check if this response contains a segment/sentence-level final result.
    pub fn is_segment_final(&self) -> bool {
        self.result
            .as_ref()
            .map(|r| r.is_segment_final())
            .unwrap_or(false)
    }

    /// Check if this is an interim (non-final) result.
    pub fn is_interim(&self) -> bool {
        self.result
            .as_ref()
            .map(|r| r.is_interim())
            .unwrap_or(false)
    }

    /// Get the transcript text if available.
    pub fn get_transcript(&self) -> Option<&str> {
        self.result.as_ref().map(|r| r.voice_text_str.as_str())
    }

    /// Get the error code as a typed enum.
    pub fn error_code(&self) -> TencentAsrErrorCode {
        TencentAsrErrorCode::from_code(self.code)
    }

    /// Get human-readable error message.
    pub fn get_error_message(&self) -> Option<String> {
        if self.is_error() {
            Some(format!(
                "Tencent ASR Error {}: {} ({})",
                self.code,
                self.error_code().description(),
                self.message
            ))
        } else {
            None
        }
    }
}

/// Recognition result within a response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TencentAsrResult {
    /// Result slice type:
    /// - 0: Interim result (may be updated)
    /// - 1: One sentence/segment complete
    /// - 2: Final result for recognition
    pub slice_type: i32,

    /// Segment index (0-based).
    pub index: i32,

    /// Start time in milliseconds.
    pub start_time: u64,

    /// End time in milliseconds.
    pub end_time: u64,

    /// Recognized text for this segment.
    pub voice_text_str: String,

    /// Number of words in word_list (if word timestamps enabled).
    #[serde(default)]
    pub word_size: Option<i32>,

    /// Word-level timestamps (if enabled).
    #[serde(default)]
    pub word_list: Option<Vec<TencentWord>>,
}

impl TencentAsrResult {
    /// Check if this is an interim result.
    pub fn is_interim(&self) -> bool {
        self.slice_type == 0
    }

    /// Check if this segment/sentence is complete.
    pub fn is_segment_final(&self) -> bool {
        self.slice_type == 1
    }

    /// Check if recognition is complete.
    pub fn is_recognition_final(&self) -> bool {
        self.slice_type == 2
    }

    /// Check if this result has word-level timestamps.
    pub fn has_word_timestamps(&self) -> bool {
        self.word_list
            .as_ref()
            .map(|w| !w.is_empty())
            .unwrap_or(false)
    }

    /// Get duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.end_time.saturating_sub(self.start_time)
    }
}

/// Word-level timing information.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TencentWord {
    /// The word text.
    pub word: String,

    /// Start time in milliseconds.
    pub start_time: u64,

    /// End time in milliseconds.
    pub end_time: u64,

    /// Stability flag:
    /// - 0: May change in subsequent results
    /// - 1: Stable (unlikely to change)
    #[serde(default)]
    pub stable_flag: i32,
}

impl TencentWord {
    /// Check if this word is stable (unlikely to change).
    pub fn is_stable(&self) -> bool {
        self.stable_flag == 1
    }

    /// Get duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.end_time.saturating_sub(self.start_time)
    }
}

// =============================================================================
// Error Codes
// =============================================================================

/// Tencent Cloud ASR error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TencentAsrErrorCode {
    /// Success (code 0).
    Success,

    /// Invalid parameters (code 4001).
    InvalidParameters,

    /// Authentication failure (code 4002).
    AuthenticationFailure,

    /// Service not activated (code 4003).
    ServiceNotActivated,

    /// Insufficient quota (code 4004).
    InsufficientQuota,

    /// Service not supported (code 4005).
    ServiceNotSupported,

    /// Audio too long (code 4006).
    AudioTooLong,

    /// Audio decoding failed (code 4007).
    AudioDecodingFailed,

    /// Client upload timeout (code 4008).
    ClientUploadTimeout,

    /// Server error (code 5000).
    ServerError,

    /// Server busy (code 5001).
    ServerBusy,

    /// Server timeout (code 5002).
    ServerTimeout,

    /// Unknown error code.
    Unknown(i32),
}

impl TencentAsrErrorCode {
    /// Parse error code from integer value.
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Success,
            4001 => Self::InvalidParameters,
            4002 => Self::AuthenticationFailure,
            4003 => Self::ServiceNotActivated,
            4004 => Self::InsufficientQuota,
            4005 => Self::ServiceNotSupported,
            4006 => Self::AudioTooLong,
            4007 => Self::AudioDecodingFailed,
            4008 => Self::ClientUploadTimeout,
            5000 => Self::ServerError,
            5001 => Self::ServerBusy,
            5002 => Self::ServerTimeout,
            _ => Self::Unknown(code),
        }
    }

    /// Get the error code value.
    pub fn code(&self) -> i32 {
        match self {
            Self::Success => 0,
            Self::InvalidParameters => 4001,
            Self::AuthenticationFailure => 4002,
            Self::ServiceNotActivated => 4003,
            Self::InsufficientQuota => 4004,
            Self::ServiceNotSupported => 4005,
            Self::AudioTooLong => 4006,
            Self::AudioDecodingFailed => 4007,
            Self::ClientUploadTimeout => 4008,
            Self::ServerError => 5000,
            Self::ServerBusy => 5001,
            Self::ServerTimeout => 5002,
            Self::Unknown(code) => *code,
        }
    }

    /// Get human-readable description.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Success => "Success",
            Self::InvalidParameters => "Invalid parameters",
            Self::AuthenticationFailure => "Authentication failure",
            Self::ServiceNotActivated => "Service not activated",
            Self::InsufficientQuota => "Insufficient quota",
            Self::ServiceNotSupported => "Service not supported",
            Self::AudioTooLong => "Audio too long",
            Self::AudioDecodingFailed => "Audio decoding failed",
            Self::ClientUploadTimeout => "Client upload timeout",
            Self::ServerError => "Server error",
            Self::ServerBusy => "Server busy",
            Self::ServerTimeout => "Server timeout",
            Self::Unknown(_) => "Unknown error",
        }
    }

    /// Check if this is a client error (4xxx).
    pub fn is_client_error(&self) -> bool {
        let code = self.code();
        (4000..5000).contains(&code)
    }

    /// Check if this is a server error (5xxx).
    pub fn is_server_error(&self) -> bool {
        let code = self.code();
        (5000..6000).contains(&code)
    }

    /// Check if this error is retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::ServerError | Self::ServerBusy | Self::ServerTimeout
        )
    }
}

// =============================================================================
// Slice Type
// =============================================================================

/// Result slice type indicating the nature of the recognition result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TencentSliceType {
    /// Interim result - may be updated by subsequent results.
    Interim,

    /// Segment/sentence complete - this sentence is finalized.
    SegmentEnd,

    /// Recognition complete - no more results expected.
    Final,

    /// Unknown slice type.
    Unknown(i32),
}

impl TencentSliceType {
    /// Parse from integer value.
    pub fn from_value(value: i32) -> Self {
        match value {
            0 => Self::Interim,
            1 => Self::SegmentEnd,
            2 => Self::Final,
            _ => Self::Unknown(value),
        }
    }

    /// Get the integer value.
    pub fn value(&self) -> i32 {
        match self {
            Self::Interim => 0,
            Self::SegmentEnd => 1,
            Self::Final => 2,
            Self::Unknown(v) => *v,
        }
    }

    /// Check if result is final (segment or full).
    pub fn is_final(&self) -> bool {
        matches!(self, Self::SegmentEnd | Self::Final)
    }
}

impl From<i32> for TencentSliceType {
    fn from(value: i32) -> Self {
        Self::from_value(value)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Response Parsing Tests
    // =========================================================================

    #[test]
    fn test_parse_success_response() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test_voice_id",
            "message_id": "msg_123",
            "result": {
                "slice_type": 1,
                "index": 0,
                "start_time": 0,
                "end_time": 1500,
                "voice_text_str": "你好世界"
            },
            "final": 0
        }"#;

        let response = TencentAsrResponse::from_json(json).unwrap();

        assert_eq!(response.code, 0);
        assert_eq!(response.message, "success");
        assert_eq!(response.voice_id, "test_voice_id");
        assert_eq!(response.message_id, Some("msg_123".to_string()));
        assert!(!response.is_error());
        assert!(!response.is_session_final());
        assert!(response.is_segment_final());
        assert_eq!(response.get_transcript(), Some("你好世界"));
    }

    #[test]
    fn test_parse_interim_response() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test_voice_id",
            "result": {
                "slice_type": 0,
                "index": 0,
                "start_time": 0,
                "end_time": 500,
                "voice_text_str": "你"
            }
        }"#;

        let response = TencentAsrResponse::from_json(json).unwrap();

        assert!(!response.is_error());
        assert!(response.is_interim());
        assert!(!response.is_segment_final());
        assert_eq!(response.get_transcript(), Some("你"));
    }

    #[test]
    fn test_parse_final_response() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test_voice_id",
            "result": {
                "slice_type": 2,
                "index": 3,
                "start_time": 0,
                "end_time": 5000,
                "voice_text_str": "完整的识别结果"
            },
            "final": 1
        }"#;

        let response = TencentAsrResponse::from_json(json).unwrap();

        assert!(response.is_session_final());
        assert!(response.result.as_ref().unwrap().is_recognition_final());
    }

    #[test]
    fn test_parse_error_response() {
        let json = r#"{
            "code": 4002,
            "message": "Authentication failed: invalid signature",
            "voice_id": "test_voice_id"
        }"#;

        let response = TencentAsrResponse::from_json(json).unwrap();

        assert!(response.is_error());
        assert_eq!(
            response.error_code(),
            TencentAsrErrorCode::AuthenticationFailure
        );
        assert!(response.get_error_message().unwrap().contains("4002"));
        assert!(response.get_transcript().is_none());
    }

    #[test]
    fn test_parse_response_with_word_list() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test_voice_id",
            "result": {
                "slice_type": 1,
                "index": 0,
                "start_time": 0,
                "end_time": 2000,
                "voice_text_str": "hello world",
                "word_size": 2,
                "word_list": [
                    {"word": "hello", "start_time": 0, "end_time": 800, "stable_flag": 1},
                    {"word": "world", "start_time": 900, "end_time": 2000, "stable_flag": 1}
                ]
            }
        }"#;

        let response = TencentAsrResponse::from_json(json).unwrap();
        let result = response.result.as_ref().unwrap();

        assert!(result.has_word_timestamps());
        assert_eq!(result.word_size, Some(2));

        let words = result.word_list.as_ref().unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].word, "hello");
        assert_eq!(words[0].start_time, 0);
        assert_eq!(words[0].end_time, 800);
        assert!(words[0].is_stable());
        assert_eq!(words[1].word, "world");
    }

    #[test]
    fn test_parse_minimal_response() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "vid"
        }"#;

        let response = TencentAsrResponse::from_json(json).unwrap();

        assert!(!response.is_error());
        assert!(response.result.is_none());
        assert!(response.get_transcript().is_none());
    }

    // =========================================================================
    // Result Tests
    // =========================================================================

    #[test]
    fn test_result_slice_types() {
        let interim = TencentAsrResult {
            slice_type: 0,
            index: 0,
            start_time: 0,
            end_time: 500,
            voice_text_str: "interim".to_string(),
            word_size: None,
            word_list: None,
        };

        assert!(interim.is_interim());
        assert!(!interim.is_segment_final());
        assert!(!interim.is_recognition_final());

        let segment_final = TencentAsrResult {
            slice_type: 1,
            ..interim.clone()
        };

        assert!(!segment_final.is_interim());
        assert!(segment_final.is_segment_final());
        assert!(!segment_final.is_recognition_final());

        let final_result = TencentAsrResult {
            slice_type: 2,
            ..interim.clone()
        };

        assert!(!final_result.is_interim());
        assert!(!final_result.is_segment_final());
        assert!(final_result.is_recognition_final());
    }

    #[test]
    fn test_result_duration() {
        let result = TencentAsrResult {
            slice_type: 1,
            index: 0,
            start_time: 1000,
            end_time: 3500,
            voice_text_str: "test".to_string(),
            word_size: None,
            word_list: None,
        };

        assert_eq!(result.duration_ms(), 2500);
    }

    #[test]
    fn test_result_no_word_timestamps() {
        let result = TencentAsrResult {
            slice_type: 1,
            index: 0,
            start_time: 0,
            end_time: 1000,
            voice_text_str: "test".to_string(),
            word_size: None,
            word_list: None,
        };

        assert!(!result.has_word_timestamps());
    }

    // =========================================================================
    // Word Tests
    // =========================================================================

    #[test]
    fn test_word_stability() {
        let stable_word = TencentWord {
            word: "test".to_string(),
            start_time: 0,
            end_time: 500,
            stable_flag: 1,
        };

        let unstable_word = TencentWord {
            word: "test".to_string(),
            start_time: 0,
            end_time: 500,
            stable_flag: 0,
        };

        assert!(stable_word.is_stable());
        assert!(!unstable_word.is_stable());
    }

    #[test]
    fn test_word_duration() {
        let word = TencentWord {
            word: "hello".to_string(),
            start_time: 100,
            end_time: 800,
            stable_flag: 1,
        };

        assert_eq!(word.duration_ms(), 700);
    }

    // =========================================================================
    // Error Code Tests
    // =========================================================================

    #[test]
    fn test_error_code_parsing() {
        assert_eq!(
            TencentAsrErrorCode::from_code(0),
            TencentAsrErrorCode::Success
        );
        assert_eq!(
            TencentAsrErrorCode::from_code(4001),
            TencentAsrErrorCode::InvalidParameters
        );
        assert_eq!(
            TencentAsrErrorCode::from_code(4002),
            TencentAsrErrorCode::AuthenticationFailure
        );
        assert_eq!(
            TencentAsrErrorCode::from_code(4003),
            TencentAsrErrorCode::ServiceNotActivated
        );
        assert_eq!(
            TencentAsrErrorCode::from_code(4004),
            TencentAsrErrorCode::InsufficientQuota
        );
        assert_eq!(
            TencentAsrErrorCode::from_code(4005),
            TencentAsrErrorCode::ServiceNotSupported
        );
        assert_eq!(
            TencentAsrErrorCode::from_code(4006),
            TencentAsrErrorCode::AudioTooLong
        );
        assert_eq!(
            TencentAsrErrorCode::from_code(4007),
            TencentAsrErrorCode::AudioDecodingFailed
        );
        assert_eq!(
            TencentAsrErrorCode::from_code(4008),
            TencentAsrErrorCode::ClientUploadTimeout
        );
        assert_eq!(
            TencentAsrErrorCode::from_code(5000),
            TencentAsrErrorCode::ServerError
        );
        assert_eq!(
            TencentAsrErrorCode::from_code(5001),
            TencentAsrErrorCode::ServerBusy
        );
        assert_eq!(
            TencentAsrErrorCode::from_code(5002),
            TencentAsrErrorCode::ServerTimeout
        );
        assert_eq!(
            TencentAsrErrorCode::from_code(9999),
            TencentAsrErrorCode::Unknown(9999)
        );
    }

    #[test]
    fn test_error_code_values() {
        assert_eq!(TencentAsrErrorCode::Success.code(), 0);
        assert_eq!(TencentAsrErrorCode::InvalidParameters.code(), 4001);
        assert_eq!(TencentAsrErrorCode::AuthenticationFailure.code(), 4002);
        assert_eq!(TencentAsrErrorCode::Unknown(7777).code(), 7777);
    }

    #[test]
    fn test_error_code_descriptions() {
        assert_eq!(TencentAsrErrorCode::Success.description(), "Success");
        assert!(
            !TencentAsrErrorCode::InvalidParameters
                .description()
                .is_empty()
        );
        assert!(!TencentAsrErrorCode::Unknown(1234).description().is_empty());
    }

    #[test]
    fn test_error_code_is_client_error() {
        assert!(TencentAsrErrorCode::InvalidParameters.is_client_error());
        assert!(TencentAsrErrorCode::AuthenticationFailure.is_client_error());
        assert!(!TencentAsrErrorCode::ServerError.is_client_error());
        assert!(!TencentAsrErrorCode::Success.is_client_error());
    }

    #[test]
    fn test_error_code_is_server_error() {
        assert!(TencentAsrErrorCode::ServerError.is_server_error());
        assert!(TencentAsrErrorCode::ServerBusy.is_server_error());
        assert!(TencentAsrErrorCode::ServerTimeout.is_server_error());
        assert!(!TencentAsrErrorCode::InvalidParameters.is_server_error());
    }

    #[test]
    fn test_error_code_is_retryable() {
        assert!(TencentAsrErrorCode::ServerError.is_retryable());
        assert!(TencentAsrErrorCode::ServerBusy.is_retryable());
        assert!(TencentAsrErrorCode::ServerTimeout.is_retryable());
        assert!(!TencentAsrErrorCode::InvalidParameters.is_retryable());
        assert!(!TencentAsrErrorCode::AuthenticationFailure.is_retryable());
    }

    // =========================================================================
    // Slice Type Tests
    // =========================================================================

    #[test]
    fn test_slice_type_parsing() {
        assert_eq!(TencentSliceType::from_value(0), TencentSliceType::Interim);
        assert_eq!(
            TencentSliceType::from_value(1),
            TencentSliceType::SegmentEnd
        );
        assert_eq!(TencentSliceType::from_value(2), TencentSliceType::Final);
        assert_eq!(
            TencentSliceType::from_value(99),
            TencentSliceType::Unknown(99)
        );
    }

    #[test]
    fn test_slice_type_values() {
        assert_eq!(TencentSliceType::Interim.value(), 0);
        assert_eq!(TencentSliceType::SegmentEnd.value(), 1);
        assert_eq!(TencentSliceType::Final.value(), 2);
        assert_eq!(TencentSliceType::Unknown(42).value(), 42);
    }

    #[test]
    fn test_slice_type_is_final() {
        assert!(!TencentSliceType::Interim.is_final());
        assert!(TencentSliceType::SegmentEnd.is_final());
        assert!(TencentSliceType::Final.is_final());
        assert!(!TencentSliceType::Unknown(0).is_final());
    }

    #[test]
    fn test_slice_type_from_i32() {
        let interim: TencentSliceType = 0.into();
        assert_eq!(interim, TencentSliceType::Interim);

        let final_type: TencentSliceType = 2.into();
        assert_eq!(final_type, TencentSliceType::Final);
    }

    // =========================================================================
    // Serialization Tests
    // =========================================================================

    #[test]
    fn test_response_to_json() {
        let response = TencentAsrResponse {
            code: 0,
            message: "success".to_string(),
            voice_id: "vid".to_string(),
            message_id: Some("mid".to_string()),
            result: Some(TencentAsrResult {
                slice_type: 1,
                index: 0,
                start_time: 0,
                end_time: 1000,
                voice_text_str: "test".to_string(),
                word_size: None,
                word_list: None,
            }),
            is_final: Some(0),
        };

        let json = response.to_json().unwrap();
        assert!(json.contains("\"code\":0"));
        assert!(json.contains("\"voice_text_str\":\"test\""));

        // Should be able to parse back
        let parsed = TencentAsrResponse::from_json(&json).unwrap();
        assert_eq!(parsed.code, response.code);
        assert_eq!(parsed.voice_id, response.voice_id);
    }
}
