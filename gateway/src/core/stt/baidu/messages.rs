//! Baidu Speech Recognition Message Types
//!
//! Message structures for both REST API and WebSocket protocol.

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use serde::{Deserialize, Serialize};

// =============================================================================
// WebSocket Messages (Client -> Server)
// =============================================================================

/// Start frame for real-time recognition.
#[derive(Debug, Clone, Serialize)]
pub struct BaiduStartFrame {
    /// Frame type: "START"
    #[serde(rename = "type")]
    pub frame_type: String,

    /// Start frame data.
    pub data: BaiduStartData,
}

/// Start frame data containing recognition parameters.
#[derive(Debug, Clone, Serialize)]
pub struct BaiduStartData {
    /// Application ID.
    pub appid: String,

    /// API Key.
    pub appkey: String,

    /// Recognition model (dev_pid).
    pub dev_pid: u32,

    /// Client unique identifier.
    pub cuid: String,

    /// Sample rate.
    pub sample: u32,

    /// Audio format.
    pub format: String,
}

impl BaiduStartFrame {
    /// Create a new start frame.
    pub fn new(
        appid: impl Into<String>,
        appkey: impl Into<String>,
        dev_pid: u32,
        cuid: impl Into<String>,
        sample_rate: u32,
        format: impl Into<String>,
    ) -> Self {
        Self {
            frame_type: "START".to_string(),
            data: BaiduStartData {
                appid: appid.into(),
                appkey: appkey.into(),
                dev_pid,
                cuid: cuid.into(),
                sample: sample_rate,
                format: format.into(),
            },
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Finish frame to signal end of audio stream.
#[derive(Debug, Clone, Serialize)]
pub struct BaiduFinishFrame {
    /// Frame type: "FINISH"
    #[serde(rename = "type")]
    pub frame_type: String,
}

impl Default for BaiduFinishFrame {
    fn default() -> Self {
        Self::new()
    }
}

impl BaiduFinishFrame {
    /// Create a new finish frame.
    pub fn new() -> Self {
        Self {
            frame_type: "FINISH".to_string(),
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Cancel frame to abort recognition.
#[derive(Debug, Clone, Serialize)]
pub struct BaiduCancelFrame {
    /// Frame type: "CANCEL"
    #[serde(rename = "type")]
    pub frame_type: String,
}

impl Default for BaiduCancelFrame {
    fn default() -> Self {
        Self::new()
    }
}

impl BaiduCancelFrame {
    /// Create a new cancel frame.
    pub fn new() -> Self {
        Self {
            frame_type: "CANCEL".to_string(),
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// =============================================================================
// WebSocket Messages (Server -> Client)
// =============================================================================

/// Server response message for real-time recognition.
#[derive(Debug, Clone, Deserialize)]
pub struct BaiduRealtimeResponse {
    /// Error code (0 = success).
    pub err_no: i32,

    /// Error message.
    pub err_msg: Option<String>,

    /// Result type: "MID_TEXT" (interim), "FIN_TEXT" (final), "ERROR"
    #[serde(rename = "type")]
    pub result_type: Option<String>,

    /// Recognition result text.
    pub result: Option<String>,

    /// Session identifier.
    pub sn: Option<String>,

    /// Log ID for debugging.
    pub log_id: Option<i64>,
}

impl BaiduRealtimeResponse {
    /// Parse from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if this is a success response.
    pub fn is_success(&self) -> bool {
        self.err_no == 0
    }

    /// Check if this is an error response.
    pub fn is_error(&self) -> bool {
        self.err_no != 0 || self.result_type.as_deref() == Some("ERROR")
    }

    /// Check if this is an interim result.
    pub fn is_interim(&self) -> bool {
        self.result_type.as_deref() == Some("MID_TEXT")
    }

    /// Check if this is a final result.
    pub fn is_final(&self) -> bool {
        self.result_type.as_deref() == Some("FIN_TEXT")
    }

    /// Get the transcript text.
    pub fn get_transcript(&self) -> Option<&str> {
        self.result.as_deref()
    }

    /// Get the error message.
    pub fn get_error(&self) -> Option<String> {
        if self.is_error() {
            Some(self.err_msg.clone().unwrap_or_else(|| {
                format!("Error code: {}", self.err_no)
            }))
        } else {
            None
        }
    }
}

// =============================================================================
// REST API Messages
// =============================================================================

/// Request body for short audio REST API.
#[derive(Debug, Clone, Serialize)]
pub struct BaiduShortAsrRequest {
    /// Audio format: pcm, wav, amr, m4a
    pub format: String,

    /// Sample rate: 16000 or 8000
    pub rate: u32,

    /// Channel count (always 1)
    pub channel: u32,

    /// Client unique identifier
    pub cuid: String,

    /// Access token
    pub token: String,

    /// Recognition model (dev_pid)
    pub dev_pid: u32,

    /// Custom vocabulary model ID (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lm_id: Option<u32>,

    /// Base64-encoded audio data
    pub speech: String,

    /// Original audio file size in bytes
    pub len: usize,
}

impl BaiduShortAsrRequest {
    /// Create a new request with audio data.
    pub fn new(
        format: impl Into<String>,
        rate: u32,
        cuid: impl Into<String>,
        token: impl Into<String>,
        dev_pid: u32,
        audio_data: &[u8],
    ) -> Self {
        Self {
            format: format.into(),
            rate,
            channel: 1,
            cuid: cuid.into(),
            token: token.into(),
            dev_pid,
            lm_id: None,
            speech: BASE64_STANDARD.encode(audio_data),
            len: audio_data.len(),
        }
    }

    /// Set custom vocabulary model ID.
    pub fn with_lm_id(mut self, lm_id: u32) -> Self {
        self.lm_id = Some(lm_id);
        self
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Response from short audio REST API.
#[derive(Debug, Clone, Deserialize)]
pub struct BaiduShortAsrResponse {
    /// Error code (0 = success).
    pub err_no: i32,

    /// Error message.
    pub err_msg: String,

    /// Session identifier.
    pub sn: Option<String>,

    /// Recognition results (array of strings).
    pub result: Option<Vec<String>>,

    /// Corpus number for reference.
    pub corpus_no: Option<String>,
}

impl BaiduShortAsrResponse {
    /// Parse from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if this is a success response.
    pub fn is_success(&self) -> bool {
        self.err_no == 0
    }

    /// Get the first recognition result.
    pub fn get_transcript(&self) -> Option<&str> {
        self.result.as_ref().and_then(|r| r.first().map(|s| s.as_str()))
    }

    /// Get all recognition results.
    pub fn get_all_transcripts(&self) -> Vec<&str> {
        self.result
            .as_ref()
            .map(|r| r.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }
}

// =============================================================================
// Error Codes
// =============================================================================

/// Baidu STT error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaiduErrorCode {
    /// Success
    Success = 0,

    /// Data empty
    DataEmpty = 2000,

    /// Input parameter error
    InputParameterError = 3300,

    /// Authentication error
    AuthenticationError = 3301,

    /// Token invalid or expired
    TokenInvalid = 3302,

    /// Audio file too long
    AudioTooLong = 3303,

    /// Audio file too large
    AudioTooLarge = 3304,

    /// Audio quality issue
    AudioQualityIssue = 3305,

    /// Bad audio format
    BadAudioFormat = 3306,

    /// Audio data error
    AudioDataError = 3307,

    /// Server busy
    ServerBusy = 3308,

    /// Audio recognition timeout
    RecognitionTimeout = 3309,

    /// Audio recognition error
    RecognitionError = 3310,

    /// QPS exceeded
    QpsExceeded = 3311,

    /// Quota exceeded
    QuotaExceeded = 3312,

    /// Unknown error
    Unknown = -1,
}

impl BaiduErrorCode {
    /// Parse from error number.
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Success,
            2000 => Self::DataEmpty,
            3300 => Self::InputParameterError,
            3301 => Self::AuthenticationError,
            3302 => Self::TokenInvalid,
            3303 => Self::AudioTooLong,
            3304 => Self::AudioTooLarge,
            3305 => Self::AudioQualityIssue,
            3306 => Self::BadAudioFormat,
            3307 => Self::AudioDataError,
            3308 => Self::ServerBusy,
            3309 => Self::RecognitionTimeout,
            3310 => Self::RecognitionError,
            3311 => Self::QpsExceeded,
            3312 => Self::QuotaExceeded,
            _ => Self::Unknown,
        }
    }

    /// Get error description.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Success => "Success",
            Self::DataEmpty => "Data empty",
            Self::InputParameterError => "Input parameter error",
            Self::AuthenticationError => "Authentication error",
            Self::TokenInvalid => "Token invalid or expired",
            Self::AudioTooLong => "Audio file too long",
            Self::AudioTooLarge => "Audio file too large",
            Self::AudioQualityIssue => "Audio quality issue",
            Self::BadAudioFormat => "Bad audio format",
            Self::AudioDataError => "Audio data error",
            Self::ServerBusy => "Server busy",
            Self::RecognitionTimeout => "Audio recognition timeout",
            Self::RecognitionError => "Audio recognition error",
            Self::QpsExceeded => "QPS limit exceeded",
            Self::QuotaExceeded => "Quota exceeded",
            Self::Unknown => "Unknown error",
        }
    }

    /// Check if this error is retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::ServerBusy | Self::RecognitionTimeout | Self::QpsExceeded
        )
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
        let frame = BaiduStartFrame::new(
            "app123",
            "key456",
            1537,
            "user789",
            16000,
            "pcm",
        );

        let json = frame.to_json().unwrap();
        assert!(json.contains("\"type\":\"START\""));
        assert!(json.contains("\"appid\":\"app123\""));
        assert!(json.contains("\"appkey\":\"key456\""));
        assert!(json.contains("\"dev_pid\":1537"));
        assert!(json.contains("\"cuid\":\"user789\""));
        assert!(json.contains("\"sample\":16000"));
        assert!(json.contains("\"format\":\"pcm\""));
    }

    #[test]
    fn test_finish_frame_serialization() {
        let frame = BaiduFinishFrame::new();
        let json = frame.to_json().unwrap();
        assert!(json.contains("\"type\":\"FINISH\""));
    }

    #[test]
    fn test_cancel_frame_serialization() {
        let frame = BaiduCancelFrame::new();
        let json = frame.to_json().unwrap();
        assert!(json.contains("\"type\":\"CANCEL\""));
    }

    #[test]
    fn test_realtime_response_parsing_success() {
        let json = r#"{
            "err_no": 0,
            "err_msg": "success",
            "type": "FIN_TEXT",
            "result": "你好世界",
            "sn": "123456"
        }"#;

        let response = BaiduRealtimeResponse::from_json(json).unwrap();
        assert!(response.is_success());
        assert!(response.is_final());
        assert!(!response.is_interim());
        assert!(!response.is_error());
        assert_eq!(response.get_transcript(), Some("你好世界"));
    }

    #[test]
    fn test_realtime_response_parsing_interim() {
        let json = r#"{
            "err_no": 0,
            "err_msg": "success",
            "type": "MID_TEXT",
            "result": "你好"
        }"#;

        let response = BaiduRealtimeResponse::from_json(json).unwrap();
        assert!(response.is_success());
        assert!(response.is_interim());
        assert!(!response.is_final());
    }

    #[test]
    fn test_realtime_response_parsing_error() {
        let json = r#"{
            "err_no": 3301,
            "err_msg": "Authentication error",
            "type": "ERROR"
        }"#;

        let response = BaiduRealtimeResponse::from_json(json).unwrap();
        assert!(!response.is_success());
        assert!(response.is_error());
        assert!(response.get_error().is_some());
    }

    #[test]
    fn test_short_asr_request() {
        let audio_data = vec![0u8; 100];
        let request = BaiduShortAsrRequest::new(
            "pcm",
            16000,
            "user123",
            "token456",
            1537,
            &audio_data,
        );

        assert_eq!(request.format, "pcm");
        assert_eq!(request.rate, 16000);
        assert_eq!(request.channel, 1);
        assert_eq!(request.len, 100);
        assert!(!request.speech.is_empty());
    }

    #[test]
    fn test_short_asr_response_parsing() {
        let json = r#"{
            "err_no": 0,
            "err_msg": "success.",
            "sn": "481D633F-73BA-726F-49EF-8659ACCC2F3D",
            "result": ["北京天气"],
            "corpus_no": "6890859905390146256"
        }"#;

        let response = BaiduShortAsrResponse::from_json(json).unwrap();
        assert!(response.is_success());
        assert_eq!(response.get_transcript(), Some("北京天气"));
    }

    #[test]
    fn test_error_code_parsing() {
        assert_eq!(BaiduErrorCode::from_code(0), BaiduErrorCode::Success);
        assert_eq!(BaiduErrorCode::from_code(3301), BaiduErrorCode::AuthenticationError);
        assert_eq!(BaiduErrorCode::from_code(3302), BaiduErrorCode::TokenInvalid);
        assert_eq!(BaiduErrorCode::from_code(9999), BaiduErrorCode::Unknown);
    }

    #[test]
    fn test_error_code_retryable() {
        assert!(BaiduErrorCode::ServerBusy.is_retryable());
        assert!(BaiduErrorCode::RecognitionTimeout.is_retryable());
        assert!(BaiduErrorCode::QpsExceeded.is_retryable());
        assert!(!BaiduErrorCode::AuthenticationError.is_retryable());
        assert!(!BaiduErrorCode::TokenInvalid.is_retryable());
    }
}
