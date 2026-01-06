//! Hume EVI WebSocket message types.
//!
//! This module defines all message types used for WebSocket communication with
//! Hume's Empathic Voice Interface (EVI).
//!
//! # Message Flow
//!
//! ```text
//! Client → Server:
//!   - SessionSettings (configure audio format)
//!   - AudioInput (base64-encoded audio chunks)
//!   - TextInput (text messages)
//!   - ToolResponse (function call results)
//!   - PauseAssistant / ResumeAssistant
//!
//! Server → Client:
//!   - ChatMetadata (on connection)
//!   - UserMessage (transcription + prosody)
//!   - AssistantMessage (response text)
//!   - AudioOutput (response audio)
//!   - AssistantEnd (response complete)
//!   - Error (error occurred)
//! ```

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Constants
// =============================================================================

/// Hume EVI WebSocket endpoint URL.
pub const HUME_EVI_WEBSOCKET_URL: &str = "wss://api.hume.ai/v0/evi/chat";

/// Default sample rate for EVI audio input (Hz).
pub const HUME_EVI_DEFAULT_SAMPLE_RATE: u32 = 44100;

/// Default number of audio channels (mono).
pub const HUME_EVI_DEFAULT_CHANNELS: u8 = 1;

/// Maximum EVI session duration in seconds (30 minutes).
pub const HUME_EVI_MAX_SESSION_DURATION: u64 = 1800;

// =============================================================================
// Prosody Scores (Emotion Dimensions)
// =============================================================================

/// Prosody scores representing emotion dimensions detected in speech.
///
/// Each field represents the model's confidence (0.0 to 1.0) that the speaker
/// is expressing that emotion in their tone of voice and language.
///
/// These labels represent categories of emotional expression that most people
/// perceive in vocal and linguistic patterns. They do not imply that the
/// person is actually experiencing these emotions.
///
/// Note: Hume's documentation references 48 core prosody dimensions, but the
/// exact fields may vary. All fields use `#[serde(default)]` so missing
/// fields will deserialize to 0.0.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ProsodyScores {
    /// Admiration - respect and approval
    #[serde(default)]
    pub admiration: f32,
    /// Adoration - strong love or devotion
    #[serde(default)]
    pub adoration: f32,
    /// Aesthetic appreciation - appreciation of beauty
    #[serde(default, rename = "Aesthetic Appreciation")]
    pub aesthetic_appreciation: f32,
    /// Amusement - finding something funny
    #[serde(default)]
    pub amusement: f32,
    /// Anger - strong displeasure
    #[serde(default)]
    pub anger: f32,
    /// Anxiety - worry or unease
    #[serde(default)]
    pub anxiety: f32,
    /// Awe - overwhelming wonder
    #[serde(default)]
    pub awe: f32,
    /// Awkwardness - social discomfort
    #[serde(default)]
    pub awkwardness: f32,
    /// Boredom - lack of interest
    #[serde(default)]
    pub boredom: f32,
    /// Calmness - peace and tranquility
    #[serde(default)]
    pub calmness: f32,
    /// Concentration - focused attention
    #[serde(default)]
    pub concentration: f32,
    /// Confusion - lack of understanding
    #[serde(default)]
    pub confusion: f32,
    /// Contemplation - deep thought
    #[serde(default)]
    pub contemplation: f32,
    /// Contempt - scorn or disdain
    #[serde(default)]
    pub contempt: f32,
    /// Contentment - satisfaction
    #[serde(default)]
    pub contentment: f32,
    /// Craving - strong desire
    #[serde(default)]
    pub craving: f32,
    /// Desire - wanting something
    #[serde(default)]
    pub desire: f32,
    /// Determination - firm resolve
    #[serde(default)]
    pub determination: f32,
    /// Disappointment - unmet expectations
    #[serde(default)]
    pub disappointment: f32,
    /// Disgust - strong aversion
    #[serde(default)]
    pub disgust: f32,
    /// Distress - extreme anxiety or suffering
    #[serde(default)]
    pub distress: f32,
    /// Doubt - uncertainty
    #[serde(default)]
    pub doubt: f32,
    /// Ecstasy - intense joy
    #[serde(default)]
    pub ecstasy: f32,
    /// Embarrassment - self-conscious discomfort
    #[serde(default)]
    pub embarrassment: f32,
    /// Empathic pain - feeling others' suffering
    #[serde(default, rename = "Empathic Pain")]
    pub empathic_pain: f32,
    /// Enthusiasm - eager interest
    #[serde(default)]
    pub enthusiasm: f32,
    /// Entrancement - captivated attention
    #[serde(default)]
    pub entrancement: f32,
    /// Envy - wanting what others have
    #[serde(default)]
    pub envy: f32,
    /// Excitement - eager anticipation
    #[serde(default)]
    pub excitement: f32,
    /// Fear - anticipation of danger
    #[serde(default)]
    pub fear: f32,
    /// Gratitude - thankfulness
    #[serde(default)]
    pub gratitude: f32,
    /// Guilt - remorse for wrongdoing
    #[serde(default)]
    pub guilt: f32,
    /// Horror - intense fear and disgust
    #[serde(default)]
    pub horror: f32,
    /// Interest - curiosity
    #[serde(default)]
    pub interest: f32,
    /// Joy - happiness
    #[serde(default)]
    pub joy: f32,
    /// Love - deep affection
    #[serde(default)]
    pub love: f32,
    /// Nostalgia - longing for the past
    #[serde(default)]
    pub nostalgia: f32,
    /// Pain - physical or emotional suffering
    #[serde(default)]
    pub pain: f32,
    /// Pride - satisfaction in achievements
    #[serde(default)]
    pub pride: f32,
    /// Realization - sudden understanding
    #[serde(default)]
    pub realization: f32,
    /// Relief - release from distress
    #[serde(default)]
    pub relief: f32,
    /// Romance - amorous feelings
    #[serde(default)]
    pub romance: f32,
    /// Sadness - sorrow
    #[serde(default)]
    pub sadness: f32,
    /// Satisfaction - fulfillment
    #[serde(default)]
    pub satisfaction: f32,
    /// Shame - painful self-awareness
    #[serde(default)]
    pub shame: f32,
    /// Surprise (negative) - unexpected negative event
    #[serde(default, rename = "Surprise (negative)")]
    pub surprise_negative: f32,
    /// Surprise (positive) - unexpected positive event
    #[serde(default, rename = "Surprise (positive)")]
    pub surprise_positive: f32,
    /// Sympathy - compassion for others
    #[serde(default)]
    pub sympathy: f32,
    /// Tiredness - fatigue
    #[serde(default)]
    pub tiredness: f32,
    /// Triumph - victory
    #[serde(default)]
    pub triumph: f32,
}

impl ProsodyScores {
    /// Get the top N emotions by score.
    pub fn top_emotions(&self, n: usize) -> Vec<(&'static str, f32)> {
        let mut scores = vec![
            ("Admiration", self.admiration),
            ("Adoration", self.adoration),
            ("Aesthetic Appreciation", self.aesthetic_appreciation),
            ("Amusement", self.amusement),
            ("Anger", self.anger),
            ("Anxiety", self.anxiety),
            ("Awe", self.awe),
            ("Awkwardness", self.awkwardness),
            ("Boredom", self.boredom),
            ("Calmness", self.calmness),
            ("Concentration", self.concentration),
            ("Confusion", self.confusion),
            ("Contemplation", self.contemplation),
            ("Contempt", self.contempt),
            ("Contentment", self.contentment),
            ("Craving", self.craving),
            ("Desire", self.desire),
            ("Determination", self.determination),
            ("Disappointment", self.disappointment),
            ("Disgust", self.disgust),
            ("Distress", self.distress),
            ("Doubt", self.doubt),
            ("Ecstasy", self.ecstasy),
            ("Embarrassment", self.embarrassment),
            ("Empathic Pain", self.empathic_pain),
            ("Enthusiasm", self.enthusiasm),
            ("Entrancement", self.entrancement),
            ("Envy", self.envy),
            ("Excitement", self.excitement),
            ("Fear", self.fear),
            ("Gratitude", self.gratitude),
            ("Guilt", self.guilt),
            ("Horror", self.horror),
            ("Interest", self.interest),
            ("Joy", self.joy),
            ("Love", self.love),
            ("Nostalgia", self.nostalgia),
            ("Pain", self.pain),
            ("Pride", self.pride),
            ("Realization", self.realization),
            ("Relief", self.relief),
            ("Romance", self.romance),
            ("Sadness", self.sadness),
            ("Satisfaction", self.satisfaction),
            ("Shame", self.shame),
            ("Surprise (negative)", self.surprise_negative),
            ("Surprise (positive)", self.surprise_positive),
            ("Sympathy", self.sympathy),
            ("Tiredness", self.tiredness),
            ("Triumph", self.triumph),
        ];
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.into_iter().take(n).collect()
    }

    /// Get the dominant emotion (highest score).
    pub fn dominant_emotion(&self) -> Option<(&'static str, f32)> {
        self.top_emotions(1).into_iter().next()
    }
}

// =============================================================================
// Client → Server Messages
// =============================================================================

/// Messages sent from client to Hume EVI server.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EVIClientMessage {
    /// Configure session settings (audio format).
    SessionSettings(SessionSettings),
    /// Send audio input chunk.
    AudioInput(AudioInput),
    /// Send text input.
    TextInput(TextInput),
    /// Submit tool response.
    ToolResponse(ToolResponse),
    /// Pause assistant speech.
    PauseAssistant(PauseAssistant),
    /// Resume assistant speech.
    ResumeAssistant(ResumeAssistant),
    /// Interrupt assistant.
    StopAssistant(StopAssistant),
}

/// Session settings for configuring audio input format.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSettings {
    /// Audio encoding format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioSettings>,
    /// System prompt override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Context messages to prepopulate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextSettings>,
}

/// Audio format settings.
#[derive(Debug, Clone, Serialize)]
pub struct AudioSettings {
    /// Encoding format (linear16 or webm).
    pub encoding: AudioEncoding,
    /// Sample rate in Hz (default: 44100).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    /// Number of channels (default: 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u8>,
}

/// Supported audio encodings for EVI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioEncoding {
    /// Linear 16-bit PCM, little-endian.
    Linear16,
    /// WebM container format.
    Webm,
}

impl Default for AudioEncoding {
    fn default() -> Self {
        AudioEncoding::Linear16
    }
}

/// Context settings for prepopulating conversation.
#[derive(Debug, Clone, Serialize)]
pub struct ContextSettings {
    /// Previous conversation messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<ContextMessage>>,
}

/// A context message for conversation history.
#[derive(Debug, Clone, Serialize)]
pub struct ContextMessage {
    /// Role (user or assistant).
    pub role: String,
    /// Message content.
    pub content: String,
}

/// Audio input message containing base64-encoded audio.
#[derive(Debug, Clone, Serialize)]
pub struct AudioInput {
    /// Base64-encoded audio data.
    pub data: String,
}

impl AudioInput {
    /// Create new AudioInput from raw audio bytes.
    pub fn from_bytes(audio_data: &[u8]) -> Self {
        Self {
            data: BASE64.encode(audio_data),
        }
    }
}

/// Text input message.
#[derive(Debug, Clone, Serialize)]
pub struct TextInput {
    /// Text content.
    pub text: String,
}

/// Tool response message for function calling.
#[derive(Debug, Clone, Serialize)]
pub struct ToolResponse {
    /// Tool call ID.
    pub tool_call_id: String,
    /// Result content.
    pub content: String,
    /// Tool name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

/// Pause assistant speech.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PauseAssistant {}

/// Resume assistant speech.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ResumeAssistant {}

/// Stop/interrupt assistant.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StopAssistant {}

// =============================================================================
// Server → Client Messages
// =============================================================================

/// Messages received from Hume EVI server.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EVIServerMessage {
    /// Chat metadata on connection.
    ChatMetadata(ChatMetadata),
    /// User message with transcription and prosody.
    UserMessage(UserMessage),
    /// Interim user message (partial transcription).
    UserInterruption(UserInterruption),
    /// Assistant message (response text).
    AssistantMessage(AssistantMessage),
    /// Assistant prosody scores.
    AssistantProsody(AssistantProsody),
    /// Audio output chunk.
    AudioOutput(AudioOutput),
    /// End of assistant response.
    AssistantEnd(AssistantEnd),
    /// Tool call request.
    ToolCall(ToolCall),
    /// Tool error.
    ToolError(ToolError),
    /// Error message.
    Error(EVIError),
    /// WebSocket error.
    WebSocketError(WebSocketError),
    /// Unknown message type (for forward compatibility).
    #[serde(other)]
    Unknown,
}

/// Chat metadata received on WebSocket connection.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatMetadata {
    /// Chat ID for this session.
    pub chat_id: String,
    /// Chat group ID for resuming conversations.
    pub chat_group_id: String,
    /// Request ID.
    #[serde(default)]
    pub request_id: Option<String>,
}

/// User message with transcription and prosody scores.
#[derive(Debug, Clone, Deserialize)]
pub struct UserMessage {
    /// Unique message ID.
    pub id: String,
    /// Transcribed text.
    pub message: UserMessageContent,
    /// Prosody scores (emotional expression).
    #[serde(default)]
    pub models: Option<ProsodyModels>,
    /// Start time in milliseconds.
    #[serde(default)]
    pub from_text: Option<bool>,
    /// Whether this is an interim transcript.
    #[serde(default)]
    pub interim: Option<bool>,
}

/// User message content.
#[derive(Debug, Clone, Deserialize)]
pub struct UserMessageContent {
    /// Role (always "user").
    pub role: String,
    /// Message text.
    pub content: String,
}

/// Prosody models container.
#[derive(Debug, Clone, Deserialize)]
pub struct ProsodyModels {
    /// Prosody scores.
    #[serde(default)]
    pub prosody: Option<ProsodyData>,
}

/// Prosody data container.
#[derive(Debug, Clone, Deserialize)]
pub struct ProsodyData {
    /// Emotion scores.
    pub scores: ProsodyScores,
}

/// User interruption event.
#[derive(Debug, Clone, Deserialize)]
pub struct UserInterruption {
    /// Interruption time in milliseconds.
    #[serde(default)]
    pub time: Option<u64>,
}

/// Assistant message (response text).
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantMessage {
    /// Unique message ID.
    pub id: String,
    /// Message content.
    pub message: AssistantMessageContent,
    /// Start time in milliseconds.
    #[serde(default)]
    pub from_text: Option<bool>,
}

/// Assistant message content.
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantMessageContent {
    /// Role (always "assistant").
    pub role: String,
    /// Message text.
    pub content: String,
}

/// Assistant prosody scores (sent separately from message).
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantProsody {
    /// Message ID this prosody corresponds to.
    pub id: String,
    /// Prosody scores.
    pub models: ProsodyModels,
}

/// Audio output chunk from assistant.
#[derive(Debug, Clone, Deserialize)]
pub struct AudioOutput {
    /// Unique ID.
    pub id: String,
    /// Base64-encoded audio data.
    pub data: String,
}

impl AudioOutput {
    /// Decode the audio data to bytes.
    pub fn decode_audio(&self) -> Result<Vec<u8>, base64::DecodeError> {
        BASE64.decode(&self.data)
    }
}

/// End of assistant response.
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantEnd {
    /// Message ID.
    #[serde(default)]
    pub id: Option<String>,
}

/// Tool call request from assistant.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCall {
    /// Tool call ID.
    pub tool_call_id: String,
    /// Tool name.
    pub name: String,
    /// JSON arguments.
    pub parameters: String,
    /// Message ID.
    #[serde(default)]
    pub id: Option<String>,
}

/// Tool error response.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolError {
    /// Tool call ID that errored.
    pub tool_call_id: String,
    /// Error message.
    pub error: String,
    /// Error code.
    #[serde(default)]
    pub code: Option<String>,
}

/// EVI error message.
#[derive(Debug, Clone, Deserialize)]
pub struct EVIError {
    /// Error code.
    pub code: String,
    /// Error message.
    pub message: String,
    /// Additional details.
    #[serde(default)]
    pub details: Option<HashMap<String, serde_json::Value>>,
}

/// WebSocket-level error.
#[derive(Debug, Clone, Deserialize)]
pub struct WebSocketError {
    /// Error code.
    #[serde(default)]
    pub code: Option<i32>,
    /// Error reason.
    #[serde(default)]
    pub reason: Option<String>,
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Serialize a client message to JSON.
pub fn serialize_client_message(msg: &EVIClientMessage) -> Result<String, serde_json::Error> {
    serde_json::to_string(msg)
}

/// Deserialize a server message from JSON.
pub fn deserialize_server_message(json: &str) -> Result<EVIServerMessage, serde_json::Error> {
    serde_json::from_str(json)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_input_from_bytes() {
        let audio_data = vec![0u8, 1, 2, 3, 4, 5];
        let input = AudioInput::from_bytes(&audio_data);
        assert!(!input.data.is_empty());

        // Verify it's valid base64
        let decoded = BASE64.decode(&input.data).unwrap();
        assert_eq!(decoded, audio_data);
    }

    #[test]
    fn test_audio_output_decode() {
        let audio_data = vec![10u8, 20, 30, 40, 50];
        let encoded = BASE64.encode(&audio_data);
        let output = AudioOutput {
            id: "test".to_string(),
            data: encoded,
        };

        let decoded = output.decode_audio().unwrap();
        assert_eq!(decoded, audio_data);
    }

    #[test]
    fn test_prosody_scores_top_emotions() {
        let scores = ProsodyScores {
            joy: 0.9,
            excitement: 0.8,
            calmness: 0.3,
            anger: 0.1,
            ..Default::default()
        };

        let top = scores.top_emotions(3);
        assert_eq!(top.len(), 3);
        assert_eq!(top[0].0, "Joy");
        assert_eq!(top[0].1, 0.9);
        assert_eq!(top[1].0, "Excitement");
        assert_eq!(top[1].1, 0.8);
    }

    #[test]
    fn test_prosody_scores_dominant_emotion() {
        let scores = ProsodyScores {
            sadness: 0.95,
            joy: 0.1,
            ..Default::default()
        };

        let dominant = scores.dominant_emotion().unwrap();
        assert_eq!(dominant.0, "Sadness");
        assert_eq!(dominant.1, 0.95);
    }

    #[test]
    fn test_serialize_session_settings() {
        let msg = EVIClientMessage::SessionSettings(SessionSettings {
            audio: Some(AudioSettings {
                encoding: AudioEncoding::Linear16,
                sample_rate: Some(44100),
                channels: Some(1),
            }),
            system_prompt: None,
            context: None,
        });

        let json = serialize_client_message(&msg).unwrap();
        assert!(json.contains("session_settings"));
        assert!(json.contains("linear16"));
        assert!(json.contains("44100"));
    }

    #[test]
    fn test_serialize_audio_input() {
        let msg = EVIClientMessage::AudioInput(AudioInput::from_bytes(&[1, 2, 3]));
        let json = serialize_client_message(&msg).unwrap();
        assert!(json.contains("audio_input"));
        assert!(json.contains("data"));
    }

    #[test]
    fn test_serialize_text_input() {
        let msg = EVIClientMessage::TextInput(TextInput {
            text: "Hello, world!".to_string(),
        });
        let json = serialize_client_message(&msg).unwrap();
        assert!(json.contains("text_input"));
        assert!(json.contains("Hello, world!"));
    }

    #[test]
    fn test_serialize_tool_response() {
        let msg = EVIClientMessage::ToolResponse(ToolResponse {
            tool_call_id: "call_123".to_string(),
            content: r#"{"result": "success"}"#.to_string(),
            tool_name: Some("get_weather".to_string()),
        });
        let json = serialize_client_message(&msg).unwrap();
        assert!(json.contains("tool_response"));
        assert!(json.contains("call_123"));
    }

    #[test]
    fn test_deserialize_chat_metadata() {
        let json = r#"{
            "type": "chat_metadata",
            "chat_id": "chat_abc123",
            "chat_group_id": "group_xyz789"
        }"#;

        let msg = deserialize_server_message(json).unwrap();
        match msg {
            EVIServerMessage::ChatMetadata(meta) => {
                assert_eq!(meta.chat_id, "chat_abc123");
                assert_eq!(meta.chat_group_id, "group_xyz789");
            }
            _ => panic!("Expected ChatMetadata"),
        }
    }

    #[test]
    fn test_deserialize_user_message() {
        let json = r#"{
            "type": "user_message",
            "id": "msg_001",
            "message": {
                "role": "user",
                "content": "Hello!"
            },
            "models": {
                "prosody": {
                    "scores": {
                        "Joy": 0.85,
                        "Excitement": 0.6
                    }
                }
            }
        }"#;

        let msg = deserialize_server_message(json).unwrap();
        match msg {
            EVIServerMessage::UserMessage(user_msg) => {
                assert_eq!(user_msg.id, "msg_001");
                assert_eq!(user_msg.message.content, "Hello!");
                let prosody = user_msg.models.unwrap().prosody.unwrap();
                assert_eq!(prosody.scores.joy, 0.85);
                assert_eq!(prosody.scores.excitement, 0.6);
            }
            _ => panic!("Expected UserMessage"),
        }
    }

    #[test]
    fn test_deserialize_assistant_message() {
        let json = r#"{
            "type": "assistant_message",
            "id": "msg_002",
            "message": {
                "role": "assistant",
                "content": "Hi there! How can I help you today?"
            }
        }"#;

        let msg = deserialize_server_message(json).unwrap();
        match msg {
            EVIServerMessage::AssistantMessage(asst_msg) => {
                assert_eq!(asst_msg.id, "msg_002");
                assert!(asst_msg.message.content.contains("help you"));
            }
            _ => panic!("Expected AssistantMessage"),
        }
    }

    #[test]
    fn test_deserialize_audio_output() {
        let audio_data = vec![1u8, 2, 3, 4, 5];
        let encoded = BASE64.encode(&audio_data);
        let json = format!(
            r#"{{
            "type": "audio_output",
            "id": "audio_001",
            "data": "{encoded}"
        }}"#
        );

        let msg = deserialize_server_message(&json).unwrap();
        match msg {
            EVIServerMessage::AudioOutput(output) => {
                assert_eq!(output.id, "audio_001");
                let decoded = output.decode_audio().unwrap();
                assert_eq!(decoded, audio_data);
            }
            _ => panic!("Expected AudioOutput"),
        }
    }

    #[test]
    fn test_deserialize_tool_call() {
        let json = r#"{
            "type": "tool_call",
            "tool_call_id": "call_456",
            "name": "get_weather",
            "parameters": "{\"location\": \"San Francisco\"}"
        }"#;

        let msg = deserialize_server_message(json).unwrap();
        match msg {
            EVIServerMessage::ToolCall(call) => {
                assert_eq!(call.tool_call_id, "call_456");
                assert_eq!(call.name, "get_weather");
                assert!(call.parameters.contains("San Francisco"));
            }
            _ => panic!("Expected ToolCall"),
        }
    }

    #[test]
    fn test_deserialize_error() {
        let json = r#"{
            "type": "error",
            "code": "rate_limit_exceeded",
            "message": "Too many requests"
        }"#;

        let msg = deserialize_server_message(json).unwrap();
        match msg {
            EVIServerMessage::Error(err) => {
                assert_eq!(err.code, "rate_limit_exceeded");
                assert!(err.message.contains("Too many"));
            }
            _ => panic!("Expected Error"),
        }
    }

    #[test]
    fn test_deserialize_assistant_end() {
        let json = r#"{
            "type": "assistant_end",
            "id": "msg_003"
        }"#;

        let msg = deserialize_server_message(json).unwrap();
        match msg {
            EVIServerMessage::AssistantEnd(end) => {
                assert_eq!(end.id, Some("msg_003".to_string()));
            }
            _ => panic!("Expected AssistantEnd"),
        }
    }

    #[test]
    fn test_deserialize_unknown_message() {
        let json = r#"{
            "type": "future_message_type",
            "data": "some data"
        }"#;

        let msg = deserialize_server_message(json).unwrap();
        assert!(matches!(msg, EVIServerMessage::Unknown));
    }

    #[test]
    fn test_audio_encoding_default() {
        assert_eq!(AudioEncoding::default(), AudioEncoding::Linear16);
    }

    #[test]
    fn test_prosody_scores_default() {
        let scores = ProsodyScores::default();
        assert_eq!(scores.joy, 0.0);
        assert_eq!(scores.anger, 0.0);
        assert_eq!(scores.sadness, 0.0);
    }

    #[test]
    fn test_pause_resume_messages() {
        let pause = EVIClientMessage::PauseAssistant(PauseAssistant::default());
        let json = serialize_client_message(&pause).unwrap();
        assert!(json.contains("pause_assistant"));

        let resume = EVIClientMessage::ResumeAssistant(ResumeAssistant::default());
        let json = serialize_client_message(&resume).unwrap();
        assert!(json.contains("resume_assistant"));
    }

    #[test]
    fn test_stop_assistant_message() {
        let stop = EVIClientMessage::StopAssistant(StopAssistant::default());
        let json = serialize_client_message(&stop).unwrap();
        assert!(json.contains("stop_assistant"));
    }
}
