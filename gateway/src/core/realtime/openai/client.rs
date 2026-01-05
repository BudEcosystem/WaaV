//! OpenAI Realtime API client implementation.
//!
//! This module provides the OpenAI Realtime client that implements the `BaseRealtime` trait
//! using OpenAI's WebSocket-based Realtime API.
//!
//! # API Reference
//!
//! - Endpoint: `wss://api.openai.com/v1/realtime?model=<model>`
//! - Protocol: WebSocket with JSON events
//! - Audio: PCM 16-bit, 24kHz, mono, little-endian, base64 encoded
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::realtime::{BaseRealtime, RealtimeConfig, OpenAIRealtime};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = RealtimeConfig {
//!         api_key: "sk-...".to_string(),
//!         model: "gpt-4o-realtime-preview".to_string(),
//!         voice: Some("alloy".to_string()),
//!         ..Default::default()
//!     };
//!
//!     let mut realtime = OpenAIRealtime::new(config).unwrap();
//!     realtime.connect().await.unwrap();
//!
//!     realtime.on_transcript(Arc::new(|t| Box::pin(async move {
//!         println!("{}: {}", t.role, t.text);
//!     }))).unwrap();
//!
//!     realtime.send_audio(audio_bytes).await.unwrap();
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use base64::prelude::*;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{self, Message};

use super::config::{
    OPENAI_REALTIME_URL, OpenAIRealtimeAudioFormat, OpenAIRealtimeModel, OpenAIRealtimeVoice,
};
use super::messages::{
    ClientEvent, ContentPart, ConversationItem, InputAudioTranscription, ResponseConfig,
    ServerEvent, SessionConfig, TurnDetection,
};
use crate::core::realtime::base::{
    AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback, FunctionCallRequest,
    RealtimeAudioData, RealtimeConfig, RealtimeError, RealtimeErrorCallback, RealtimeResult,
    ReconnectionCallback, ReconnectionConfig, ReconnectionEvent, ResponseDoneCallback, SpeechEvent,
    SpeechEventCallback, TranscriptCallback, TranscriptResult, TranscriptRole,
};

/// Channel capacity for WebSocket message sending.
const WS_CHANNEL_CAPACITY: usize = 256;

// =============================================================================
// OpenAI Realtime Client
// =============================================================================

/// OpenAI Realtime API client implementation.
///
/// This client provides bidirectional audio streaming with real-time
/// transcription and TTS using OpenAI's Realtime API.
///
/// # Thread Safety
///
/// This struct uses `Arc` wrappers for all mutable state to allow safe
/// sharing between the main struct and the spawned WebSocket task.
/// The `connected` flag uses `Arc<AtomicBool>` for lock-free status checks.
///
/// # Automatic Reconnection
///
/// The client supports automatic reconnection with exponential backoff when
/// the connection is lost. Configure via `ReconnectionConfig` in the `RealtimeConfig`.
/// Default behavior: up to 5 retry attempts with exponential backoff (1s, 2s, 4s, 8s, 16s).
pub struct OpenAIRealtime {
    /// Configuration
    config: RealtimeConfig,
    /// Parsed model
    model: OpenAIRealtimeModel,
    /// Parsed voice
    voice: OpenAIRealtimeVoice,
    /// Audio format
    audio_format: OpenAIRealtimeAudioFormat,
    /// Connection state
    state: Arc<RwLock<ConnectionState>>,
    /// Connected flag for fast checks (shared with connection task)
    /// Uses Arc to share state between main struct and spawned task
    connected: Arc<AtomicBool>,
    /// Session ID
    session_id: Arc<RwLock<Option<String>>>,

    /// WebSocket sender channel
    ws_sender: Arc<Mutex<Option<mpsc::Sender<ClientEvent>>>>,

    /// Callbacks
    transcript_callback: Arc<Mutex<Option<TranscriptCallback>>>,
    audio_callback: Arc<Mutex<Option<AudioOutputCallback>>>,
    error_callback: Arc<Mutex<Option<RealtimeErrorCallback>>>,
    function_call_callback: Arc<Mutex<Option<FunctionCallCallback>>>,
    speech_event_callback: Arc<Mutex<Option<SpeechEventCallback>>>,
    response_done_callback: Arc<Mutex<Option<ResponseDoneCallback>>>,

    /// Connection task handle
    connection_handle: Arc<Mutex<Option<JoinHandle<()>>>>,

    /// Accumulated transcript for assistant responses
    assistant_transcript: Arc<RwLock<String>>,

    /// Pending function calls: maps call_id -> function_name
    /// Populated by OutputItemAdded event, consumed by FunctionCallArgumentsDone event.
    /// This is necessary because FunctionCallArgumentsDone doesn't include the function name.
    pending_function_calls: Arc<RwLock<HashMap<String, String>>>,

    /// Reconnection configuration
    reconnection_config: ReconnectionConfig,

    /// Flag to indicate intentional disconnection (suppress reconnection)
    intentional_disconnect: Arc<AtomicBool>,

    /// Last session config sent to OpenAI (for restoration after reconnection)
    last_session_config: Arc<RwLock<Option<SessionConfig>>>,

    /// Reconnection event callback
    reconnection_callback: Arc<Mutex<Option<ReconnectionCallback>>>,
}

impl OpenAIRealtime {
    /// Get the configured model.
    pub fn model(&self) -> OpenAIRealtimeModel {
        self.model
    }

    /// Get the configured voice.
    pub fn voice(&self) -> OpenAIRealtimeVoice {
        self.voice
    }

    /// Get the configured audio format.
    pub fn audio_format(&self) -> OpenAIRealtimeAudioFormat {
        self.audio_format
    }

    /// Get the session ID if connected.
    pub async fn session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// Build the WebSocket URL with model parameter.
    fn build_ws_url(&self) -> String {
        format!("{}?model={}", OPENAI_REALTIME_URL, self.model.as_str())
    }

    /// Build the initial session configuration.
    fn build_session_config(&self) -> SessionConfig {
        SessionConfig {
            modalities: Some(vec!["text".to_string(), "audio".to_string()]),
            voice: Some(self.voice.as_str().to_string()),
            instructions: self.config.instructions.clone(),
            input_audio_format: Some(self.audio_format.as_str().to_string()),
            output_audio_format: Some(self.audio_format.as_str().to_string()),
            input_audio_transcription: self.config.input_audio_transcription.as_ref().map(|t| {
                InputAudioTranscription {
                    model: t.model.clone(),
                }
            }),
            turn_detection: self.config.turn_detection.as_ref().map(|td| match td {
                crate::core::realtime::base::TurnDetectionConfig::ServerVad {
                    threshold,
                    prefix_padding_ms,
                    silence_duration_ms,
                    create_response,
                    interrupt_response,
                } => TurnDetection::ServerVad {
                    threshold: *threshold,
                    prefix_padding_ms: *prefix_padding_ms,
                    silence_duration_ms: *silence_duration_ms,
                    create_response: *create_response,
                    interrupt_response: *interrupt_response,
                },
                crate::core::realtime::base::TurnDetectionConfig::SemanticVad {
                    eagerness,
                    create_response,
                    interrupt_response,
                } => TurnDetection::SemanticVad {
                    eagerness: eagerness.clone(),
                    create_response: *create_response,
                    interrupt_response: *interrupt_response,
                },
                crate::core::realtime::base::TurnDetectionConfig::None => TurnDetection::None {},
            }),
            tools: self.config.tools.as_ref().map(|tools| {
                tools
                    .iter()
                    .map(|t| super::messages::ToolDef {
                        tool_type: t.tool_type.clone(),
                        name: t.function.name.clone(),
                        description: t.function.description.clone(),
                        parameters: t.function.parameters.clone(),
                    })
                    .collect()
            }),
            tool_choice: self.config.tool_choice.clone(),
            temperature: self.config.temperature,
            max_response_output_tokens: self.config.max_response_output_tokens.map(|t| {
                if t < 0 {
                    super::messages::MaxTokens::Infinite("inf".to_string())
                } else {
                    super::messages::MaxTokens::Number(t)
                }
            }),
        }
    }

    /// Handle a server event.
    ///
    /// This method processes incoming WebSocket events from the OpenAI Realtime API
    /// and dispatches them to the appropriate callbacks.
    ///
    /// # Arguments
    ///
    /// * `event` - The server event to handle
    /// * `transcript_cb` - Callback for transcript events
    /// * `audio_cb` - Callback for audio output events
    /// * `error_cb` - Callback for error events
    /// * `function_call_cb` - Callback for function call events
    /// * `speech_event_cb` - Callback for speech detection events
    /// * `response_done_cb` - Callback for response completion events
    /// * `session_id` - Shared session ID storage
    /// * `assistant_transcript` - Accumulated assistant transcript buffer
    /// * `pending_function_calls` - Map of call_id -> function_name for tracking function calls
    #[allow(clippy::too_many_arguments)]
    async fn handle_server_event(
        event: ServerEvent,
        transcript_cb: &Arc<Mutex<Option<TranscriptCallback>>>,
        audio_cb: &Arc<Mutex<Option<AudioOutputCallback>>>,
        error_cb: &Arc<Mutex<Option<RealtimeErrorCallback>>>,
        function_call_cb: &Arc<Mutex<Option<FunctionCallCallback>>>,
        speech_event_cb: &Arc<Mutex<Option<SpeechEventCallback>>>,
        response_done_cb: &Arc<Mutex<Option<ResponseDoneCallback>>>,
        session_id: &Arc<RwLock<Option<String>>>,
        assistant_transcript: &Arc<RwLock<String>>,
        pending_function_calls: &Arc<RwLock<HashMap<String, String>>>,
    ) {
        match event {
            ServerEvent::SessionCreated { session } => {
                tracing::info!("OpenAI Realtime session created: {}", session.id);
                *session_id.write().await = Some(session.id);
            }

            ServerEvent::SessionUpdated { session } => {
                tracing::debug!("OpenAI Realtime session updated: {}", session.id);
            }

            ServerEvent::Error { error } => {
                tracing::error!(
                    "OpenAI Realtime error: {} - {}",
                    error.error_type,
                    error.message
                );
                if let Some(cb) = error_cb.lock().await.as_ref() {
                    let err = RealtimeError::ProviderError(format!(
                        "{}: {}",
                        error.error_type, error.message
                    ));
                    cb(err).await;
                }
            }

            ServerEvent::SpeechStarted {
                audio_start_ms,
                item_id,
            } => {
                tracing::debug!("Speech started at {}ms", audio_start_ms);
                if let Some(cb) = speech_event_cb.lock().await.as_ref() {
                    cb(SpeechEvent::Started {
                        audio_start_ms,
                        item_id: Some(item_id),
                    })
                    .await;
                }
            }

            ServerEvent::SpeechStopped {
                audio_end_ms,
                item_id,
            } => {
                tracing::debug!("Speech stopped at {}ms", audio_end_ms);
                if let Some(cb) = speech_event_cb.lock().await.as_ref() {
                    cb(SpeechEvent::Stopped {
                        audio_end_ms,
                        item_id: Some(item_id),
                    })
                    .await;
                }
            }

            ServerEvent::TranscriptionCompleted {
                item_id,
                transcript,
                ..
            } => {
                tracing::debug!("User transcript: {}", transcript);
                if let Some(cb) = transcript_cb.lock().await.as_ref() {
                    cb(TranscriptResult {
                        text: transcript,
                        role: TranscriptRole::User,
                        is_final: true,
                        item_id: Some(item_id),
                    })
                    .await;
                }
            }

            ServerEvent::AudioTranscriptDelta { delta, .. } => {
                // Accumulate assistant transcript
                assistant_transcript.write().await.push_str(&delta);

                // Send partial transcript
                if let Some(cb) = transcript_cb.lock().await.as_ref() {
                    let current = assistant_transcript.read().await.clone();
                    cb(TranscriptResult {
                        text: current,
                        role: TranscriptRole::Assistant,
                        is_final: false,
                        item_id: None,
                    })
                    .await;
                }
            }

            ServerEvent::AudioTranscriptDone {
                transcript,
                item_id,
                ..
            } => {
                tracing::debug!("Assistant transcript: {}", transcript);
                // Clear accumulated transcript
                *assistant_transcript.write().await = String::new();

                if let Some(cb) = transcript_cb.lock().await.as_ref() {
                    cb(TranscriptResult {
                        text: transcript,
                        role: TranscriptRole::Assistant,
                        is_final: true,
                        item_id: Some(item_id),
                    })
                    .await;
                }
            }

            ServerEvent::AudioDelta {
                delta,
                item_id,
                response_id,
                ..
            } => {
                // Decode base64 audio and forward to callback
                if let Some(cb) = audio_cb.lock().await.as_ref() {
                    match BASE64_STANDARD.decode(&delta) {
                        Ok(audio_bytes) => {
                            cb(RealtimeAudioData {
                                data: Bytes::from(audio_bytes),
                                sample_rate: 24000,
                                item_id: Some(item_id),
                                response_id: Some(response_id),
                            })
                            .await;
                        }
                        Err(e) => {
                            tracing::error!("Failed to decode audio delta: {}", e);
                        }
                    }
                }
            }

            // Track function calls when output items are added
            // This captures the function name before FunctionCallArgumentsDone is received
            ServerEvent::OutputItemAdded { item, .. } => {
                // Check if this is a function_call item with valid call_id and name
                if item.item_type == "function_call"
                    && let (Some(call_id), Some(name)) = (&item.call_id, &item.name)
                {
                    tracing::debug!("Tracking function call: call_id={}, name={}", call_id, name);
                    pending_function_calls
                        .write()
                        .await
                        .insert(call_id.clone(), name.clone());
                }
            }

            ServerEvent::FunctionCallArgumentsDone {
                call_id,
                arguments,
                item_id,
                ..
            } => {
                // Retrieve the function name from our tracking map
                let name = pending_function_calls
                    .write()
                    .await
                    .remove(&call_id)
                    .unwrap_or_else(|| {
                        tracing::warn!(
                            "Function name not found for call_id: {}. This may indicate a protocol issue.",
                            call_id
                        );
                        String::new()
                    });

                tracing::debug!(
                    "Function call complete: name={}, call_id={}, args={}",
                    name,
                    call_id,
                    arguments
                );

                if let Some(cb) = function_call_cb.lock().await.as_ref() {
                    cb(FunctionCallRequest {
                        call_id,
                        name,
                        arguments,
                        item_id: Some(item_id),
                    })
                    .await;
                }
            }

            ServerEvent::ResponseDone { response } => {
                tracing::debug!("Response done: {}", response.id);
                if let Some(cb) = response_done_cb.lock().await.as_ref() {
                    cb(response.id).await;
                }
            }

            // Handle other events as needed
            _ => {
                tracing::trace!("Unhandled server event");
            }
        }
    }
}

#[async_trait]
impl BaseRealtime for OpenAIRealtime {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self> {
        // Validate API key
        if config.api_key.is_empty() {
            return Err(RealtimeError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }

        // Parse model
        let model = if config.model.is_empty() {
            OpenAIRealtimeModel::default()
        } else {
            OpenAIRealtimeModel::from_str_or_default(&config.model)
        };

        // Parse voice
        let voice = if let Some(ref v) = config.voice {
            OpenAIRealtimeVoice::from_str_or_default(v)
        } else {
            OpenAIRealtimeVoice::default()
        };

        // Parse audio format
        let audio_format = if let Some(ref f) = config.input_audio_format {
            OpenAIRealtimeAudioFormat::from_str_or_default(f)
        } else {
            OpenAIRealtimeAudioFormat::default()
        };

        // Get reconnection config or use default
        let reconnection_config = config.reconnection.clone().unwrap_or_default();

        Ok(Self {
            config,
            model,
            voice,
            audio_format,
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            connected: Arc::new(AtomicBool::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            ws_sender: Arc::new(Mutex::new(None)),
            transcript_callback: Arc::new(Mutex::new(None)),
            audio_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            function_call_callback: Arc::new(Mutex::new(None)),
            speech_event_callback: Arc::new(Mutex::new(None)),
            response_done_callback: Arc::new(Mutex::new(None)),
            connection_handle: Arc::new(Mutex::new(None)),
            assistant_transcript: Arc::new(RwLock::new(String::new())),
            pending_function_calls: Arc::new(RwLock::new(HashMap::new())),
            reconnection_config,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            last_session_config: Arc::new(RwLock::new(None)),
            reconnection_callback: Arc::new(Mutex::new(None)),
        })
    }

    async fn connect(&mut self) -> RealtimeResult<()> {
        // Check if already connected
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        // Reset intentional disconnect flag
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        // Update state
        *self.state.write().await = ConnectionState::Connecting;

        // Build WebSocket URL
        let url = self.build_ws_url();

        // Build request with headers
        let request = http::Request::builder()
            .uri(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("OpenAI-Beta", "realtime=v1")
            .header("Sec-WebSocket-Protocol", "realtime")
            .header(
                "Sec-WebSocket-Key",
                tungstenite::handshake::client::generate_key(),
            )
            .header("Sec-WebSocket-Version", "13")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Host", "api.openai.com")
            .body(())
            .map_err(|e| RealtimeError::ConnectionFailed(e.to_string()))?;

        // Connect WebSocket
        let (ws_stream, _response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| RealtimeError::ConnectionFailed(e.to_string()))?;

        tracing::info!("Connected to OpenAI Realtime API");

        // Split the WebSocket
        let (ws_sink, ws_stream) = ws_stream.split();

        // Create channel for sending messages
        let (tx, mut rx) = mpsc::channel::<ClientEvent>(WS_CHANNEL_CAPACITY);
        *self.ws_sender.lock().await = Some(tx);

        // Clone references for the connection task
        let transcript_cb = self.transcript_callback.clone();
        let audio_cb = self.audio_callback.clone();
        let error_cb = self.error_callback.clone();
        let function_call_cb = self.function_call_callback.clone();
        let speech_event_cb = self.speech_event_callback.clone();
        let response_done_cb = self.response_done_callback.clone();
        let session_id = self.session_id.clone();
        let state = self.state.clone();
        let ws_sender = self.ws_sender.clone();
        // Use the struct's connected flag so state is shared with the spawned task
        let connected = self.connected.clone();
        let assistant_transcript = self.assistant_transcript.clone();
        let pending_function_calls = self.pending_function_calls.clone();

        // Clone reconnection-related state
        let reconnection_config = self.reconnection_config.clone();
        let intentional_disconnect = self.intentional_disconnect.clone();
        let api_key = self.config.api_key.clone();
        let ws_url = url.clone();
        let last_session_config = self.last_session_config.clone();
        let reconnection_callback = self.reconnection_callback.clone();

        // Mark as connected before spawning task
        self.connected.store(true, Ordering::SeqCst);
        *self.state.write().await = ConnectionState::Connected;

        // Spawn connection task with reconnection support
        let handle = tokio::spawn(async move {
            let mut current_ws_sink = ws_sink;
            let mut current_ws_stream = ws_stream;
            let mut reconnect_attempt: u32 = 0;

            'outer: loop {
                // Main message processing loop
                loop {
                    tokio::select! {
                        // Handle outgoing messages
                        Some(event) = rx.recv() => {
                            let json = match serde_json::to_string(&event) {
                                Ok(j) => j,
                                Err(e) => {
                                    tracing::error!("Failed to serialize event: {}", e);
                                    continue;
                                }
                            };

                            if let Err(e) = current_ws_sink.send(Message::Text(json.into())).await {
                                tracing::error!("Failed to send WebSocket message: {}", e);
                                break;
                            }
                        }

                        // Handle incoming messages
                        Some(msg) = current_ws_stream.next() => {
                            match msg {
                                Ok(Message::Text(text)) => {
                                    // Reset reconnect counter on successful message
                                    reconnect_attempt = 0;

                                    match serde_json::from_str::<ServerEvent>(&text) {
                                        Ok(event) => {
                                            Self::handle_server_event(
                                                event,
                                                &transcript_cb,
                                                &audio_cb,
                                                &error_cb,
                                                &function_call_cb,
                                                &speech_event_cb,
                                                &response_done_cb,
                                                &session_id,
                                                &assistant_transcript,
                                                &pending_function_calls,
                                            ).await;
                                        }
                                        Err(e) => {
                                            tracing::warn!("Failed to parse server event: {} - {}", e, text);
                                        }
                                    }
                                }
                                Ok(Message::Close(_)) => {
                                    tracing::info!("WebSocket closed by server");
                                    break;
                                }
                                Ok(Message::Ping(data)) => {
                                    if let Err(e) = current_ws_sink.send(Message::Pong(data)).await {
                                        tracing::error!("Failed to send pong: {}", e);
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("WebSocket error: {}", e);
                                    break;
                                }
                                _ => {}
                            }
                        }

                        else => break,
                    }
                }

                // Connection ended - check if we should reconnect
                connected.store(false, Ordering::SeqCst);

                // Check if disconnect was intentional
                if intentional_disconnect.load(Ordering::SeqCst) {
                    tracing::info!("Intentional disconnect, not attempting reconnection");
                    *state.write().await = ConnectionState::Disconnected;
                    break 'outer;
                }

                // Check if reconnection is enabled and we have attempts left
                if !reconnection_config.should_retry(reconnect_attempt) {
                    tracing::warn!(
                        "Reconnection disabled or max attempts ({}) reached",
                        reconnection_config.max_attempts
                    );

                    // Notify error callback
                    if let Some(cb) = error_cb.lock().await.as_ref() {
                        let err = RealtimeError::ConnectionFailed(format!(
                            "Connection lost after {} reconnection attempts",
                            reconnect_attempt
                        ));
                        cb(err).await;
                    }

                    *state.write().await = ConnectionState::Failed;
                    break 'outer;
                }

                // Increment attempt counter
                reconnect_attempt += 1;

                // Update state to reconnecting
                *state.write().await = ConnectionState::Reconnecting;

                // Calculate delay with exponential backoff
                let delay_ms = reconnection_config.calculate_delay(reconnect_attempt);
                tracing::info!(
                    "Attempting reconnection {}/{} in {}ms",
                    reconnect_attempt,
                    if reconnection_config.max_attempts == 0 {
                        "∞".to_string()
                    } else {
                        reconnection_config.max_attempts.to_string()
                    },
                    delay_ms
                );

                // Wait before reconnecting
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;

                // Check again if disconnect was requested during sleep
                if intentional_disconnect.load(Ordering::SeqCst) {
                    tracing::info!("Disconnect requested during reconnection delay");
                    *state.write().await = ConnectionState::Disconnected;
                    break 'outer;
                }

                // Attempt to reconnect
                let request = match http::Request::builder()
                    .uri(&ws_url)
                    .header("Authorization", format!("Bearer {}", api_key))
                    .header("OpenAI-Beta", "realtime=v1")
                    .header("Sec-WebSocket-Protocol", "realtime")
                    .header(
                        "Sec-WebSocket-Key",
                        tungstenite::handshake::client::generate_key(),
                    )
                    .header("Sec-WebSocket-Version", "13")
                    .header("Connection", "Upgrade")
                    .header("Upgrade", "websocket")
                    .header("Host", "api.openai.com")
                    .body(())
                {
                    Ok(req) => req,
                    Err(e) => {
                        tracing::error!("Failed to build reconnection request: {}", e);
                        continue;
                    }
                };

                match tokio_tungstenite::connect_async(request).await {
                    Ok((new_ws_stream, _)) => {
                        tracing::info!("Reconnected to OpenAI Realtime API");

                        let (new_sink, new_stream) = new_ws_stream.split();
                        current_ws_sink = new_sink;
                        current_ws_stream = new_stream;

                        // Update state
                        connected.store(true, Ordering::SeqCst);
                        *state.write().await = ConnectionState::Connected;

                        // Clear old session ID (new session will be created)
                        *session_id.write().await = None;

                        // Clear pending function calls to prevent memory leak
                        pending_function_calls.write().await.clear();
                        tracing::debug!("Cleared pending function calls after reconnection");

                        // Restore session configuration if we have a previous one
                        if let Some(saved_config) = last_session_config.read().await.clone() {
                            tracing::info!("Restoring session configuration after reconnection");
                            let event = ClientEvent::SessionUpdate {
                                session: saved_config,
                            };
                            if let Ok(json) = serde_json::to_string(&event) {
                                if let Err(e) =
                                    current_ws_sink.send(Message::Text(json.into())).await
                                {
                                    tracing::error!(
                                        "Failed to restore session config after reconnection: {}",
                                        e
                                    );
                                } else {
                                    tracing::info!("Session configuration restored successfully");
                                }
                            }
                        }

                        // Invoke reconnection callback
                        if let Some(cb) = reconnection_callback.lock().await.as_ref() {
                            cb(ReconnectionEvent {
                                attempt: reconnect_attempt,
                                success: true,
                                error: None,
                            })
                            .await;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Reconnection attempt {} failed: {}", reconnect_attempt, e);
                        // Continue to next iteration which will retry or give up
                        continue;
                    }
                }
            }

            // Final cleanup - clear sender
            *ws_sender.lock().await = None;
            tracing::info!("OpenAI Realtime connection task ended");
        });

        *self.connection_handle.lock().await = Some(handle);

        // Send initial session update
        let session_config = self.build_session_config();
        self.send_session_update(session_config).await?;

        Ok(())
    }

    async fn disconnect(&mut self) -> RealtimeResult<()> {
        // Set intentional disconnect flag to suppress reconnection
        self.intentional_disconnect.store(true, Ordering::SeqCst);

        // Clear sender to stop the connection loop
        *self.ws_sender.lock().await = None;

        // Abort the connection task
        if let Some(handle) = self.connection_handle.lock().await.take() {
            handle.abort();
        }

        // Update state
        self.connected.store(false, Ordering::SeqCst);
        *self.state.write().await = ConnectionState::Disconnected;
        *self.session_id.write().await = None;

        tracing::info!("Disconnected from OpenAI Realtime API");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn get_connection_state(&self) -> ConnectionState {
        // Use cached value for performance
        if self.connected.load(Ordering::SeqCst) {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> RealtimeResult<()> {
        if !self.is_ready() {
            return Err(RealtimeError::NotConnected);
        }

        let event = ClientEvent::audio_append(&audio_data);
        self.send_event(event).await
    }

    async fn send_text(&mut self, text: &str) -> RealtimeResult<()> {
        if !self.is_ready() {
            return Err(RealtimeError::NotConnected);
        }

        // Create a conversation item with the text
        let event = ClientEvent::ConversationItemCreate {
            item: ConversationItem {
                id: None,
                item_type: "message".to_string(),
                status: None,
                role: Some("user".to_string()),
                content: Some(vec![ContentPart {
                    content_type: "input_text".to_string(),
                    text: Some(text.to_string()),
                    audio: None,
                    transcript: None,
                }]),
                call_id: None,
                name: None,
                arguments: None,
                output: None,
            },
            previous_item_id: None,
        };

        self.send_event(event).await
    }

    async fn create_response(&mut self) -> RealtimeResult<()> {
        if !self.is_ready() {
            return Err(RealtimeError::NotConnected);
        }

        let event = ClientEvent::ResponseCreate {
            response: Some(ResponseConfig::default()),
        };
        self.send_event(event).await
    }

    async fn cancel_response(&mut self) -> RealtimeResult<()> {
        if !self.is_ready() {
            return Err(RealtimeError::NotConnected);
        }

        self.send_event(ClientEvent::ResponseCancel).await
    }

    async fn commit_audio_buffer(&mut self) -> RealtimeResult<()> {
        if !self.is_ready() {
            return Err(RealtimeError::NotConnected);
        }

        self.send_event(ClientEvent::InputAudioBufferCommit).await
    }

    async fn clear_audio_buffer(&mut self) -> RealtimeResult<()> {
        if !self.is_ready() {
            return Err(RealtimeError::NotConnected);
        }

        self.send_event(ClientEvent::InputAudioBufferClear).await
    }

    fn on_transcript(&mut self, callback: TranscriptCallback) -> RealtimeResult<()> {
        // Use try_lock to avoid blocking, fall back to spawn if lock is held
        // This ensures the callback is registered synchronously if possible,
        // avoiding race conditions where messages arrive before callback is set
        if let Ok(mut guard) = self.transcript_callback.try_lock() {
            *guard = Some(callback);
        } else {
            // Lock is held (unlikely in normal usage), spawn to avoid deadlock
            let cb = self.transcript_callback.clone();
            tokio::spawn(async move {
                *cb.lock().await = Some(callback);
            });
        }
        Ok(())
    }

    fn on_audio(&mut self, callback: AudioOutputCallback) -> RealtimeResult<()> {
        if let Ok(mut guard) = self.audio_callback.try_lock() {
            *guard = Some(callback);
        } else {
            let cb = self.audio_callback.clone();
            tokio::spawn(async move {
                *cb.lock().await = Some(callback);
            });
        }
        Ok(())
    }

    fn on_error(&mut self, callback: RealtimeErrorCallback) -> RealtimeResult<()> {
        if let Ok(mut guard) = self.error_callback.try_lock() {
            *guard = Some(callback);
        } else {
            let cb = self.error_callback.clone();
            tokio::spawn(async move {
                *cb.lock().await = Some(callback);
            });
        }
        Ok(())
    }

    fn on_function_call(&mut self, callback: FunctionCallCallback) -> RealtimeResult<()> {
        if let Ok(mut guard) = self.function_call_callback.try_lock() {
            *guard = Some(callback);
        } else {
            let cb = self.function_call_callback.clone();
            tokio::spawn(async move {
                *cb.lock().await = Some(callback);
            });
        }
        Ok(())
    }

    fn on_speech_event(&mut self, callback: SpeechEventCallback) -> RealtimeResult<()> {
        if let Ok(mut guard) = self.speech_event_callback.try_lock() {
            *guard = Some(callback);
        } else {
            let cb = self.speech_event_callback.clone();
            tokio::spawn(async move {
                *cb.lock().await = Some(callback);
            });
        }
        Ok(())
    }

    fn on_response_done(&mut self, callback: ResponseDoneCallback) -> RealtimeResult<()> {
        if let Ok(mut guard) = self.response_done_callback.try_lock() {
            *guard = Some(callback);
        } else {
            let cb = self.response_done_callback.clone();
            tokio::spawn(async move {
                *cb.lock().await = Some(callback);
            });
        }
        Ok(())
    }

    fn on_reconnection(&mut self, callback: ReconnectionCallback) -> RealtimeResult<()> {
        if let Ok(mut guard) = self.reconnection_callback.try_lock() {
            *guard = Some(callback);
        } else {
            let cb = self.reconnection_callback.clone();
            tokio::spawn(async move {
                *cb.lock().await = Some(callback);
            });
        }
        Ok(())
    }

    async fn update_session(&mut self, config: RealtimeConfig) -> RealtimeResult<()> {
        if !self.is_ready() {
            return Err(RealtimeError::NotConnected);
        }

        // Preserve existing API key if new config has empty key
        // This allows session updates without re-providing the API key
        let api_key = if config.api_key.is_empty() {
            std::mem::take(&mut self.config.api_key)
        } else {
            config.api_key.clone()
        };

        // Update internal config with preserved API key
        self.config = RealtimeConfig { api_key, ..config };

        // Update parsed voice if changed
        if let Some(ref v) = self.config.voice {
            self.voice = super::config::OpenAIRealtimeVoice::from_str_or_default(v);
        }

        // Rebuild and send session config
        let session_config = self.build_session_config();
        self.send_session_update(session_config).await
    }

    async fn submit_function_result(&mut self, call_id: &str, result: &str) -> RealtimeResult<()> {
        if !self.is_ready() {
            return Err(RealtimeError::NotConnected);
        }

        // Create a function call output item
        let event = ClientEvent::ConversationItemCreate {
            item: ConversationItem {
                id: None,
                item_type: "function_call_output".to_string(),
                status: None,
                role: None,
                content: None,
                call_id: Some(call_id.to_string()),
                name: None,
                arguments: None,
                output: Some(result.to_string()),
            },
            previous_item_id: None,
        };

        self.send_event(event).await
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "openai",
            "api_type": "WebSocket Realtime",
            "version": "1.0.0",
            "endpoint": OPENAI_REALTIME_URL,
            "supported_models": [
                "gpt-4o-realtime-preview",
                "gpt-4o-realtime-preview-2024-10-01",
                "gpt-4o-realtime-preview-2024-12-17",
                "gpt-4o-mini-realtime-preview",
                "gpt-4o-mini-realtime-preview-2024-12-17"
            ],
            "supported_voices": [
                "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"
            ],
            "supported_audio_formats": [
                "pcm16", "g711_ulaw", "g711_alaw"
            ],
            "default_sample_rate": 24000,
            "features": {
                "bidirectional_audio": true,
                "vad": true,
                "function_calling": true,
                "text_and_audio": true,
                "transcription": true
            },
            "documentation": "https://platform.openai.com/docs/guides/realtime"
        })
    }
}

impl OpenAIRealtime {
    /// Send an event to the WebSocket.
    async fn send_event(&self, event: ClientEvent) -> RealtimeResult<()> {
        if let Some(sender) = self.ws_sender.lock().await.as_ref() {
            sender
                .send(event)
                .await
                .map_err(|e| RealtimeError::WebSocketError(e.to_string()))?;
            Ok(())
        } else {
            Err(RealtimeError::NotConnected)
        }
    }

    /// Send a session update event and save the config for reconnection.
    async fn send_session_update(&self, session: SessionConfig) -> RealtimeResult<()> {
        // Save the session config for restoration after reconnection
        *self.last_session_config.write().await = Some(session.clone());
        tracing::debug!("Saved session configuration for potential reconnection");

        let event = ClientEvent::SessionUpdate { session };
        self.send_event(event).await
    }
}

impl Default for OpenAIRealtime {
    fn default() -> Self {
        Self::new(RealtimeConfig::default()).unwrap_or_else(|_| {
            // Create with empty config - will fail on connect if no API key
            Self {
                config: RealtimeConfig::default(),
                model: OpenAIRealtimeModel::default(),
                voice: OpenAIRealtimeVoice::default(),
                audio_format: OpenAIRealtimeAudioFormat::default(),
                state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
                connected: Arc::new(AtomicBool::new(false)),
                session_id: Arc::new(RwLock::new(None)),
                ws_sender: Arc::new(Mutex::new(None)),
                transcript_callback: Arc::new(Mutex::new(None)),
                audio_callback: Arc::new(Mutex::new(None)),
                error_callback: Arc::new(Mutex::new(None)),
                function_call_callback: Arc::new(Mutex::new(None)),
                speech_event_callback: Arc::new(Mutex::new(None)),
                response_done_callback: Arc::new(Mutex::new(None)),
                connection_handle: Arc::new(Mutex::new(None)),
                assistant_transcript: Arc::new(RwLock::new(String::new())),
                pending_function_calls: Arc::new(RwLock::new(HashMap::new())),
                reconnection_config: ReconnectionConfig::default(),
                intentional_disconnect: Arc::new(AtomicBool::new(false)),
                last_session_config: Arc::new(RwLock::new(None)),
                reconnection_callback: Arc::new(Mutex::new(None)),
            }
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_openai_realtime_creation() {
        let config = RealtimeConfig {
            api_key: "test_key".to_string(),
            model: "gpt-4o-realtime-preview".to_string(),
            voice: Some("shimmer".to_string()),
            ..Default::default()
        };

        let realtime = OpenAIRealtime::new(config).unwrap();
        assert!(!realtime.is_ready());
        assert_eq!(
            realtime.get_connection_state(),
            ConnectionState::Disconnected
        );
        assert_eq!(realtime.model(), OpenAIRealtimeModel::Gpt4oRealtimePreview);
        assert_eq!(realtime.voice(), OpenAIRealtimeVoice::Shimmer);
    }

    #[tokio::test]
    async fn test_api_key_required() {
        let config = RealtimeConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = OpenAIRealtime::new(config);
        assert!(result.is_err());
        match result {
            Err(RealtimeError::AuthenticationFailed(_)) => {}
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_send_audio_requires_connection() {
        let config = RealtimeConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        let mut realtime = OpenAIRealtime::new(config).unwrap();
        let result = realtime.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_err());
        match result {
            Err(RealtimeError::NotConnected) => {}
            _ => panic!("Expected NotConnected error"),
        }
    }

    #[test]
    fn test_provider_info() {
        let realtime = OpenAIRealtime::default();
        let info = realtime.get_provider_info();

        assert_eq!(info["provider"], "openai");
        assert_eq!(info["api_type"], "WebSocket Realtime");
        assert!(info["features"]["bidirectional_audio"].as_bool().unwrap());
        assert!(info["features"]["vad"].as_bool().unwrap());
    }

    #[test]
    fn test_build_ws_url() {
        let config = RealtimeConfig {
            api_key: "test".to_string(),
            model: "gpt-4o-realtime-preview".to_string(),
            ..Default::default()
        };

        let realtime = OpenAIRealtime::new(config).unwrap();
        let url = realtime.build_ws_url();
        assert!(url.contains("wss://api.openai.com"));
        assert!(url.contains("gpt-4o-realtime-preview"));
    }

    #[test]
    fn test_default_reconnection_config() {
        let config = RealtimeConfig {
            api_key: "test".to_string(),
            ..Default::default()
        };

        let realtime = OpenAIRealtime::new(config).unwrap();

        // Default reconnection should be enabled
        assert!(realtime.reconnection_config.enabled);
        assert_eq!(realtime.reconnection_config.max_attempts, 5);
    }

    #[test]
    fn test_custom_reconnection_config() {
        let config = RealtimeConfig {
            api_key: "test".to_string(),
            reconnection: Some(ReconnectionConfig {
                enabled: true,
                max_attempts: 10,
                initial_delay_ms: 500,
                max_delay_ms: 60000,
                backoff_multiplier: 1.5,
                jitter: false,
            }),
            ..Default::default()
        };

        let realtime = OpenAIRealtime::new(config).unwrap();

        assert!(realtime.reconnection_config.enabled);
        assert_eq!(realtime.reconnection_config.max_attempts, 10);
        assert_eq!(realtime.reconnection_config.initial_delay_ms, 500);
        assert_eq!(realtime.reconnection_config.max_delay_ms, 60000);
        assert_eq!(realtime.reconnection_config.backoff_multiplier, 1.5);
        assert!(!realtime.reconnection_config.jitter);
    }

    #[test]
    fn test_reconnection_disabled() {
        let config = RealtimeConfig {
            api_key: "test".to_string(),
            reconnection: Some(ReconnectionConfig::disabled()),
            ..Default::default()
        };

        let realtime = OpenAIRealtime::new(config).unwrap();

        assert!(!realtime.reconnection_config.enabled);
        assert!(!realtime.reconnection_config.should_retry(0));
    }
}
