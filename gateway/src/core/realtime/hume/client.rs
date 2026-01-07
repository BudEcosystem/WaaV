//! Hume EVI WebSocket client implementation.
//!
//! This module implements the `BaseRealtime` trait for Hume's Empathic Voice
//! Interface (EVI), providing full-duplex audio streaming with emotional
//! intelligence.
//!
//! # Features
//!
//! - Real-time bidirectional audio streaming
//! - 48-dimension prosody (emotion) analysis
//! - Empathic response generation
//! - Function calling support
//! - Automatic reconnection
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::realtime::hume::{HumeEVI, HumeEVIConfig};
//! use waav_gateway::core::realtime::BaseRealtime;
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = HumeEVIConfig::new("your-api-key")
//!         .with_config_id("your-config-id");
//!
//!     let mut evi = HumeEVI::new(config).unwrap();
//!     evi.connect().await.unwrap();
//!
//!     // Register callbacks
//!     evi.on_transcript(Arc::new(|t| Box::pin(async move {
//!         println!("[{}] {}", t.role, t.text);
//!     }))).unwrap();
//!
//!     // Send audio
//!     evi.send_audio(audio_bytes).await.unwrap();
//! }
//! ```

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{RwLock, mpsc};
use tokio::time::timeout;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tracing::{debug, error, info, trace, warn};

use super::config::HumeEVIConfig;
use super::messages::{
    AudioInput, AudioSettings, EVIClientMessage, EVIServerMessage, HUME_EVI_DEFAULT_SAMPLE_RATE,
    SessionSettings, StopAssistant, TextInput, ToolResponse, deserialize_server_message,
    serialize_client_message,
};
use crate::core::realtime::base::{
    AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback, FunctionCallRequest,
    RealtimeAudioData, RealtimeConfig, RealtimeError, RealtimeErrorCallback, RealtimeResult,
    ReconnectionCallback, ResponseDoneCallback, SpeechEvent, SpeechEventCallback,
    TranscriptCallback, TranscriptResult, TranscriptRole,
};

// =============================================================================
// HumeEVI Client
// =============================================================================

/// Hume EVI (Empathic Voice Interface) realtime client.
///
/// This client implements full-duplex audio streaming with Hume's EVI,
/// which provides emotional intelligence through prosody analysis.
pub struct HumeEVI {
    /// Configuration for this EVI session.
    config: HumeEVIConfig,

    /// Current connection state.
    state: Arc<RwLock<ConnectionState>>,

    /// WebSocket sender for outgoing messages.
    ws_sender: Option<mpsc::UnboundedSender<EVIClientMessage>>,

    /// Chat metadata from connection.
    chat_metadata: Arc<RwLock<Option<ChatMetadataInfo>>>,

    /// Current response ID being generated.
    current_response_id: Arc<RwLock<Option<String>>>,

    // Callbacks
    transcript_callback: Option<TranscriptCallback>,
    audio_callback: Option<AudioOutputCallback>,
    error_callback: Option<RealtimeErrorCallback>,
    function_call_callback: Option<FunctionCallCallback>,
    speech_event_callback: Option<SpeechEventCallback>,
    response_done_callback: Option<ResponseDoneCallback>,
    reconnection_callback: Option<ReconnectionCallback>,

    /// Handle to the message processing task.
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Chat metadata information.
#[derive(Debug, Clone)]
struct ChatMetadataInfo {
    chat_id: String,
    chat_group_id: String,
}

impl HumeEVI {
    /// Create a new HumeEVI client from HumeEVIConfig.
    pub fn from_hume_config(config: HumeEVIConfig) -> RealtimeResult<Self> {
        config
            .validate()
            .map_err(RealtimeError::InvalidConfiguration)?;

        Ok(Self {
            config,
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            ws_sender: None,
            chat_metadata: Arc::new(RwLock::new(None)),
            current_response_id: Arc::new(RwLock::new(None)),
            transcript_callback: None,
            audio_callback: None,
            error_callback: None,
            function_call_callback: None,
            speech_event_callback: None,
            response_done_callback: None,
            reconnection_callback: None,
            task_handle: None,
        })
    }

    /// Get the chat ID for the current session.
    pub async fn get_chat_id(&self) -> Option<String> {
        self.chat_metadata
            .read()
            .await
            .as_ref()
            .map(|m| m.chat_id.clone())
    }

    /// Get the chat group ID for resuming conversations.
    pub async fn get_chat_group_id(&self) -> Option<String> {
        self.chat_metadata
            .read()
            .await
            .as_ref()
            .map(|m| m.chat_group_id.clone())
    }

    /// Connect to Hume EVI WebSocket.
    async fn connect_internal(&mut self) -> RealtimeResult<()> {
        // Build WebSocket URL with query parameters
        let url = self.config.build_websocket_url();
        debug!(
            "Connecting to Hume EVI: {}",
            url.split('?').next().unwrap_or(&url)
        );

        // Update state to connecting
        *self.state.write().await = ConnectionState::Connecting;

        // Connect with timeout
        let connect_timeout = Duration::from_secs(self.config.connection_timeout_seconds);
        let connect_result = timeout(connect_timeout, connect_async(&url)).await;

        let (ws_stream, response) = match connect_result {
            Ok(Ok((stream, response))) => (stream, response),
            Ok(Err(e)) => {
                *self.state.write().await = ConnectionState::Failed;
                return Err(RealtimeError::ConnectionFailed(format!(
                    "WebSocket connection failed: {e}"
                )));
            }
            Err(_) => {
                *self.state.write().await = ConnectionState::Failed;
                return Err(RealtimeError::Timeout("Connection timed out".to_string()));
            }
        };

        info!("Connected to Hume EVI (status: {})", response.status());

        // Split the WebSocket stream
        let (ws_write, ws_read) = ws_stream.split();

        // Create channel for outgoing messages
        let (tx, rx) = mpsc::unbounded_channel();
        self.ws_sender = Some(tx);

        // Clone state and callbacks for the processing task
        let state = self.state.clone();
        let chat_metadata = self.chat_metadata.clone();
        let current_response_id = self.current_response_id.clone();
        let transcript_cb = self.transcript_callback.clone();
        let audio_cb = self.audio_callback.clone();
        let error_cb = self.error_callback.clone();
        let function_call_cb = self.function_call_callback.clone();
        let speech_event_cb = self.speech_event_callback.clone();
        let response_done_cb = self.response_done_callback.clone();

        // Spawn message processing task
        let handle = tokio::spawn(async move {
            Self::message_loop(
                ws_write,
                ws_read,
                rx,
                state,
                chat_metadata,
                current_response_id,
                transcript_cb,
                audio_cb,
                error_cb,
                function_call_cb,
                speech_event_cb,
                response_done_cb,
            )
            .await;
        });

        self.task_handle = Some(handle);

        // Send session settings if needed
        if self.config.input_encoding != super::messages::AudioEncoding::default()
            || self.config.sample_rate != super::messages::HUME_EVI_DEFAULT_SAMPLE_RATE
            || self.config.system_prompt.is_some()
        {
            self.send_session_settings().await?;
        }

        // Update state to connected
        *self.state.write().await = ConnectionState::Connected;

        Ok(())
    }

    /// Send session settings message.
    async fn send_session_settings(&self) -> RealtimeResult<()> {
        let settings = SessionSettings {
            audio: Some(AudioSettings {
                encoding: self.config.input_encoding,
                sample_rate: Some(self.config.sample_rate),
                channels: Some(self.config.channels),
            }),
            system_prompt: self.config.system_prompt.clone(),
            context: None,
        };

        self.send_message(EVIClientMessage::SessionSettings(settings))
            .await
    }

    /// Send a client message through the WebSocket.
    async fn send_message(&self, msg: EVIClientMessage) -> RealtimeResult<()> {
        let sender = self.ws_sender.as_ref().ok_or(RealtimeError::NotConnected)?;

        sender
            .send(msg)
            .map_err(|e| RealtimeError::WebSocketError(format!("Failed to queue message: {e}")))?;

        Ok(())
    }

    /// Main message processing loop.
    async fn message_loop(
        mut ws_write: futures_util::stream::SplitSink<
            WebSocketStream<MaybeTlsStream<TcpStream>>,
            Message,
        >,
        mut ws_read: futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        mut rx: mpsc::UnboundedReceiver<EVIClientMessage>,
        state: Arc<RwLock<ConnectionState>>,
        chat_metadata: Arc<RwLock<Option<ChatMetadataInfo>>>,
        current_response_id: Arc<RwLock<Option<String>>>,
        transcript_cb: Option<TranscriptCallback>,
        audio_cb: Option<AudioOutputCallback>,
        error_cb: Option<RealtimeErrorCallback>,
        function_call_cb: Option<FunctionCallCallback>,
        speech_event_cb: Option<SpeechEventCallback>,
        response_done_cb: Option<ResponseDoneCallback>,
    ) {
        loop {
            tokio::select! {
                // Handle outgoing messages
                Some(msg) = rx.recv() => {
                    match serialize_client_message(&msg) {
                        Ok(json) => {
                            trace!("Sending EVI message: {}", json.chars().take(100).collect::<String>());
                            if let Err(e) = ws_write.send(Message::Text(json.into())).await {
                                error!("Failed to send WebSocket message: {e}");
                                break;
                            }
                        }
                        Err(e) => {
                            error!("Failed to serialize message: {e}");
                        }
                    }
                }

                // Handle incoming messages
                Some(result) = ws_read.next() => {
                    match result {
                        Ok(Message::Text(text)) => {
                            trace!("Received EVI message: {}", text.chars().take(100).collect::<String>());
                            Self::handle_server_message(
                                &text,
                                &chat_metadata,
                                &current_response_id,
                                &transcript_cb,
                                &audio_cb,
                                &error_cb,
                                &function_call_cb,
                                &speech_event_cb,
                                &response_done_cb,
                            ).await;
                        }
                        Ok(Message::Binary(data)) => {
                            // Binary messages are typically audio output
                            debug!("Received binary message: {} bytes", data.len());
                            if let Some(ref cb) = audio_cb {
                                let audio_data = RealtimeAudioData {
                                    data: Bytes::from(data.to_vec()),
                                    sample_rate: HUME_EVI_DEFAULT_SAMPLE_RATE,
                                    item_id: None,
                                    response_id: current_response_id.read().await.clone(),
                                };
                                cb(audio_data).await;
                            }
                        }
                        Ok(Message::Close(frame)) => {
                            info!("WebSocket closed: {:?}", frame);
                            break;
                        }
                        Ok(Message::Ping(data)) => {
                            let _ = ws_write.send(Message::Pong(data)).await;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            error!("WebSocket error: {e}");
                            break;
                        }
                    }
                }

                else => break,
            }
        }

        // Connection closed
        *state.write().await = ConnectionState::Disconnected;
        info!("Hume EVI message loop ended");
    }

    /// Handle a server message.
    async fn handle_server_message(
        text: &str,
        chat_metadata: &Arc<RwLock<Option<ChatMetadataInfo>>>,
        current_response_id: &Arc<RwLock<Option<String>>>,
        transcript_cb: &Option<TranscriptCallback>,
        audio_cb: &Option<AudioOutputCallback>,
        error_cb: &Option<RealtimeErrorCallback>,
        function_call_cb: &Option<FunctionCallCallback>,
        speech_event_cb: &Option<SpeechEventCallback>,
        response_done_cb: &Option<ResponseDoneCallback>,
    ) {
        let msg = match deserialize_server_message(text) {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to deserialize EVI message: {e}");
                return;
            }
        };

        match msg {
            EVIServerMessage::ChatMetadata(meta) => {
                info!(
                    "EVI chat metadata: chat_id={}, chat_group_id={}",
                    meta.chat_id, meta.chat_group_id
                );
                *chat_metadata.write().await = Some(ChatMetadataInfo {
                    chat_id: meta.chat_id,
                    chat_group_id: meta.chat_group_id,
                });
            }

            EVIServerMessage::UserMessage(user_msg) => {
                debug!(
                    "User message: {} (interim: {:?})",
                    user_msg.message.content, user_msg.interim
                );

                if let Some(cb) = transcript_cb {
                    let transcript = TranscriptResult {
                        text: user_msg.message.content,
                        role: TranscriptRole::User,
                        is_final: user_msg.interim != Some(true),
                        item_id: Some(user_msg.id),
                    };
                    cb(transcript).await;
                }

                // If prosody data is available, could emit as speech event
                if let Some(models) = user_msg.models {
                    if let Some(prosody) = models.prosody {
                        if let Some(dominant) = prosody.scores.dominant_emotion() {
                            trace!("User emotion: {} ({:.2})", dominant.0, dominant.1);
                        }
                    }
                }
            }

            EVIServerMessage::UserInterruption(interruption) => {
                debug!("User interruption at {:?}ms", interruption.time);
                if let Some(cb) = speech_event_cb {
                    cb(SpeechEvent::Started {
                        audio_start_ms: interruption.time.unwrap_or(0),
                        item_id: None,
                    })
                    .await;
                }
            }

            EVIServerMessage::AssistantMessage(asst_msg) => {
                debug!("Assistant message: {}", asst_msg.message.content);

                // Store current response ID
                *current_response_id.write().await = Some(asst_msg.id.clone());

                if let Some(cb) = transcript_cb {
                    let transcript = TranscriptResult {
                        text: asst_msg.message.content,
                        role: TranscriptRole::Assistant,
                        is_final: true,
                        item_id: Some(asst_msg.id),
                    };
                    cb(transcript).await;
                }
            }

            EVIServerMessage::AssistantProsody(prosody) => {
                trace!("Assistant prosody for message {}", prosody.id);
                // Prosody data for assistant's voice
                if let Some(p) = prosody.models.prosody {
                    if let Some(dominant) = p.scores.dominant_emotion() {
                        trace!("Assistant emotion: {} ({:.2})", dominant.0, dominant.1);
                    }
                }
            }

            EVIServerMessage::AudioOutput(output) => {
                if let Some(cb) = audio_cb {
                    match output.decode_audio() {
                        Ok(audio_bytes) => {
                            let audio_data = RealtimeAudioData {
                                data: Bytes::from(audio_bytes),
                                sample_rate: HUME_EVI_DEFAULT_SAMPLE_RATE,
                                item_id: Some(output.id),
                                response_id: current_response_id.read().await.clone(),
                            };
                            cb(audio_data).await;
                        }
                        Err(e) => {
                            warn!("Failed to decode audio output: {e}");
                        }
                    }
                }
            }

            EVIServerMessage::AssistantEnd(end) => {
                debug!("Assistant response ended: {:?}", end.id);

                if let Some(cb) = response_done_cb {
                    cb(end.id.unwrap_or_default()).await;
                }

                // Clear current response ID
                *current_response_id.write().await = None;
            }

            EVIServerMessage::ToolCall(call) => {
                debug!("Tool call: {} ({})", call.name, call.tool_call_id);

                if let Some(cb) = function_call_cb {
                    let request = FunctionCallRequest {
                        call_id: call.tool_call_id,
                        name: call.name,
                        arguments: call.parameters,
                        item_id: call.id,
                    };
                    cb(request).await;
                }
            }

            EVIServerMessage::ToolError(err) => {
                warn!(
                    "Tool error for {}: {} ({:?})",
                    err.tool_call_id, err.error, err.code
                );
                if let Some(cb) = error_cb {
                    cb(RealtimeError::ProviderError(format!(
                        "Tool error: {}",
                        err.error
                    )))
                    .await;
                }
            }

            EVIServerMessage::Error(err) => {
                error!("EVI error: {} - {}", err.code, err.message);
                if let Some(cb) = error_cb {
                    cb(RealtimeError::ProviderError(format!(
                        "{}: {}",
                        err.code, err.message
                    )))
                    .await;
                }
            }

            EVIServerMessage::WebSocketError(err) => {
                error!("WebSocket error: {:?} - {:?}", err.code, err.reason);
                if let Some(cb) = error_cb {
                    cb(RealtimeError::WebSocketError(
                        err.reason.unwrap_or_else(|| "Unknown error".to_string()),
                    ))
                    .await;
                }
            }

            EVIServerMessage::Unknown => {
                trace!("Unknown message type received");
            }
        }
    }
}

// =============================================================================
// BaseRealtime Implementation
// =============================================================================

#[async_trait]
impl BaseRealtime for HumeEVI {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self>
    where
        Self: Sized,
    {
        // Convert RealtimeConfig to HumeEVIConfig
        let hume_config = HumeEVIConfig {
            api_key: config.api_key,
            config_id: None,
            resumed_chat_group_id: None,
            evi_version: super::config::EVIVersion::V3,
            voice_id: config.voice,
            verbose_transcription: false,
            input_encoding: super::messages::AudioEncoding::Linear16,
            sample_rate: HUME_EVI_DEFAULT_SAMPLE_RATE,
            channels: 1,
            system_prompt: config.instructions,
            websocket_url: super::messages::HUME_EVI_WEBSOCKET_URL.to_string(),
            connection_timeout_seconds: 30,
            reconnection: config.reconnection,
        };

        Self::from_hume_config(hume_config)
    }

    async fn connect(&mut self) -> RealtimeResult<()> {
        self.connect_internal().await
    }

    async fn disconnect(&mut self) -> RealtimeResult<()> {
        // Close the WebSocket sender
        self.ws_sender.take();

        // Abort the message processing task
        if let Some(handle) = self.task_handle.take() {
            handle.abort();
        }

        *self.state.write().await = ConnectionState::Disconnected;
        info!("Disconnected from Hume EVI");

        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.ws_sender.is_some()
    }

    fn get_connection_state(&self) -> ConnectionState {
        // Can't await in sync function, so we use try_read
        self.state
            .try_read()
            .map(|s| *s)
            .unwrap_or(ConnectionState::Disconnected)
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> RealtimeResult<()> {
        let input = AudioInput::from_bytes(&audio_data);
        self.send_message(EVIClientMessage::AudioInput(input)).await
    }

    async fn send_text(&mut self, text: &str) -> RealtimeResult<()> {
        let input = TextInput {
            text: text.to_string(),
        };
        self.send_message(EVIClientMessage::TextInput(input)).await
    }

    async fn create_response(&mut self) -> RealtimeResult<()> {
        // EVI automatically generates responses based on turn detection
        // This is a no-op for EVI
        Ok(())
    }

    async fn cancel_response(&mut self) -> RealtimeResult<()> {
        self.send_message(EVIClientMessage::StopAssistant(StopAssistant::default()))
            .await
    }

    async fn commit_audio_buffer(&mut self) -> RealtimeResult<()> {
        // EVI doesn't have a separate commit concept - it uses turn detection
        Ok(())
    }

    async fn clear_audio_buffer(&mut self) -> RealtimeResult<()> {
        // EVI doesn't expose audio buffer clearing
        Ok(())
    }

    fn on_transcript(&mut self, callback: TranscriptCallback) -> RealtimeResult<()> {
        self.transcript_callback = Some(callback);
        Ok(())
    }

    fn on_audio(&mut self, callback: AudioOutputCallback) -> RealtimeResult<()> {
        self.audio_callback = Some(callback);
        Ok(())
    }

    fn on_error(&mut self, callback: RealtimeErrorCallback) -> RealtimeResult<()> {
        self.error_callback = Some(callback);
        Ok(())
    }

    fn on_function_call(&mut self, callback: FunctionCallCallback) -> RealtimeResult<()> {
        self.function_call_callback = Some(callback);
        Ok(())
    }

    fn on_speech_event(&mut self, callback: SpeechEventCallback) -> RealtimeResult<()> {
        self.speech_event_callback = Some(callback);
        Ok(())
    }

    fn on_response_done(&mut self, callback: ResponseDoneCallback) -> RealtimeResult<()> {
        self.response_done_callback = Some(callback);
        Ok(())
    }

    fn on_reconnection(&mut self, callback: ReconnectionCallback) -> RealtimeResult<()> {
        self.reconnection_callback = Some(callback);
        Ok(())
    }

    async fn update_session(&mut self, config: RealtimeConfig) -> RealtimeResult<()> {
        // Update system prompt if provided
        if let Some(instructions) = config.instructions {
            let settings = SessionSettings {
                audio: None,
                system_prompt: Some(instructions),
                context: None,
            };
            self.send_message(EVIClientMessage::SessionSettings(settings))
                .await?;
        }
        Ok(())
    }

    async fn submit_function_result(&mut self, call_id: &str, result: &str) -> RealtimeResult<()> {
        let response = ToolResponse {
            tool_call_id: call_id.to_string(),
            content: result.to_string(),
            tool_name: None,
        };
        self.send_message(EVIClientMessage::ToolResponse(response))
            .await
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "hume",
            "product": "evi",
            "evi_version": self.config.evi_version.as_str(),
            "sample_rate": self.config.sample_rate,
            "channels": self.config.channels,
            "encoding": format!("{:?}", self.config.input_encoding),
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hume_evi_creation() {
        let config = HumeEVIConfig::new("test-api-key");
        let evi = HumeEVI::from_hume_config(config);
        assert!(evi.is_ok());
    }

    #[test]
    fn test_hume_evi_creation_empty_key() {
        let config = HumeEVIConfig::default();
        let evi = HumeEVI::from_hume_config(config);
        assert!(evi.is_err());
    }

    #[test]
    fn test_hume_evi_from_realtime_config() {
        let config = RealtimeConfig {
            api_key: "test-key".to_string(),
            voice: Some("kora".to_string()),
            instructions: Some("Be helpful".to_string()),
            ..Default::default()
        };

        let evi = HumeEVI::new(config);
        assert!(evi.is_ok());

        let evi = evi.unwrap();
        assert_eq!(evi.config.api_key, "test-key");
        assert_eq!(evi.config.voice_id, Some("kora".to_string()));
        assert_eq!(evi.config.system_prompt, Some("Be helpful".to_string()));
    }

    #[test]
    fn test_hume_evi_initial_state() {
        let config = HumeEVIConfig::new("test-key");
        let evi = HumeEVI::from_hume_config(config).unwrap();

        assert_eq!(evi.get_connection_state(), ConnectionState::Disconnected);
        assert!(!evi.is_ready());
    }

    #[test]
    fn test_hume_evi_provider_info() {
        let config =
            HumeEVIConfig::new("test-key").with_version(super::super::config::EVIVersion::V4Mini);
        let evi = HumeEVI::from_hume_config(config).unwrap();

        let info = evi.get_provider_info();
        assert_eq!(info["provider"], "hume");
        assert_eq!(info["product"], "evi");
        assert_eq!(info["evi_version"], "4-mini");
    }

    #[tokio::test]
    async fn test_hume_evi_callbacks() {
        let config = HumeEVIConfig::new("test-key");
        let mut evi = HumeEVI::from_hume_config(config).unwrap();

        // Test setting callbacks (should not error)
        let result = evi.on_transcript(Arc::new(|_| Box::pin(async {})));
        assert!(result.is_ok());

        let result = evi.on_audio(Arc::new(|_| Box::pin(async {})));
        assert!(result.is_ok());

        let result = evi.on_error(Arc::new(|_| Box::pin(async {})));
        assert!(result.is_ok());

        let result = evi.on_function_call(Arc::new(|_| Box::pin(async {})));
        assert!(result.is_ok());

        let result = evi.on_speech_event(Arc::new(|_| Box::pin(async {})));
        assert!(result.is_ok());

        let result = evi.on_response_done(Arc::new(|_| Box::pin(async {})));
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_hume_evi_send_without_connection() {
        let config = HumeEVIConfig::new("test-key");
        let mut evi = HumeEVI::from_hume_config(config).unwrap();

        let result = evi.send_audio(Bytes::from_static(&[1, 2, 3])).await;
        assert!(matches!(result, Err(RealtimeError::NotConnected)));

        let result = evi.send_text("Hello").await;
        assert!(matches!(result, Err(RealtimeError::NotConnected)));
    }

    #[tokio::test]
    async fn test_hume_evi_noop_methods() {
        let config = HumeEVIConfig::new("test-key");
        let mut evi = HumeEVI::from_hume_config(config).unwrap();

        // These are no-ops for EVI
        let result = evi.create_response().await;
        assert!(result.is_ok());

        let result = evi.commit_audio_buffer().await;
        assert!(result.is_ok());

        let result = evi.clear_audio_buffer().await;
        assert!(result.is_ok());
    }
}
