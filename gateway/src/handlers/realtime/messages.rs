//! Realtime WebSocket message types
//!
//! This module defines all message types for the Realtime audio-to-audio API.
//! The protocol is designed to be provider-agnostic, abstracting away
//! provider-specific details (OpenAI, etc.) behind a unified interface.

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Maximum allowed size for instructions (100 KB)
pub const MAX_INSTRUCTIONS_SIZE: usize = 100 * 1024;

/// Maximum allowed size for text messages (50 KB)
pub const MAX_TEXT_SIZE: usize = 50 * 1024;

/// Maximum allowed size for function result (100 KB)
pub const MAX_FUNCTION_RESULT_SIZE: usize = 100 * 1024;

// =============================================================================
// Incoming Messages (Client -> Server)
// =============================================================================

/// Incoming WebSocket messages from client
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RealtimeIncomingMessage {
    /// Session configuration
    #[serde(rename = "config")]
    Config(RealtimeSessionConfig),

    /// Send text message to the conversation
    #[serde(rename = "text")]
    Text {
        /// Text content
        text: String,
    },

    /// Request model to generate a response, optionally with per-response
    /// overrides (instructions/modalities/voice/token cap/out-of-band). A bare
    /// `{"type":"create_response"}` keeps the old no-override behavior.
    #[serde(rename = "create_response")]
    CreateResponse {
        #[serde(default)]
        response: Option<ClientResponseConfig>,
    },

    /// Cancel current response generation
    #[serde(rename = "cancel_response")]
    CancelResponse,

    /// Commit audio buffer (for manual turn detection)
    #[serde(rename = "commit_audio")]
    CommitAudio,

    /// Clear audio buffer
    #[serde(rename = "clear_audio")]
    ClearAudio,

    /// Submit function call result
    #[serde(rename = "function_result")]
    FunctionResult {
        /// Function call ID
        call_id: String,
        /// Function result as JSON string
        result: String,
    },

    /// Update session configuration mid-stream
    #[serde(rename = "update_session")]
    UpdateSession(RealtimeSessionConfig),
}

/// Per-response override carried by the `create_response` message.
///
/// Provider-agnostic; maps to [`crate::core::realtime::base::RealtimeResponseOverride`]
/// (OpenAI Realtime GA `response.create`). All fields optional — omit for the
/// session defaults.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ClientResponseConfig {
    /// Output modalities for this response (e.g. `["text"]` or `["audio"]`).
    #[serde(default)]
    pub modalities: Option<Vec<String>>,
    /// Per-response system instructions.
    #[serde(default)]
    pub instructions: Option<String>,
    /// Output voice for this response.
    #[serde(default)]
    pub voice: Option<String>,
    /// Max output tokens for this response (negative ⇒ unlimited).
    #[serde(default)]
    pub max_output_tokens: Option<i32>,
    /// Out-of-band: don't add this response to the default conversation.
    #[serde(default)]
    pub out_of_band: Option<bool>,
    /// Opaque metadata echoed back on the response.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// Realtime session configuration
///
/// This configuration is provider-agnostic. Provider-specific options
/// are abstracted into common patterns.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RealtimeSessionConfig {
    /// Provider to use (e.g., "openai")
    #[serde(default)]
    pub provider: Option<String>,

    /// Model to use (provider-specific)
    /// For OpenAI: "gpt-4o-realtime-preview", "gpt-4o-mini-realtime-preview"
    #[serde(default)]
    pub model: Option<String>,

    /// Voice for TTS output (provider-specific)
    /// For OpenAI: "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"
    #[serde(default)]
    pub voice: Option<String>,

    /// System instructions for the assistant
    #[serde(default)]
    pub instructions: Option<String>,

    /// Temperature for response generation (0.0 to 2.0)
    #[serde(default)]
    pub temperature: Option<f32>,

    /// Maximum response tokens (-1 for infinite)
    #[serde(default)]
    pub max_response_tokens: Option<i32>,

    /// Turn detection configuration
    #[serde(default)]
    pub turn_detection: Option<TurnDetectionConfig>,

    /// Tool definitions for function calling
    #[serde(default)]
    pub tools: Option<Vec<ToolConfig>>,

    /// Response modalities
    #[serde(default)]
    pub modalities: Option<Vec<String>>,

    /// Enable input audio transcription
    #[serde(default)]
    pub transcribe_input: Option<bool>,

    /// Transcription model for input audio
    /// For OpenAI: "whisper-1", "gpt-4o-transcribe", "gpt-4o-mini-transcribe"
    #[serde(default)]
    pub transcription_model: Option<String>,

    /// Input audio format override
    #[serde(default)]
    pub input_audio_format: Option<String>,

    /// Output audio format override
    #[serde(default)]
    pub output_audio_format: Option<String>,

    /// S2S (REALTIME_REASONING.md §6): reasoning effort for reasoning-capable
    /// realtime models (e.g. gpt-realtime-2). `minimal|low|medium|high`; `off`
    /// (or omitted) sends nothing — recommended start for production voice: `low`.
    #[serde(default)]
    pub reasoning_effort: Option<crate::core::llm::ReasoningEffort>,

    /// Input-audio noise reduction: `near_field` (headsets / close mics) or
    /// `far_field` (laptop / room mics). Omitted = off.
    #[serde(default)]
    pub input_audio_noise_reduction: Option<String>,

    /// Optional server-side ALIAS name (P3). Resolves a server-defined
    /// `{realtime provider+model+voice}` bundle (and/or `llm` instructions) BEFORE the
    /// provider/credential is selected; explicit fields above OVERRIDE the alias
    /// default. The client supplies only the NAME — alias DEFINITIONS are
    /// server-config-only (`aliases:` in `config.yaml`), SSRF-safe like
    /// `realtime_endpoint_overrides`. An unknown alias is non-fatal (the session
    /// proceeds with the client config + an `alias_unknown` error advisory).
    #[serde(default)]
    pub alias: Option<String>,
}

/// Turn detection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum TurnDetectionConfig {
    /// Server-side Voice Activity Detection
    #[serde(rename = "server_vad")]
    ServerVad {
        /// Activation threshold (0.0 to 1.0)
        #[serde(default)]
        threshold: Option<f32>,
        /// Silence duration before end of turn (ms)
        #[serde(default)]
        silence_duration_ms: Option<u32>,
        /// Amount of audio to include before voice detection (ms)
        #[serde(default)]
        prefix_padding_ms: Option<u32>,
    },
    /// Semantic turn detection (provider-specific)
    #[serde(rename = "semantic")]
    Semantic {
        /// Eagerness level (low, medium, high, auto)
        #[serde(default)]
        eagerness: Option<String>,
    },
    /// Manual turn detection (no auto-detection)
    #[serde(rename = "manual")]
    Manual,
}

/// Tool configuration for function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ToolConfig {
    /// Tool type (e.g., "function")
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Function definition
    pub function: FunctionConfig,
}

/// Function definition for tool calling
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FunctionConfig {
    /// Function name
    pub name: String,
    /// Function description
    #[serde(default)]
    pub description: Option<String>,
    /// JSON schema for parameters
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

// =============================================================================
// Outgoing Messages (Server -> Client)
// =============================================================================

/// Outgoing WebSocket messages to client
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RealtimeOutgoingMessage {
    /// Session created/ready
    #[serde(rename = "session_created")]
    SessionCreated {
        /// Session ID
        session_id: String,
        /// Provider used
        provider: String,
        /// Model in use
        model: String,
    },

    /// Session updated
    #[serde(rename = "session_updated")]
    SessionUpdated,

    /// Transcript from user speech
    #[serde(rename = "transcript")]
    Transcript {
        /// Transcribed text
        text: String,
        /// Role (user or assistant)
        role: String,
        /// Whether this is a final transcript
        is_final: bool,
    },

    /// Speech detection event
    #[serde(rename = "speech_event")]
    SpeechEvent {
        /// Event type (started, stopped)
        event: String,
        /// Audio timestamp in milliseconds
        audio_ms: u64,
    },

    /// Function call request from model
    #[serde(rename = "function_call")]
    FunctionCall {
        /// Call ID
        call_id: String,
        /// Function name
        name: String,
        /// JSON arguments
        arguments: String,
    },

    /// Response generation started
    #[serde(rename = "response_started")]
    ResponseStarted {
        /// Response ID
        response_id: String,
    },

    /// Response generation completed
    #[serde(rename = "response_done")]
    ResponseDone {
        /// Response ID
        response_id: String,
    },

    /// Error message
    #[serde(rename = "error")]
    Error {
        /// Error code (optional)
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        /// Error message
        message: String,
    },

    /// Connection closing
    #[serde(rename = "closing")]
    Closing {
        /// Reason for closing
        reason: String,
    },
}

// =============================================================================
// Message Routing
// =============================================================================

/// Message routing for optimized throughput
pub enum RealtimeMessageRoute {
    /// JSON text message
    Outgoing(RealtimeOutgoingMessage),
    /// Binary audio data
    Audio(Bytes),
    /// Close connection
    Close,
}

/// Delivery class for outbound realtime WebSocket messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeMessageClass {
    /// Realtime audio egress. Stale audio is worthless if the client is not
    /// reading, so a full channel drops the frame immediately.
    DroppableAudio,
    /// Transcript-bearing events. These can wait briefly for the client but must
    /// not wedge provider callback tasks indefinitely.
    Transcript,
    /// Errors, lifecycle, function-call, speech-event and close messages.
    Critical,
}

impl RealtimeMessageRoute {
    fn class(&self) -> RealtimeMessageClass {
        match self {
            Self::Audio(_) => RealtimeMessageClass::DroppableAudio,
            Self::Outgoing(RealtimeOutgoingMessage::Transcript { .. }) => {
                RealtimeMessageClass::Transcript
            }
            Self::Outgoing(_) | Self::Close => RealtimeMessageClass::Critical,
        }
    }
}

/// Send a realtime route under its delivery policy.
///
/// This mirrors the voice WebSocket `send_with_policy` behavior for the
/// realtime endpoint's route type: audio is shed on full queues, transcripts
/// wait a bounded window, and critical messages apply backpressure.
pub async fn send_realtime_with_policy(
    tx: &tokio::sync::mpsc::Sender<RealtimeMessageRoute>,
    route: RealtimeMessageRoute,
) {
    use tokio::sync::mpsc::error::{SendTimeoutError, TrySendError};

    match route.class() {
        RealtimeMessageClass::DroppableAudio => match tx.try_send(route) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                metrics::counter!(
                    crate::handlers::ws::messages::WS_DROPPED_FRAMES_TOTAL,
                    "class" => "realtime_audio"
                )
                .increment(1);
                tracing::debug!("Realtime WS egress channel full; dropped droppable audio frame");
            }
            Err(TrySendError::Closed(_)) => {
                tracing::debug!("Realtime WS egress channel closed; dropped audio frame");
            }
        },
        RealtimeMessageClass::Transcript => {
            match tx
                .send_timeout(
                    route,
                    crate::handlers::ws::messages::TRANSCRIPT_SEND_TIMEOUT,
                )
                .await
            {
                Ok(()) => {}
                Err(SendTimeoutError::Timeout(_)) => {
                    metrics::counter!(
                        crate::handlers::ws::messages::WS_DROPPED_FRAMES_TOTAL,
                        "class" => "realtime_transcript"
                    )
                    .increment(1);
                    tracing::warn!(
                        timeout_ms = crate::handlers::ws::messages::TRANSCRIPT_SEND_TIMEOUT
                            .as_millis() as u64,
                        "Realtime WS egress channel full beyond transcript timeout; dropped transcript"
                    );
                }
                Err(SendTimeoutError::Closed(_)) => {
                    tracing::debug!("Realtime WS egress channel closed; dropped transcript");
                }
            }
        }
        RealtimeMessageClass::Critical => {
            if tx.send(route).await.is_err() {
                tracing::warn!("Realtime WS egress channel closed; critical message not delivered");
            }
        }
    }
}

// =============================================================================
// Validation
// =============================================================================

/// Error type for message validation failures
#[derive(Debug, Clone)]
pub enum RealtimeValidationError {
    /// Instructions exceed maximum allowed size
    InstructionsTooLarge { size: usize, max: usize },
    /// Text content exceeds maximum allowed size
    TextTooLarge { size: usize, max: usize },
    /// Function result exceeds maximum allowed size
    FunctionResultTooLarge { size: usize, max: usize },
    /// Invalid provider
    InvalidProvider { provider: String },
}

impl std::fmt::Display for RealtimeValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InstructionsTooLarge { size, max } => {
                write!(
                    f,
                    "Instructions too large: {} bytes (max: {} bytes)",
                    size, max
                )
            }
            Self::TextTooLarge { size, max } => {
                write!(f, "Text too large: {} bytes (max: {} bytes)", size, max)
            }
            Self::FunctionResultTooLarge { size, max } => {
                write!(
                    f,
                    "Function result too large: {} bytes (max: {} bytes)",
                    size, max
                )
            }
            Self::InvalidProvider { provider } => {
                write!(f, "Invalid provider: {}", provider)
            }
        }
    }
}

impl std::error::Error for RealtimeValidationError {}

impl RealtimeIncomingMessage {
    /// Validates message field sizes to prevent resource exhaustion attacks.
    pub fn validate_size(&self) -> Result<(), RealtimeValidationError> {
        match self {
            RealtimeIncomingMessage::Config(config)
            | RealtimeIncomingMessage::UpdateSession(config) => {
                if let Some(instructions) = &config.instructions {
                    let size = instructions.len();
                    if size > MAX_INSTRUCTIONS_SIZE {
                        return Err(RealtimeValidationError::InstructionsTooLarge {
                            size,
                            max: MAX_INSTRUCTIONS_SIZE,
                        });
                    }
                }
            }
            RealtimeIncomingMessage::Text { text } => {
                let size = text.len();
                if size > MAX_TEXT_SIZE {
                    return Err(RealtimeValidationError::TextTooLarge {
                        size,
                        max: MAX_TEXT_SIZE,
                    });
                }
            }
            RealtimeIncomingMessage::FunctionResult { result, .. } => {
                let size = result.len();
                if size > MAX_FUNCTION_RESULT_SIZE {
                    return Err(RealtimeValidationError::FunctionResultTooLarge {
                        size,
                        max: MAX_FUNCTION_RESULT_SIZE,
                    });
                }
            }
            // Other messages don't have user-provided content that needs size limits
            RealtimeIncomingMessage::CreateResponse { .. }
            | RealtimeIncomingMessage::CancelResponse
            | RealtimeIncomingMessage::CommitAudio
            | RealtimeIncomingMessage::ClearAudio => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_message_deserialization() {
        let json = r#"{
            "type": "config",
            "provider": "openai",
            "model": "gpt-4o-realtime-preview",
            "voice": "alloy",
            "instructions": "You are a helpful assistant."
        }"#;

        let msg: RealtimeIncomingMessage = serde_json::from_str(json).expect("Should deserialize");
        match msg {
            RealtimeIncomingMessage::Config(config) => {
                assert_eq!(config.provider.as_deref(), Some("openai"));
                assert_eq!(config.model.as_deref(), Some("gpt-4o-realtime-preview"));
                assert_eq!(config.voice.as_deref(), Some("alloy"));
            }
            _ => panic!("Expected Config variant"),
        }
    }

    #[test]
    fn test_text_message_deserialization() {
        let json = r#"{"type": "text", "text": "Hello, world!"}"#;
        let msg: RealtimeIncomingMessage = serde_json::from_str(json).expect("Should deserialize");
        match msg {
            RealtimeIncomingMessage::Text { text } => {
                assert_eq!(text, "Hello, world!");
            }
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_function_result_deserialization() {
        let json = r#"{"type": "function_result", "call_id": "call_123", "result": "{\"weather\": \"sunny\"}"}"#;
        let msg: RealtimeIncomingMessage = serde_json::from_str(json).expect("Should deserialize");
        match msg {
            RealtimeIncomingMessage::FunctionResult { call_id, result } => {
                assert_eq!(call_id, "call_123");
                assert!(result.contains("sunny"));
            }
            _ => panic!("Expected FunctionResult variant"),
        }
    }

    #[test]
    fn test_session_created_serialization() {
        let msg = RealtimeOutgoingMessage::SessionCreated {
            session_id: "sess_123".to_string(),
            provider: "openai".to_string(),
            model: "gpt-4o-realtime-preview".to_string(),
        };

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains(r#""type":"session_created""#));
        assert!(json.contains(r#""session_id":"sess_123""#));
    }

    #[test]
    fn test_transcript_serialization() {
        let msg = RealtimeOutgoingMessage::Transcript {
            text: "Hello".to_string(),
            role: "user".to_string(),
            is_final: true,
        };

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains(r#""type":"transcript""#));
        assert!(json.contains(r#""text":"Hello""#));
        assert!(json.contains(r#""is_final":true"#));
    }

    #[test]
    fn test_error_serialization() {
        let msg = RealtimeOutgoingMessage::Error {
            code: Some("invalid_config".to_string()),
            message: "Provider not supported".to_string(),
        };

        let json = serde_json::to_string(&msg).expect("Should serialize");
        assert!(json.contains(r#""type":"error""#));
        assert!(json.contains(r#""code":"invalid_config""#));
    }

    #[test]
    fn test_validation_instructions_within_limit() {
        let config = RealtimeSessionConfig {
            instructions: Some("a".repeat(MAX_INSTRUCTIONS_SIZE)),
            ..Default::default()
        };
        let msg = RealtimeIncomingMessage::Config(config);
        assert!(msg.validate_size().is_ok());
    }

    #[test]
    fn test_validation_instructions_exceeds_limit() {
        let config = RealtimeSessionConfig {
            instructions: Some("a".repeat(MAX_INSTRUCTIONS_SIZE + 1)),
            ..Default::default()
        };
        let msg = RealtimeIncomingMessage::Config(config);
        let err = msg.validate_size().unwrap_err();
        match err {
            RealtimeValidationError::InstructionsTooLarge { .. } => {}
            _ => panic!("Expected InstructionsTooLarge error"),
        }
    }

    #[test]
    fn test_validation_text_exceeds_limit() {
        let msg = RealtimeIncomingMessage::Text {
            text: "a".repeat(MAX_TEXT_SIZE + 1),
        };
        let err = msg.validate_size().unwrap_err();
        match err {
            RealtimeValidationError::TextTooLarge { .. } => {}
            _ => panic!("Expected TextTooLarge error"),
        }
    }

    #[test]
    fn test_turn_detection_config_deserialization() {
        let json = r#"{
            "type": "config",
            "turn_detection": {
                "mode": "server_vad",
                "threshold": 0.5,
                "silence_duration_ms": 500
            }
        }"#;

        let msg: RealtimeIncomingMessage = serde_json::from_str(json).expect("Should deserialize");
        match msg {
            RealtimeIncomingMessage::Config(config) => {
                let td = config.turn_detection.expect("Should have turn_detection");
                match td {
                    TurnDetectionConfig::ServerVad {
                        threshold,
                        silence_duration_ms,
                        ..
                    } => {
                        assert_eq!(threshold, Some(0.5));
                        assert_eq!(silence_duration_ms, Some(500));
                    }
                    _ => panic!("Expected ServerVad variant"),
                }
            }
            _ => panic!("Expected Config variant"),
        }
    }

    use tokio::sync::mpsc;

    fn full_realtime_channel() -> (
        mpsc::Sender<RealtimeMessageRoute>,
        mpsc::Receiver<RealtimeMessageRoute>,
    ) {
        let (tx, rx) = mpsc::channel::<RealtimeMessageRoute>(1);
        tx.try_send(RealtimeMessageRoute::Audio(Bytes::from_static(b"prefill")))
            .expect("capacity-1 channel accepts the first message");
        (tx, rx)
    }

    #[tokio::test(start_paused = true)]
    async fn p5_realtime_send_policy_drops_audio_on_full_channel() {
        let (tx, mut rx) = full_realtime_channel();

        tokio::time::timeout(
            std::time::Duration::from_millis(1),
            send_realtime_with_policy(
                &tx,
                RealtimeMessageRoute::Audio(Bytes::from_static(b"dropped")),
            ),
        )
        .await
        .expect("droppable realtime audio must not block on a full channel");

        match rx.try_recv().expect("prefill frame present") {
            RealtimeMessageRoute::Audio(b) => assert_eq!(b.as_ref(), b"prefill"),
            _ => panic!("expected prefill audio frame"),
        }
        assert!(
            rx.try_recv().is_err(),
            "dropped realtime audio must not be delivered"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn p5_realtime_send_policy_transcript_times_out_on_full_channel() {
        let (tx, mut rx) = full_realtime_channel();

        let start = tokio::time::Instant::now();
        send_realtime_with_policy(
            &tx,
            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Transcript {
                text: "hello".to_string(),
                role: "assistant".to_string(),
                is_final: true,
            }),
        )
        .await;

        assert!(
            start.elapsed() >= crate::handlers::ws::messages::TRANSCRIPT_SEND_TIMEOUT,
            "realtime transcript send must wait the bounded window before dropping"
        );
        match rx.try_recv().expect("prefill frame present") {
            RealtimeMessageRoute::Audio(b) => assert_eq!(b.as_ref(), b"prefill"),
            _ => panic!("expected prefill audio frame"),
        }
        assert!(
            rx.try_recv().is_err(),
            "timed-out realtime transcript must not be delivered"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn p5_realtime_send_policy_critical_waits_for_capacity() {
        let (tx, mut rx) = full_realtime_channel();

        let reader = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let first = rx.recv().await;
            (rx, first)
        });

        send_realtime_with_policy(
            &tx,
            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                code: Some("critical".to_string()),
                message: "deliver me".to_string(),
            }),
        )
        .await;

        let (mut rx, first) = reader.await.expect("reader task");
        assert!(matches!(first, Some(RealtimeMessageRoute::Audio(_))));
        match rx.recv().await.expect("critical message delivered") {
            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error { message, .. }) => {
                assert_eq!(message, "deliver me");
            }
            _ => panic!("expected critical error message"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn p5_realtime_send_policy_survives_closed_channel() {
        let (tx, rx) = mpsc::channel::<RealtimeMessageRoute>(1);
        drop(rx);

        send_realtime_with_policy(
            &tx,
            RealtimeMessageRoute::Audio(Bytes::from_static(b"audio")),
        )
        .await;
        send_realtime_with_policy(
            &tx,
            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Transcript {
                text: "gone".to_string(),
                role: "user".to_string(),
                is_final: false,
            }),
        )
        .await;
        send_realtime_with_policy(&tx, RealtimeMessageRoute::Close).await;
    }
}
