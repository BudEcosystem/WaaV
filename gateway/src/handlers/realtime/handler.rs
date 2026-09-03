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
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::mpsc;
use tokio::{select, time::Duration};
use tracing::{debug, error, info, warn};

use crate::auth::Auth;
use crate::core::realtime::{
    BaseRealtime, RealtimeAudioData, RealtimeConfig, RealtimeError, ReconnectionEvent,
    TranscriptResult, TranscriptRole, create_realtime_provider, get_supported_realtime_providers,
};
use crate::state::AppState;

use super::messages::{
    RealtimeIncomingMessage, RealtimeMessageRoute, RealtimeOutgoingMessage, RealtimeSessionConfig,
    send_realtime_with_policy,
};

/// Optimized channel buffer size for audio workloads
const CHANNEL_BUFFER_SIZE: usize = 1024;

/// How long realtime teardown waits for the sender task to drain queued critical
/// messages and emit a close frame before aborting it.
const SENDER_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

/// Maximum idle time before closing an inactive realtime connection.
const REALTIME_IDLE_BASE_SECS: u64 = 300;

/// ±10% jitter around [`REALTIME_IDLE_BASE_SECS`] to avoid same-second timeout bursts.
const REALTIME_IDLE_JITTER_RANGE_SECS: u64 = 30;

static REALTIME_IDLE_JITTER_SEQ: AtomicU64 = AtomicU64::new(0);

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
    request_id: Option<Extension<crate::middleware::request_id::RequestId>>,
) -> Response {
    info!(
        auth_id = ?auth.id,
        "Realtime WebSocket connection upgrade requested"
    );

    // GW-17: mint (or propagate) the connection's W3C `traceparent` ONCE here — reusing the inbound
    // request's correlation id when it is a valid trace id (set by the request-id middleware from an
    // inbound `traceparent`), else minting a fresh one. The Infer-S2S adapter forwards it on the handshake
    // so one distributed trace spans the gateway turn AND the intra-Infer stages. Other providers ignore it.
    let inbound = request_id.as_ref().map(|ext| ext.0.as_str());
    let trace_parent = crate::middleware::request_id::mint_traceparent(inbound);

    ws.max_frame_size(MAX_WS_FRAME_SIZE)
        .max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| handle_realtime_socket(socket, state, auth, trace_parent))
}

/// Handle the realtime WebSocket connection. `trace_parent` is the connection's W3C `traceparent`
/// (GW-17), forwarded onto the WaaV Infer handshake when the selected provider is the Infer-S2S tier.
async fn handle_realtime_socket(
    socket: WebSocket,
    app_state: Arc<AppState>,
    auth: Auth,
    trace_parent: String,
) {
    info!(auth_id = ?auth.id, "Realtime WebSocket connection established");

    let (mut sender, mut receiver) = socket.split();
    let (message_tx, mut message_rx) = mpsc::channel::<RealtimeMessageRoute>(CHANNEL_BUFFER_SIZE);

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    // Sender task for outgoing messages
    let mut sender_task = tokio::spawn(async move {
        loop {
            select! {
                route_opt = message_rx.recv() => {
                    let Some(route) = route_opt else {
                        break;
                    };
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
                _ = &mut shutdown_rx => {
                    while let Ok(route) = message_rx.try_recv() {
                        let result = match route {
                            RealtimeMessageRoute::Outgoing(message) => match serde_json::to_string(&message) {
                                Ok(json_str) => sender.send(Message::Text(json_str.into())).await,
                                Err(e) => {
                                    error!("Failed to serialize outgoing message during shutdown: {}", e);
                                    continue;
                                }
                            },
                            RealtimeMessageRoute::Audio(data) => sender.send(Message::Binary(data)).await,
                            RealtimeMessageRoute::Close => sender.send(Message::Close(None)).await,
                        };
                        if result.is_err() {
                            break;
                        }
                    }
                    let _ = sender.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    });

    // State for the realtime session
    let mut realtime_provider: Option<Box<dyn BaseRealtime>> = None;
    let mut session_id: Option<String> = None;

    // How often we check if the connection is stale (configurable via REALTIME_PROCESSING_TIMEOUT_SECS)
    let processing_timeout = Duration::from_secs(app_state.config.realtime_processing_timeout_secs);

    let idle_timeout =
        realtime_idle_timeout(REALTIME_IDLE_BASE_SECS, REALTIME_IDLE_JITTER_RANGE_SECS);

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
                            &trace_parent,
                        ).await;

                        if !continue_processing {
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        warn!("Realtime WebSocket error: {}", e);
                        send_realtime_with_policy(
                            &message_tx,
                            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                                code: Some("websocket_error".to_string()),
                                message: format!("WebSocket error: {e}"),
                            }),
                        )
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
                    send_realtime_with_policy(
                        &message_tx,
                        RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                            code: Some("idle_timeout".to_string()),
                            message: "Connection closed due to inactivity".to_string(),
                        }),
                    )
                    .await;
                    break;
                }
                debug!("Realtime WebSocket connection idle check - still active");
            }
        }
    }

    // Cleanup: give the sender task a bounded chance to drain already-queued
    // critical messages (for example idle-timeout errors) before aborting it.
    shutdown_realtime_sender_task(shutdown_tx, &mut sender_task).await;

    // Disconnect realtime provider if connected
    if let Some(mut provider) = realtime_provider
        && let Err(e) = provider.disconnect().await
    {
        error!("Failed to disconnect realtime provider: {:?}", e);
    }

    info!("Realtime WebSocket connection terminated");
}

async fn shutdown_realtime_sender_task(
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    sender_task: &mut tokio::task::JoinHandle<()>,
) -> bool {
    let _ = shutdown_tx.send(());
    match tokio::time::timeout(SENDER_SHUTDOWN_TIMEOUT, &mut *sender_task).await {
        Ok(Ok(())) => {
            debug!("Realtime sender task completed gracefully");
            true
        }
        Ok(Err(e)) => {
            if e.is_panic() {
                error!("Realtime sender task panicked during shutdown: {}", e);
            } else {
                debug!("Realtime sender task cancelled during shutdown: {}", e);
            }
            true
        }
        Err(_) => {
            warn!("Realtime sender task did not complete within timeout; aborting");
            sender_task.abort();
            crate::core::metrics::bridge::record_session_dangling_task();
            let _ = sender_task.await;
            false
        }
    }
}

fn realtime_idle_timeout(base_idle_secs: u64, jitter_range_secs: u64) -> Duration {
    let seq = REALTIME_IDLE_JITTER_SEQ.fetch_add(1, Ordering::Relaxed);
    realtime_idle_timeout_for_seq(base_idle_secs, jitter_range_secs, seq)
}

fn realtime_idle_timeout_for_seq(
    base_idle_secs: u64,
    jitter_range_secs: u64,
    seq: u64,
) -> Duration {
    let offset = realtime_idle_jitter_offset_for_seq(seq, jitter_range_secs);
    let idle_secs = if offset.is_negative() {
        base_idle_secs.saturating_sub(offset.unsigned_abs())
    } else {
        base_idle_secs.saturating_add(offset as u64)
    };
    Duration::from_secs(idle_secs.max(1))
}

fn realtime_idle_jitter_offset_for_seq(seq: u64, jitter_range_secs: u64) -> i64 {
    if jitter_range_secs == 0 {
        return 0;
    }

    let width = jitter_range_secs.saturating_mul(2).saturating_add(1);
    (seq % width) as i64 - jitter_range_secs as i64
}

/// Process incoming WebSocket message
#[inline(always)]
async fn process_realtime_message(
    msg: Message,
    realtime_provider: &mut Option<Box<dyn BaseRealtime>>,
    session_id: &mut Option<String>,
    message_tx: &mpsc::Sender<RealtimeMessageRoute>,
    app_state: &Arc<AppState>,
    trace_parent: &str,
) -> bool {
    match msg {
        Message::Text(text) => {
            debug!("Received text message: {} bytes", text.len());

            let incoming_msg: RealtimeIncomingMessage = match serde_json::from_str(&text) {
                Ok(msg) => msg,
                Err(e) => {
                    error!("Failed to parse realtime message: {}", e);
                    send_realtime_with_policy(
                        message_tx,
                        RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                            code: Some("parse_error".to_string()),
                            message: format!("Invalid message format: {e}"),
                        }),
                    )
                    .await;
                    return true;
                }
            };

            // Validate message size
            if let Err(e) = incoming_msg.validate_size() {
                warn!("Message validation failed: {}", e);
                send_realtime_with_policy(
                    message_tx,
                    RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                        code: Some("validation_error".to_string()),
                        message: e.to_string(),
                    }),
                )
                .await;
                return true;
            }

            handle_realtime_incoming(
                incoming_msg,
                realtime_provider,
                session_id,
                message_tx,
                app_state,
                trace_parent,
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
                        send_realtime_with_policy(
                            message_tx,
                            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                                code: Some("audio_error".to_string()),
                                message: format!("Failed to send audio: {e}"),
                            }),
                        )
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
    trace_parent: &str,
) -> bool {
    match msg {
        RealtimeIncomingMessage::Config(config) => {
            handle_config(
                config,
                realtime_provider,
                session_id,
                message_tx,
                app_state,
                trace_parent,
            )
            .await
        }
        RealtimeIncomingMessage::Text { text } => {
            if let Some(provider) = realtime_provider
                && provider.is_ready()
                && let Err(e) = provider.send_text(&text).await
            {
                send_realtime_with_policy(
                    message_tx,
                    RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                        code: Some("text_error".to_string()),
                        message: format!("Failed to send text: {e}"),
                    }),
                )
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
                        let overrides = crate::core::realtime::RealtimeResponseOverride {
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
                    send_realtime_with_policy(
                        message_tx,
                        RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                            code: Some("response_error".to_string()),
                            message: format!("Failed to create response: {e}"),
                        }),
                    )
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
                        send_realtime_with_policy(
                            message_tx,
                            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                                code: Some("cancel_error".to_string()),
                                message: format!("Failed to cancel response: {e}"),
                            }),
                        )
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
                send_realtime_with_policy(
                    message_tx,
                    RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                        code: Some("commit_error".to_string()),
                        message: format!("Failed to commit audio: {e}"),
                    }),
                )
                .await;
            }
            true
        }
        RealtimeIncomingMessage::ClearAudio => {
            if let Some(provider) = realtime_provider
                && let Err(e) = provider.clear_audio_buffer().await
            {
                send_realtime_with_policy(
                    message_tx,
                    RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                        code: Some("clear_error".to_string()),
                        message: format!("Failed to clear audio: {e}"),
                    }),
                )
                .await;
            }
            true
        }
        RealtimeIncomingMessage::FunctionResult { call_id, result } => {
            if let Some(provider) = realtime_provider
                && let Err(e) = provider.submit_function_result(&call_id, &result).await
            {
                send_realtime_with_policy(
                    message_tx,
                    RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                        code: Some("function_error".to_string()),
                        message: format!("Failed to submit function result: {e}"),
                    }),
                )
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
    mut config: RealtimeSessionConfig,
    realtime_provider: &mut Option<Box<dyn BaseRealtime>>,
    session_id: &mut Option<String>,
    message_tx: &mpsc::Sender<RealtimeMessageRoute>,
    app_state: &Arc<AppState>,
    trace_parent: &str,
) -> bool {
    // P3: resolve a server-side ALIAS into the session config BEFORE the provider /
    // credential is selected. Definitions are server-config-only (SSRF-safe); explicit
    // client fields win. Unknown alias is non-fatal (proceed + advisory). This mirrors
    // the `/ws` config-handler resolution point.
    if let Some(alias_name) = config.alias.clone() {
        match crate::core::alias::global_aliases().resolve(&alias_name) {
            Some(def) => {
                let echo = crate::core::alias::splice_alias_realtime(
                    &alias_name,
                    &def,
                    &mut config.provider,
                    &mut config.model,
                    &mut config.voice,
                    &mut config.instructions,
                );
                info!(alias = %alias_name, resolved = ?echo, "Resolved server-side realtime alias");
            }
            None => {
                let (code, message) = crate::core::alias::unknown_alias_message(&alias_name);
                send_realtime_with_policy(
                    message_tx,
                    RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                        code: Some(code),
                        message,
                    }),
                )
                .await;
            }
        }
    }

    let provider_name = config.provider.as_deref().unwrap_or(DEFAULT_PROVIDER);
    let model = config.model.as_deref().unwrap_or(DEFAULT_MODEL);

    // Validate provider
    let supported = get_supported_realtime_providers();
    if !supported
        .iter()
        .any(|p| p.eq_ignore_ascii_case(provider_name))
    {
        send_realtime_with_policy(
            message_tx,
            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                code: Some("invalid_provider".to_string()),
                message: format!(
                    "Unsupported provider: {}. Supported: {:?}",
                    provider_name, supported
                ),
            }),
        )
        .await;
        return true;
    }

    // Get API key from config based on provider
    let api_key = match provider_name.to_lowercase().as_str() {
        "openai" => app_state.config.openai_api_key.clone(),
        "hume" => app_state.config.hume_api_key.clone(),
        // OpenAI-protocol clones (own credentials, GA wire reused by delegation).
        "azure" | "azure-openai" | "azure_openai" => app_state.config.azure_openai_api_key.clone(),
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
        // Yandex Cloud AI Studio Realtime (OpenAI-protocol clone) uses the dedicated
        // Yandex API key (a Yandex IAM token / static API key), passed as the
        // `Authorization: Bearer <token>` value. The folder id is injected below.
        "yandex" | "yandexgpt" | "yandex-cloud" => app_state.config.yandex_api_key.clone(),
        _ => None,
    };

    let Some(api_key) = api_key else {
        send_realtime_with_policy(
            message_tx,
            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                code: Some("missing_api_key".to_string()),
                message: format!("API key not configured for provider: {}", provider_name),
            }),
        )
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
    // Yandex Cloud AI Studio Realtime needs the server-configured FOLDER ID (the
    // `<folder>` in the `gpt://<folder>/<model>` model URI) — it is NOT a client
    // session field. Injected into the provider's `endpoint` slot (mirroring the
    // Azure resource injection above); `YandexProtocol::from_config` reads it there.
    if matches!(
        provider_name.to_lowercase().as_str(),
        "yandex" | "yandexgpt" | "yandex-cloud"
    ) {
        realtime_config.endpoint = app_state.config.yandex_folder_id.clone();
    }

    // SERVER-CONFIG-ONLY realtime upstream URL override (SSRF-safe). Injected from
    // the TRUSTED server config (`<PROVIDER>_REALTIME_URL` env vars →
    // `ServerConfig::realtime_endpoint_overrides`), keyed by the CANONICAL provider
    // name (so aliases like `azure-openai` / `11labs` / `gemini-live` resolve to
    // the same override). It is DELIBERATELY set here and NOT inside
    // `build_realtime_config` (the client→config converter), so the untrusted
    // client `RealtimeSessionConfig` can never influence which upstream the gateway
    // dials. When set + `ws://`/`wss://`, the provider's `connect_spec` uses it
    // verbatim (proxy / self-hosted / gov-cloud / local mock).
    if let Some(override_url) = canonical_realtime_provider(provider_name)
        .and_then(|canon| app_state.config.realtime_endpoint_overrides.get(canon))
    {
        realtime_config.realtime_endpoint_override = Some(override_url.clone());
    }

    // GW-17: forward the connection's propagated W3C `traceparent` so the WaaV Infer-S2S adapter injects it
    // on the handshake (`session.config` `trace` + a `traceparent` connect header) and the engine parents
    // its per-turn / per-stage spans under it — one distributed trace spans the gateway turn AND the
    // intra-Infer STT/LLM/TTS stages. Only well-formed (validated at mint); other providers ignore it.
    if crate::middleware::request_id::is_w3c_traceparent(trace_parent) {
        realtime_config.trace = Some(trace_parent.to_string());
    }

    // Create provider
    let mut provider = match create_realtime_provider(provider_name, realtime_config) {
        Ok(p) => p,
        Err(e) => {
            send_realtime_with_policy(
                message_tx,
                RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                    code: Some("provider_error".to_string()),
                    message: format!("Failed to create provider: {e}"),
                }),
            )
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
                send_realtime_with_policy(
                    &tx,
                    RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Transcript {
                        text: result.text,
                        role: role.to_string(),
                        is_final: result.is_final,
                    }),
                )
                .await;
            })
        }))
        .ok();

    let tx_clone = message_tx.clone();
    provider
        .on_audio(Arc::new(move |audio: RealtimeAudioData| {
            let tx = tx_clone.clone();
            Box::pin(async move {
                send_realtime_with_policy(&tx, RealtimeMessageRoute::Audio(audio.data)).await;
            })
        }))
        .ok();

    let tx_clone = message_tx.clone();
    provider
        .on_error(Arc::new(move |error: RealtimeError| {
            let tx = tx_clone.clone();
            Box::pin(async move {
                send_realtime_with_policy(
                    &tx,
                    RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                        code: Some("provider_error".to_string()),
                        message: error.to_string(),
                    }),
                )
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
                    send_realtime_with_policy(
                        &tx,
                        RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::FunctionCall {
                            call_id: call.call_id,
                            name: call.name,
                            arguments: call.arguments,
                        }),
                    )
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
                    send_realtime_with_policy(
                        &tx,
                        RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::SpeechEvent {
                            event: event_type.to_string(),
                            audio_ms,
                        }),
                    )
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
                send_realtime_with_policy(
                    &tx,
                    RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::ResponseDone {
                        response_id,
                    }),
                )
                .await;
            })
        }))
        .ok();

    // On a TERMINAL reconnection give-up — the provider supervisor exhausted its
    // retry budget or hit the quick-failure cutoff — the upstream session can
    // NEVER recover. Surface that to the CLIENT and close the socket. Otherwise
    // the connection is held open with client audio silently dropped
    // (`is_ready()` is false), and because a continuously-streaming client
    // resets the handler's idle timer on every inbound frame, the ~5-min idle
    // timeout NEVER fires — the session hangs open indefinitely with no error
    // (review wf_fb932e8d: "dead session never surfaced"). A SUCCESSFUL
    // reconnect (`success == true`) is intentionally silent: playback resumes.
    let tx_clone = message_tx.clone();
    provider
        .on_reconnection(Arc::new(move |event: ReconnectionEvent| {
            let tx = tx_clone.clone();
            Box::pin(async move {
                if event.success {
                    return;
                }
                send_realtime_with_policy(
                    &tx,
                    RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                        code: Some("connection_lost".to_string()),
                        message: event.error.unwrap_or_else(|| {
                            "Realtime provider connection lost and could not be re-established"
                                .to_string()
                        }),
                    }),
                )
                .await;
                // Terminally dead — tell the sender task to close the client WS.
                send_realtime_with_policy(&tx, RealtimeMessageRoute::Close).await;
            })
        }))
        .ok();

    // Connect to provider
    info!("Connecting to {} realtime provider", provider_name);
    if let Err(e) = provider.connect().await {
        send_realtime_with_policy(
            message_tx,
            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                code: Some("connection_error".to_string()),
                message: format!("Failed to connect: {e}"),
            }),
        )
        .await;
        return true;
    }

    // Generate session ID
    let new_session_id = uuid::Uuid::new_v4().to_string();
    *session_id = Some(new_session_id.clone());

    // Store provider
    *realtime_provider = Some(provider);

    // Send session created message
    send_realtime_with_policy(
        message_tx,
        RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::SessionCreated {
            session_id: new_session_id,
            provider: provider_name.to_string(),
            model: model.to_string(),
        }),
    )
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
        send_realtime_with_policy(
            message_tx,
            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                code: Some("no_session".to_string()),
                message: "No active session to update".to_string(),
            }),
        )
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
        send_realtime_with_policy(
            message_tx,
            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::Error {
                code: Some("update_error".to_string()),
                message: format!("Failed to update session: {e}"),
            }),
        )
        .await;
    } else {
        send_realtime_with_policy(
            message_tx,
            RealtimeMessageRoute::Outgoing(RealtimeOutgoingMessage::SessionUpdated),
        )
        .await;
    }

    true
}

/// Map a client-supplied realtime provider name (incl. its accepted aliases) to
/// the CANONICAL provider id used as the key in
/// [`ServerConfig::realtime_endpoint_overrides`](crate::config::ServerConfig::realtime_endpoint_overrides).
/// Returns `None` for providers that have no WS endpoint override (e.g.
/// `nova_sonic`, which is a Bedrock HTTP/2 stream, or an unknown name). The alias
/// set mirrors the api-key resolution `match` above so an override applies
/// regardless of which alias the client used.
fn canonical_realtime_provider(provider_name: &str) -> Option<&'static str> {
    match provider_name.to_lowercase().as_str() {
        "openai" => Some("openai"),
        "azure" | "azure-openai" | "azure_openai" => Some("azure"),
        "grok" | "xai" => Some("grok"),
        "inworld" => Some("inworld"),
        "deepgram" | "deepgram-agent" | "deepgram_voice_agent" => Some("deepgram"),
        "elevenlabs" | "elevenlabs-convai" | "11labs" => Some("elevenlabs"),
        "gemini" | "gemini-live" | "google" => Some("gemini"),
        "ultravox" | "fixie" => Some("ultravox"),
        "hume" => Some("hume"),
        "speechmatics" | "flow" => Some("speechmatics"),
        "yandex" | "yandexgpt" | "yandex-cloud" => Some("yandex"),
        _ => None,
    }
}

/// Build RealtimeConfig from session config.
///
/// `pub` + re-exported (`handlers::realtime::build_realtime_config`) so the
/// config-plumbing integration proof (`tests/realtime_full_integration.rs`) can
/// exercise the REAL `RealtimeSessionConfig -> RealtimeConfig` converter instead
/// of an in-test mirror that could silently drift from this mapping.
///
/// SECURITY: this converter does NOT populate
/// [`RealtimeConfig::realtime_endpoint_override`](crate::core::realtime::RealtimeConfig::realtime_endpoint_override)
/// — `RealtimeSessionConfig` (the untrusted client message) carries no endpoint
/// field, and the upstream override is injected SEPARATELY by the handler from
/// trusted server config only. Keep it that way (no SSRF via client input).
pub fn build_realtime_config(api_key: String, config: &RealtimeSessionConfig) -> RealtimeConfig {
    use crate::core::realtime::{InputTranscriptionConfig, TurnDetectionConfig};

    let turn_detection = config.turn_detection.as_ref().map(|td| match td {
        crate::handlers::realtime::messages::TurnDetectionConfig::ServerVad {
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
        crate::handlers::realtime::messages::TurnDetectionConfig::Semantic { eagerness } => {
            TurnDetectionConfig::SemanticVad {
                eagerness: eagerness.clone(),
                create_response: Some(true),
                interrupt_response: Some(true),
            }
        }
        crate::handlers::realtime::messages::TurnDetectionConfig::Manual => {
            TurnDetectionConfig::None
        }
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

    /// SECURITY (SSRF): the client→config converter MUST NEVER populate
    /// `realtime_endpoint_override`. The untrusted `RealtimeSessionConfig` has no
    /// endpoint field, and the upstream override is injected SEPARATELY by the
    /// handler from trusted server config only. A client cannot redirect the
    /// gateway's upstream connection. (If someone adds a client endpoint field
    /// that flows here, this test fails — that is the point.)
    #[test]
    fn build_realtime_config_never_sets_endpoint_override_from_client() {
        // A fully-populated client config (every client-settable field) must still
        // leave BOTH server-injected endpoint fields untouched.
        let session_config = RealtimeSessionConfig {
            provider: Some("openai".to_string()),
            model: Some("gpt-realtime".to_string()),
            voice: Some("alloy".to_string()),
            instructions: Some("hi".to_string()),
            transcribe_input: Some(true),
            transcription_model: Some("whisper-1".to_string()),
            input_audio_format: Some("pcm16".to_string()),
            output_audio_format: Some("pcm16".to_string()),
            modalities: Some(vec!["audio".to_string()]),
            ..Default::default()
        };
        let realtime_config = build_realtime_config("test-key".to_string(), &session_config);
        assert!(
            realtime_config.realtime_endpoint_override.is_none(),
            "client input must NEVER set realtime_endpoint_override (SSRF guard)"
        );
        assert!(
            realtime_config.endpoint.is_none(),
            "client input must NEVER set `endpoint` either (server-injected only)"
        );
    }

    /// The override is keyed by the CANONICAL provider id, so every accepted alias
    /// resolves to the same server-config override (and non-WS providers map to
    /// `None`).
    #[test]
    fn canonical_realtime_provider_maps_aliases() {
        assert_eq!(canonical_realtime_provider("azure-openai"), Some("azure"));
        assert_eq!(canonical_realtime_provider("azure_openai"), Some("azure"));
        assert_eq!(canonical_realtime_provider("xai"), Some("grok"));
        assert_eq!(canonical_realtime_provider("11labs"), Some("elevenlabs"));
        assert_eq!(canonical_realtime_provider("gemini-live"), Some("gemini"));
        assert_eq!(canonical_realtime_provider("fixie"), Some("ultravox"));
        assert_eq!(canonical_realtime_provider("flow"), Some("speechmatics"));
        assert_eq!(canonical_realtime_provider("hume"), Some("hume"));
        // nova_sonic is a Bedrock HTTP/2 stream — NO ws endpoint override.
        assert_eq!(canonical_realtime_provider("nova_sonic"), None);
        assert_eq!(canonical_realtime_provider("bogus"), None);
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

    #[test]
    fn realtime_idle_jitter_spreads_connection_deadlines() {
        let offsets: Vec<i64> = (0..=REALTIME_IDLE_JITTER_RANGE_SECS * 2)
            .map(|seq| realtime_idle_jitter_offset_for_seq(seq, REALTIME_IDLE_JITTER_RANGE_SECS))
            .collect();

        assert_eq!(
            offsets.first().copied(),
            Some(-(REALTIME_IDLE_JITTER_RANGE_SECS as i64))
        );
        assert!(offsets.contains(&0));
        assert_eq!(
            offsets.last().copied(),
            Some(REALTIME_IDLE_JITTER_RANGE_SECS as i64)
        );
        assert!(
            offsets.windows(2).any(|pair| pair[0] != pair[1]),
            "jitter must not collapse to one fixed offset"
        );
        assert_eq!(
            realtime_idle_timeout_for_seq(
                REALTIME_IDLE_BASE_SECS,
                REALTIME_IDLE_JITTER_RANGE_SECS,
                0,
            ),
            Duration::from_secs(270)
        );
        assert_eq!(
            realtime_idle_timeout_for_seq(
                REALTIME_IDLE_BASE_SECS,
                REALTIME_IDLE_JITTER_RANGE_SECS,
                REALTIME_IDLE_JITTER_RANGE_SECS,
            ),
            Duration::from_secs(300)
        );
        assert_eq!(
            realtime_idle_timeout_for_seq(
                REALTIME_IDLE_BASE_SECS,
                REALTIME_IDLE_JITTER_RANGE_SECS,
                REALTIME_IDLE_JITTER_RANGE_SECS * 2,
            ),
            Duration::from_secs(330)
        );
    }

    #[test]
    fn realtime_idle_timeout_clamps_when_jitter_exceeds_base() {
        assert_eq!(
            realtime_idle_timeout_for_seq(10, 30, 0),
            Duration::from_secs(1)
        );
        assert_eq!(
            realtime_idle_timeout_for_seq(0, 0, 0),
            Duration::from_secs(1)
        );
    }

    #[tokio::test]
    async fn p5_realtime_sender_shutdown_completes_gracefully() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let observed_shutdown = Arc::new(AtomicBool::new(false));
        let observed = observed_shutdown.clone();
        let mut sender_task = tokio::spawn(async move {
            let _ = shutdown_rx.await;
            observed.store(true, Ordering::SeqCst);
        });

        assert!(shutdown_realtime_sender_task(shutdown_tx, &mut sender_task).await);
        assert!(
            observed_shutdown.load(Ordering::SeqCst),
            "sender task must observe the shutdown signal"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn p5_realtime_sender_shutdown_aborts_stuck_task() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let observed_shutdown = Arc::new(AtomicBool::new(false));
        let observed = observed_shutdown.clone();
        let mut sender_task = tokio::spawn(async move {
            let _ = shutdown_rx.await;
            observed.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        });

        assert!(
            !shutdown_realtime_sender_task(shutdown_tx, &mut sender_task).await,
            "stuck sender task must time out and be aborted"
        );
        assert!(
            observed_shutdown.load(Ordering::SeqCst),
            "sender task should still receive shutdown before timing out"
        );
        assert!(
            sender_task.is_finished(),
            "aborted task handle must be joined"
        );
    }

    /// Full-surface round-trip: a `RealtimeSessionConfig` carrying EVERY feature
    /// (turn detection, tools, transcription, noise reduction, reasoning effort,
    /// modalities, audio formats, temperature, max tokens) must convert through
    /// `build_realtime_config` AND then construct a provider for EVERY supported
    /// realtime provider without panicking — the exact two-step the live handler
    /// runs before connect. Bug class: a feature field that converts fine for
    /// OpenAI but trips a sibling provider's `from_config` (a panic / hard error
    /// on a field it should ignore), which would otherwise only surface live.
    #[test]
    fn full_feature_config_round_trips_for_every_provider() {
        let session_config = RealtimeSessionConfig {
            provider: Some("openai".to_string()),
            model: Some("gpt-realtime".to_string()),
            voice: Some("alloy".to_string()),
            instructions: Some("Be concise and helpful.".to_string()),
            temperature: Some(0.7),
            max_response_tokens: Some(2048),
            turn_detection: Some(
                crate::handlers::realtime::messages::TurnDetectionConfig::ServerVad {
                    threshold: Some(0.55),
                    silence_duration_ms: Some(700),
                    prefix_padding_ms: Some(120),
                },
            ),
            tools: Some(vec![crate::handlers::realtime::messages::ToolConfig {
                tool_type: "function".to_string(),
                function: crate::handlers::realtime::messages::FunctionConfig {
                    name: "get_weather".to_string(),
                    description: Some("Look up the weather".to_string()),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": { "city": { "type": "string" } },
                        "required": ["city"]
                    })),
                },
            }]),
            modalities: Some(vec!["audio".to_string(), "text".to_string()]),
            transcribe_input: Some(true),
            transcription_model: Some("whisper-1".to_string()),
            input_audio_format: Some("pcm16".to_string()),
            output_audio_format: Some("pcm16".to_string()),
            reasoning_effort: Some(crate::core::llm::ReasoningEffort::Low),
            input_audio_noise_reduction: Some("near_field".to_string()),
            alias: None,
        };

        // Step 1: the client→config converter must carry the full surface through
        // and (SSRF) never set the server-only endpoint fields.
        let mut realtime_config = build_realtime_config("test-key".to_string(), &session_config);
        assert!(realtime_config.turn_detection.is_some());
        assert!(realtime_config.tools.is_some());
        assert!(realtime_config.input_audio_transcription.is_some());
        assert_eq!(
            realtime_config.input_audio_noise_reduction.as_deref(),
            Some("near_field")
        );
        assert_eq!(
            realtime_config.reasoning_effort,
            Some(crate::core::llm::ReasoningEffort::Low)
        );
        assert!(realtime_config.realtime_endpoint_override.is_none());

        // Step 2: that config must construct EVERY provider (offline, no connect).
        // Azure needs its resource endpoint (server-injected in production).
        realtime_config.endpoint = Some("https://my-resource.openai.azure.com".to_string());
        for provider in get_supported_realtime_providers() {
            let result = create_realtime_provider(provider, realtime_config.clone());
            assert!(
                result.is_ok(),
                "full-feature config must construct `{provider}`, got {:?}",
                result.err()
            );
        }
    }
}
