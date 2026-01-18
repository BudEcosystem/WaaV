//! Alibaba Cloud DashScope TTS WebSocket Messages
//!
//! This module defines the WebSocket message types for DashScope TTS providers.
//! Supports both Qwen3-TTS (OpenAI-like protocol) and CosyVoice (inference protocol).

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// =============================================================================
// Qwen3-TTS Messages (OpenAI-like Realtime Protocol)
// =============================================================================

/// Session update message for Qwen3-TTS.
#[derive(Debug, Clone, Serialize)]
pub struct QwenTtsSessionUpdate {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub session: QwenTtsSession,
}

/// Session configuration for Qwen3-TTS.
#[derive(Debug, Clone, Serialize)]
pub struct QwenTtsSession {
    pub voice: String,
    pub response_format: String,
    pub sample_rate: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
}

impl QwenTtsSessionUpdate {
    /// Create a new session update message.
    pub fn new(voice: &str, format: &str, sample_rate: u32, speed: Option<f32>) -> Self {
        Self {
            msg_type: "session.update".to_string(),
            session: QwenTtsSession {
                voice: voice.to_string(),
                response_format: format.to_string(),
                sample_rate,
                speed,
            },
        }
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Input text append message for Qwen3-TTS.
#[derive(Debug, Clone, Serialize)]
pub struct QwenTtsTextAppend {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub text: String,
}

impl QwenTtsTextAppend {
    /// Create a new text append message.
    pub fn new(text: &str) -> Self {
        Self {
            msg_type: "input_text_buffer.append".to_string(),
            text: text.to_string(),
        }
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Input text commit message for Qwen3-TTS.
#[derive(Debug, Clone, Serialize)]
pub struct QwenTtsTextCommit {
    #[serde(rename = "type")]
    pub msg_type: String,
}

impl QwenTtsTextCommit {
    /// Create a new text commit message.
    pub fn new() -> Self {
        Self {
            msg_type: "input_text_buffer.commit".to_string(),
        }
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Default for QwenTtsTextCommit {
    fn default() -> Self {
        Self::new()
    }
}

/// Server response for Qwen3-TTS.
#[derive(Debug, Clone, Deserialize)]
pub struct QwenTtsServerMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub delta: Option<String>,
    #[serde(default)]
    pub error: Option<QwenTtsError>,
}

/// Error details for Qwen3-TTS.
#[derive(Debug, Clone, Deserialize)]
pub struct QwenTtsError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: String,
    pub message: String,
}

impl QwenTtsServerMessage {
    /// Parse from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if this is an audio delta message.
    pub fn is_audio_delta(&self) -> bool {
        self.msg_type == "response.audio.delta"
    }

    /// Check if this is an error message.
    pub fn is_error(&self) -> bool {
        self.msg_type == "error"
    }

    /// Check if this is a session created message.
    pub fn is_session_created(&self) -> bool {
        self.msg_type == "session.created"
    }

    /// Check if this is a response done message.
    pub fn is_response_done(&self) -> bool {
        self.msg_type == "response.done" || self.msg_type == "response.audio.done"
    }

    /// Get decoded audio data if present.
    pub fn get_audio_data(&self) -> Option<Vec<u8>> {
        self.delta.as_ref().and_then(|d| BASE64.decode(d).ok())
    }
}

// =============================================================================
// CosyVoice Messages (DashScope Inference Protocol)
// =============================================================================

/// Run task message for CosyVoice.
#[derive(Debug, Clone, Serialize)]
pub struct CosyVoiceRunTask {
    pub header: CosyVoiceHeader,
    pub payload: CosyVoiceRunPayload,
}

/// Header for CosyVoice messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CosyVoiceHeader {
    pub action: String,
    pub task_id: String,
    pub streaming: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// Payload for run-task message.
#[derive(Debug, Clone, Serialize)]
pub struct CosyVoiceRunPayload {
    pub model: String,
    pub task: String,
    pub task_group: String,
    pub function: String,
    pub input: serde_json::Value,
    pub parameters: CosyVoiceParameters,
}

/// Parameters for CosyVoice.
#[derive(Debug, Clone, Serialize)]
pub struct CosyVoiceParameters {
    pub voice: String,
    pub format: String,
    pub sample_rate: u32,
    pub volume: u8,
    pub rate: f32,
    pub pitch: f32,
}

impl CosyVoiceRunTask {
    /// Create a new run-task message.
    pub fn new(
        model: &str,
        voice: &str,
        format: &str,
        sample_rate: u32,
        volume: u8,
        rate: f32,
        pitch: f32,
    ) -> Self {
        let task_id = Uuid::new_v4().to_string();
        Self {
            header: CosyVoiceHeader {
                action: "run-task".to_string(),
                task_id,
                streaming: "out".to_string(),
                event: None,
                error_code: None,
                error_message: None,
            },
            payload: CosyVoiceRunPayload {
                model: model.to_string(),
                task: "tts".to_string(),
                task_group: "audio".to_string(),
                function: "SpeechSynthesizer".to_string(),
                input: serde_json::json!({}),
                parameters: CosyVoiceParameters {
                    voice: voice.to_string(),
                    format: format.to_string(),
                    sample_rate,
                    volume,
                    rate,
                    pitch,
                },
            },
        }
    }

    /// Get the task ID.
    pub fn task_id(&self) -> &str {
        &self.header.task_id
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Continue task message for CosyVoice (send text).
#[derive(Debug, Clone, Serialize)]
pub struct CosyVoiceContinueTask {
    pub header: CosyVoiceHeader,
    pub payload: CosyVoiceContinuePayload,
}

/// Payload for continue-task message.
#[derive(Debug, Clone, Serialize)]
pub struct CosyVoiceContinuePayload {
    pub input: CosyVoiceTextInput,
}

/// Text input for CosyVoice.
#[derive(Debug, Clone, Serialize)]
pub struct CosyVoiceTextInput {
    pub text: String,
}

impl CosyVoiceContinueTask {
    /// Create a new continue-task message.
    pub fn new(task_id: &str, text: &str) -> Self {
        Self {
            header: CosyVoiceHeader {
                action: "continue-task".to_string(),
                task_id: task_id.to_string(),
                streaming: "out".to_string(),
                event: None,
                error_code: None,
                error_message: None,
            },
            payload: CosyVoiceContinuePayload {
                input: CosyVoiceTextInput {
                    text: text.to_string(),
                },
            },
        }
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Finish task message for CosyVoice.
#[derive(Debug, Clone, Serialize)]
pub struct CosyVoiceFinishTask {
    pub header: CosyVoiceHeader,
    pub payload: CosyVoiceEmptyPayload,
}

/// Empty payload for finish-task.
#[derive(Debug, Clone, Serialize)]
pub struct CosyVoiceEmptyPayload {
    pub input: serde_json::Value,
}

impl CosyVoiceFinishTask {
    /// Create a new finish-task message.
    pub fn new(task_id: &str) -> Self {
        Self {
            header: CosyVoiceHeader {
                action: "finish-task".to_string(),
                task_id: task_id.to_string(),
                streaming: "out".to_string(),
                event: None,
                error_code: None,
                error_message: None,
            },
            payload: CosyVoiceEmptyPayload {
                input: serde_json::json!({}),
            },
        }
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Server response for CosyVoice.
#[derive(Debug, Clone, Deserialize)]
pub struct CosyVoiceResponse {
    pub header: CosyVoiceHeader,
    #[serde(default)]
    pub payload: Option<CosyVoiceResponsePayload>,
}

/// Payload for CosyVoice response.
#[derive(Debug, Clone, Deserialize)]
pub struct CosyVoiceResponsePayload {
    #[serde(default)]
    pub output: Option<CosyVoiceOutput>,
    #[serde(default)]
    pub usage: Option<CosyVoiceUsage>,
}

/// Output for CosyVoice response.
#[derive(Debug, Clone, Deserialize)]
pub struct CosyVoiceOutput {
    #[serde(default)]
    pub audio: Option<String>,
}

/// Usage statistics for CosyVoice.
#[derive(Debug, Clone, Deserialize)]
pub struct CosyVoiceUsage {
    #[serde(default)]
    pub characters: Option<u32>,
}

impl CosyVoiceResponse {
    /// Parse from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if this is a task-started event.
    pub fn is_task_started(&self) -> bool {
        self.header.event.as_deref() == Some("task-started")
    }

    /// Check if this is a result-generated event.
    pub fn is_result_generated(&self) -> bool {
        self.header.event.as_deref() == Some("result-generated")
    }

    /// Check if this is a task-finished event.
    pub fn is_task_finished(&self) -> bool {
        self.header.event.as_deref() == Some("task-finished")
    }

    /// Check if this is a task-failed event.
    pub fn is_task_failed(&self) -> bool {
        self.header.event.as_deref() == Some("task-failed")
    }

    /// Get error information if present.
    pub fn get_error(&self) -> Option<(&str, &str)> {
        if let (Some(code), Some(msg)) = (&self.header.error_code, &self.header.error_message) {
            Some((code.as_str(), msg.as_str()))
        } else {
            None
        }
    }

    /// Get decoded audio data if present.
    pub fn get_audio_data(&self) -> Option<Vec<u8>> {
        self.payload
            .as_ref()?
            .output
            .as_ref()?
            .audio
            .as_ref()
            .and_then(|a| BASE64.decode(a).ok())
    }

    /// Get the task ID.
    pub fn task_id(&self) -> &str {
        &self.header.task_id
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qwen_session_update() {
        let msg = QwenTtsSessionUpdate::new("Cherry", "pcm16", 24000, Some(1.0));
        let json = msg.to_json().unwrap();

        assert!(json.contains("session.update"));
        assert!(json.contains("Cherry"));
        assert!(json.contains("pcm16"));
        assert!(json.contains("24000"));
    }

    #[test]
    fn test_qwen_text_append() {
        let msg = QwenTtsTextAppend::new("Hello world");
        let json = msg.to_json().unwrap();

        assert!(json.contains("input_text_buffer.append"));
        assert!(json.contains("Hello world"));
    }

    #[test]
    fn test_qwen_text_commit() {
        let msg = QwenTtsTextCommit::new();
        let json = msg.to_json().unwrap();

        assert!(json.contains("input_text_buffer.commit"));
    }

    #[test]
    fn test_qwen_server_message_audio_delta() {
        let json = r#"{
            "type": "response.audio.delta",
            "delta": "SGVsbG8gV29ybGQ="
        }"#;

        let msg = QwenTtsServerMessage::from_json(json).unwrap();
        assert!(msg.is_audio_delta());
        assert!(msg.get_audio_data().is_some());
    }

    #[test]
    fn test_qwen_server_message_error() {
        let json = r#"{
            "type": "error",
            "error": {
                "type": "invalid_request",
                "code": "400",
                "message": "Invalid voice"
            }
        }"#;

        let msg = QwenTtsServerMessage::from_json(json).unwrap();
        assert!(msg.is_error());
    }

    #[test]
    fn test_cosyvoice_run_task() {
        let msg = CosyVoiceRunTask::new(
            "cosyvoice-v3-flash",
            "longxiaochun",
            "mp3",
            22050,
            50,
            1.0,
            1.0,
        );
        let json = msg.to_json().unwrap();

        assert!(json.contains("run-task"));
        assert!(json.contains("cosyvoice-v3-flash"));
        assert!(json.contains("longxiaochun"));
        assert!(!msg.task_id().is_empty());
    }

    #[test]
    fn test_cosyvoice_continue_task() {
        let msg = CosyVoiceContinueTask::new("test-task-id", "Hello world");
        let json = msg.to_json().unwrap();

        assert!(json.contains("continue-task"));
        assert!(json.contains("test-task-id"));
        assert!(json.contains("Hello world"));
    }

    #[test]
    fn test_cosyvoice_finish_task() {
        let msg = CosyVoiceFinishTask::new("test-task-id");
        let json = msg.to_json().unwrap();

        assert!(json.contains("finish-task"));
        assert!(json.contains("test-task-id"));
    }

    #[test]
    fn test_cosyvoice_response_result_generated() {
        let json = r#"{
            "header": {
                "task_id": "test-task-id",
                "action": "run-task",
                "streaming": "out",
                "event": "result-generated"
            },
            "payload": {
                "output": {
                    "audio": "SGVsbG8gV29ybGQ="
                }
            }
        }"#;

        let msg = CosyVoiceResponse::from_json(json).unwrap();
        assert!(msg.is_result_generated());
        assert!(msg.get_audio_data().is_some());
    }

    #[test]
    fn test_cosyvoice_response_task_failed() {
        let json = r#"{
            "header": {
                "task_id": "test-task-id",
                "action": "run-task",
                "streaming": "out",
                "event": "task-failed",
                "error_code": "401",
                "error_message": "Unauthorized"
            }
        }"#;

        let msg = CosyVoiceResponse::from_json(json).unwrap();
        assert!(msg.is_task_failed());
        assert!(msg.get_error().is_some());
    }

    #[test]
    fn test_cosyvoice_response_task_finished() {
        let json = r#"{
            "header": {
                "task_id": "test-task-id",
                "action": "run-task",
                "streaming": "out",
                "event": "task-finished"
            },
            "payload": {
                "usage": {
                    "characters": 100
                }
            }
        }"#;

        let msg = CosyVoiceResponse::from_json(json).unwrap();
        assert!(msg.is_task_finished());
    }
}
