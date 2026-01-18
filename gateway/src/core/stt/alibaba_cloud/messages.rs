//! Alibaba Cloud DashScope STT WebSocket Messages
//!
//! This module defines the WebSocket message types for DashScope STT API.
//!
//! # Message Formats
//!
//! DashScope supports two message formats:
//!
//! 1. **Qwen Realtime Format**: OpenAI-like format for Qwen3-ASR models
//! 2. **Inference Format**: Traditional DashScope format for Paraformer models

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// =============================================================================
// Qwen Realtime Format (OpenAI-like)
// =============================================================================

/// Session update request for Qwen realtime API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QwenSessionUpdate {
    /// Message type: "session.update".
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Session configuration.
    pub session: QwenSessionConfig,
}

/// Session configuration for Qwen realtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QwenSessionConfig {
    /// Output modalities (e.g., ["text"]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Vec<String>>,

    /// Input audio format (e.g., "pcm16").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_audio_format: Option<String>,

    /// Input audio transcription config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_audio_transcription: Option<QwenAudioTranscriptionConfig>,

    /// Turn detection configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_detection: Option<QwenTurnDetection>,
}

/// Audio transcription configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QwenAudioTranscriptionConfig {
    /// Audio sample rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,

    /// Language code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// Turn detection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QwenTurnDetection {
    /// Detection type (e.g., "server_vad", "manual", "none").
    #[serde(rename = "type")]
    pub detection_type: String,

    /// Silence duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub silence_duration_ms: Option<u32>,

    /// VAD threshold (0.0 - 1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,
}

impl QwenSessionUpdate {
    /// Create a new session update.
    pub fn new(
        language: &str,
        sample_rate: u32,
        audio_format: &str,
        silence_duration_ms: u32,
        turn_detection_type: &str,
    ) -> Self {
        Self {
            msg_type: "session.update".to_string(),
            session: QwenSessionConfig {
                modalities: Some(vec!["text".to_string()]),
                input_audio_format: Some(audio_format.to_string()),
                input_audio_transcription: Some(QwenAudioTranscriptionConfig {
                    sample_rate: Some(sample_rate),
                    language: Some(language.to_string()),
                }),
                turn_detection: Some(QwenTurnDetection {
                    detection_type: turn_detection_type.to_string(),
                    silence_duration_ms: Some(silence_duration_ms),
                    threshold: Some(0.5),
                }),
            },
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Audio buffer append message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QwenAudioBufferAppend {
    /// Message type: "input_audio_buffer.append".
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Base64-encoded audio data.
    pub audio: String,
}

impl QwenAudioBufferAppend {
    /// Create audio buffer append from raw bytes.
    pub fn from_bytes(audio: &[u8]) -> Self {
        Self {
            msg_type: "input_audio_buffer.append".to_string(),
            audio: BASE64.encode(audio),
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Audio buffer commit message (for manual mode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QwenAudioBufferCommit {
    /// Message type: "input_audio_buffer.commit".
    #[serde(rename = "type")]
    pub msg_type: String,
}

impl QwenAudioBufferCommit {
    /// Create new commit message.
    pub fn new() -> Self {
        Self {
            msg_type: "input_audio_buffer.commit".to_string(),
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Default for QwenAudioBufferCommit {
    fn default() -> Self {
        Self::new()
    }
}

/// Session finish message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QwenSessionFinish {
    /// Message type: "session.finish".
    #[serde(rename = "type")]
    pub msg_type: String,
}

impl QwenSessionFinish {
    /// Create new session finish message.
    pub fn new() -> Self {
        Self {
            msg_type: "session.finish".to_string(),
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Default for QwenSessionFinish {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Qwen Realtime Response Types
// =============================================================================

/// Qwen realtime server message.
#[derive(Debug, Clone, Deserialize)]
pub struct QwenServerMessage {
    /// Message type.
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Session ID (for session events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<QwenSessionInfo>,

    /// Transcript text (for transcription events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,

    /// Error information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<QwenError>,

    /// Emotion recognition result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<QwenEmotion>,

    /// Raw JSON for additional fields.
    #[serde(flatten)]
    pub extra: Value,
}

/// Session information.
#[derive(Debug, Clone, Deserialize)]
pub struct QwenSessionInfo {
    /// Session ID.
    pub id: String,
}

/// Emotion recognition result.
#[derive(Debug, Clone, Deserialize)]
pub struct QwenEmotion {
    /// Emotion tag.
    pub tag: String,

    /// Confidence score.
    pub confidence: f32,
}

/// Error information.
#[derive(Debug, Clone, Deserialize)]
pub struct QwenError {
    /// Error type.
    #[serde(rename = "type")]
    pub error_type: Option<String>,

    /// Error code.
    pub code: Option<String>,

    /// Error message.
    pub message: String,
}

impl QwenServerMessage {
    /// Parse from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if this is a session created event.
    pub fn is_session_created(&self) -> bool {
        self.msg_type == "session.created"
    }

    /// Check if this is a session updated event.
    pub fn is_session_updated(&self) -> bool {
        self.msg_type == "session.updated"
    }

    /// Check if this is a transcription completed event.
    pub fn is_transcription_completed(&self) -> bool {
        self.msg_type == "conversation.item.input_audio_transcription.completed"
    }

    /// Check if this is an error event.
    pub fn is_error(&self) -> bool {
        self.msg_type == "error"
    }

    /// Check if this is a session finished event.
    pub fn is_session_finished(&self) -> bool {
        self.msg_type == "session.finished"
    }

    /// Get transcript text if available.
    pub fn get_transcript(&self) -> Option<&str> {
        self.transcript.as_deref()
    }
}

// =============================================================================
// Paraformer Inference Format
// =============================================================================

/// Paraformer run-task request header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParaformerHeader {
    /// Action type.
    pub action: String,

    /// Task ID (UUID).
    pub task_id: String,

    /// Streaming mode ("duplex" for bidirectional).
    pub streaming: String,
}

/// Paraformer run-task request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParaformerPayload {
    /// Model name.
    pub model: String,

    /// Task type.
    pub task: String,

    /// Task group.
    pub task_group: String,

    /// Function name.
    pub function: String,

    /// Input (empty for initial request).
    pub input: Value,

    /// Parameters.
    pub parameters: ParaformerParameters,
}

/// Paraformer STT parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParaformerParameters {
    /// Audio format.
    pub format: String,

    /// Sample rate.
    pub sample_rate: u32,

    /// Vocabulary ID for hotwords.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vocabulary_id: Option<String>,

    /// Enable disfluency removal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disfluency_removal_enabled: Option<bool>,

    /// Language hints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_hints: Option<Vec<String>>,

    /// Enable semantic punctuation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_punctuation_enabled: Option<bool>,

    /// Max sentence silence (ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_sentence_silence: Option<u32>,

    /// Enable punctuation prediction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub punctuation_prediction_enabled: Option<bool>,

    /// Enable heartbeat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat: Option<bool>,

    /// Enable inverse text normalization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inverse_text_normalization_enabled: Option<bool>,
}

/// Complete Paraformer run-task request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParaformerRunTask {
    /// Request header.
    pub header: ParaformerHeader,

    /// Request payload.
    pub payload: ParaformerPayload,
}

impl ParaformerRunTask {
    /// Create a new run-task request.
    pub fn new(
        model: &str,
        format: &str,
        sample_rate: u32,
        language: &str,
        disfluency_removal: bool,
        punctuation: bool,
    ) -> Self {
        Self {
            header: ParaformerHeader {
                action: "run-task".to_string(),
                task_id: Uuid::new_v4().to_string(),
                streaming: "duplex".to_string(),
            },
            payload: ParaformerPayload {
                model: model.to_string(),
                task: "asr".to_string(),
                task_group: "audio".to_string(),
                function: "recognition".to_string(),
                input: Value::Object(serde_json::Map::new()),
                parameters: ParaformerParameters {
                    format: format.to_string(),
                    sample_rate,
                    vocabulary_id: None,
                    disfluency_removal_enabled: Some(disfluency_removal),
                    language_hints: if language != "auto" {
                        Some(vec![language.to_string()])
                    } else {
                        None
                    },
                    semantic_punctuation_enabled: Some(punctuation),
                    max_sentence_silence: Some(800),
                    punctuation_prediction_enabled: Some(punctuation),
                    heartbeat: Some(true),
                    inverse_text_normalization_enabled: Some(true),
                },
            },
        }
    }

    /// Get the task ID.
    pub fn task_id(&self) -> &str {
        &self.header.task_id
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Paraformer finish-task request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParaformerFinishTask {
    /// Request header.
    pub header: ParaformerHeader,

    /// Request payload.
    pub payload: ParaformerFinishPayload,
}

/// Finish task payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParaformerFinishPayload {
    /// Empty input.
    pub input: Value,
}

impl ParaformerFinishTask {
    /// Create a new finish-task request.
    pub fn new(task_id: &str) -> Self {
        Self {
            header: ParaformerHeader {
                action: "finish-task".to_string(),
                task_id: task_id.to_string(),
                streaming: "duplex".to_string(),
            },
            payload: ParaformerFinishPayload {
                input: Value::Object(serde_json::Map::new()),
            },
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// =============================================================================
// Paraformer Response Types
// =============================================================================

/// Paraformer server response.
#[derive(Debug, Clone, Deserialize)]
pub struct ParaformerResponse {
    /// Response header.
    pub header: ParaformerResponseHeader,

    /// Response payload.
    #[serde(default)]
    pub payload: Option<ParaformerResponsePayload>,
}

/// Response header.
#[derive(Debug, Clone, Deserialize)]
pub struct ParaformerResponseHeader {
    /// Task ID.
    pub task_id: String,

    /// Event type.
    pub event: String,

    /// Error code (if error).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,

    /// Error message (if error).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// Response payload.
#[derive(Debug, Clone, Deserialize)]
pub struct ParaformerResponsePayload {
    /// Output data.
    #[serde(default)]
    pub output: Option<ParaformerOutput>,

    /// Usage statistics.
    #[serde(default)]
    pub usage: Option<ParaformerUsage>,
}

/// Output data.
#[derive(Debug, Clone, Deserialize)]
pub struct ParaformerOutput {
    /// Sentence result.
    #[serde(default)]
    pub sentence: Option<ParaformerSentence>,
}

/// Sentence result.
#[derive(Debug, Clone, Deserialize)]
pub struct ParaformerSentence {
    /// Begin time in milliseconds.
    pub begin_time: u64,

    /// End time in milliseconds (null for intermediate).
    pub end_time: Option<u64>,

    /// Transcribed text.
    pub text: String,

    /// Word-level results.
    #[serde(default)]
    pub words: Vec<ParaformerWord>,

    /// Is this the end of sentence.
    #[serde(default)]
    pub sentence_end: bool,

    /// Emotion tag (for 8k models).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emo_tag: Option<String>,

    /// Emotion confidence (for 8k models).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emo_confidence: Option<f32>,
}

/// Word-level result.
#[derive(Debug, Clone, Deserialize)]
pub struct ParaformerWord {
    /// Begin time in milliseconds.
    pub begin_time: u64,

    /// End time in milliseconds.
    pub end_time: u64,

    /// Word text.
    pub text: String,
}

/// Usage statistics.
#[derive(Debug, Clone, Deserialize)]
pub struct ParaformerUsage {
    /// Audio duration in seconds.
    #[serde(default)]
    pub duration: f64,
}

impl ParaformerResponse {
    /// Parse from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if this is a task-started event.
    pub fn is_task_started(&self) -> bool {
        self.header.event == "task-started"
    }

    /// Check if this is a result-generated event.
    pub fn is_result_generated(&self) -> bool {
        self.header.event == "result-generated"
    }

    /// Check if this is a task-finished event.
    pub fn is_task_finished(&self) -> bool {
        self.header.event == "task-finished"
    }

    /// Check if this is a task-failed event.
    pub fn is_task_failed(&self) -> bool {
        self.header.event == "task-failed"
    }

    /// Get error message if this is an error.
    pub fn get_error(&self) -> Option<(&str, &str)> {
        if self.is_task_failed() {
            Some((
                self.header.error_code.as_deref().unwrap_or("unknown"),
                self.header.error_message.as_deref().unwrap_or("Unknown error"),
            ))
        } else {
            None
        }
    }

    /// Get the sentence result if available.
    pub fn get_sentence(&self) -> Option<&ParaformerSentence> {
        self.payload
            .as_ref()
            .and_then(|p| p.output.as_ref())
            .and_then(|o| o.sentence.as_ref())
    }

    /// Get transcript text if available.
    pub fn get_transcript(&self) -> Option<&str> {
        self.get_sentence().map(|s| s.text.as_str())
    }

    /// Check if this is a final result (sentence_end = true).
    pub fn is_final(&self) -> bool {
        self.get_sentence().map(|s| s.sentence_end).unwrap_or(false)
    }
}

// =============================================================================
// Error Codes
// =============================================================================

/// DashScope error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashScopeErrorCode {
    /// Success.
    Success,
    /// Bad request (invalid parameters).
    BadRequest,
    /// Unauthorized (invalid API key).
    Unauthorized,
    /// Forbidden (region/quota issue).
    Forbidden,
    /// Rate limit exceeded.
    RateLimitExceeded,
    /// Internal server error.
    InternalError,
    /// Service unavailable.
    ServiceUnavailable,
    /// Unknown error.
    Unknown,
}

impl DashScopeErrorCode {
    /// Parse error code from string.
    pub fn from_str(code: &str) -> Self {
        match code {
            "200" | "0" => Self::Success,
            "400" | "InvalidParameter" => Self::BadRequest,
            "401" | "Unauthorized" => Self::Unauthorized,
            "403" | "Forbidden" => Self::Forbidden,
            "429" | "RateLimitExceeded" | "TooManyRequests" => Self::RateLimitExceeded,
            "500" | "InternalError" => Self::InternalError,
            "503" | "ServiceUnavailable" => Self::ServiceUnavailable,
            _ => Self::Unknown,
        }
    }

    /// Get human-readable description.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Success => "Success",
            Self::BadRequest => "Bad request - invalid parameters",
            Self::Unauthorized => "Unauthorized - invalid API key",
            Self::Forbidden => "Forbidden - region or quota issue",
            Self::RateLimitExceeded => "Rate limit exceeded",
            Self::InternalError => "Internal server error",
            Self::ServiceUnavailable => "Service unavailable",
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

    // Qwen format tests
    #[test]
    fn test_qwen_session_update() {
        let msg = QwenSessionUpdate::new("zh", 16000, "pcm16", 400, "server_vad");
        let json = msg.to_json().unwrap();

        assert!(json.contains("session.update"));
        assert!(json.contains("pcm16"));
        assert!(json.contains("server_vad"));
    }

    #[test]
    fn test_qwen_audio_buffer_append() {
        let audio = vec![0u8, 1, 2, 3, 4, 5];
        let msg = QwenAudioBufferAppend::from_bytes(&audio);
        let json = msg.to_json().unwrap();

        assert!(json.contains("input_audio_buffer.append"));
        assert!(json.contains("audio"));
    }

    #[test]
    fn test_qwen_audio_buffer_commit() {
        let msg = QwenAudioBufferCommit::new();
        let json = msg.to_json().unwrap();

        assert!(json.contains("input_audio_buffer.commit"));
    }

    #[test]
    fn test_qwen_session_finish() {
        let msg = QwenSessionFinish::new();
        let json = msg.to_json().unwrap();

        assert!(json.contains("session.finish"));
    }

    #[test]
    fn test_qwen_server_message_parsing() {
        let json = r#"{
            "type": "session.created",
            "session": {"id": "test-session-id"}
        }"#;

        let msg = QwenServerMessage::from_json(json).unwrap();
        assert!(msg.is_session_created());
        assert_eq!(msg.session.unwrap().id, "test-session-id");
    }

    #[test]
    fn test_qwen_transcription_completed() {
        let json = r#"{
            "type": "conversation.item.input_audio_transcription.completed",
            "transcript": "你好世界"
        }"#;

        let msg = QwenServerMessage::from_json(json).unwrap();
        assert!(msg.is_transcription_completed());
        assert_eq!(msg.get_transcript(), Some("你好世界"));
    }

    #[test]
    fn test_qwen_error_message() {
        let json = r#"{
            "type": "error",
            "error": {
                "type": "invalid_request",
                "code": "400",
                "message": "Invalid audio format"
            }
        }"#;

        let msg = QwenServerMessage::from_json(json).unwrap();
        assert!(msg.is_error());
        assert_eq!(msg.error.as_ref().unwrap().message, "Invalid audio format");
    }

    // Paraformer format tests
    #[test]
    fn test_paraformer_run_task() {
        let msg = ParaformerRunTask::new(
            "paraformer-realtime-v2",
            "pcm",
            16000,
            "zh",
            true,
            true,
        );
        let json = msg.to_json().unwrap();

        assert!(json.contains("run-task"));
        assert!(json.contains("paraformer-realtime-v2"));
        assert!(json.contains("asr"));
        assert!(!msg.task_id().is_empty());
    }

    #[test]
    fn test_paraformer_finish_task() {
        let msg = ParaformerFinishTask::new("test-task-id");
        let json = msg.to_json().unwrap();

        assert!(json.contains("finish-task"));
        assert!(json.contains("test-task-id"));
    }

    #[test]
    fn test_paraformer_response_task_started() {
        let json = r#"{
            "header": {
                "task_id": "test-task-id",
                "event": "task-started"
            }
        }"#;

        let msg = ParaformerResponse::from_json(json).unwrap();
        assert!(msg.is_task_started());
        assert_eq!(msg.header.task_id, "test-task-id");
    }

    #[test]
    fn test_paraformer_response_result_generated() {
        let json = r#"{
            "header": {
                "task_id": "test-task-id",
                "event": "result-generated"
            },
            "payload": {
                "output": {
                    "sentence": {
                        "begin_time": 0,
                        "end_time": 1500,
                        "text": "你好世界",
                        "words": [],
                        "sentence_end": false
                    }
                }
            }
        }"#;

        let msg = ParaformerResponse::from_json(json).unwrap();
        assert!(msg.is_result_generated());
        assert_eq!(msg.get_transcript(), Some("你好世界"));
        assert!(!msg.is_final());
    }

    #[test]
    fn test_paraformer_response_final_result() {
        let json = r#"{
            "header": {
                "task_id": "test-task-id",
                "event": "result-generated"
            },
            "payload": {
                "output": {
                    "sentence": {
                        "begin_time": 0,
                        "end_time": 2000,
                        "text": "你好世界",
                        "words": [
                            {"begin_time": 0, "end_time": 1000, "text": "你好"},
                            {"begin_time": 1000, "end_time": 2000, "text": "世界"}
                        ],
                        "sentence_end": true
                    }
                }
            }
        }"#;

        let msg = ParaformerResponse::from_json(json).unwrap();
        assert!(msg.is_final());
        let sentence = msg.get_sentence().unwrap();
        assert_eq!(sentence.words.len(), 2);
    }

    #[test]
    fn test_paraformer_response_task_failed() {
        let json = r#"{
            "header": {
                "task_id": "test-task-id",
                "event": "task-failed",
                "error_code": "401",
                "error_message": "Unauthorized"
            }
        }"#;

        let msg = ParaformerResponse::from_json(json).unwrap();
        assert!(msg.is_task_failed());
        let (code, message) = msg.get_error().unwrap();
        assert_eq!(code, "401");
        assert_eq!(message, "Unauthorized");
    }

    #[test]
    fn test_paraformer_response_with_emotion() {
        let json = r#"{
            "header": {
                "task_id": "test-task-id",
                "event": "result-generated"
            },
            "payload": {
                "output": {
                    "sentence": {
                        "begin_time": 0,
                        "end_time": 1500,
                        "text": "你好",
                        "words": [],
                        "sentence_end": true,
                        "emo_tag": "happy",
                        "emo_confidence": 0.85
                    }
                }
            }
        }"#;

        let msg = ParaformerResponse::from_json(json).unwrap();
        let sentence = msg.get_sentence().unwrap();
        assert_eq!(sentence.emo_tag.as_deref(), Some("happy"));
        assert!((sentence.emo_confidence.unwrap() - 0.85).abs() < 0.01);
    }

    // Error code tests
    #[test]
    fn test_error_code_parsing() {
        assert_eq!(DashScopeErrorCode::from_str("200"), DashScopeErrorCode::Success);
        assert_eq!(DashScopeErrorCode::from_str("401"), DashScopeErrorCode::Unauthorized);
        assert_eq!(DashScopeErrorCode::from_str("429"), DashScopeErrorCode::RateLimitExceeded);
        assert_eq!(DashScopeErrorCode::from_str("unknown"), DashScopeErrorCode::Unknown);
    }

    #[test]
    fn test_error_code_description() {
        assert_eq!(DashScopeErrorCode::Unauthorized.description(), "Unauthorized - invalid API key");
        assert_eq!(DashScopeErrorCode::RateLimitExceeded.description(), "Rate limit exceeded");
    }
}
