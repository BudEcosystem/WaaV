//! Realtime WebSocket handler
//!
//! This module provides the WebSocket handler for real-time audio-to-audio
//! streaming using providers like OpenAI's Realtime API.
//!
//! The handler abstracts provider-specific details, providing a unified
//! interface for clients to interact with different realtime providers.

use axum::{
    Extension,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::{select, time::Duration};
use tracing::{debug, error, info, warn};

use crate::auth::Auth;
use crate::core::realtime::{
    BaseRealtime, RealtimeAudioData, RealtimeConfig, RealtimeError, TranscriptResult,
    TranscriptRole, create_realtime_provider, get_supported_realtime_providers,
};
use crate::state::AppState;

use super::messages::{
    RealtimeIncomingMessage, RealtimeMessageRoute, RealtimeOutgoingMessage, RealtimeSessionConfig,
};

/// Optimized channel buffer size for audio workloads
const CHANNEL_BUFFER_SIZE: usize = 1024;

/// Maximum WebSocket frame size (10 MB)
const MAX_WS_FRAME_SIZE: usize = 10 * 1024 * 1024;

/// Maximum WebSocket message size (10 MB)
const MAX_WS_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// Default provider if not specified
const DEFAULT_PROVIDER: &str = "openai";

/// Default model if not specified — GA `gpt-realtime` (the Beta-era
/// `gpt-4o-realtime-preview` is retired; a model-less session must not default
/// to it).
const DEFAULT_MODEL: &str = "gpt-realtime";

/// Realtime WebSocket handler
///
/// Upgrades the HTTP connection to WebSocket for real-time audio-to-audio processing.
/// This endpoint provides bidirectional audio streaming with transcription and TTS.
///
/// # Arguments
/// * `ws` - The WebSocket upgrade request from Axum
/// * `state` - Application state containing configuration
/// * `auth` - Auth context from middleware
///
/// # Returns
/// * `Response` - HTTP response that upgrades the connection to WebSocket
pub async fn realtime_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<Auth>,
) -> Response {
    info!(
        auth_id = ?auth.id,
        "Realtime WebSocket connection upgrade requested"
    );

    ws.max_frame_size(MAX_WS_FRAME_SIZE)
        .max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| handle_realtime_socket(socket, state, auth))
}

/// Handle the realtime WebSocket connection
async fn handle_realtime_socket(socket: WebSocket, app_state: Arc<AppState>, auth: Auth) {
    info!(auth_id = ?auth.id, "Realtime WebSocket connection established");

    let (mut sender, mut receiver) = socket.split();
    let (message_tx, mut message_rx) = mpsc::channel::<RealtimeMessageRoute>(CHANNEL_BUFFER_SIZE);

    // Sender task for outgoing messages
    let sender_task = tokio::spawn(async move {
        while let Some(route) = message_rx.recv().await {
            let should_close = matches!(route, RealtimeMessageRoute::Close);

            let result = match route {
                RealtimeMessageRoute::Outgoing(message) => match serde_json::to_string(&message) {
                    Ok(json_str) => sender.send(Message::Text(json_str.into())).await,
                    Err(e) => {
                        error!("Failed to serialize outgoing message: {}", e);
                        continue;
                    }
                },
                RealtimeMessageRoute::Audio(data) => sender.send(Message::Binary(data)).await,
                RealtimeMessageRoute::Close => {
                    info!("Closing realtime WebSocket connection");
                    sender.send(Message::Close(None)).await
                }
            };

            if let Err(e) = result {
                error!("Failed to send WebSocket message: {}", e);
                break;
            }

            if should_close {
                break;
            }
        }
    });

    // State for the realtime session
    let mut realtime_provider: Option<Box<dyn BaseRealtime>> = None;
    let mut session_id: Option<String> = None;

    // How often we check if the connection is stale (configurable via REALTIME_PROCESSING_TIMEOUT_SECS)
    let processing_timeout = Duration::from_secs(app_state.config.realtime_processing_timeout_secs);

    // Maximum idle time before closing the connection (5 minutes with ±10% jitter)
    // Jitter prevents thundering herd when many connections timeout simultaneously
    let base_idle_secs: u64 = 300;
    let jitter_range: u64 = 30; // ±10% = 30 seconds
    let jitter_offset = (std::time::Instant::now().elapsed().as_nanos() as u64 % (jitter_range * 2))
        as i64
        - jitter_range as i64;
    let idle_secs = (base_idle_secs as i64 + jitter_offset).max(1) as u64;
    let idle_timeout = Duration::from_secs(idle_secs);

    // Track last activity time for idle connection detection
    let mut last_activity = std::time::Instant::now();

    loop {
        select! {
            msg_result = receiver.next() => {
                // Update activity time on any message
                last_activity = std::time::Instant::now();

                match msg_result {
                    Some(Ok(msg)) => {
                        let continue_processing = process_realtime_message(
                            msg,
                            &mut realtime_provider,
                            &mut session_id,
                            &message_tx,
                            &app_state,
                        ).await;

                        if !continue_processing {
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        warn!("Realtime WebSocket error: {}", e);
                        let _ = message_tx
                            .send(RealtimeMessageRoute::Outgoing(
                                RealtimeOutgoingMessage::Error {
                                    code: Some("websocket_error".to_string()),
                                    message: format!("WebSocket error: {e}"),
                                },
                            ))
                            .await;
                        break;
                    }
                    None => {
                        info!("Realtime WebSocket connection closed by client");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(processing_timeout) => {
                // Check if connection has been idle too long
                if last_activity.elapsed() > idle_timeout {
                    warn!(
                        "Realtime WebSocket connection idle for {}s, closing stale connection",
                        last_activity.elapsed().as_secs()
                    );
                    let _ = message_tx
                        .send(RealtimeMessageRoute::Outgoing(
                            RealtimeOutgoingMessage::Error {
                                code: Some("idle_timeout".to_string()),
                                message: "Connection closed due to inactivity".to_string(),
                            },
                        ))
                        .await;
                    break;
                }
                debug!("Realtime WebSocket connection idle check - still active");
            }
        }
    }

    // Cleanup
    sender_task.abort();

    // Disconnect realtime provider if connected
    if let Some(mut provider) = realtime_provider
        && let Err(e) = provider.disconnect().await
    {
        error!("Failed to disconnect realtime provider: {:?}", e);
    }

    info!("Realtime WebSocket connection terminated");
}

/// Process incoming WebSocket message
#[inline(always)]
async fn process_realtime_message(
    msg: Message,
    realtime_provider: &mut Option<Box<dyn BaseRealtime>>,
    session_id: &mut Option<String>,
    message_tx: &mpsc::Sender<RealtimeMessageRoute>,
    app_state: &Arc<AppState>,
) -> bool {
    match msg {
        Message::Text(text) => {
            debug!("Received text message: {} bytes", text.len());

            let incoming_msg: RealtimeIncomingMessage = match serde_json::from_str(&text) {
                Ok(msg) => msg,
                Err(e) => {
                    error!("Failed to parse realtime message: {}", e);
                    let _ = message_tx
                        .send(RealtimeMessageRoute::Outgoing(
                            RealtimeOutgoingMessage::Error {
                                code: Some("parse_error".to_string()),
                                message: format!("Invalid message format: {e}"),
                            },
                        ))
                        .await;
                    return true;
                }
            };

            // Validate message size
            if let Err(e) = incoming_msg.validate_size() {
                warn!("Message validation failed: {}", e);
                let _ = message_tx
                    .send(RealtimeMessageRoute::Outgoing(
                        RealtimeOutgoingMessage::Error {
                            code: Some("validation_error".to_string()),
                            message: e.to_string(),
                        },
                    ))
                    .await;
                return true;
            }

            handle_realtime_incoming(
                incoming_msg,
                realtime_provider,
                session_id,
                message_tx,
                app_state,
            )
            .await
        }
        Message::Binary(data) => {
            debug!("Received binary audio: {} bytes", data.len());

            // Send audio to provider if connected
            if let Some(provider) = realtime_provider {
                if provider.is_ready() {
                    // data is already Bytes, use it directly without allocation
                    if let Err(e) = provider.send_audio(data).await {
                        warn!("Failed to send audio to provider: {:?}", e);
                        let _ = message_tx
                            .send(RealtimeMessageRoute::Outgoing(
                                RealtimeOutgoingMessage::Error {
                                    code: Some("audio_error".to_string()),
                                    message: format!("Failed to send audio: {e}"),
                                },
                            ))
                            .await;
                    }
                } else {
                    debug!("Provider not ready, dropping audio");
                }
            } else {
                debug!("No provider configured, dropping audio");
            }
            true
        }
        Message::Ping(_) => {
            debug!("Received ping");
            true
        }
        Message::Pong(_) => {
            debug!("Received pong");
            true
        }
        Message::Close(_) => {
            info!("Realtime WebSocket close received");
            false
        }
    }
}

/// Handle typed incoming messages
#[allow(clippy::too_many_arguments)]
async fn handle_realtime_incoming(
    msg: RealtimeIncomingMessage,
    realtime_provider: &mut Option<Box<dyn BaseRealtime>>,
    session_id: &mut Option<String>,
    message_tx: &mpsc::Sender<RealtimeMessageRoute>,
    app_state: &Arc<AppState>,
) -> bool {
    match msg {
        RealtimeIncomingMessage::Config(config) => {
            handle_config(config, realtime_provider, session_id, message_tx, app_state).await
        }
        RealtimeIncomingMessage::Text { text } => {
            if let Some(provider) = realtime_provider
                && provider.is_ready()
                && let Err(e) = provider.send_text(&text).await
            {
                let _ = message_tx
                    .send(RealtimeMessageRoute::Outgoing(
                        RealtimeOutgoingMessage::Error {
                            code: Some("text_error".to_string()),
                            message: format!("Failed to send text: {e}"),
                        },
                    ))
                    .await;
            }
            true
        }
        RealtimeIncomingMessage::CreateResponse { response } => {
            if let Some(provider) = realtime_provider {
                // With overrides → per-response `create_response_with`; bare
                // message → the session-default `create_response`.
                let result = match response {
                    Some(ov) => {
                        let overrides =
                            crate::core::realtime::RealtimeResponseOverride {
                                modalities: ov.modalities,
                                instructions: ov.instructions,
                                voice: ov.voice,
                                max_output_tokens: ov.max_output_tokens,
                                out_of_band: ov.out_of_band.unwrap_or(false),
                                metadata: ov.metadata,
                            };
                        provider.create_response_with(overrides).await
                    }
                    None => provider.create_response().await,
                };
                if let Err(e) = result {
                    let _ = message_tx
                        .send(RealtimeMessageRoute::Outgoing(
                            RealtimeOutgoingMessage::Error {
                                code: Some("response_error".to_string()),
                                message: format!("Failed to create response: {e}"),
                            },
                        ))
                        .await;
                }
            }
            true
        }
        RealtimeIncomingMessage::CancelResponse => {
            // B-G2: a client cancel IS a barge-in — run the FULL sequence
            // (clear input buffer → replay preroll → cancel → truncate the
            // partially-heard item) so the provider's conversation state
            // matches what the user actually heard, instead of the old
            // shallow cancel-only forward.
            if let Some(provider) = realtime_provider {
                match crate::core::realtime::run_barge_in_sequence(provider.as_mut()).await {
                    Ok(Some((item_id, end_ms))) => {
                        tracing::debug!(item_id, end_ms, "barge-in: response truncated");
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let _ = message_tx
                            .send(RealtimeMessageRoute::Outgoing(
                                RealtimeOutgoingMessage::Error {
                                    code: Some("cancel_error".to_string()),
                                    message: format!("Failed to cancel response: {e}"),
                                },
                            ))
                            .await;
                    }
                }
            }
            true
        }
        RealtimeIncomingMessage::CommitAudio => {
            if let Some(provider) = realtime_provider
                && let Err(e) = provider.commit_audio_buffer().await
            {
                let _ = message_tx
                    .send(RealtimeMessageRoute::Outgoing(
                        RealtimeOutgoingMessage::Error {
                            code: Some("commit_error".to_string()),
                            message: format!("Failed to commit audio: {e}"),
                        },
                    ))
                    .await;
            }
            true
        }
        RealtimeIncomingMessage::ClearAudio => {
            if let Some(provider) = realtime_provider
                && let Err(e) = provider.clear_audio_buffer().await
            {
                let _ = message_tx
                    .send(RealtimeMessageRoute::Outgoing(
                        RealtimeOutgoingMessage::Error {
                            code: Some("clear_error".to_string()),
                            message: format!("Failed to clear audio: {e}"),
                        },
                    ))
                    .await;
            }
            true
        }
        RealtimeIncomingMessage::FunctionResult { call_id, result } => {
            if let Some(provider) = realtime_provider
                && let Err(e) = provider.submit_function_result(&call_id, &result).await
            {
                let _ = message_tx
                    .send(RealtimeMessageRoute::Outgoing(
                        RealtimeOutgoingMessage::Error {
                            code: Some("function_error".to_string()),
                            message: format!("Failed to submit function result: {e}"),
                        },
                    ))
                    .await;
            }
            true
        }
        RealtimeIncomingMessage::UpdateSession(config) => {
            handle_session_update(config, realtime_provider, message_tx).await
        }
    }
}

/// Handle config message - create and connect provider
async fn handle_config(
    config: RealtimeSessionConfig,
    realtime_provider: &mut Option<Box<dyn BaseRealtime>>,
    session_id: &mut Option<String>,
    message_tx: &mpsc::Sender<RealtimeMessageRoute>,
    app_state: &Arc<AppState>,
) -> bool {
    let provider_name = config.provider.as_deref().unwrap_or(DEFAULT_PROVIDER);
    let model = config.model.as_deref().unwrap_or(DEFAULT_MODEL);

    // Validate provider
    let supported = get_supported_realtime_providers();
    if !supported
        .iter()
        .any(|p| p.eq_ignore_ascii_case(provider_name))
    {
        let _ = message_tx
            .send(RealtimeMessageRoute::Outgoing(
                RealtimeOutgoingMessage::Error {
                    code: Some("invalid_provider".to_string()),
                    message: format!(
                        "Unsupported provider: {}. Supported: {:?}",
                        provider_name, supported
                    ),
                },
            ))
            .await;
        return true;
    }

    // Get API key from config based on provider
    let api_key = match provider_name.to_lowercase().as_str() {
        "openai" => app_state.config.openai_api_key.clone(),
        "hume" => app_state.config.hume_api_key.clone(),
        // OpenAI-protocol clones (own credentials, GA wire reused by delegation).
        "azure" | "azure-openai" | "azure_openai" => {
            app_state.config.azure_openai_api_key.clone()
        }
        "grok" | "xai" => app_state.config.grok_api_key.clone(),
        "inworld" => app_state.config.inworld_api_key.clone(),
        // Deepgram Voice Agent (S2S) reuses the EXISTING deepgram credential
        // (shared with Deepgram STT/TTS — no new config field).
        "deepgram" | "deepgram-agent" | "deepgram_voice_agent" => {
            app_state.config.deepgram_api_key.clone()
        }
        // ElevenLabs Conversational AI (S2S) reuses the EXISTING elevenlabs
        // credential (shared with ElevenLabs STT/TTS — no new config field).
        "elevenlabs" | "elevenlabs-convai" | "11labs" => {
            app_state.config.elevenlabs_api_key.clone()
        }
        // Google Gemini Live (BidiGenerateContent S2S) uses the dedicated Gemini
        // API key (the `?key=` query param) — distinct from `google_credentials`
        // (the service-account JSON for Google STT/TTS).
        "gemini" | "gemini-live" | "google" => app_state.config.gemini_api_key.clone(),
        // Ultravox (hosted S2S model API) uses the dedicated Ultravox API key
        // (the `X-API-Key` create-call header).
        "ultravox" | "fixie" => app_state.config.ultravox_api_key.clone(),
        // AWS Nova Sonic (Amazon Bedrock S2S) authenticates with AWS credentials
        // via the `aws-config` default chain (env / shared config / IAM role), NOT
        // an api-key — so there is NO `nova_sonic_api_key` config field. Supply a
        // present-but-empty key so the "missing_api_key" guard below is satisfied;
        // `NovaSonicProtocol::from_config` ignores `cfg.api_key` entirely and the
        // transport resolves SigV4 credentials at connect (exactly like the AWS
        // Transcribe STT provider).
        "nova_sonic" | "nova-sonic" | "aws" => Some(String::new()),
        // Speechmatics Flow (Voice AI) uses the dedicated Speechmatics API key,
        // passed as the `Authorization: Bearer <token>` value (a JWT / temporary
        // token). The same credential the Speechmatics STT/TTS providers use.
        "speechmatics" | "flow" => app_state.config.speechmatics_api_key.clone(),
        _ => None,
    };

    let Some(api_key) = api_key else {
        let _ = message_tx
            .send(RealtimeMessageRoute::Outgoing(
                RealtimeOutgoingMessage::Error {
                    code: Some("missing_api_key".to_string()),
                    message: format!("API key not configured for provider: {}", provider_name),
                },
            ))
            .await;
        return true;
    };

    // Build realtime config from session config
    let mut realtime_config = build_realtime_config(api_key, &config);
    // Azure OpenAI Realtime needs the server-configured resource/endpoint (the
    // `<resource>.openai.azure.com` host) — it is NOT a client-session field.
    if matches!(
        provider_name.to_lowercase().as_str(),
        "azure" | "azure-openai" | "azure_openai"
    ) {
        realtime_config.endpoint = app_state.config.azure_openai_endpoint.clone();
    }

    // Create provider
    let mut provider = match create_realtime_provider(provider_name, realtime_config) {
        Ok(p) => p,
        Err(e) => {
            let _ = message_tx
                .send(RealtimeMessageRoute::Outgoing(
                    RealtimeOutgoingMessage::Error {
                        code: Some("provider_error".to_string()),
                        message: format!("Failed to create provider: {e}"),
                    },
                ))
                .await;
            return true;
        }
    };

    // W-D2 fleet adoption: inject the shared, process-global resilience handles so the realtime
    // reconnect loop participates in process-wide storm control + per-provider breaker tripping and
    // publishes `waav_circuit_breaker_state{provider}` on transition. A no-op for providers that
    // don't consume the handles (default trait method).
    provider.set_resilience(app_state.core_state.resilience().handles_for(provider_name));

    // Register callbacks before connecting
    let tx_clone = message_tx.clone();
    provider
        .on_transcript(Arc::new(move |result: TranscriptResult| {
            let tx = tx_clone.clone();
            Box::pin(async move {
                let role = match result.role {
                    TranscriptRole::User => "user",
                    TranscriptRole::Assistant => "assistant",
                };
                let _ = tx
                    .send(RealtimeMessageRoute::Outgoing(
                        RealtimeOutgoingMessage::Transcript {
                            text: result.text,
                            role: role.to_string(),
                            is_final: result.is_final,
                        },
                    ))
                    .await;
            })
        }))
        .ok();

    let tx_clone = message_tx.clone();
    provider
        .on_audio(Arc::new(move |audio: RealtimeAudioData| {
            let tx = tx_clone.clone();
            Box::pin(async move {
                let _ = tx.send(RealtimeMessageRoute::Audio(audio.data)).await;
            })
        }))
        .ok();

    let tx_clone = message_tx.clone();
    provider
        .on_error(Arc::new(move |error: RealtimeError| {
            let tx = tx_clone.clone();
            Box::pin(async move {
                let _ = tx
                    .send(RealtimeMessageRoute::Outgoing(
                        RealtimeOutgoingMessage::Error {
                            code: Some("provider_error".to_string()),
                            message: error.to_string(),
                        },
                    ))
                    .await;
            })
        }))
        .ok();

    let tx_clone = message_tx.clone();
    provider
        .on_function_call(Arc::new(
            move |call: crate::core::realtime::FunctionCallRequest| {
                let tx = tx_clone.clone();
                Box::pin(async move {
                    let _ = tx
                        .send(RealtimeMessageRoute::Outgoing(
                            RealtimeOutgoingMessage::FunctionCall {
                                call_id: call.call_id,
                                name: call.name,
                                arguments: call.arguments,
                            },
                        ))
                        .await;
                })
            },
        ))
        .ok();

    let tx_clone = message_tx.clone();
    provider
        .on_speech_event(Arc::new(
            move |event: crate::core::realtime::SpeechEvent| {
                let tx = tx_clone.clone();
                Box::pin(async move {
                    let (event_type, audio_ms) = match event {
                        crate::core::realtime::SpeechEvent::Started { audio_start_ms, .. } => {
                            ("started", audio_start_ms)
                        }
                        crate::core::realtime::SpeechEvent::Stopped { audio_end_ms, .. } => {
                            ("stopped", audio_end_ms)
                        }
                    };
                    let _ = tx
                        .send(RealtimeMessageRoute::Outgoing(
                            RealtimeOutgoingMessage::SpeechEvent {
                                event: event_type.to_string(),
                                audio_ms,
                            },
                        ))
                        .await;
                })
            },
        ))
        .ok();

    let tx_clone = message_tx.clone();
    provider
        .on_response_done(Arc::new(move |response_id: String| {
            let tx = tx_clone.clone();
            Box::pin(async move {
                let _ = tx
                    .send(RealtimeMessageRoute::Outgoing(
                        RealtimeOutgoingMessage::ResponseDone { response_id },
                    ))
                    .await;
            })
        }))
        .ok();

    // Connect to provider
    info!("Connecting to {} realtime provider", provider_name);
    if let Err(e) = provider.connect().await {
        let _ = message_tx
            .send(RealtimeMessageRoute::Outgoing(
                RealtimeOutgoingMessage::Error {
                    code: Some("connection_error".to_string()),
                    message: format!("Failed to connect: {e}"),
                },
            ))
            .await;
        return true;
    }

    // Generate session ID
    let new_session_id = uuid::Uuid::new_v4().to_string();
    *session_id = Some(new_session_id.clone());

    // Store provider
    *realtime_provider = Some(provider);

    // Send session created message
    let _ = message_tx
        .send(RealtimeMessageRoute::Outgoing(
            RealtimeOutgoingMessage::SessionCreated {
                session_id: new_session_id,
                provider: provider_name.to_string(),
                model: model.to_string(),
            },
        ))
        .await;

    info!("Realtime session created with provider: {}", provider_name);
    true
}

/// Handle session update
async fn handle_session_update(
    config: RealtimeSessionConfig,
    realtime_provider: &mut Option<Box<dyn BaseRealtime>>,
    message_tx: &mpsc::Sender<RealtimeMessageRoute>,
) -> bool {
    let Some(provider) = realtime_provider else {
        let _ = message_tx
            .send(RealtimeMessageRoute::Outgoing(
                RealtimeOutgoingMessage::Error {
                    code: Some("no_session".to_string()),
                    message: "No active session to update".to_string(),
                },
            ))
            .await;
        return true;
    };

    // Build update config (reuse existing API key)
    let update_config = RealtimeConfig {
        api_key: String::new(), // Provider should retain existing key
        model: config.model.unwrap_or_default(),
        voice: config.voice,
        instructions: config.instructions,
        temperature: config.temperature,
        max_response_output_tokens: config.max_response_tokens,
        modalities: config.modalities,
        reasoning_effort: config.reasoning_effort, // S2S
        input_audio_noise_reduction: config.input_audio_noise_reduction.clone(),
        ..Default::default()
    };

    if let Err(e) = provider.update_session(update_config).await {
        let _ = message_tx
            .send(RealtimeMessageRoute::Outgoing(
                RealtimeOutgoingMessage::Error {
                    code: Some("update_error".to_string()),
                    message: format!("Failed to update session: {e}"),
                },
            ))
            .await;
    } else {
        let _ = message_tx
            .send(RealtimeMessageRoute::Outgoing(
                RealtimeOutgoingMessage::SessionUpdated,
            ))
            .await;
    }

    true
}

/// Build RealtimeConfig from session config.
///
/// `pub` + re-exported (`handlers::realtime::build_realtime_config`) so the
/// config-plumbing integration proof (`tests/realtime_full_integration.rs`) can
/// exercise the REAL `RealtimeSessionConfig -> RealtimeConfig` converter instead
/// of an in-test mirror that could silently drift from this mapping.
pub fn build_realtime_config(api_key: String, config: &RealtimeSessionConfig) -> RealtimeConfig {
    use crate::core::realtime::{InputTranscriptionConfig, TurnDetectionConfig};

    let turn_detection = config.turn_detection.as_ref().map(|td| match td {
        super::messages::TurnDetectionConfig::ServerVad {
            threshold,
            silence_duration_ms,
            prefix_padding_ms,
        } => TurnDetectionConfig::ServerVad {
            threshold: *threshold,
            prefix_padding_ms: *prefix_padding_ms,
            silence_duration_ms: *silence_duration_ms,
            create_response: Some(true),
            interrupt_response: Some(true),
        },
        super::messages::TurnDetectionConfig::Semantic { eagerness } => {
            TurnDetectionConfig::SemanticVad {
                eagerness: eagerness.clone(),
                create_response: Some(true),
                interrupt_response: Some(true),
            }
        }
        super::messages::TurnDetectionConfig::Manual => TurnDetectionConfig::None,
    });

    let tools = config.tools.as_ref().map(|tools| {
        tools
            .iter()
            .map(|t| crate::core::realtime::ToolDefinition {
                tool_type: t.tool_type.clone(),
                function: crate::core::realtime::FunctionDefinition {
                    name: t.function.name.clone(),
                    description: t.function.description.clone(),
                    parameters: t.function.parameters.clone(),
                },
            })
            .collect()
    });

    let input_audio_transcription = if config.transcribe_input.unwrap_or(true) {
        Some(InputTranscriptionConfig {
            // Use configured model or default to whisper-1
            model: config
                .transcription_model
                .clone()
                .unwrap_or_else(|| "whisper-1".to_string()),
        })
    } else {
        None
    };

    RealtimeConfig {
        api_key,
        model: config
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        voice: config.voice.clone(),
        instructions: config.instructions.clone(),
        temperature: config.temperature,
        max_response_output_tokens: config.max_response_tokens,
        input_audio_format: config.input_audio_format.clone(),
        output_audio_format: config.output_audio_format.clone(),
        input_audio_transcription,
        turn_detection,
        tools,
        modalities: config.modalities.clone(),
        reasoning_effort: config.reasoning_effort, // S2S
        input_audio_noise_reduction: config.input_audio_noise_reduction.clone(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_realtime_config_defaults() {
        let session_config = RealtimeSessionConfig::default();
        let realtime_config = build_realtime_config("test-key".to_string(), &session_config);

        assert_eq!(realtime_config.api_key, "test-key");
        assert_eq!(realtime_config.model, DEFAULT_MODEL);
    }

    #[test]
    fn test_build_realtime_config_with_options() {
        let session_config = RealtimeSessionConfig {
            model: Some("gpt-4o-mini-realtime-preview".to_string()),
            voice: Some("alloy".to_string()),
            instructions: Some("Be helpful".to_string()),
            temperature: Some(0.8),
            ..Default::default()
        };
        let realtime_config = build_realtime_config("test-key".to_string(), &session_config);

        assert_eq!(realtime_config.model, "gpt-4o-mini-realtime-preview");
        assert_eq!(realtime_config.voice.as_deref(), Some("alloy"));
        assert_eq!(realtime_config.instructions.as_deref(), Some("Be helpful"));
        assert_eq!(realtime_config.temperature, Some(0.8));
    }

    #[test]
    fn test_default_provider() {
        assert_eq!(DEFAULT_PROVIDER, "openai");
    }

    #[test]
    fn test_default_model() {
        // GA default — the Beta-era preview is retired (see DEFAULT_MODEL).
        assert_eq!(DEFAULT_MODEL, "gpt-realtime");
    }
}
