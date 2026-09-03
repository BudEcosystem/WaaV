//! iFlytek STT Messages Module
//!
//! This module defines the request and response message formats for iFlytek's
//! WebSocket-based Speech-to-Text API.
//!
//! # Message Format
//!
//! iFlytek uses JSON-formatted messages with three main sections:
//! - `common`: Application ID
//! - `business`: Recognition parameters (language, domain, VAD settings)
//! - `data`: Audio data (base64 encoded) with status indicator
//!
//! # Status Values
//!
//! - 0: First frame (includes app_id)
//! - 1: Middle frames (audio data)
//! - 2: Last frame (end of audio)

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};

// =============================================================================
// Error Codes
// =============================================================================

/// iFlytek API error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IFlytekErrorCode {
    /// Success (0).
    Success,
    /// APPID authorization failure (10005).
    AuthorizationFailure,
    /// Insufficient balance (10006).
    InsufficientBalance,
    /// Invalid parameter (10007).
    InvalidParameter,
    /// Audio decoding failed (10043).
    AudioDecodingFailed,
    /// Request expired - clock skew (10160).
    RequestExpired,
    /// Base64 decoding failed (10161).
    Base64DecodingFailed,
    /// Read timeout - 10s inactivity (10200).
    ReadTimeout,
    /// Missing app_id in first frame (10313).
    MissingAppId,
    /// Unauthorized speaker/feature (11200).
    UnauthorizedFeature,
    /// Daily quota exceeded (11201).
    QuotaExceeded,
    /// Unknown error code.
    Unknown(i32),
}

impl IFlytekErrorCode {
    /// Create from integer code.
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Success,
            10005 => Self::AuthorizationFailure,
            10006 => Self::InsufficientBalance,
            10007 => Self::InvalidParameter,
            10043 => Self::AudioDecodingFailed,
            10160 => Self::RequestExpired,
            10161 => Self::Base64DecodingFailed,
            10200 => Self::ReadTimeout,
            10313 => Self::MissingAppId,
            11200 => Self::UnauthorizedFeature,
            11201 => Self::QuotaExceeded,
            other => Self::Unknown(other),
        }
    }

    /// Get the integer code.
    pub fn as_code(&self) -> i32 {
        match self {
            Self::Success => 0,
            Self::AuthorizationFailure => 10005,
            Self::InsufficientBalance => 10006,
            Self::InvalidParameter => 10007,
            Self::AudioDecodingFailed => 10043,
            Self::RequestExpired => 10160,
            Self::Base64DecodingFailed => 10161,
            Self::ReadTimeout => 10200,
            Self::MissingAppId => 10313,
            Self::UnauthorizedFeature => 11200,
            Self::QuotaExceeded => 11201,
            Self::Unknown(code) => *code,
        }
    }

    /// Check if this is a success code.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    /// Check if this is a retryable error.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::ReadTimeout | Self::RequestExpired)
    }

    /// Get a human-readable description.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Success => "Success",
            Self::AuthorizationFailure => "APPID authorization failure - check credentials",
            Self::InsufficientBalance => "Insufficient balance - add credits or subscribe",
            Self::InvalidParameter => "Invalid parameter in request",
            Self::AudioDecodingFailed => "Audio decoding failed - verify audio format",
            Self::RequestExpired => "Request expired - check system clock sync",
            Self::Base64DecodingFailed => "Base64 decoding failed - verify encoding",
            Self::ReadTimeout => "Read timeout - send data more frequently",
            Self::MissingAppId => "Missing app_id in first frame",
            Self::UnauthorizedFeature => "Unauthorized speaker/feature - enable in console",
            Self::QuotaExceeded => "Daily quota exceeded - wait for reset or upgrade",
            Self::Unknown(_) => "Unknown error",
        }
    }
}

impl std::fmt::Display for IFlytekErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.description(), self.as_code())
    }
}

// =============================================================================
// Data Status
// =============================================================================

/// Audio data status indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i32", try_from = "i32")]
pub enum DataStatus {
    /// First frame (status=0).
    First,
    /// Middle frame (status=1).
    Continue,
    /// Last frame (status=2).
    Last,
}

impl From<DataStatus> for i32 {
    fn from(status: DataStatus) -> i32 {
        match status {
            DataStatus::First => 0,
            DataStatus::Continue => 1,
            DataStatus::Last => 2,
        }
    }
}

impl TryFrom<i32> for DataStatus {
    type Error = &'static str;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::First),
            1 => Ok(Self::Continue),
            2 => Ok(Self::Last),
            _ => Err("Invalid data status"),
        }
    }
}

// =============================================================================
// STT Request Messages
// =============================================================================

/// Common section of the request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttRequestCommon {
    /// Application ID.
    pub app_id: String,
}

/// Business section of the request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttRequestBusiness {
    /// Recognition language (e.g., "zh_cn", "en_us").
    pub language: String,

    /// Recognition domain ("iat" or "medical").
    pub domain: String,

    /// Accent for Chinese (e.g., "mandarin", "cantonese").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,

    /// VAD end-of-speech timeout in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vad_eos: Option<u32>,

    /// Dynamic word correction ("wpgs" for Chinese).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dwa: Option<String>,

    /// Punctuation mode (0=off, 1=on).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ptt: Option<i32>,

    /// Number conversion mode (0=off, 1=on).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nunum: Option<i32>,

    /// Word-level frame-offset info (per-word start/end timing). iFlytek `vinfo` (1=on).
    /// Maps the standardized `word_timestamps` feature. The recognizer returns a `vad`/word
    /// offset block when this is set. Ref: iFlytek IAT/IST business params (`vinfo`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vinfo: Option<i32>,

    /// Sentence-level N-best alternatives (1–5). iFlytek `nbest`. Maps the standardized
    /// `alternatives` feature. Ref: iFlytek IAT business params (`nbest`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nbest: Option<i32>,

    /// Word-level N-best alternatives (1–5). iFlytek `wbest`. Maps the
    /// `word_alternatives` extra. Ref: iFlytek IAT business params (`wbest`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wbest: Option<i32>,

    /// Personalization domain / vertical (e.g. "game", "health", "shopping", "trip").
    /// iFlytek `pd`. Maps the `domain_personalization` extra. Ref: iFlytek IAT business
    /// params (`pd`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pd: Option<String>,

    /// Result character set / language variant (e.g. "zh-cn" simplified vs traditional).
    /// iFlytek `rlang`. Maps the `result_character_set` extra. Ref: iFlytek IAT business
    /// params (`rlang`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rlang: Option<String>,
}

/// Data section of the request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttRequestData {
    /// Frame status (0=first, 1=continue, 2=last).
    pub status: i32,

    /// Audio format string.
    pub format: String,

    /// Audio encoding ("raw", "speex", "speex-wb", "lame").
    pub encoding: String,

    /// Base64-encoded audio data.
    pub audio: String,
}

/// Complete STT request message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttRequest {
    /// Common section (app_id).
    pub common: SttRequestCommon,

    /// Business section (language, domain, etc.).
    pub business: SttRequestBusiness,

    /// Data section (audio).
    pub data: SttRequestData,
}

/// Advanced iFlytek business parameters that map standardized/extras features onto the wire
/// `business` block. Every field is `Option`/empty by default so a `..Default::default()`
/// first frame is unchanged. These are the params surfaced by the W2 feature-wiring pass:
/// `vinfo` (word timestamps), `nbest`/`wbest` (sentence/word N-best), `pd` (domain
/// personalization), `rlang` (result character set / language variant).
#[derive(Debug, Clone, Default)]
pub struct IFlytekBusinessExtras {
    /// `vinfo` — word-level frame-offset info (word timestamps). `Some(true)` ⇒ `vinfo=1`.
    pub word_timestamps: Option<bool>,
    /// `nbest` — sentence-level N-best alternatives (1–5).
    pub nbest: Option<u8>,
    /// `wbest` — word-level N-best alternatives (1–5).
    pub wbest: Option<u8>,
    /// `pd` — personalization domain / vertical.
    pub pd: Option<String>,
    /// `rlang` — result character set / language variant.
    pub rlang: Option<String>,
}

impl SttRequest {
    /// Create a new first frame request.
    #[allow(clippy::too_many_arguments)]
    pub fn first_frame(
        app_id: &str,
        language: &str,
        domain: &str,
        accent: Option<&str>,
        vad_eos_ms: u32,
        dynamic_correction: bool,
        punctuation: bool,
        convert_numbers: bool,
        audio_format: &str,
        encoding: &str,
        audio_data: &[u8],
    ) -> Self {
        Self::first_frame_with_extras(
            app_id,
            language,
            domain,
            accent,
            vad_eos_ms,
            dynamic_correction,
            punctuation,
            convert_numbers,
            audio_format,
            encoding,
            audio_data,
            &IFlytekBusinessExtras::default(),
        )
    }

    /// Create a first frame request including the advanced `business` parameters
    /// (`vinfo`/`nbest`/`wbest`/`pd`/`rlang`). The base `first_frame` delegates here with empty
    /// extras, so the wire shape is identical when no advanced feature is requested.
    #[allow(clippy::too_many_arguments)]
    pub fn first_frame_with_extras(
        app_id: &str,
        language: &str,
        domain: &str,
        accent: Option<&str>,
        vad_eos_ms: u32,
        dynamic_correction: bool,
        punctuation: bool,
        convert_numbers: bool,
        audio_format: &str,
        encoding: &str,
        audio_data: &[u8],
        extras: &IFlytekBusinessExtras,
    ) -> Self {
        Self {
            common: SttRequestCommon {
                app_id: app_id.to_string(),
            },
            business: SttRequestBusiness {
                language: language.to_string(),
                domain: domain.to_string(),
                accent: accent.map(|s| s.to_string()),
                vad_eos: Some(vad_eos_ms),
                dwa: if dynamic_correction {
                    Some("wpgs".to_string())
                } else {
                    None
                },
                ptt: Some(if punctuation { 1 } else { 0 }),
                nunum: Some(if convert_numbers { 1 } else { 0 }),
                vinfo: extras.word_timestamps.map(|b| if b { 1 } else { 0 }),
                nbest: extras.nbest.map(|n| n as i32),
                wbest: extras.wbest.map(|n| n as i32),
                pd: extras.pd.clone(),
                rlang: extras.rlang.clone(),
            },
            data: SttRequestData {
                status: 0, // First frame
                format: audio_format.to_string(),
                encoding: encoding.to_string(),
                audio: BASE64.encode(audio_data),
            },
        }
    }

    /// Create a continue frame request.
    pub fn continue_frame(
        app_id: &str,
        audio_format: &str,
        encoding: &str,
        audio_data: &[u8],
    ) -> Self {
        Self {
            common: SttRequestCommon {
                app_id: app_id.to_string(),
            },
            business: SttRequestBusiness {
                language: String::new(),
                domain: String::new(),
                accent: None,
                vad_eos: None,
                dwa: None,
                ptt: None,
                nunum: None,
                vinfo: None,
                nbest: None,
                wbest: None,
                pd: None,
                rlang: None,
            },
            data: SttRequestData {
                status: 1, // Continue frame
                format: audio_format.to_string(),
                encoding: encoding.to_string(),
                audio: BASE64.encode(audio_data),
            },
        }
    }

    /// Create a last frame request.
    pub fn last_frame(app_id: &str, audio_format: &str, encoding: &str, audio_data: &[u8]) -> Self {
        Self {
            common: SttRequestCommon {
                app_id: app_id.to_string(),
            },
            business: SttRequestBusiness {
                language: String::new(),
                domain: String::new(),
                accent: None,
                vad_eos: None,
                dwa: None,
                ptt: None,
                nunum: None,
                vinfo: None,
                nbest: None,
                wbest: None,
                pd: None,
                rlang: None,
            },
            data: SttRequestData {
                status: 2, // Last frame
                format: audio_format.to_string(),
                encoding: encoding.to_string(),
                audio: BASE64.encode(audio_data),
            },
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// =============================================================================
// STT Response Messages
// =============================================================================

/// Word recognition result.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SttWord {
    /// The recognized word.
    pub w: String,

    /// Confidence score (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sc: Option<f32>,
}

/// Word segment with timing.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SttWordSegment {
    /// Start time (character position).
    pub bg: i32,

    /// Character/word results.
    pub cw: Vec<SttWord>,
}

/// Recognition result data.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SttResultData {
    /// Word segments.
    pub ws: Vec<SttWordSegment>,

    /// Sentence number.
    pub sn: i32,

    /// Is this the last sentence?
    pub ls: bool,

    /// Progressive result status ("apd" = append, "rpl" = replace).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pgs: Option<String>,

    /// Sentence start position for replacement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rg: Option<Vec<i32>>,
}

impl SttResultData {
    /// Get the full transcript from word segments.
    pub fn transcript(&self) -> String {
        self.ws
            .iter()
            .flat_map(|ws| ws.cw.iter().map(|cw| cw.w.as_str()))
            .collect()
    }

    /// Get the average confidence score.
    pub fn confidence(&self) -> f32 {
        let scores: Vec<f32> = self
            .ws
            .iter()
            .flat_map(|ws| ws.cw.iter().filter_map(|cw| cw.sc))
            .collect();

        if scores.is_empty() {
            0.95 // Default confidence if not provided
        } else {
            scores.iter().sum::<f32>() / scores.len() as f32
        }
    }

    /// Check if this is a replacement result (dynamic correction).
    pub fn is_replacement(&self) -> bool {
        self.pgs.as_deref() == Some("rpl")
    }

    /// Check if this is an append result.
    pub fn is_append(&self) -> bool {
        self.pgs.as_deref() == Some("apd") || self.pgs.is_none()
    }
}

/// Response data section.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SttResponseData {
    /// Recognition result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<SttResultData>,

    /// Response status (1 = partial, 2 = final).
    pub status: i32,
}

/// Complete STT response message.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SttResponse {
    /// Response code (0 = success).
    pub code: i32,

    /// Response message.
    pub message: String,

    /// Session ID.
    pub sid: String,

    /// Response data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<SttResponseData>,
}

impl SttResponse {
    /// Get the error code.
    pub fn error_code(&self) -> IFlytekErrorCode {
        IFlytekErrorCode::from_code(self.code)
    }

    /// Check if the response indicates success.
    pub fn is_success(&self) -> bool {
        self.code == 0
    }

    /// Check if this is the final response.
    pub fn is_final(&self) -> bool {
        self.data.as_ref().map(|d| d.status == 2).unwrap_or(false)
    }

    /// Get the transcript from the response.
    pub fn transcript(&self) -> Option<String> {
        self.data
            .as_ref()
            .and_then(|d| d.result.as_ref())
            .map(|r| r.transcript())
    }

    /// Get the confidence score.
    pub fn confidence(&self) -> f32 {
        self.data
            .as_ref()
            .and_then(|d| d.result.as_ref())
            .map(|r| r.confidence())
            .unwrap_or(0.95)
    }

    /// Check if this is a dynamic correction replacement.
    pub fn is_replacement(&self) -> bool {
        self.data
            .as_ref()
            .and_then(|d| d.result.as_ref())
            .map(|r| r.is_replacement())
            .unwrap_or(false)
    }

    /// Get the sentence number (for tracking corrections).
    pub fn sentence_number(&self) -> Option<i32> {
        self.data
            .as_ref()
            .and_then(|d| d.result.as_ref())
            .map(|r| r.sn)
    }

    /// Parse from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Error code tests
    #[test]
    fn test_error_code_from_code() {
        assert_eq!(IFlytekErrorCode::from_code(0), IFlytekErrorCode::Success);
        assert_eq!(
            IFlytekErrorCode::from_code(10005),
            IFlytekErrorCode::AuthorizationFailure
        );
        assert_eq!(
            IFlytekErrorCode::from_code(99999),
            IFlytekErrorCode::Unknown(99999)
        );
    }

    #[test]
    fn test_error_code_is_success() {
        assert!(IFlytekErrorCode::Success.is_success());
        assert!(!IFlytekErrorCode::AuthorizationFailure.is_success());
    }

    #[test]
    fn test_error_code_is_retryable() {
        assert!(IFlytekErrorCode::ReadTimeout.is_retryable());
        assert!(IFlytekErrorCode::RequestExpired.is_retryable());
        assert!(!IFlytekErrorCode::AuthorizationFailure.is_retryable());
    }

    #[test]
    fn test_error_code_description() {
        assert!(IFlytekErrorCode::Success.description().contains("Success"));
        assert!(
            IFlytekErrorCode::AuthorizationFailure
                .description()
                .contains("authorization")
        );
    }

    #[test]
    fn test_error_code_display() {
        let display = format!("{}", IFlytekErrorCode::Success);
        assert!(display.contains("Success"));
        assert!(display.contains("(0)"));
    }

    // Data status tests
    #[test]
    fn test_data_status_conversion() {
        assert_eq!(i32::from(DataStatus::First), 0);
        assert_eq!(i32::from(DataStatus::Continue), 1);
        assert_eq!(i32::from(DataStatus::Last), 2);
    }

    #[test]
    fn test_data_status_try_from() {
        assert_eq!(DataStatus::try_from(0), Ok(DataStatus::First));
        assert_eq!(DataStatus::try_from(1), Ok(DataStatus::Continue));
        assert_eq!(DataStatus::try_from(2), Ok(DataStatus::Last));
        assert!(DataStatus::try_from(3).is_err());
    }

    // Request tests
    #[test]
    fn test_first_frame_request() {
        let request = SttRequest::first_frame(
            "test_app_id",
            "zh_cn",
            "iat",
            Some("mandarin"),
            2000,
            true,
            true,
            true,
            "audio/L16;rate=16000",
            "raw",
            &[0u8; 100],
        );

        assert_eq!(request.common.app_id, "test_app_id");
        assert_eq!(request.business.language, "zh_cn");
        assert_eq!(request.business.domain, "iat");
        assert_eq!(request.business.accent, Some("mandarin".to_string()));
        assert_eq!(request.data.status, 0);
        assert!(!request.data.audio.is_empty());
    }

    #[test]
    fn test_continue_frame_request() {
        let request =
            SttRequest::continue_frame("test_app_id", "audio/L16;rate=16000", "raw", &[0u8; 100]);

        assert_eq!(request.data.status, 1);
    }

    #[test]
    fn test_last_frame_request() {
        let request =
            SttRequest::last_frame("test_app_id", "audio/L16;rate=16000", "raw", &[0u8; 100]);

        assert_eq!(request.data.status, 2);
    }

    #[test]
    fn test_request_to_json() {
        let request = SttRequest::first_frame(
            "test_app_id",
            "zh_cn",
            "iat",
            Some("mandarin"),
            2000,
            true,
            true,
            true,
            "audio/L16;rate=16000",
            "raw",
            &[0u8; 10],
        );

        let json = request.to_json();
        assert!(json.is_ok());

        let json_str = json.unwrap();
        assert!(json_str.contains("test_app_id"));
        assert!(json_str.contains("zh_cn"));
    }

    // Response tests
    #[test]
    fn test_response_parsing() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "result": {
                    "ws": [
                        {
                            "bg": 0,
                            "cw": [{"w": "你", "sc": 0.95}]
                        },
                        {
                            "bg": 1,
                            "cw": [{"w": "好", "sc": 0.98}]
                        }
                    ],
                    "sn": 1,
                    "ls": false,
                    "pgs": "apd"
                },
                "status": 1
            }
        }"#;

        let response = SttResponse::from_json(json);
        assert!(response.is_ok());

        let response = response.unwrap();
        assert!(response.is_success());
        assert!(!response.is_final());
        assert_eq!(response.transcript(), Some("你好".to_string()));
    }

    #[test]
    fn test_response_final() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "result": {
                    "ws": [{"bg": 0, "cw": [{"w": "测试"}]}],
                    "sn": 1,
                    "ls": true
                },
                "status": 2
            }
        }"#;

        let response = SttResponse::from_json(json).unwrap();
        assert!(response.is_final());
    }

    #[test]
    fn test_response_error() {
        let json = r#"{
            "code": 10005,
            "message": "authorization failure",
            "sid": "test_sid"
        }"#;

        let response = SttResponse::from_json(json).unwrap();
        assert!(!response.is_success());
        assert_eq!(
            response.error_code(),
            IFlytekErrorCode::AuthorizationFailure
        );
    }

    #[test]
    fn test_response_replacement() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "result": {
                    "ws": [{"bg": 0, "cw": [{"w": "替换"}]}],
                    "sn": 1,
                    "ls": false,
                    "pgs": "rpl",
                    "rg": [0, 1]
                },
                "status": 1
            }
        }"#;

        let response = SttResponse::from_json(json).unwrap();
        assert!(response.is_replacement());
    }

    #[test]
    fn test_result_data_transcript() {
        let result = SttResultData {
            ws: vec![
                SttWordSegment {
                    bg: 0,
                    cw: vec![SttWord {
                        w: "Hello".to_string(),
                        sc: Some(0.9),
                    }],
                },
                SttWordSegment {
                    bg: 1,
                    cw: vec![SttWord {
                        w: " ".to_string(),
                        sc: None,
                    }],
                },
                SttWordSegment {
                    bg: 2,
                    cw: vec![SttWord {
                        w: "World".to_string(),
                        sc: Some(0.95),
                    }],
                },
            ],
            sn: 1,
            ls: false,
            pgs: None,
            rg: None,
        };

        assert_eq!(result.transcript(), "Hello World");
    }

    #[test]
    fn test_result_data_confidence() {
        let result = SttResultData {
            ws: vec![
                SttWordSegment {
                    bg: 0,
                    cw: vec![SttWord {
                        w: "A".to_string(),
                        sc: Some(0.8),
                    }],
                },
                SttWordSegment {
                    bg: 1,
                    cw: vec![SttWord {
                        w: "B".to_string(),
                        sc: Some(1.0),
                    }],
                },
            ],
            sn: 1,
            ls: false,
            pgs: None,
            rg: None,
        };

        assert!((result.confidence() - 0.9).abs() < 0.01);
    }

    #[test]
    fn test_result_data_confidence_default() {
        let result = SttResultData {
            ws: vec![SttWordSegment {
                bg: 0,
                cw: vec![SttWord {
                    w: "Test".to_string(),
                    sc: None,
                }],
            }],
            sn: 1,
            ls: false,
            pgs: None,
            rg: None,
        };

        // Default confidence when not provided
        assert_eq!(result.confidence(), 0.95);
    }

    #[test]
    fn test_sentence_number() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "result": {
                    "ws": [{"bg": 0, "cw": [{"w": "test"}]}],
                    "sn": 5,
                    "ls": false
                },
                "status": 1
            }
        }"#;

        let response = SttResponse::from_json(json).unwrap();
        assert_eq!(response.sentence_number(), Some(5));
    }
}
