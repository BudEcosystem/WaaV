//! iFlytek TTS Messages Module
//!
//! This module defines the request and response message formats for iFlytek's
//! WebSocket-based Text-to-Speech API.
//!
//! # Message Format
//!
//! iFlytek TTS uses JSON-formatted messages with three main sections:
//! - `common`: Application ID
//! - `business`: Voice parameters (voice name, speed, volume, pitch, encoding)
//! - `data`: Text data (base64 encoded) with status indicator
//!
//! # Status Values
//!
//! For TTS, only status 2 is used (single complete request).

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};

// Re-export error code from STT module
pub use crate::core::stt::iflytek::IFlytekErrorCode;

// =============================================================================
// TTS Request Messages
// =============================================================================

/// Common section of the TTS request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsRequestCommon {
    /// Application ID.
    pub app_id: String,
}

/// Business section of the TTS request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsRequestBusiness {
    /// Audio encoding output ("raw", "lame", "speex", "speex-wb").
    pub aue: String,

    /// Sample rate format string ("audio/L16;rate=16000" or "audio/L16;rate=8000").
    pub auf: String,

    /// Voice name (e.g., "xiaoyan", "john_ce").
    pub vcn: String,

    /// Speaking speed (0-100, 50 is normal).
    pub speed: u32,

    /// Volume (0-100, 50 is normal).
    pub volume: u32,

    /// Pitch (0-100, 50 is normal).
    pub pitch: u32,

    /// Text encoding ("UTF8", "GB2312", "GBK", "BIG5", "UNICODE").
    pub tte: String,

    /// Background sound (0=off, 1=on).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bgs: Option<i32>,

    /// English pronunciation mode (0=auto, 1=letter, 2=word).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reg: Option<i32>,

    /// Number pronunciation mode (0=auto, 1=digit, 2=value, 3=auto v2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rdn: Option<i32>,
}

/// Data section of the TTS request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsRequestData {
    /// Frame status (always 2 for TTS - single complete request).
    pub status: i32,

    /// Base64-encoded text to synthesize.
    pub text: String,
}

/// Complete TTS request message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsRequest {
    /// Common section (app_id).
    pub common: TtsRequestCommon,

    /// Business section (voice parameters).
    pub business: TtsRequestBusiness,

    /// Data section (text).
    pub data: TtsRequestData,
}

impl TtsRequest {
    /// Create a new TTS request.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app_id: &str,
        voice: &str,
        encoding: &str,
        sample_rate: u32,
        text_encoding: &str,
        speed: u32,
        volume: u32,
        pitch: u32,
        background_sound: bool,
        english_pronunciation: u32,
        number_pronunciation: u32,
        text: &str,
    ) -> Self {
        Self {
            common: TtsRequestCommon {
                app_id: app_id.to_string(),
            },
            business: TtsRequestBusiness {
                aue: encoding.to_string(),
                auf: format!("audio/L16;rate={}", sample_rate),
                vcn: voice.to_string(),
                speed,
                volume,
                pitch,
                tte: text_encoding.to_string(),
                bgs: if background_sound { Some(1) } else { Some(0) },
                reg: Some(english_pronunciation as i32),
                rdn: Some(number_pronunciation as i32),
            },
            data: TtsRequestData {
                status: 2, // Single complete request
                text: BASE64.encode(text.as_bytes()),
            },
        }
    }

    /// Create a minimal TTS request with default options.
    pub fn simple(app_id: &str, voice: &str, text: &str) -> Self {
        Self::new(
            app_id, voice, "raw", 16000, "UTF8", 50, // Normal speed
            50, // Normal volume
            50, // Normal pitch
            false, 0, 0, text,
        )
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// =============================================================================
// TTS Response Messages
// =============================================================================

/// Response data section.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TtsResponseData {
    /// Base64-encoded audio data.
    pub audio: String,

    /// Response status (0 = first chunk, 1 = middle, 2 = last chunk).
    pub status: i32,

    /// Synthesized audio character endpoint (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ced: Option<String>,
}

impl TtsResponseData {
    /// Decode the audio data from base64.
    pub fn decode_audio(&self) -> Result<Vec<u8>, base64::DecodeError> {
        BASE64.decode(&self.audio)
    }

    /// Check if this is the last chunk.
    pub fn is_last(&self) -> bool {
        self.status == 2
    }

    /// Check if this is the first chunk.
    pub fn is_first(&self) -> bool {
        self.status == 0
    }
}

/// Complete TTS response message.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TtsResponse {
    /// Response code (0 = success).
    pub code: i32,

    /// Response message.
    pub message: String,

    /// Session ID.
    pub sid: String,

    /// Response data (audio).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<TtsResponseData>,
}

impl TtsResponse {
    /// Get the error code.
    pub fn error_code(&self) -> IFlytekErrorCode {
        IFlytekErrorCode::from_code(self.code)
    }

    /// Check if the response indicates success.
    pub fn is_success(&self) -> bool {
        self.code == 0
    }

    /// Check if this is the last response.
    pub fn is_last(&self) -> bool {
        self.data.as_ref().map(|d| d.is_last()).unwrap_or(false)
    }

    /// Decode and return the audio data.
    pub fn decode_audio(&self) -> Option<Result<Vec<u8>, base64::DecodeError>> {
        self.data.as_ref().map(|d| d.decode_audio())
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

    // Request tests
    #[test]
    fn test_tts_request_creation() {
        let request = TtsRequest::new(
            "test_app_id",
            "xiaoyan",
            "raw",
            16000,
            "UTF8",
            50,
            50,
            50,
            false,
            0,
            0,
            "你好世界",
        );

        assert_eq!(request.common.app_id, "test_app_id");
        assert_eq!(request.business.vcn, "xiaoyan");
        assert_eq!(request.business.aue, "raw");
        assert_eq!(request.business.speed, 50);
        assert_eq!(request.data.status, 2);
        assert!(!request.data.text.is_empty());
    }

    #[test]
    fn test_tts_request_simple() {
        let request = TtsRequest::simple("app_id", "john_ce", "Hello World");

        assert_eq!(request.common.app_id, "app_id");
        assert_eq!(request.business.vcn, "john_ce");
        assert_eq!(request.business.speed, 50);
    }

    #[test]
    fn test_tts_request_text_encoding() {
        let text = "Hello 你好";
        let request = TtsRequest::simple("app_id", "xiaoyan", text);

        // Decode the base64 text to verify
        let decoded = BASE64.decode(&request.data.text).unwrap();
        let decoded_text = String::from_utf8(decoded).unwrap();
        assert_eq!(decoded_text, text);
    }

    #[test]
    fn test_tts_request_to_json() {
        let request = TtsRequest::simple("app_id", "xiaoyan", "Test");
        let json = request.to_json();
        assert!(json.is_ok());

        let json_str = json.unwrap();
        assert!(json_str.contains("app_id"));
        assert!(json_str.contains("xiaoyan"));
    }

    // Response tests
    #[test]
    fn test_tts_response_parsing_success() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "audio": "AAAA",
                "status": 1,
                "ced": "12345"
            }
        }"#;

        let response = TtsResponse::from_json(json);
        assert!(response.is_ok());

        let response = response.unwrap();
        assert!(response.is_success());
        assert!(!response.is_last());
    }

    #[test]
    fn test_tts_response_last_chunk() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "audio": "AAAA",
                "status": 2
            }
        }"#;

        let response = TtsResponse::from_json(json).unwrap();
        assert!(response.is_last());
    }

    #[test]
    fn test_tts_response_error() {
        let json = r#"{
            "code": 10005,
            "message": "authorization failure",
            "sid": "test_sid"
        }"#;

        let response = TtsResponse::from_json(json).unwrap();
        assert!(!response.is_success());
        assert_eq!(
            response.error_code(),
            IFlytekErrorCode::AuthorizationFailure
        );
    }

    #[test]
    fn test_tts_response_decode_audio() {
        let audio_bytes = vec![0u8, 1, 2, 3, 4, 5];
        let audio_base64 = BASE64.encode(&audio_bytes);

        let json = format!(
            r#"{{"code": 0, "message": "success", "sid": "test", "data": {{"audio": "{}", "status": 1}}}}"#,
            audio_base64
        );

        let response = TtsResponse::from_json(&json).unwrap();
        let decoded = response.decode_audio().unwrap().unwrap();
        assert_eq!(decoded, audio_bytes);
    }

    #[test]
    fn test_tts_response_data_is_first() {
        let data = TtsResponseData {
            audio: "AAAA".to_string(),
            status: 0,
            ced: None,
        };
        assert!(data.is_first());
        assert!(!data.is_last());
    }

    #[test]
    fn test_tts_response_data_is_last() {
        let data = TtsResponseData {
            audio: "AAAA".to_string(),
            status: 2,
            ced: Some("12345".to_string()),
        };
        assert!(!data.is_first());
        assert!(data.is_last());
    }

    #[test]
    fn test_tts_request_business_params() {
        let request = TtsRequest::new(
            "app_id", "xiaoyan", "lame", 8000, "GBK", 30, // Slow
            80, // Loud
            70, // High pitch
            true, 1, // Letter by letter
            2, // Read as value
            "Test",
        );

        assert_eq!(request.business.aue, "lame");
        assert_eq!(request.business.auf, "audio/L16;rate=8000");
        assert_eq!(request.business.tte, "GBK");
        assert_eq!(request.business.speed, 30);
        assert_eq!(request.business.volume, 80);
        assert_eq!(request.business.pitch, 70);
        assert_eq!(request.business.bgs, Some(1));
        assert_eq!(request.business.reg, Some(1));
        assert_eq!(request.business.rdn, Some(2));
    }
}
