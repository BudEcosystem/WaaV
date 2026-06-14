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

/// B-G2: cap on the local conversation log replayed on every reconnect
/// (review wf_d43814c3): a long session must not grow it unbounded. ~100
/// turns is far more context than any reconnect needs.
const MAX_REPLAY_LOG_ITEMS: usize = 100;

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
/// B-G2: playback state of the currently-streaming assistant item.
struct ItemPlayback {
    item_id: String,
    first_delta: std::time::Instant,
    duration_ms: u64,
}

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

    /// B-G2 playback tracking for truncate: the currently-playing assistant
    /// item (id, first-delta instant, audio duration received so far).
    playback: Arc<std::sync::Mutex<Option<ItemPlayback>>>,
    /// B-G2 rolling preroll of recently SENT user audio (re-appended after
    /// an input-buffer clear so the speech onset isn't lost). Reuses the
    /// D-G1 ring (push/evict-by-cap/snapshot).
    preroll: Arc<crate::core::websocket::AudioReplayBuffer>,
    /// B-G2 local conversation log (finalized user/assistant transcripts) —
    /// replayed as conversation items after a reconnect (no response
    /// requested): the socket is disposable, the context is the truth.
    conversation_log: Arc<RwLock<Vec<crate::core::realtime::ReplayConversationItem>>>,

    /// Shared, process-global resilience handles (W-D2 fleet adoption): the single reconnect
    /// governor + this provider's shared circuit breaker. Unlike the streaming STT providers, the
    /// realtime client already owns a mature bespoke reconnect loop (backoff + session restore +
    /// intentional-disconnect). Rather than rewrite that proven loop onto `ReconnectableStream`,
    /// we make it *participate* in the shared primitives: it consults the breaker before each
    /// reconnect dial (storm control + provider tripping) and records the outcome on it — and
    /// because the registry's breaker self-publishes `waav_circuit_breaker_state{provider="openai"}`
    /// on every transition, the realtime path now moves the gauge too. `None` (a direct
    /// construction) → the loop reconnects exactly as before.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl OpenAIRealtime {
    /// The shared circuit breaker this session feeds, if injected (for metrics/tests).
    pub fn resilience_breaker(
        &self,
    ) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }
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
        use super::messages::{AudioConfig, AudioFormat, AudioInput, AudioOutput};

        // GA audio format object (Beta sent a bare `"pcm16"` string).
        let ga_format = || match self.audio_format {
            OpenAIRealtimeAudioFormat::Pcm16 => AudioFormat {
                format_type: "audio/pcm".to_string(),
                rate: Some(24000),
            },
            OpenAIRealtimeAudioFormat::G711Ulaw => AudioFormat {
                format_type: "audio/pcmu".to_string(),
                rate: None,
            },
            OpenAIRealtimeAudioFormat::G711Alaw => AudioFormat {
                format_type: "audio/pcma".to_string(),
                rate: None,
            },
        };

        let turn_detection = self.config.turn_detection.as_ref().map(|td| match td {
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
        });

        // GA nests the Beta-era flat audio fields under audio.input / audio.output.
        let audio = AudioConfig {
            input: Some(AudioInput {
                format: Some(ga_format()),
                transcription: self.config.input_audio_transcription.as_ref().map(|t| {
                    InputAudioTranscription {
                        model: t.model.clone(),
                    }
                }),
                noise_reduction: super::messages::NoiseReduction::from_opt(
                    self.config.input_audio_noise_reduction.as_deref(),
                ),
                turn_detection,
            }),
            output: Some(AudioOutput {
                format: Some(ga_format()),
                voice: Some(self.voice.as_str().to_string()),
                speed: None,
            }),
        };

        // NOTE: GA `gpt-realtime` exposes no session-level `temperature` or
        // `reasoning` (confirmed against the live `session.created` schema), so
        // `self.config.{temperature,reasoning_effort}` are intentionally NOT
        // mapped here — sending either 400s the session. The cascade dial still
        // applies to the LLM path; S2S reasoning awaits a reasoning-capable
        // realtime model exposing the field.
        SessionConfig {
            session_type: "realtime".to_string(),
            output_modalities: Some(vec!["audio".to_string()]),
            instructions: self.config.instructions.clone(),
            audio: Some(audio),
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
            max_output_tokens: self.config.max_response_output_tokens.map(|t| {
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
        playback: &Arc<std::sync::Mutex<Option<ItemPlayback>>>,
        conversation_log: &Arc<RwLock<Vec<crate::core::realtime::ReplayConversationItem>>>,
        audio_format: OpenAIRealtimeAudioFormat,
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
                // B-G2: finalized user turn → the local conversation log
                // (replayed after a reconnect).
                conversation_log.write().await.push(
                    crate::core::realtime::ReplayConversationItem {
                        role: TranscriptRole::User,
                        text: transcript.clone(),
                    },
                );
                {
                    // Bound the replay log (review wf_d43814c3): keep the most
                    // recent turns so a long session cannot grow it unbounded
                    // (the full log is replayed on every reconnect).
                    let mut log = conversation_log.write().await;
                    let len = log.len();
                    if len > MAX_REPLAY_LOG_ITEMS {
                        log.drain(0..len - MAX_REPLAY_LOG_ITEMS);
                    }
                }
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

                // B-G2: finalized assistant turn → the local conversation
                // log (replayed after a reconnect).
                conversation_log.write().await.push(
                    crate::core::realtime::ReplayConversationItem {
                        role: TranscriptRole::Assistant,
                        text: transcript.clone(),
                    },
                );
                {
                    // Bound the replay log (review wf_d43814c3): keep the most
                    // recent turns so a long session cannot grow it unbounded
                    // (the full log is replayed on every reconnect).
                    let mut log = conversation_log.write().await;
                    let len = log.len();
                    if len > MAX_REPLAY_LOG_ITEMS {
                        log.drain(0..len - MAX_REPLAY_LOG_ITEMS);
                    }
                }

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

            // Text-modality output (GA `response.output_text.*`) — e.g. a
            // text-only per-response override. Deliver it as an assistant
            // transcript, mirroring the audio-transcript path.
            ServerEvent::TextDelta { delta, .. } => {
                assistant_transcript.write().await.push_str(&delta);
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

            ServerEvent::TextDone { text, item_id, .. } => {
                *assistant_transcript.write().await = String::new();
                conversation_log.write().await.push(
                    crate::core::realtime::ReplayConversationItem {
                        role: TranscriptRole::Assistant,
                        text: text.clone(),
                    },
                );
                {
                    let mut log = conversation_log.write().await;
                    let len = log.len();
                    if len > MAX_REPLAY_LOG_ITEMS {
                        log.drain(0..len - MAX_REPLAY_LOG_ITEMS);
                    }
                }
                if let Some(cb) = transcript_cb.lock().await.as_ref() {
                    cb(TranscriptResult {
                        text,
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
                            // B-G2 truncate bookkeeping: FORMAT-aware bytes/ms
                            // (PCM16 @24k = 48, G.711 @8k = 8 — review
                            // wf_d43814c3 #6). Same item extends; a new item
                            // restarts the playback estimate.
                            {
                                let chunk_ms =
                                    (audio_bytes.len() as u64) / audio_format.bytes_per_ms();
                                let mut pb = playback.lock().expect("playback lock");
                                match pb.as_mut() {
                                    Some(p) if p.item_id == item_id => {
                                        p.duration_ms += chunk_ms;
                                    }
                                    _ => {
                                        *pb = Some(ItemPlayback {
                                            item_id: item_id.clone(),
                                            first_delta: std::time::Instant::now(),
                                            duration_ms: chunk_ms,
                                        });
                                    }
                                }
                            }
                            cb(RealtimeAudioData {
                                data: Bytes::from(audio_bytes),
                                sample_rate: audio_format.sample_rate(),
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
                // B-G2 (review wc71hewlx #10): do NOT clear `playback` here.
                // response.done is GENERATION end — the client is still
                // PLAYING OUT the audio, so a barge-in during playout must
                // still truncate to what the user heard. Clearing here sent
                // NO truncate (OpenAI kept the full response while the user
                // heard only part). The estimate is instead cleared exactly
                // when WE truncate (truncate_current_response, preventing the
                // double-truncate of an already-truncated item), and a fully-
                // drained item truncates at min(elapsed, duration)=duration —
                // a harmless no-op. The next response's first audio delta
                // overwrites it with the new item.
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
            resilience: None,
            playback: Arc::new(std::sync::Mutex::new(None)),
            // ~660ms of 24k PCM16 — comfortably covers VAD onset latency.
            preroll: Arc::new(crate::core::websocket::AudioReplayBuffer::new(32_000)),
            conversation_log: Arc::new(RwLock::new(Vec::new())),
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
            // GA Realtime API (gpt-realtime): the `OpenAI-Beta: realtime=v1` header
            // is RETIRED. Sending it now makes OpenAI reject the session with
            // "The Realtime Beta API is no longer supported. Please use
            // /v1/realtime for the GA API." (live-caught). GA auth = bearer alone.
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
        let playback = self.playback.clone();
        let conversation_log = self.conversation_log.clone();
        let audio_format = self.audio_format; // Copy

        // Clone reconnection-related state
        let reconnection_config = self.reconnection_config.clone();
        let intentional_disconnect = self.intentional_disconnect.clone();
        let api_key = self.config.api_key.clone();
        let ws_url = url.clone();
        let last_session_config = self.last_session_config.clone();
        let reconnection_callback = self.reconnection_callback.clone();
        // Shared resilience handles (W-D2): the realtime loop consults the breaker before each
        // reconnect dial (storm control + provider tripping) and records the outcome. The registry
        // breaker self-publishes `waav_circuit_breaker_state{provider="openai"}` on transition.
        let (breaker, governor) = match &self.resilience {
            Some(r) => (
                Some(std::sync::Arc::clone(&r.breaker)),
                Some((*r.governor).clone()),
            ),
            None => (None, None),
        };

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
                                                &playback,
                                                &conversation_log,
                                                audio_format,
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

                // W-D2 storm control: consult the shared per-provider breaker before dialing. If a
                // wide OpenAI outage tripped it (from this or any other realtime session), do NOT
                // hammer it — skip this attempt and let the next backoff tick re-check. The breaker
                // self-publishes `waav_circuit_breaker_state{provider="openai"}` on transition.
                if let Some(b) = &breaker {
                    if !b.allow_request() {
                        tracing::warn!(
                            "OpenAI realtime breaker open; deferring reconnect attempt {}",
                            reconnect_attempt
                        );
                        crate::core::metrics::bridge::record_reconnect("openai", "circuit_open");
                        continue;
                    }
                }
                // Hold a governed slot across the dial so a fleet-wide outage can't make every
                // realtime session reconnect at once (shared process-global cap).
                let _permit = match &governor {
                    Some(g) => Some(g.acquire().await),
                    None => None,
                };

                // Attempt to reconnect
                let request = match http::Request::builder()
                    .uri(&ws_url)
                    .header("Authorization", format!("Bearer {}", api_key))
                    // GA Realtime: no `OpenAI-Beta` header (retired). See connect().
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
                        // Tell the shared breaker the dial succeeded (closes a half-open probe /
                        // tallies a healthy sample) and bump the reconnect counter.
                        if let Some(b) = &breaker {
                            b.record_success();
                        }
                        crate::core::metrics::bridge::record_reconnect("openai", "success");
                        drop(_permit);

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

                        // B-G2: rebuild server-side conversation state by
                        // replaying the LOCAL log as conversation items —
                        // WITHOUT a response.create (no duplicate inference;
                        // the context is the durable truth, the socket is
                        // disposable).
                        {
                            let log = conversation_log.read().await.clone();
                            if !log.is_empty() {
                                tracing::info!(
                                    items = log.len(),
                                    "Replaying conversation context after reconnection"
                                );
                                for item in &log {
                                    let event = ClientEvent::ConversationItemCreate {
                                        item: Self::replay_item_to_conversation_item(item),
                                        previous_item_id: None,
                                    };
                                    if let Ok(json) = serde_json::to_string(&event)
                                        && let Err(e) = current_ws_sink
                                            .send(Message::Text(json.into()))
                                            .await
                                    {
                                        tracing::error!(
                                            "Failed to replay conversation item: {e}"
                                        );
                                        break;
                                    }
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
                        // Record the failed dial on the shared breaker so a persistently-down
                        // OpenAI trips it for every realtime session (self-publishes the gauge).
                        if let Some(b) = &breaker {
                            b.record_failure();
                        }
                        crate::core::metrics::bridge::record_reconnect("openai", "failure");
                        drop(_permit);
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

        // B-G2: rolling preroll — an input-buffer clear (barge-in) wipes the
        // speech onset; this ring re-appends it.
        self.preroll.push(audio_data.clone());
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

    async fn create_response_with(
        &mut self,
        overrides: crate::core::realtime::base::RealtimeResponseOverride,
    ) -> RealtimeResult<()> {
        if !self.is_ready() {
            return Err(RealtimeError::NotConnected);
        }
        use super::messages::{AudioConfig, AudioOutput, MaxTokens};
        // Map the provider-agnostic override onto the GA `response.create`
        // `response` object (live-confirmed accepted shape). A per-response
        // voice nests under audio.output; `out_of_band` ⇒ conversation:"none".
        let audio = overrides.voice.as_ref().map(|v| AudioConfig {
            input: None,
            output: Some(AudioOutput {
                format: None,
                voice: Some(v.clone()),
                speed: None,
            }),
        });
        let response = ResponseConfig {
            output_modalities: overrides.modalities.clone(),
            instructions: overrides.instructions.clone(),
            audio,
            tools: None,
            tool_choice: None,
            max_output_tokens: overrides.max_output_tokens.map(|t| {
                if t < 0 {
                    MaxTokens::Infinite("inf".to_string())
                } else {
                    MaxTokens::Number(t)
                }
            }),
            conversation: overrides.out_of_band.then(|| "none".to_string()),
            metadata: overrides.metadata.clone(),
            input: None,
        };
        self.send_event(ClientEvent::ResponseCreate {
            response: Some(response),
        })
        .await
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

    // ── B-G2: S2S-as-a-service surface ──

    fn emits_user_turn_frames(&self) -> bool {
        // review wf_d43814c3 #7: OpenAI's `session.turn_detection` defaults to
        // server VAD ON, and WaaV OMITS the field when `config.turn_detection`
        // is None (`skip_serializing_if`), so the server STILL runs VAD and
        // produces turn frames. Server VAD is therefore on UNLESS the config
        // explicitly selects the `None` (manual) variant.
        !matches!(
            self.config.turn_detection,
            Some(crate::core::realtime::base::TurnDetectionConfig::None)
        )
    }

    async fn truncate_response(
        &mut self,
        item_id: &str,
        audio_end_ms: u64,
    ) -> RealtimeResult<()> {
        self.send_event(ClientEvent::ConversationItemTruncate {
            item_id: item_id.to_string(),
            content_index: 0,
            audio_end_ms: audio_end_ms as u32,
        })
        .await
    }

    async fn truncate_current_response(&mut self) -> RealtimeResult<Option<(String, u64)>> {
        let target = {
            let pb = self.playback.lock().expect("playback lock");
            pb.as_ref().map(|p| {
                let elapsed = p.first_delta.elapsed().as_millis() as u64;
                (
                    p.item_id.clone(),
                    crate::core::realtime::clamp_truncate_ms(elapsed, p.duration_ms),
                )
            })
        };
        match target {
            Some((item_id, end_ms)) => {
                self.truncate_response(&item_id, end_ms).await?;
                // The item is truncated: playback estimate is spent.
                *self.playback.lock().expect("playback lock") = None;
                Ok(Some((item_id, end_ms)))
            }
            None => Ok(None),
        }
    }

    async fn replay_user_audio_preroll(&mut self) -> RealtimeResult<()> {
        let tail = self.preroll.snapshot();
        if tail.is_empty() {
            return Ok(());
        }
        let bytes: usize = tail.iter().map(|c| c.len()).sum();
        tracing::debug!(chunks = tail.len(), bytes, "replaying user-audio preroll");
        for chunk in tail {
            self.send_event(ClientEvent::audio_append(&chunk)).await?;
        }
        Ok(())
    }

    async fn replay_conversation(
        &mut self,
        items: &[crate::core::realtime::ReplayConversationItem],
    ) -> RealtimeResult<()> {
        for item in items {
            self.send_event(ClientEvent::ConversationItemCreate {
                item: Self::replay_item_to_conversation_item(item),
                previous_item_id: None,
            })
            .await?;
        }
        Ok(())
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

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so the bespoke reconnect loop consults the
        // breaker before each dial (storm control + per-provider tripping) and records the
        // outcome — moving `waav_circuit_breaker_state{provider="openai"}` on transition (W-D2).
        self.resilience = Some(resilience);
    }
}

impl OpenAIRealtime {
    /// Send an event to the WebSocket.
    /// B-G2: map a replay-log item to the wire conversation item. Pure —
    /// pinned by tests (`input_text` for user, `text` for assistant; never
    /// a response request).
    fn replay_item_to_conversation_item(
        item: &crate::core::realtime::ReplayConversationItem,
    ) -> ConversationItem {
        let (role, content_type) = match item.role {
            TranscriptRole::User => ("user", "input_text"),
            TranscriptRole::Assistant => ("assistant", "text"),
        };
        ConversationItem {
            id: None,
            item_type: "message".to_string(),
            status: Some("completed".to_string()),
            role: Some(role.to_string()),
            content: Some(vec![ContentPart {
                content_type: content_type.to_string(),
                text: Some(item.text.clone()),
                audio: None,
                transcript: None,
            }]),
            call_id: None,
            name: None,
            arguments: None,
            output: None,
        }
    }

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
                resilience: None,
                playback: Arc::new(std::sync::Mutex::new(None)),
                preroll: Arc::new(crate::core::websocket::AudioReplayBuffer::new(32_000)),
                conversation_log: Arc::new(RwLock::new(Vec::new())),
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
    async fn emits_user_turn_frames_tracks_server_vad_default(){
        use crate::core::realtime::TurnDetectionConfig;
        let mk = |td: Option<TurnDetectionConfig>| {
            OpenAIRealtime::new(RealtimeConfig {
                provider: "openai".into(),
                api_key: "k".into(),
                model: "gpt-4o-realtime-preview".into(),
                turn_detection: td,
                ..Default::default()
            })
            .unwrap()
        };
        // Explicit server VAD → server produces turn frames.
        assert!(mk(Some(TurnDetectionConfig::default())).emits_user_turn_frames());
        // review wf_d43814c3 #7: OMITTED turn_detection is serialized away, so
        // OpenAI keeps its SERVER-VAD DEFAULT (on) — frames still come from the
        // server.
        assert!(
            mk(None).emits_user_turn_frames(),
            "omitted turn_detection ⇒ OpenAI server-VAD default is ON"
        );
        // ONLY the explicit manual variant flips it off.
        assert!(
            !mk(Some(TurnDetectionConfig::None)).emits_user_turn_frames(),
            "explicit None (manual) ⇒ the gateway drives turns"
        );
    }

    #[test]
    fn ga_build_session_config_shape_omits_temperature_and_reasoning() {
        use crate::core::llm::ReasoningEffort;
        // GA `gpt-realtime` has no session-level temperature/reasoning. Even with
        // both set on the config, build_session_config must emit the GA NESTED
        // shape and leak NEITHER field (either 400s session.update).
        let c = OpenAIRealtime::new(RealtimeConfig {
            provider: "openai".into(),
            api_key: "k".into(),
            model: "gpt-realtime".into(),
            temperature: Some(0.7),
            reasoning_effort: Some(ReasoningEffort::Low),
            ..Default::default()
        })
        .unwrap();
        let sc = c.build_session_config();
        assert_eq!(sc.session_type, "realtime", "GA requires session.type");
        assert_eq!(
            sc.output_modalities,
            Some(vec!["audio".to_string()]),
            "GA renamed modalities ⇒ output_modalities"
        );
        let audio = sc.audio.as_ref().expect("GA nests audio.input/output");
        assert!(
            audio.output.as_ref().unwrap().voice.is_some(),
            "voice nests under audio.output"
        );
        assert_eq!(
            audio.input.as_ref().unwrap().format.as_ref().unwrap().format_type,
            "audio/pcm",
            "PCM16 ⇒ {{type: audio/pcm, rate: 24000}}"
        );
        let json = serde_json::to_value(&sc).unwrap();
        assert!(json.get("temperature").is_none(), "GA: no session temperature");
        assert!(json.get("reasoning").is_none(), "GA: no session reasoning");
    }

    #[test]
    fn audio_format_bytes_per_ms_matches_rate() {
        // review wf_d43814c3 #6: telephony g711 is 1 byte/sample @8kHz = 8
        // B/ms; hardcoding PCM16's 48 over-truncated 6×.
        assert_eq!(OpenAIRealtimeAudioFormat::Pcm16.bytes_per_ms(), 48);
        assert_eq!(OpenAIRealtimeAudioFormat::G711Ulaw.bytes_per_ms(), 8);
        assert_eq!(OpenAIRealtimeAudioFormat::G711Alaw.bytes_per_ms(), 8);
        // 200ms of g711 = 1600 bytes (not 1600/48 ≈ 33ms).
        assert_eq!(1600 / OpenAIRealtimeAudioFormat::G711Ulaw.bytes_per_ms(), 200);
    }

    #[tokio::test]
    async fn truncate_clears_playback_preventing_double_truncate() {
        // review wc71hewlx #10: a barge-in DURING playout truncates the
        // playing item; a SECOND truncate (e.g. a duplicate cancel) finds the
        // estimate cleared and is a no-op (no double-truncate). The estimate
        // is cleared by US on truncate — NOT on response.done (which is only
        // generation end while the client is still playing).
        let mut rt = OpenAIRealtime::new(RealtimeConfig {
            provider: "openai".into(),
            api_key: "k".into(),
            model: "gpt-4o-realtime-preview".into(),
            ..Default::default()
        })
        .unwrap();
        // Inject a connected sender so the truncate event has somewhere to go
        // (we only care that the playback estimate is consumed, not the wire).
        let (tx, mut rx) = mpsc::channel::<ClientEvent>(16);
        *rt.ws_sender.lock().await = Some(tx);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        *rt.playback.lock().unwrap() = Some(ItemPlayback {
            item_id: "item_1".into(),
            first_delta: std::time::Instant::now(),
            duration_ms: 800,
        });
        // First barge-in: the playing item IS truncated (not None).
        let truncated = rt.truncate_current_response().await.unwrap();
        assert!(truncated.is_some(), "a playing item must be truncated on barge-in");
        // Second barge-in: estimate cleared ⇒ no double-truncate.
        assert!(rt.truncate_current_response().await.unwrap().is_none());
    }

    #[test]
    fn replay_items_render_as_completed_messages_never_responses() {
        use crate::core::realtime::{ReplayConversationItem, TranscriptRole};
        let user = OpenAIRealtime::replay_item_to_conversation_item(&ReplayConversationItem {
            role: TranscriptRole::User,
            text: "hello there".into(),
        });
        assert_eq!(user.item_type, "message");
        assert_eq!(user.role.as_deref(), Some("user"));
        let c = &user.content.as_ref().unwrap()[0];
        assert_eq!(c.content_type, "input_text", "user side speaks input_text");
        assert_eq!(c.text.as_deref(), Some("hello there"));

        let bot = OpenAIRealtime::replay_item_to_conversation_item(&ReplayConversationItem {
            role: TranscriptRole::Assistant,
            text: "hi!".into(),
        });
        assert_eq!(bot.role.as_deref(), Some("assistant"));
        assert_eq!(bot.content.as_ref().unwrap()[0].content_type, "text");
        // The replay event is ConversationItemCreate — a RESPONSE is never
        // requested by replay (no duplicate inference on reconnect).
        let evt = ClientEvent::ConversationItemCreate {
            item: bot,
            previous_item_id: None,
        };
        let wire = serde_json::to_string(&evt).unwrap();
        assert!(wire.contains("conversation.item.create"));
        assert!(!wire.contains("response.create"));
    }

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
