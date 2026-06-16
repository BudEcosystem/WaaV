//! Configuration handler for WebSocket connections
//!
//! This module handles the initialization and configuration of voice processing
//! and LiveKit connections, including provider setup and callback registration.

use base64::{Engine as _, engine::general_purpose};
use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio::time::{Duration, timeout};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    core::{
        conversation::ConversationOrchestrator,
        stt::STTResult,
        tts::AudioData,
        voice_manager::{VoiceManager, VoiceManagerConfig},
    },
    livekit::LiveKitClient,
    state::AppState,
};

#[cfg(feature = "dag-routing")]
use crate::dag::{
    compiler::DAGCompiler,
    context::{DAGContext, DagOutput},
    definition::DAGDefinition,
    executor::DAGExecutor,
    global_templates,
    nodes::STTResultData,
};

use super::{
    config::{
        ConversationWebSocketConfig, DAGWebSocketConfig, LiveKitWebSocketConfig,
        STTWebSocketConfig, TTSWebSocketConfig, compute_tts_config_hash,
    },
    messages::{MessageRoute, OutgoingMessage, ParticipantDisconnectedInfo, UnifiedMessage},
    state::ConnectionState,
};

/// Maximum wait time for providers to become ready (in seconds)
const PROVIDER_READY_TIMEOUT_SECS: u64 = 30;

/// Maximum wait time for LiveKit audio source (in milliseconds)
const LIVEKIT_AUDIO_WAIT_MS: u64 = 10000;

/// Polling interval for LiveKit audio source check (in milliseconds)
const LIVEKIT_POLL_INTERVAL_MS: u64 = 100;

/// Lock timeout for LiveKit operations (in milliseconds)
const LIVEKIT_LOCK_TIMEOUT_MS: u64 = 100;

/// Resolve the stream identifier for the current session
fn resolve_stream_id(stream_id: Option<String>) -> String {
    stream_id.unwrap_or_else(|| {
        let generated = Uuid::new_v4().to_string();
        debug!("Generated stream_id: {}", generated);
        generated
    })
}

/// Handle configuration message and initialize providers
///
/// This function sets up the voice processing pipeline including:
/// - STT (Speech-to-Text) provider initialization
/// - TTS (Text-to-Speech) provider initialization
/// - LiveKit client connection (optional)
/// - DAG routing initialization (optional, when dag_config provided)
/// - Callback registration for audio routing
///
/// # Arguments
/// * `stream_id` - Optional unique identifier for the WebSocket session
/// * `audio` - Enable/disable audio processing (default: true)
/// * `stt_ws_config` - STT provider configuration
/// * `tts_ws_config` - TTS provider configuration
/// * `livekit_ws_config` - Optional LiveKit configuration
/// * `dag_ws_config` - Optional DAG routing configuration
/// * `state` - Connection state to update
/// * `message_tx` - Channel for sending response messages
/// * `app_state` - Application state containing API keys
///
/// # Returns
/// * `bool` - true to continue processing, false to terminate connection
#[allow(clippy::too_many_arguments)]
pub async fn handle_config_message(
    stream_id: Option<String>,
    audio: Option<bool>,
    stt_ws_config: Option<STTWebSocketConfig>,
    tts_ws_config: Option<TTSWebSocketConfig>,
    livekit_ws_config: Option<LiveKitWebSocketConfig>,
    dag_ws_config: Option<DAGWebSocketConfig>,
    conversation_ws_config: Option<ConversationWebSocketConfig>,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
    app_state: &Arc<AppState>,
) -> bool {
    // P2 (RC6): one config per session. A second config used to silently
    // REPLACE the VoiceManager without tearing the old one down — provider
    // streams and callback tasks leaked. Reject explicitly; reconfiguration
    // is a session restart.
    {
        let state_guard = state.read().await;
        if state_guard.voice_manager.is_some() || state_guard.stream_id.is_some() {
            warn!("Rejecting second config message on an already-configured session");
            let _ = message_tx
                .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                    message: "Session already configured — open a new connection to \
                              reconfigure (one config message per session)"
                        .to_string(),
                }))
                .await;
            return true;
        }
    }

    // Generate stream_id if not provided by client
    let stream_id = resolve_stream_id(stream_id);
    info!("Session stream_id: {}", stream_id);

    // Determine if audio processing is enabled (default to true)
    let audio_enabled = audio.unwrap_or(true);

    info!(
        "Configuring connection with audio_enabled: {}, LiveKit: {}",
        audio_enabled,
        livekit_ws_config.is_some()
    );

    // Validate required configurations when audio is enabled
    if audio_enabled && !validate_audio_configs(&stt_ws_config, &tts_ws_config, message_tx).await {
        return true;
    }

    // Store audio_enabled flag in connection state
    {
        let mut state_guard = state.write().await;
        state_guard.set_audio_enabled(audio_enabled);
        state_guard.stream_id = Some(stream_id.clone());
    }
    debug!(stream_id = %stream_id, "Stored stream_id in connection state");

    // Initialize voice manager if audio is enabled
    let voice_manager = if audio_enabled {
        match initialize_voice_manager(
            stt_ws_config.as_ref().unwrap(),
            tts_ws_config.as_ref().unwrap(),
            app_state,
            message_tx,
        )
        .await
        {
            Some(vm) => {
                // Store in connection state
                let mut state_guard = state.write().await;
                state_guard.voice_manager = Some(vm.clone());
                // D-G10: optional pipeline liveness heartbeat (env-gated, off
                // by default) probing audio-provider readiness; aborted when
                // the connection state drops.
                if let Some(period_ms) = std::env::var("WAAV_HEARTBEAT_PERIOD_MS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .filter(|v| *v > 0)
                {
                    use crate::core::observability::{
                        HeartbeatConfig, HeartbeatMonitor, LivenessProbe,
                    };
                    let vm_probe = vm.clone();
                    let probe: LivenessProbe = Arc::new(move || {
                        let vm = vm_probe.clone();
                        Box::pin(async move { vm.is_ready().await })
                    });
                    let cfg = HeartbeatConfig {
                        period: std::time::Duration::from_millis(period_ms),
                        timeout: std::time::Duration::from_secs(5),
                    };
                    state_guard.heartbeat = Some(HeartbeatMonitor::spawn(
                        cfg,
                        stream_id.clone(),
                        probe,
                    ));
                }
                Some(vm)
            }
            None => return true,
        }
    } else {
        info!("Audio processing disabled - skipping voice manager initialization");
        None
    };

    // C-G5: one egress-resampling context for the whole session (None when
    // the client didn't ask for a playback rate — zero cost).
    let egress_audio = EgressAudio::from_tts_config(tts_ws_config.as_ref());
    // E-G2: optional WS egress re-framer (None = legacy verbatim frames).
    let ws_chunker = WsEgressChunker::from_tts_config(tts_ws_config.as_ref());

    // Register early TTS callback for cached audio
    if let Some(ref vm) = voice_manager {
        register_early_tts_callback(vm, message_tx, egress_audio.clone(), ws_chunker.clone())
            .await;
        // E-G2: on barge-in clear, the chunker's carried remainder is STALE
        // bot audio — drop it. Raw-WS sessions have a free audio-clear slot;
        // LiveKit sessions overwrite this with their own clear callback
        // below (their egress path bypasses the chunker anyway).
        if let Some(chunker) = ws_chunker.clone()
            && livekit_ws_config.is_none()
        {
            let _ = vm
                .on_audio_clear(move || {
                    let chunker = Arc::clone(&chunker);
                    Box::pin(async move {
                        chunker.clear();
                    })
                })
                .await;
        }
    }

    // Initialize LiveKit client if configured
    let (livekit_client, livekit_room_name, waav_identity, waav_name) =
        if let Some(mut livekit_ws_config) = livekit_ws_config {
            // Normalize room name with auth prefix for tenant isolation
            {
                let state_guard = state.read().await;
                livekit_ws_config.room_name = state_guard
                    .auth
                    .normalize_room_name(&livekit_ws_config.room_name);
                info!(
                    "Normalized LiveKit room name to: {}",
                    livekit_ws_config.room_name
                );
            }

            // Get auth_id for tenant-scoped recording paths + the D-G4 task
            // tracker so the LiveKit audio forwarder is audited at teardown.
            let (auth_id, task_tracker) = {
                let state_guard = state.read().await;
                (state_guard.auth.id.clone(), state_guard.task_tracker.clone())
            };

            match initialize_livekit_client(
                livekit_ws_config,
                tts_ws_config.as_ref(),
                // INGRESS: deliver remote-participant audio at the rate the
                // STT pipeline declares downstream (16k default).
                stt_ws_config.as_ref().map(|s| s.sample_rate).unwrap_or(16_000),
                &app_state.config.livekit_url,
                voice_manager.as_ref(),
                message_tx,
                app_state.livekit_room_handler.as_ref(),
                &stream_id,
                auth_id.as_deref(),
                &task_tracker,
            )
            .await
            {
                Some((client, operation_queue, room_name, egress_id, identity, name)) => {
                    // Store in connection state
                    let mut state_guard = state.write().await;
                    state_guard.livekit_client = Some(client.clone());
                    state_guard.livekit_operation_queue = operation_queue;
                    state_guard.livekit_room_name = Some(room_name.clone());
                    state_guard.livekit_local_identity = Some(identity.clone());
                    state_guard.recording_egress_id = egress_id;
                    (Some(client), Some(room_name), Some(identity), Some(name))
                }
                None => return true,
            }
        } else {
            (None, None, None, None)
        };

    // Register final TTS callback with LiveKit routing
    if let Some(ref vm) = voice_manager {
        let operation_queue = if let Some(ref _client) = livekit_client {
            let state_guard = state.read().await;
            state_guard.livekit_operation_queue.clone()
        } else {
            None
        };
        register_final_tts_callback(
            vm,
            livekit_client.as_ref(),
            operation_queue.as_ref(),
            message_tx,
            egress_audio.clone(),
            ws_chunker.clone(),
        )
        .await;
        // Register the utterance-end flush when EITHER the resampler OR the
        // chunker holds a sub-chunk tail (review wc71hewlx #9: a chunker-only
        // session — audio_out_chunk_ms set, no client_playback_rate — left
        // its remainder unflushed, clipping tails + leaking carry across
        // utterances).
        if egress_audio.is_some() || ws_chunker.is_some() {
            register_tts_complete_with_flush(
                vm,
                livekit_client.as_ref(),
                operation_queue.as_ref(),
                message_tx,
                egress_audio.clone(),
                ws_chunker.clone(),
            )
            .await;
        }
    }

    // Initialize DAG routing if configured
    #[cfg(feature = "dag-routing")]
    let dag_enabled = if let Some(dag_config) = dag_ws_config {
        // The DAG data-plane reuses the same LiveKit delivery path as the simple path.
        let dag_operation_queue = if livekit_client.is_some() {
            let state_guard = state.read().await;
            state_guard.livekit_operation_queue.clone()
        } else {
            None
        };
        match initialize_dag_routing(
            &dag_config,
            &stream_id,
            state,
            voice_manager.as_ref(),
            livekit_client.as_ref(),
            dag_operation_queue.as_ref(),
            message_tx,
            app_state.core_state.profiler.clone(),
            egress_audio.clone(),
            Some(app_state.core_state.resilience().clone()),
        )
        .await
        {
            Ok(true) => {
                info!("DAG routing initialized for stream {}", stream_id);
                true
            }
            Ok(false) => false,
            Err(e) => {
                error!("DAG routing initialization failed: {}", e);
                let _ = message_tx
                    .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                        message: format!("DAG initialization failed: {}", e),
                    }))
                    .await;
                false
            }
        }
    } else {
        false
    };

    // Log DAG status (stub when feature disabled)
    #[cfg(not(feature = "dag-routing"))]
    let dag_enabled = {
        if dag_ws_config.is_some() {
            warn!(
                "DAG routing requested but feature not enabled. Build with --features dag-routing"
            );
            let _ = message_tx
                .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                    message: "DAG routing is not enabled. Build with --features dag-routing"
                        .to_string(),
                }))
                .await;
        }
        false
    };

    // Initialize the built-in conversation loop if configured (plan W-O2).
    //
    // Mutually exclusive with the DAG: the DAG owns the post-STT pipeline when
    // enabled, so we only wire the conversation orchestrator when DAG routing is
    // not active. When neither is set, the gateway keeps raw STT/TTS behavior.
    let conversation_enabled = if dag_enabled {
        if conversation_ws_config.is_some() {
            warn!(
                "Both dag_config and conversation_config provided; DAG takes precedence, \
                 conversation loop not started"
            );
        }
        false
    } else if let (Some(conv_config), Some(vm)) =
        (conversation_ws_config.as_ref(), voice_manager.as_ref())
    {
        match initialize_conversation_loop(conv_config, &stream_id, vm, message_tx).await {
            Ok(true) => {
                info!("Conversation loop initialized for stream {}", stream_id);
                emit_reasoning_config_warnings(conv_config, message_tx).await;
                true
            }
            Ok(false) => false,
            Err(e) => {
                error!("Conversation loop initialization failed: {}", e);
                let _ = message_tx
                    .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                        message: format!("Conversation loop initialization failed: {}", e),
                    }))
                    .await;
                false
            }
        }
    } else {
        if conversation_ws_config.is_some() && voice_manager.is_none() {
            warn!("conversation_config provided but audio disabled; conversation loop not started");
        }
        false
    };

    // Send ready message with optional LiveKit room information
    let _ = message_tx
        .send(MessageRoute::Outgoing(OutgoingMessage::Ready {
            protocol_version: crate::handlers::ws::messages::PROTOCOL_VERSION.to_string(),
            stream_id: stream_id.clone(),
            livekit_room_name: livekit_room_name.clone(),
            livekit_url: Some(app_state.config.livekit_public_url.clone()),
            waav_participant_identity: waav_identity,
            waav_participant_name: waav_name,
        }))
        .await;

    info!(
        stream_id = %stream_id,
        audio_enabled = audio_enabled,
        dag_enabled = dag_enabled,
        conversation_enabled = conversation_enabled,
        livekit = livekit_room_name.is_some(),
        "Connection configured and ready"
    );

    true
}

/// Initialize the built-in conversation loop for a session (plan W-O2).
///
/// Constructs a [`ConversationOrchestrator`] (validating the client-supplied LLM
/// `base_url` for SSRF) and registers it as the VoiceManager's STT-result
/// callback. The callback still forwards every transcript to the client (so
/// `stt_result` egress is preserved, matching the simple path), then drives the
/// LLM→TTS loop on each finalized turn. Barge-in and history are handled inside
/// D1: is this model id known to do extended reasoning by default (a poor choice
/// for the low-latency spoken path)? Single source of truth in the conversation
/// module (also drives D5's eager-disable).
fn is_known_reasoning_model(model: &str) -> bool {
    crate::core::conversation::is_reasoning_model(model)
}

/// D1 (REALTIME_REASONING.md §7.4): emit non-fatal config advisories — a
/// reasoning model on the spoken path (high TTFT) and an effort clamped up to an
/// adaptive-only model's floor. Active, in-band guidance instead of silent
/// Grafana-only signals; reuses the `record_degraded` idiom.
async fn emit_reasoning_config_warnings(
    conv_config: &ConversationWebSocketConfig,
    message_tx: &mpsc::Sender<MessageRoute>,
) {
    if is_known_reasoning_model(&conv_config.model) {
        crate::core::metrics::bridge::record_degraded(
            "reasoning_model_on_voice_path",
            "reasoning_model_id",
        );
        warn!(
            model = %conv_config.model,
            "a reasoning model is on the spoken path — first-audio latency will be seconds"
        );
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::ConfigWarning {
                code: "reasoning_model_on_voice_path".to_string(),
                message: format!(
                    "model '{}' is a reasoning model on the spoken path; first-audio latency \
                     will be seconds. Keep a FAST model on `model` (add a `reasoning_model` for \
                     two-tier when available), or lower `reasoning_effort`.",
                    conv_config.model
                ),
                detail: None,
            }))
            .await;
    }
    // D5: eager speculation is auto-disabled on a reasoning model (it would pay
    // full think-time per speculative fire) — tell the operator so it isn't a
    // silent no-op.
    if conv_config.eager_eot && is_known_reasoning_model(&conv_config.model) {
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::ConfigWarning {
                code: "eager_eot_disabled_for_reasoning_model".to_string(),
                message: format!(
                    "eager_eot is ignored for reasoning model '{}' — each speculative \
                     turn would pay full think-time and be discarded on resume. Use a \
                     fast `model` to benefit from eager.",
                    conv_config.model
                ),
                detail: None,
            }))
            .await;
    }
    // Floor-clamp echo: surface when the applied effort differs from the request
    // (an adaptive-only model can't go as low as asked).
    if let Some(req) = conv_config.reasoning_effort {
        let (applied, floor) = conv_config.to_conversation_config().resolved_reasoning_effort();
        if applied != Some(req) {
            let _ = message_tx
                .send(MessageRoute::Outgoing(OutgoingMessage::ConfigWarning {
                    code: "reasoning_effort_clamped".to_string(),
                    message: format!(
                        "model '{}' cannot honor reasoning_effort '{}'; clamped to its floor '{}'.",
                        conv_config.model,
                        req.as_str(),
                        floor.as_str()
                    ),
                    detail: Some(serde_json::json!({
                        "requested": req.as_str(),
                        "applied": applied.map(|e| e.as_str()),
                        "floor": floor.as_str(),
                    })),
                }))
                .await;
        }
    }
    // Reasoning-tier sub-knobs are INERT without a `reasoning_model`:
    // build_reasoning_tier returns None, so select_tier can never reach the
    // reasoning tier (route="always" silently runs the FAST model every turn).
    // Surface it instead of silently ignoring. (reasoning_effort is deliberately
    // excluded — it also tunes the fast tier, so it is NOT inert.)
    if conv_config.reasoning_model.is_none() {
        let mut inert: Vec<&str> = Vec::new();
        if conv_config.reasoning_route == crate::core::conversation::RoutingMode::Always {
            inert.push("reasoning_route");
        }
        if conv_config.reasoning_base_url.is_some() {
            inert.push("reasoning_base_url");
        }
        if conv_config.reasoning_api_key.is_some() {
            inert.push("reasoning_api_key");
        }
        if conv_config.reasoning_provider_kind.is_some() {
            inert.push("reasoning_provider_kind");
        }
        if conv_config.reasoning_budget_ms.is_some() {
            inert.push("reasoning_budget_ms");
        }
        if conv_config.max_reasoning_tokens.is_some() {
            inert.push("max_reasoning_tokens");
        }
        if !inert.is_empty() {
            warn!(fields = ?inert, "reasoning-tier fields set without reasoning_model — inert");
            let _ = message_tx
                .send(MessageRoute::Outgoing(OutgoingMessage::ConfigWarning {
                    code: "reasoning_tier_without_model".to_string(),
                    message: format!(
                        "these reasoning-tier fields are ignored because no `reasoning_model` is \
                         set, so every turn uses the fast `model`: {}. Set `reasoning_model` to \
                         enable the two-tier router.",
                        inert.join(", ")
                    ),
                    detail: Some(serde_json::json!({ "ignored_fields": inert })),
                }))
                .await;
        }
    }
}

/// the orchestrator. Returns `Ok(true)` when the loop is active.
async fn initialize_conversation_loop(
    conv_config: &ConversationWebSocketConfig,
    stream_id: &str,
    voice_manager: &Arc<VoiceManager>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> Result<bool, String> {
    let orchestrator = ConversationOrchestrator::new(
        stream_id.to_string(),
        conv_config.to_conversation_config(),
        voice_manager.clone(),
    )
    .map_err(|e| e.to_string())?;

    let orchestrator = Arc::new(orchestrator);

    // S3 (REALTIME_REASONING.md §5): wire the async-tool final sink so an async
    // tool's result is recorded to history and (turn-id-gated) volunteered.
    orchestrator.wire_async_sink();

    // P1.2b eager end-of-turn: a smart-turn PREDICTION (turn-complete before
    // the provider's speech_final) starts a held+staged speculative LLM turn.
    // A-G0: the per-session turn-policy controller. Policy improvements land
    // as strategy swaps here — never as new hardcoded branches. Start
    // strategy: the MinWords barge-in gate (A-G3) when configured, else the
    // legacy any-speech behavior. The bot-speaking truth comes from the
    // VoiceManager's live playout estimate (probe), not a stale flag.
    let start_strategy: Box<dyn crate::core::turn::UserTurnStartStrategy> =
        match conv_config.barge_in_min_words {
            Some(n) if n >= 1 => {
                Box::new(crate::core::turn::strategies::MinWordsStart::new(n))
            }
            _ => Box::new(crate::core::turn::strategies::AnySpeechStart),
        };
    // D-G3: a FATAL turn error (auth/config — every turn would fail
    // identically) surfaces to the client as a Critical error message.
    {
        let message_tx = message_tx.clone();
        orchestrator.set_fatal_handler(Arc::new(move |error: String| {
            let message_tx = message_tx.clone();
            tokio::spawn(async move {
                let _ = message_tx
                    .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                        message: format!(
                            "fatal provider error (session cannot recover): {error}"
                        ),
                    }))
                    .await;
            });
        }));
    }

    // A-G5: optional user-mute strategy (OR-combined in the controller;
    // user input drops while active, lifecycle signals always flow).
    // `first_bot_complete_latch` is the shared session-lifecycle flag the
    // greeting guard reads; flipped on the first TTS-complete below.
    let mut first_bot_complete_latch: Option<Arc<std::sync::atomic::AtomicBool>> = None;
    let mute_strategies: Vec<Box<dyn crate::core::turn::UserMuteStrategy>> =
        match conv_config.mute_strategy.as_deref() {
            Some("always_while_bot_speaks") => vec![Box::new(
                crate::core::turn::strategies::AlwaysMuteWhileBotSpeaks,
            )],
            Some("until_first_bot_complete") => {
                let (strategy, latch) =
                    crate::core::turn::strategies::MuteUntilFirstBotComplete::new();
                first_bot_complete_latch = Some(latch);
                vec![Box::new(strategy)]
            }
            Some("first_speech_only") => vec![Box::new(
                crate::core::turn::strategies::FirstSpeechMute::default(),
            )],
            Some(other) => {
                warn!(strategy = other, "unknown mute_strategy; muting disabled");
                vec![]
            }
            None => vec![],
        };

    // A-G5/wf_d43814c3 #4: the greeting-guard latch is flipped by the
    // orchestrator when the bot's FIRST spoken turn completes (a SILENTLY
    // listening user is then unmuted with no signal-sampling dependency).
    // NOTE: `until_first_bot_complete` is only meaningful when the bot speaks
    // proactively (greeting/disclaimer-on-join); without such an utterance
    // input stays guarded — the conservative disclaimer semantics.
    if let Some(latch) = first_bot_complete_latch.clone() {
        orchestrator.set_first_bot_complete_latch(latch);
    }
    let vm_probe = voice_manager.clone();
    let orch_probe = orchestrator.clone();
    let turn_controller = Arc::new(
        crate::core::turn::TurnController::new(
            vec![start_strategy],
            vec![
                Box::new(crate::core::turn::strategies::EagerSmartTurnSpeculate),
                Box::new(crate::core::turn::strategies::LegacySpeechFinalStop),
            ],
            mute_strategies,
        )
        // Bot-BUSY truth: audibly speaking OR an LLM turn in flight — the
        // TTFT thinking window must keep the MinWords gate up (a backchannel
        // canceling the in-flight turn is the exact failure A-G3 prevents).
        .with_bot_speaking_probe(move || {
            vm_probe.is_bot_speaking() || orch_probe.has_active_turn()
        }),
    );

    // Smart-turn verdicts feed the controller as an OPAQUE complete signal
    // (the TurnDecisionEngine stays the detector); a Speculate event drives
    // the eager path. No-op unless the conversation config opted in
    // (eager_eot) — that gate lives in trigger_eager_turn.
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    {
        let orch_eager = orchestrator.clone();
        let ctrl_eager = turn_controller.clone();
        if let Err(e) = voice_manager
            .on_smart_turn(move |result| {
                let orch = orch_eager.clone();
                let ctrl = ctrl_eager.clone();
                Box::pin(async move {
                    let events = ctrl.feed(&crate::core::turn::ControllerSignal::SmartTurn {
                        is_complete: result.is_turn_complete,
                    });
                    if !events.is_empty() {
                        orch.handle_turn_events(&events).await;
                    }
                })
            })
            .await
        {
            warn!("failed to register eager smart-turn callback: {e}");
        }
    }

    // Store on the connection so teardown can cancel any in-flight turn.
    // (Reuses the STT-callback slot in the VoiceManager; the orchestrator both
    // forwards the transcript and runs the loop, so there is no double delivery.)
    let message_tx_clone = message_tx.clone();
    let orch = orchestrator.clone();
    let ctrl = turn_controller.clone();
    let register_result = voice_manager
        .on_stt_result(move |stt: STTResult| {
            let message_tx = message_tx_clone.clone();
            let orch = orch.clone();
            let ctrl = ctrl.clone();
            Box::pin(async move {
                // Transcript egress (interim + final) FIRST, as a synchronous
                // side-effect of this callback — the actor-bug guardrail
                // (PIPECAT_FIX_PLAN §6.2 X3): the controller only decides
                // turn policy, it never owns transcript delivery.
                let _ = message_tx
                    .send(MessageRoute::Outgoing(OutgoingMessage::STTResult {
                        transcript: stt.transcript.clone(),
                        is_final: stt.is_final,
                        is_speech_final: stt.is_speech_final,
                        confidence: stt.confidence,
                        segment_transcript: stt.segment_transcript.clone(),
                    }))
                    .await;

                // Turn policy via the controller; the orchestrator acts on the
                // events from THIS task (caller-fires-callback preserved).
                let signal = if stt.is_final || stt.is_speech_final {
                    crate::core::turn::ControllerSignal::SttFinal {
                        // Turn policy sees the FULL segment; client egress
                        // above kept the raw fragment.
                        text: stt.turn_transcript().to_string(),
                        is_speech_final: stt.is_speech_final,
                        is_finalized: stt.is_finalized,
                    }
                } else {
                    crate::core::turn::ControllerSignal::SttInterim {
                        text: stt.transcript.clone(),
                        confidence: stt.confidence,
                    }
                };
                let events = ctrl.feed(&signal);
                if !events.is_empty() {
                    orch.handle_turn_events(&events).await;
                }
                // A-G7: any STT activity (and the turns it starts) re-arms
                // the idle re-engagement timer.
                orch.poke_idle_timer();
            })
        })
        .await;

    if let Err(e) = register_result {
        return Err(format!("failed to register conversation STT callback: {e}"));
    }

    Ok(true)
}

/// Validate audio configurations are present
async fn validate_audio_configs(
    stt_config: &Option<STTWebSocketConfig>,
    tts_config: &Option<TTSWebSocketConfig>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> bool {
    if stt_config.is_none() {
        let error_msg = "STT configuration is required when audio=true".to_string();
        error!("{}", error_msg);
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: error_msg,
            }))
            .await;
        return false;
    }

    if tts_config.is_none() {
        let error_msg = "TTS configuration is required when audio=true".to_string();
        error!("{}", error_msg);
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: error_msg,
            }))
            .await;
        return false;
    }

    true
}

/// Initialize voice manager with STT and TTS providers
async fn initialize_voice_manager(
    stt_ws_config: &STTWebSocketConfig,
    tts_ws_config: &TTSWebSocketConfig,
    app_state: &Arc<AppState>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> Option<Arc<VoiceManager>> {
    info!(
        "Initializing voice manager with STT provider: {} and TTS provider: {}",
        stt_ws_config.provider, tts_ws_config.provider
    );

    // Get API keys - prefer client-provided keys, fall back to server config
    let stt_api_key = if let Some(ref client_key) = stt_ws_config.api_key {
        if !client_key.is_empty() {
            info!(
                "Using client-provided API key for STT provider: {}",
                stt_ws_config.provider
            );
            client_key.clone()
        } else {
            match app_state.config.get_api_key(&stt_ws_config.provider) {
                Ok(key) => key,
                Err(error_msg) => {
                    error!("{}", error_msg);
                    let _ = message_tx
                        .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                            message: error_msg,
                        }))
                        .await;
                    return None;
                }
            }
        }
    } else {
        match app_state.config.get_api_key(&stt_ws_config.provider) {
            Ok(key) => key,
            Err(error_msg) => {
                error!("{}", error_msg);
                let _ = message_tx
                    .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                        message: error_msg,
                    }))
                    .await;
                return None;
            }
        }
    };

    let tts_api_key = if let Some(ref client_key) = tts_ws_config.api_key {
        if !client_key.is_empty() {
            info!(
                "Using client-provided API key for TTS provider: {}",
                tts_ws_config.provider
            );
            client_key.clone()
        } else {
            match app_state.config.get_api_key(&tts_ws_config.provider) {
                Ok(key) => key,
                Err(error_msg) => {
                    error!("{}", error_msg);
                    let _ = message_tx
                        .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                            message: error_msg,
                        }))
                        .await;
                    return None;
                }
            }
        }
    } else {
        match app_state.config.get_api_key(&tts_ws_config.provider) {
            Ok(key) => key,
            Err(error_msg) => {
                error!("{}", error_msg);
                let _ = message_tx
                    .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                        message: error_msg,
                    }))
                    .await;
                return None;
            }
        }
    };

    // Build standardized configs (W1 keystone): they carry the flat base PLUS advanced
    // features/extras the client supplied, so the VoiceManager builds providers via
    // `create_stt_standard`/`create_tts_standard` → `from_standard` instead of dropping features
    // on the flat factory path. The flat `stt_config`/`tts_config` are still derived (== the
    // standardized bases) for cache hashing and other flat consumers below.
    let standard_stt = stt_ws_config.to_standard_stt(stt_api_key);
    let standard_tts = tts_ws_config.to_standard_tts(tts_api_key);
    // The TTS cache key is computed HERE from the FULL standardized config — base + advanced
    // features + extras — before `standard_tts` is moved into the manager, so audio-changing
    // features (voice settings, emotion, ssml, seed, …) are part of the key and cannot collide.
    let tts_cfg_hash = compute_tts_config_hash(&standard_tts);

    // Create voice manager configuration with default speech final settings, routed through the
    // standardized keystone path, and attach the SHARED process-global resilience handles
    // (W-D2): the single reconnect governor + this STT provider's shared circuit breaker, both
    // owned once by CoreState. This is what makes storm control + provider-level tripping work
    // cross-session — every session of a provider now trips the same breaker and draws from the
    // one process-wide reconnect cap.
    let stt_provider = standard_stt.base.provider.clone();
    #[allow(unused_mut)]
    let mut voice_config = VoiceManagerConfig::from_standard(standard_stt, standard_tts)
        .with_resilience(app_state.core_state.resilience().handles_for(&stt_provider));

    // P1.2: ML turn detection, surfaced via the STANDARDIZED WS config —
    // provider-agnostic (runs on the gateway's frame path for every STT
    // provider). When the build lacks the features: LOUD degradation (RC5).
    if let Some(td) = &stt_ws_config.turn_detection
        && td.enabled
    {
        #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
        {
            // C-G5 ingress: the VoiceManager resamples the frame path to
            // 16 kHz BEFORE the VAD/smart-turn models, so the processor is
            // always constructed at 16 kHz — non-16k clients (8 kHz
            // telephony, 48 kHz WebRTC) no longer fail the processor's
            // config validation (the silero/vad rate cross-check).
            let mut st_cfg = crate::core::smart_turn::SmartTurnProcessorConfig::new()
                .with_sample_rate(16_000);
            if let Some(threshold) = td.threshold {
                st_cfg = st_cfg.with_threshold(threshold);
            }
            voice_config.smart_turn_config = Some(st_cfg);
            info!(
                threshold = ?td.threshold,
                eager = td.eager,
                "ML turn detection enabled for session"
            );
        }
        #[cfg(not(any(feature = "silero-vad", feature = "smart-turn")))]
        {
            tracing::warn!(
                "turn_detection requested but this build lacks the smart-turn/\
                 silero-vad features — session degrades to timer fallback"
            );
            crate::core::metrics::bridge::record_degraded(
                "turn_detection",
                "feature_not_built",
            );
        }
    }

    let turn_detector = app_state.core_state.get_turn_detector();

    // Create voice manager
    let voice_manager = match VoiceManager::new(voice_config, turn_detector) {
        Ok(vm) => Arc::new(vm),
        Err(e) => {
            error!("Failed to create voice manager: {}", e);
            let _ = message_tx
                .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                    message: format!("Failed to create voice manager: {e}"),
                }))
                .await;
            return None;
        }
    };

    // Live latency profiling: attach a per-session observer registry feeding the
    // process-wide LatencyProfiler hub (Prometheus waav_turn_*/waav_frame_* +
    // /debug/profile). Env-gated by WAAV_TURN_PROFILING (default on); when off,
    // no registry is attached and every hook site is a single cheap read.
    if app_state.core_state.profiler.is_enabled() {
        use crate::core::observability::{
            FrameProfiler, ObserverRegistry, TurnProfiler, TurnSink, UserBotLatencyObserver,
        };
        let hub = app_state.core_state.profiler.clone();
        let registry = Arc::new(ObserverRegistry::new());
        registry.register(Arc::new(TurnProfiler::new(
            uuid::Uuid::new_v4().to_string(),
            hub.clone() as Arc<dyn TurnSink>,
        )));
        registry.register(Arc::new(FrameProfiler::new(hub)));
        // Finally activates the long-dormant user↔bot latency observer too.
        registry.register(Arc::new(UserBotLatencyObserver::new(256)));
        voice_manager.set_observers(registry);
    }

    // Set up TTS cache (hash computed above from the full standardized config, incl. features/extras)
    if let Err(e) = voice_manager
        .set_tts_cache(app_state.cache(), Some(tts_cfg_hash))
        .await
    {
        error!("Failed to set TTS cache: {}", e);
    }

    // ORDERING CONTRACT (P0.5 / RC6): error callbacks are registered BEFORE
    // start() so a connect-time provider failure (e.g. a 401 during the STT
    // WebSocket handshake) reaches the client as its real cause instead of the
    // generic "Connection channel closed". Result/complete callbacks are also
    // registered up-front — providers must tolerate callbacks before connect
    // (they all store them; nothing fires until the stream runs).

    // Set up STT error callback - critical for propagating streaming errors
    if !register_stt_error_callback(&voice_manager, message_tx).await {
        return None;
    }

    // Set up TTS error callback
    if !register_tts_error_callback(&voice_manager, message_tx).await {
        return None;
    }

    // Set up STT result callback
    if !register_stt_callback(&voice_manager, message_tx).await {
        return None;
    }

    // Set up TTS completion callback
    if !register_tts_complete_callback(&voice_manager, message_tx).await {
        return None;
    }

    // Start voice manager (callbacks above are live for any connect-time error)
    if let Err(e) = voice_manager.start().await {
        error!("Failed to start voice manager: {}", e);
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: format!("Failed to start voice manager: {e}"),
            }))
            .await;
        return None;
    }

    // Wait for providers to be ready
    if !wait_for_providers_ready(&voice_manager, message_tx).await {
        return None;
    }

    Some(voice_manager)
}

/// Register STT result callback
async fn register_stt_callback(
    voice_manager: &Arc<VoiceManager>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> bool {
    let message_tx_clone = message_tx.clone();
    if let Err(e) = voice_manager
        .on_stt_result(move |result: STTResult| {
            let message_tx = message_tx_clone.clone();
            Box::pin(async move {
                let msg = OutgoingMessage::STTResult {
                    transcript: result.transcript,
                    is_final: result.is_final,
                    is_speech_final: result.is_speech_final,
                    confidence: result.confidence,
                    segment_transcript: result.segment_transcript,
                };
                let _ = message_tx.send(MessageRoute::Outgoing(msg)).await;
            })
        })
        .await
    {
        error!("Failed to set up STT callback: {}", e);
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: format!("Failed to set up STT callback: {e}"),
            }))
            .await;
        return false;
    }
    true
}

/// Register STT error callback to propagate streaming errors to clients
async fn register_stt_error_callback(
    voice_manager: &Arc<VoiceManager>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> bool {
    let message_tx_clone = message_tx.clone();
    if let Err(e) = voice_manager
        .on_stt_error(move |error| {
            let message_tx = message_tx_clone.clone();
            Box::pin(async move {
                let msg = OutgoingMessage::Error {
                    message: format!("STT streaming error: {error}"),
                };
                let _ = message_tx.send(MessageRoute::Outgoing(msg)).await;
            })
        })
        .await
    {
        error!("Failed to set up STT error callback: {}", e);
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: format!("Failed to set up STT error callback: {e}"),
            }))
            .await;
        return false;
    }
    true
}

/// Register TTS error callback
async fn register_tts_error_callback(
    voice_manager: &Arc<VoiceManager>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> bool {
    let message_tx_clone = message_tx.clone();
    if let Err(e) = voice_manager
        .on_tts_error(move |error| {
            let message_tx = message_tx_clone.clone();
            Box::pin(async move {
                let msg = OutgoingMessage::Error {
                    message: format!("TTS error: {error}"),
                };
                let _ = message_tx.send(MessageRoute::Outgoing(msg)).await;
            })
        })
        .await
    {
        error!("Failed to set up TTS error callback: {}", e);
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: format!("Failed to set up TTS error callback: {e}"),
            }))
            .await;
        return false;
    }
    true
}

/// Register TTS completion callback to send WebSocket notifications
///
/// This callback is invoked by the TTS provider's dispatcher after all audio
/// chunks for a given `speak()` command have been generated and sent.
///
/// # Arguments
/// * `voice_manager` - VoiceManager instance to register callback with
/// * `message_tx` - Channel for sending WebSocket messages
///
/// # Returns
/// * `bool` - true on success, false on error (triggers connection termination)
async fn register_tts_complete_callback(
    voice_manager: &Arc<VoiceManager>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> bool {
    let message_tx_clone = message_tx.clone();

    if let Err(e) = voice_manager
        .on_tts_complete(move || {
            let message_tx = message_tx_clone.clone();
            Box::pin(async move {
                // Calculate timestamp when completion occurred
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;

                // Send completion message to WebSocket client
                let msg = OutgoingMessage::TTSPlaybackComplete { timestamp };

                // Ignore send errors - client may have disconnected
                let _ = message_tx.send(MessageRoute::Outgoing(msg)).await;

                debug!(
                    "TTS playback completion event sent at timestamp {}",
                    timestamp
                );
            })
        })
        .await
    {
        error!("Failed to set up TTS completion callback: {}", e);
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: format!("Failed to set up TTS completion callback: {e}"),
            }))
            .await;
        return false;
    }

    true
}

/// Wait for voice providers to become ready
async fn wait_for_providers_ready(
    voice_manager: &Arc<VoiceManager>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> bool {
    let ready_timeout = Duration::from_secs(PROVIDER_READY_TIMEOUT_SECS);
    let ready_check = async {
        while !voice_manager.is_ready().await {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };

    if timeout(ready_timeout, ready_check).await.is_err() {
        error!("Timeout waiting for voice providers to be ready");
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: "Timeout waiting for voice providers to be ready".to_string(),
            }))
            .await;
        return false;
    }

    true
}

/// Register early TTS audio callback for cached audio
async fn register_early_tts_callback(
    voice_manager: &Arc<VoiceManager>,
    message_tx: &mpsc::Sender<MessageRoute>,
    egress_audio: Option<Arc<EgressAudio>>,
    ws_chunker: Option<Arc<WsEgressChunker>>,
) {
    let message_tx_for_early_tts = message_tx.clone();

    if let Err(e) = voice_manager
        .on_tts_audio(move |audio_data: AudioData| {
            let message_tx = message_tx_for_early_tts.clone();
            let egress_audio = egress_audio.clone();
            let ws_chunker = ws_chunker.clone();

            Box::pin(async move {
                debug!(
                    "Early TTS audio callback triggered: {} bytes",
                    audio_data.data.len()
                );

                // C-G5: client playback rate (passthrough when unset);
                // 0-byte frames (sub-chunk buffering) are never sent.
                let data = match &egress_audio {
                    Some(e) => {
                        e.convert(audio_data.data, &audio_data.format, audio_data.sample_rate)
                    }
                    None => audio_data.data,
                };
                if data.is_empty() {
                    return;
                }

                // E-G2: re-frame for tight barge-in; legacy verbatim when
                // unset. Send audio as binary data to WebSocket with the
                // DroppableAudio policy (RC8): a slow client sheds stale
                // frames instead of stalling the voice path.
                let frames: Vec<Bytes> = match &ws_chunker {
                    Some(c) => c.push(&data),
                    None => vec![Bytes::from(data)],
                };
                for frame in frames {
                    crate::handlers::ws::messages::send_with_policy(
                        &message_tx,
                        MessageRoute::Binary(frame),
                        crate::handlers::ws::messages::MessageClass::DroppableAudio,
                    )
                    .await;
                }
            })
        })
        .await
    {
        warn!("Failed to register early TTS audio callback: {:?}", e);
    }
}

/// C-G5 pt3: per-session egress resampling context — built once when the
/// client configured `client_playback_rate`; shared by EVERY TTS delivery
/// path (early callback, final callback, DAG audio drain) so the resampler's
/// filter state stays continuous across the session's single egress stream.
pub(crate) struct EgressAudio {
    target_rate: u32,
    provider_rate: u32,
    resampler: parking_lot::Mutex<crate::core::audio::StreamResampler>,
}

/// Sanity bounds for a client-requested playback rate: 0 would configure a
/// 0 Hz LiveKit track; absurd rates build pathological resamplers (review
/// wf_85659e16 — zero validation). 8 kHz (telephony) to 192 kHz (hi-res).
pub(crate) fn valid_playback_rate(rate: u32) -> bool {
    (8_000..=192_000).contains(&rate)
}

/// E-G2: per-session WS egress re-framer — bounds the barge-in residual to
/// one chunk on the RAW-WS path (LiveKit already frames internally).
pub(crate) struct WsEgressChunker {
    inner: parking_lot::Mutex<crate::core::audio::OutputChunker>,
}

impl WsEgressChunker {
    fn from_tts_config(tts: Option<&TTSWebSocketConfig>) -> Option<Arc<Self>> {
        let tts = tts?;
        let chunk_ms = tts.audio_out_chunk_ms?;
        if chunk_ms == 0 || chunk_ms > 1000 {
            warn!(chunk_ms, "audio_out_chunk_ms outside (0,1000]; WS chunking disabled");
            return None;
        }
        // The chunk size follows the rate of the bytes actually delivered:
        // the client playback rate when egress resampling is on, else the
        // provider rate.
        let rate = tts
            .client_playback_rate
            .filter(|r| valid_playback_rate(*r))
            .or(tts.sample_rate)
            .unwrap_or(24_000);
        Some(Arc::new(Self {
            inner: parking_lot::Mutex::new(crate::core::audio::OutputChunker::new(
                chunk_ms, rate,
            )),
        }))
    }

    fn push(&self, data: &[u8]) -> Vec<bytes::Bytes> {
        self.inner.lock().push(data)
    }

    fn flush(&self) -> Option<bytes::Bytes> {
        self.inner.lock().flush_remainder()
    }

    pub(crate) fn clear(&self) {
        self.inner.lock().clear();
    }
}

impl EgressAudio {
    /// `None` unless the session asked for a VALID client playback rate
    /// (invalid values are rejected loudly, not half-honored).
    pub(crate) fn from_tts_config(tts: Option<&TTSWebSocketConfig>) -> Option<Arc<Self>> {
        let tts = tts?;
        let target_rate = tts.client_playback_rate?;
        if !valid_playback_rate(target_rate) {
            warn!(
                target_rate,
                "client_playback_rate outside the sane range (8000-192000 Hz);                  egress resampling DISABLED for this session"
            );
            return None;
        }
        Some(Arc::new(Self {
            target_rate,
            provider_rate: tts.sample_rate.unwrap_or(24000),
            resampler: parking_lot::Mutex::new(crate::core::audio::StreamResampler::new()),
        }))
    }

    /// Flush the buffered resampler tail at an utterance boundary (TTS
    /// complete). Empty when nothing is pending (review wf_85659e16 #8/#11:
    /// without this the END of every bot utterance was dropped).
    pub(crate) fn flush(&self) -> Vec<u8> {
        let mut r = self.resampler.lock();
        crate::core::audio::resampler::flush_pcm16(&mut r).unwrap_or_default()
    }

    /// Convert one chunk to the client rate; passthrough whenever the
    /// standardized seam says no work applies (non-PCM, identity, unknown
    /// input rate).
    pub(crate) fn convert(&self, data: Vec<u8>, format: &str, chunk_rate: u32) -> Vec<u8> {
        let mut r = self.resampler.lock();
        crate::core::audio::egress_to_client_rate(
            &mut r,
            &data,
            format,
            chunk_rate,
            self.provider_rate,
            self.target_rate,
        )
        .unwrap_or(data)
    }
}

/// Deliver a single TTS audio buffer to the client: LiveKit when connected
/// (preferring the non-blocking operation queue), else the WS binary sink.
///
/// Extracted from `register_final_tts_callback` so the DAG data-plane drain task
/// (W-O1) reuses exactly the same delivery path as the simple STT→TTS path —
/// `DagOutput::Audio` produced by a DAG `audio_output` node lands here.
async fn deliver_tts_audio(
    audio: Vec<u8>,
    livekit_client: Option<&Arc<RwLock<LiveKitClient>>>,
    operation_queue: Option<&crate::livekit::OperationQueue>,
    message_tx: &mpsc::Sender<MessageRoute>,
    ws_chunker: Option<&Arc<WsEgressChunker>>,
) {
    let mut sent_to_livekit = false;

    // Try to send to LiveKit using the operation queue if available
    if let Some(queue) = operation_queue {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if queue
            .queue(crate::livekit::LiveKitOperation::SendAudio {
                audio_data: audio.clone(),
                response_tx: tx,
            })
            .await
            .is_ok()
        {
            match rx.await {
                Ok(Ok(())) => {
                    debug!(
                        "TTS audio successfully sent to LiveKit via queue: {} bytes",
                        audio.len()
                    );
                    sent_to_livekit = true;
                }
                Ok(Err(e)) => {
                    error!("Failed to send TTS audio to LiveKit: {:?}", e);
                }
                Err(_) => {
                    error!("Operation worker disconnected while sending TTS audio");
                }
            }
        }
    } else if let Some(livekit_client_arc) = livekit_client {
        // Fallback to lock-based approach
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(LIVEKIT_LOCK_TIMEOUT_MS),
            livekit_client_arc.write(),
        )
        .await
        {
            Ok(client) => {
                if client.is_connected() {
                    match client.send_tts_audio(audio.clone()).await {
                        Ok(()) => {
                            debug!(
                                "TTS audio successfully sent to LiveKit: {} bytes",
                                audio.len()
                            );
                            sent_to_livekit = true;
                        }
                        Err(e) => {
                            error!("Failed to send TTS audio to LiveKit: {:?}", e);
                        }
                    }
                } else {
                    debug!("LiveKit client not connected, falling back to WebSocket");
                }
            }
            Err(_) => {
                error!(
                    "Failed to acquire LiveKit lock within {}ms timeout, falling back to WebSocket",
                    LIVEKIT_LOCK_TIMEOUT_MS
                );
            }
        }
    }

    // Fall back to WebSocket if LiveKit is not available or failed
    if !sent_to_livekit {
        debug!("Sending TTS audio to WebSocket client: {} bytes", audio.len());
        // E-G2: re-frame on the RAW-WS path only — LiveKit frames at 10ms
        // internally and clear_buffer flushes its queue.
        let frames: Vec<Bytes> = match ws_chunker {
            Some(c) => c.push(&audio),
            None => vec![Bytes::from(audio)],
        };
        for frame in frames {
            crate::handlers::ws::messages::send_with_policy(
                message_tx,
                MessageRoute::Binary(frame),
                crate::handlers::ws::messages::MessageClass::DroppableAudio,
            )
            .await;
        }
    }
}

/// Re-register the TTS completion callback with egress-flush semantics
/// (C-G5 + review wf_85659e16 #8/#11): at utterance end the resampler holds
/// up to one chunk of un-emitted tail — deliver it BEFORE the completion
/// event, through the same delivery path as every other chunk. Replaces the
/// early plain registration (single callback slot) only when egress
/// resampling is active.
async fn register_tts_complete_with_flush(
    voice_manager: &Arc<VoiceManager>,
    livekit_client: Option<&Arc<RwLock<LiveKitClient>>>,
    operation_queue: Option<&crate::livekit::OperationQueue>,
    message_tx: &mpsc::Sender<MessageRoute>,
    egress_audio: Option<Arc<EgressAudio>>,
    ws_chunker: Option<Arc<WsEgressChunker>>,
) {
    let message_tx = message_tx.clone();
    let livekit_client = livekit_client.cloned();
    let operation_queue = operation_queue.cloned();
    if let Err(e) = voice_manager
        .on_tts_complete(move || {
            let message_tx = message_tx.clone();
            let livekit_client = livekit_client.clone();
            let operation_queue = operation_queue.clone();
            let egress_audio = egress_audio.clone();
            let ws_chunker = ws_chunker.clone();
            Box::pin(async move {
                // The resampler tail (when egress resampling is on) flows
                // through the chunker like every other chunk.
                let tail = egress_audio.as_ref().map(|e| e.flush()).unwrap_or_default();
                if !tail.is_empty() {
                    deliver_tts_audio(
                        tail,
                        livekit_client.as_ref(),
                        operation_queue.as_ref(),
                        &message_tx,
                        ws_chunker.as_ref(),
                    )
                    .await;
                }
                // E-G2: the chunker's sub-chunk remainder is REAL audio at
                // utterance end — deliver it before the completion event.
                if let Some(c) = &ws_chunker
                    && let Some(rem) = c.flush()
                {
                    crate::handlers::ws::messages::send_with_policy(
                        &message_tx,
                        MessageRoute::Binary(rem),
                        crate::handlers::ws::messages::MessageClass::DroppableAudio,
                    )
                    .await;
                }
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let _ = message_tx
                    .send(MessageRoute::Outgoing(OutgoingMessage::TTSPlaybackComplete {
                        timestamp,
                    }))
                    .await;
            })
        })
        .await
    {
        error!("Failed to re-register flush-aware TTS completion callback: {e}");
    }
}

/// Register final TTS audio callback with LiveKit routing
async fn register_final_tts_callback(
    voice_manager: &Arc<VoiceManager>,
    livekit_client: Option<&Arc<RwLock<LiveKitClient>>>,
    operation_queue: Option<&crate::livekit::OperationQueue>,
    message_tx: &mpsc::Sender<MessageRoute>,
    egress_audio: Option<Arc<EgressAudio>>,
    ws_chunker: Option<Arc<WsEgressChunker>>,
) {
    let message_tx_for_tts = message_tx.clone();
    let livekit_client_for_tts = livekit_client.cloned();
    let operation_queue_for_tts = operation_queue.cloned();

    if let Err(e) = voice_manager
        .on_tts_audio(move |audio_data: AudioData| {
            let message_tx = message_tx_for_tts.clone();
            let livekit_client = livekit_client_for_tts.clone();
            let operation_queue = operation_queue_for_tts.clone();
            let egress_audio = egress_audio.clone();
            let ws_chunker = ws_chunker.clone();

            Box::pin(async move {
                // C-G5: client playback rate (passthrough when unset). A
                // sub-chunk input may convert to NOTHING yet (buffered in
                // the resampler; emitted with a later chunk or the
                // utterance-end flush) — never deliver a 0-byte frame.
                let data = match &egress_audio {
                    Some(e) => {
                        e.convert(audio_data.data, &audio_data.format, audio_data.sample_rate)
                    }
                    None => audio_data.data,
                };
                if data.is_empty() {
                    return;
                }
                deliver_tts_audio(
                    data,
                    livekit_client.as_ref(),
                    operation_queue.as_ref(),
                    &message_tx,
                    ws_chunker.as_ref(),
                )
                .await;
            })
        })
        .await
    {
        error!("Failed to set up TTS audio callback: {}", e);
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: format!("Failed to set up TTS audio callback: {e}"),
            }))
            .await;
    }
}

/// Initialize LiveKit client and set up callbacks
#[allow(clippy::too_many_arguments)]
async fn initialize_livekit_client(
    livekit_ws_config: LiveKitWebSocketConfig,
    tts_config: Option<&TTSWebSocketConfig>,
    ingress_sample_rate: u32,
    livekit_url: &str,
    voice_manager: Option<&Arc<VoiceManager>>,
    message_tx: &mpsc::Sender<MessageRoute>,
    room_handler: Option<&Arc<crate::livekit::room_handler::LiveKitRoomHandler>>,
    stream_id: &str,
    auth_id: Option<&str>,
    task_tracker: &Arc<crate::core::observability::SessionTaskTracker>,
) -> Option<(
    Arc<RwLock<LiveKitClient>>,
    Option<crate::livekit::OperationQueue>,
    String,         // Room name for client info
    Option<String>, // Egress ID for recording cleanup
    String,         // WaaV participant identity
    String,         // WaaV participant name
)> {
    info!(
        "Setting up LiveKit client with URL: {}, room: {}",
        livekit_url, livekit_ws_config.room_name
    );

    // Get room handler or return error
    let room_handler = match room_handler {
        Some(handler) => handler,
        None => {
            error!("LiveKit room handler not initialized. Check API keys configuration.");
            let _ = message_tx
                .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                    message: "LiveKit room handler not initialized. Check API keys configuration."
                        .to_string(),
                }))
                .await;
            return None;
        }
    };

    // Create the room
    if let Err(e) = room_handler.create_room(&livekit_ws_config.room_name).await {
        error!("Failed to create LiveKit room: {:?}", e);
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: format!("Failed to create LiveKit room: {e:?}"),
            }))
            .await;
        return None;
    }

    info!(
        "LiveKit room '{}' created successfully",
        livekit_ws_config.room_name
    );

    // Get participant identity and name with defaults
    let waav_identity = livekit_ws_config
        .waav_participant_identity
        .as_deref()
        .unwrap_or("waav-ai");
    let waav_name = livekit_ws_config
        .waav_participant_name
        .as_deref()
        .unwrap_or("WaaV AI");

    // Generate agent token for AI participant with custom identity and name
    let agent_token = match room_handler.agent_token_with_sip_admin(
        &livekit_ws_config.room_name,
        waav_identity,
        waav_name,
    ) {
        Ok(token) => token,
        Err(e) => {
            error!("Failed to generate agent token: {:?}", e);
            let _ = message_tx
                .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                    message: format!("Failed to generate agent token: {e:?}"),
                }))
                .await;
            return None;
        }
    };

    info!("LiveKit agent token generated successfully");
    info!("Enabling recording: {}", livekit_ws_config.enable_recording);

    // Start recording if requested
    let egress_id = if livekit_ws_config.enable_recording {
        match room_handler
            .setup_room_recording(&livekit_ws_config.room_name, auth_id, stream_id)
            .await
        {
            Ok(id) => {
                info!(
                    "Room recording started with egress ID: {} for stream: {} auth_id: {:?}",
                    id, stream_id, auth_id
                );
                Some(id)
            }
            Err(e) => {
                error!("Failed to start room recording: {:?}", e);
                // Continue without recording - not a critical error
                None
            }
        }
    } else {
        None
    };

    // Use default TTS config for LiveKit audio parameters when TTS is not configured
    let default_tts_config = TTSWebSocketConfig {
        provider: "deepgram".to_string(),
        voice_id: None,
        speaking_rate: None,
        audio_format: None,
        sample_rate: Some(24000), // Default sample rate for LiveKit
        client_playback_rate: None,
            audio_out_chunk_ms: None,
        connection_timeout: None,
        request_timeout: None,
        model: "".to_string(),
        pronunciations: Vec::new(),
        api_key: None, // No client-provided key for default config
        emotion: None,
        emotion_intensity: None,
        delivery_style: None,
        emotion_description: None,
        features: Default::default(),
        extras: Default::default(),
    };

    let tts_config_for_livekit = tts_config.unwrap_or(&default_tts_config);
    let livekit_config =
        livekit_ws_config.to_livekit_config(
            agent_token,
            tts_config_for_livekit,
            ingress_sample_rate,
            livekit_url,
        );
    let mut livekit_client = LiveKitClient::new(livekit_config);

    // Set up audio callback to forward to STT processing
    if let Some(vm) = voice_manager {
        setup_livekit_audio_callback(&mut livekit_client, vm, message_tx, task_tracker);
    }

    // Set up data callback
    setup_livekit_data_callback(&mut livekit_client, message_tx);

    // Set up participant disconnect callback
    setup_livekit_disconnect_callback(&mut livekit_client, message_tx);

    // Connect to LiveKit room
    if let Err(e) = livekit_client.connect().await {
        error!("Failed to connect to LiveKit room: {:?}", e);
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: format!("Failed to connect to LiveKit room: {e:?}"),
            }))
            .await;
        return None;
    }

    // Wait for audio source to be available
    wait_for_livekit_audio(&livekit_client).await;

    // Get the operation queue for non-blocking operations
    let operation_queue = livekit_client.get_operation_queue();

    let livekit_client_arc = Arc::new(RwLock::new(livekit_client));

    // Register audio clear callback with VoiceManager
    if let Some(vm) = voice_manager {
        register_audio_clear_callback(vm, &livekit_client_arc, operation_queue.as_ref()).await;
    }

    info!("LiveKit client connected and ready");
    Some((
        livekit_client_arc,
        operation_queue,
        livekit_ws_config.room_name.clone(),
        egress_id,
        waav_identity.to_string(),
        waav_name.to_string(),
    ))
}

/// Set up LiveKit audio callback to forward audio to STT.
///
/// ORDERED single-consumer design (review wf_85659e16 #4): a spawned task
/// PER FRAME ran `receive_audio` concurrently on different workers — frames
/// could reorder or interleave, scrambling the ingress resampler's filter
/// state (time-swapped audio into VAD/smart-turn) and even the STT byte
/// stream itself. One bounded channel + one forwarder task preserves frame
/// order while keeping the LiveKit callback non-blocking; when the consumer
/// can't keep up the NEWEST frame is shed (the channel already holds older,
/// contiguous audio — dropping the head would tear the stream mid-utterance)
/// with a rate-limited warning.
fn setup_livekit_audio_callback(
    livekit_client: &mut LiveKitClient,
    voice_manager: &Arc<VoiceManager>,
    message_tx: &mpsc::Sender<MessageRoute>,
    task_tracker: &Arc<crate::core::observability::SessionTaskTracker>,
) {
    let (frame_tx, mut frame_rx) = mpsc::channel::<Vec<u8>>(64);

    let voice_manager = voice_manager.clone();
    let message_tx = message_tx.clone();
    // D-G4: this forwarder runs for the session lifetime. Its `frame_tx` lives
    // inside the LiveKit client's audio callback, which is NOT dropped before
    // teardown's audit, so the loop is stopped by the audit's abort (it cancels
    // cleanly at the recv await point — not counted dangling). Track it so that
    // abort happens and a genuinely-wedged forwarder would surface.
    let handle = tokio::spawn(async move {
        while let Some(audio_data) = frame_rx.recv().await {
            debug!("Received LiveKit audio: {} bytes", audio_data.len());
            if let Err(e) = voice_manager.receive_audio(audio_data.into()).await {
                error!("Failed to process LiveKit audio: {:?}", e);
                let _ = message_tx
                    .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                        message: format!("Failed to process LiveKit audio: {e:?}"),
                    }))
                    .await;
            }
        }
    });
    task_tracker.track("livekit-audio-forwarder", handle);

    let dropped = std::sync::atomic::AtomicUsize::new(0);
    livekit_client.set_audio_callback(move |audio_data: Vec<u8>| {
        if let Err(mpsc::error::TrySendError::Full(_)) = frame_tx.try_send(audio_data) {
            // Consumer behind by >64 frames (~640ms at 10ms frames): shed
            // the NEW frame (the channel already holds older, more
            // contiguous audio) and let the pipeline catch up. Warn once
            // per 100 sheds — per-frame warns flood at frame rate.
            let n = dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n.is_multiple_of(100) {
                warn!(total_dropped = n + 1, "LiveKit ingress queue full; shedding frames");
            }
        }
    });
}

/// Set up LiveKit data callback to forward messages
fn setup_livekit_data_callback(
    livekit_client: &mut LiveKitClient,
    message_tx: &mpsc::Sender<MessageRoute>,
) {
    let message_tx_clone = message_tx.clone();

    livekit_client.set_data_callback(move |data_message| {
        let message_tx = message_tx_clone.clone();

        // Spawn task for async send to ensure delivery
        tokio::spawn(async move {
            debug!(
                "Received LiveKit data from {}: {} bytes",
                data_message.participant_identity,
                data_message.data.len()
            );

            // Create unified message structure for LiveKit data
            // Extract fields upfront to avoid borrow checker issues with if/else branches
            let topic = data_message.topic.unwrap_or_else(|| "default".to_string());
            let identity = data_message.participant_identity;
            let room_name = data_message.room_name;
            let timestamp = data_message.timestamp;

            let unified_message =
                if let Ok(text_message) = String::from_utf8(data_message.data.clone()) {
                    // Data can be decoded as UTF-8 text
                    UnifiedMessage {
                        message: Some(text_message),
                        data: None,
                        identity,
                        topic,
                        room: room_name,
                        timestamp,
                    }
                } else {
                    // Binary data - encode as base64
                    UnifiedMessage {
                        message: None,
                        data: Some(general_purpose::STANDARD.encode(&data_message.data)),
                        identity,
                        topic,
                        room: room_name,
                        timestamp,
                    }
                };

            let outgoing_msg = OutgoingMessage::Message {
                message: unified_message,
            };

            // Use send for guaranteed delivery
            let _ = message_tx.send(MessageRoute::Outgoing(outgoing_msg)).await;
        });
    });
}

/// Set up LiveKit participant disconnect callback
fn setup_livekit_disconnect_callback(
    livekit_client: &mut LiveKitClient,
    message_tx: &mpsc::Sender<MessageRoute>,
) {
    let message_tx_clone = message_tx.clone();

    livekit_client.set_participant_disconnect_callback(move |disconnect_event| {
        let message_tx = message_tx_clone.clone();

        // Spawn task for async send to ensure delivery
        tokio::spawn(async move {
            info!(
                "Participant {} disconnected from LiveKit room {} - closing WebSocket connection",
                disconnect_event.participant_identity, disconnect_event.room_name
            );

            // Create participant disconnected info for WebSocket client
            let participant_info = ParticipantDisconnectedInfo {
                identity: disconnect_event.participant_identity,
                name: disconnect_event.participant_name,
                room: disconnect_event.room_name,
                timestamp: disconnect_event.timestamp,
            };

            let outgoing_msg = OutgoingMessage::ParticipantDisconnected {
                participant: participant_info,
            };

            // Send notification about participant disconnection
            let _ = message_tx.send(MessageRoute::Outgoing(outgoing_msg)).await;

            // Give a brief moment for the message to be sent
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // Close the WebSocket connection to trigger cleanup
            let _ = message_tx.send(MessageRoute::Close).await;
        });
    });
}

/// Wait for LiveKit audio source to become available
async fn wait_for_livekit_audio(livekit_client: &LiveKitClient) {
    let mut wait_count = 0;

    while wait_count * LIVEKIT_POLL_INTERVAL_MS < LIVEKIT_AUDIO_WAIT_MS {
        // Check if connected and audio source is available
        if livekit_client.is_connected() && livekit_client.has_audio_source() {
            info!(
                "LiveKit audio source is ready after {}ms",
                wait_count * LIVEKIT_POLL_INTERVAL_MS
            );
            break;
        }

        tokio::time::sleep(Duration::from_millis(LIVEKIT_POLL_INTERVAL_MS)).await;
        wait_count += 1;
    }

    // Final check
    if !livekit_client.is_connected() || !livekit_client.has_audio_source() {
        warn!(
            "LiveKit connected but audio source not available after {}ms wait",
            LIVEKIT_AUDIO_WAIT_MS
        );
        // Continue anyway - audio will be routed through WebSocket as fallback
    }
}

/// Register audio clear callback with VoiceManager for LiveKit integration
async fn register_audio_clear_callback(
    voice_manager: &Arc<VoiceManager>,
    livekit_client: &Arc<RwLock<LiveKitClient>>,
    operation_queue: Option<&crate::livekit::OperationQueue>,
) {
    if let Some(queue) = operation_queue {
        // Use operation queue for non-blocking clear
        let queue_clone = queue.clone();

        if let Err(e) = voice_manager
            .on_audio_clear(move || {
                let queue = queue_clone.clone();
                Box::pin(async move {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    if let Err(e) = queue
                        .queue(crate::livekit::LiveKitOperation::ClearAudio { response_tx: tx })
                        .await
                    {
                        warn!("Failed to queue clear audio operation: {:?}", e);
                    } else {
                        // Wait for the operation to complete
                        match rx.await {
                            Ok(Ok(())) => {
                                debug!("Cleared LiveKit audio buffer during interruption");
                            }
                            Ok(Err(e)) => {
                                warn!("Failed to clear LiveKit audio buffer: {:?}", e);
                            }
                            Err(_) => {
                                warn!("Operation worker disconnected during audio clear");
                            }
                        }
                    }
                })
            })
            .await
        {
            warn!("Failed to register audio clear callback: {:?}", e);
        }
    } else {
        // Fallback to lock-based approach
        let livekit_client_clone = livekit_client.clone();

        if let Err(e) = voice_manager
            .on_audio_clear(move || {
                let livekit = livekit_client_clone.clone();
                Box::pin(async move {
                    // Use try_write to avoid blocking in callback
                    // If we can't get the lock, it's okay - audio will be cleared eventually
                    match livekit.try_write() {
                        Ok(client) => {
                            if let Err(e) = client.clear_audio().await {
                                warn!("Failed to clear LiveKit audio buffer: {:?}", e);
                            } else {
                                debug!("Cleared LiveKit audio buffer during interruption");
                            }
                        }
                        Err(_) => {
                            debug!("LiveKit client busy during audio clear - skipping");
                        }
                    }
                })
            })
            .await
        {
            warn!("Failed to register audio clear callback: {:?}", e);
        }
    }
}

/// Initialize DAG routing for the connection
///
/// Compiles a DAG from template or inline definition and sets up the executor.
/// The DAG will route audio through its nodes instead of direct STT→TTS flow.
///
/// # Arguments
/// * `dag_config` - DAG configuration from WebSocket message
/// * `stream_id` - Session identifier for the DAG context
/// * `state` - Connection state to store compiled DAG
/// * `message_tx` - Channel for sending error messages
///
/// # Returns
/// * `Ok(true)` - DAG successfully initialized and enabled
/// * `Ok(false)` - DAG not configured or disabled
/// * `Err(String)` - DAG initialization failed
#[cfg(feature = "dag-routing")]
#[allow(clippy::too_many_arguments)]
async fn initialize_dag_routing(
    dag_config: &DAGWebSocketConfig,
    stream_id: &str,
    state: &Arc<RwLock<ConnectionState>>,
    voice_manager: Option<&Arc<VoiceManager>>,
    livekit_client: Option<&Arc<RwLock<LiveKitClient>>>,
    operation_queue: Option<&crate::livekit::OperationQueue>,
    message_tx: &mpsc::Sender<MessageRoute>,
    profiler: Arc<crate::core::observability::LatencyProfiler>,
    egress_audio: Option<Arc<EgressAudio>>,
    resilience: Option<Arc<crate::core::resilience::ResilienceRegistry>>,
) -> Result<bool, String> {
    // Get DAG definition from template or inline
    let dag_definition: DAGDefinition = if let Some(ref def) = dag_config.definition {
        // Parse inline definition
        serde_json::from_value(def.clone()).map_err(|e| format!("Invalid DAG definition: {}", e))?
    } else if let Some(ref template_name) = dag_config.template {
        // Load from template registry
        let templates = global_templates();
        templates
            .get(template_name)
            .ok_or_else(|| format!("DAG template '{}' not found", template_name))?
    } else {
        // No DAG specified
        return Ok(false);
    };

    info!(
        dag_id = %dag_definition.id,
        dag_name = %dag_definition.name,
        nodes = dag_definition.nodes.len(),
        edges = dag_definition.edges.len(),
        "Compiling DAG for session"
    );

    // Compile the DAG
    let compiler = DAGCompiler::new();
    let compiled_dag = compiler
        .compile(dag_definition)
        .map_err(|e| format!("DAG compilation failed: {}", e))?;

    let compiled_dag = Arc::new(compiled_dag);

    // Determine the "post-STT node": the node directly downstream of the STT provider
    // node. The StreamDriver injects the finalized transcript there and runs the rest
    // of the graph once (STT itself runs in the VoiceManager, not the DAG). If the DAG
    // has no STT provider node, fall back to the entry node.
    let post_stt_node_id = find_post_stt_node(&compiled_dag);

    // Create DAG context with auth info from connection state.
    let dag_context = {
        let conn_state = state.read().await;
        DAGContext::with_auth(
            stream_id.to_string(),
            None, // API key is not stored in Auth context (only in initial request)
            conn_state.auth.id.clone(),
        )
    };
    let dag_context = if let Some(timeout_ms) = dag_config.timeout_ms {
        dag_context.with_timeout(std::time::Duration::from_millis(timeout_ms))
    } else {
        dag_context
    };

    // Create executor (decoupled from the DAG; uses the DAG at execute time).
    let executor = Arc::new(DAGExecutor::new());

    // ── Output sink + drain task (W-O1) ─────────────────────────────────────────
    // Output nodes push `DagOutput`s here; the drain task maps them to LiveKit / WS.
    let (output_tx, mut output_rx) = mpsc::channel::<DagOutput>(64);
    let mut dag_context = dag_context.with_output_tx(output_tx.clone());

    // B-G2: insert the persistent realtime-session map so a `RealtimeProviderNode`
    // reuses ONE upstream WebSocket across turns (retaining server-side
    // conversation state) instead of paying a full WS handshake + session.update
    // per utterance. The bounded teardown owner is `handle_disconnect`, which
    // calls `crate::dag::nodes::disconnect_realtime_sessions` on this map BEFORE
    // the D-G4 task-tracker audit (the persistent `RealtimeSession` supervisor is
    // an untracked spawn the audit cannot reach, so it needs an explicit close).
    // The map must be set BEFORE `dag_context` is cloned into the connection state
    // below, so the teardown path reaches the SAME `Arc<RealtimeSessionMap>` via
    // `state.dag_context`. A caller without this resource (direct executor use,
    // unit tests) still falls back to the legacy per-turn path.
    {
        let realtime_sessions: std::sync::Arc<crate::dag::nodes::RealtimeSessionMap> =
            std::sync::Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
        dag_context.set_resource(
            crate::dag::nodes::realtime_sessions_key(),
            realtime_sessions,
        );
        // Wire the SHARED resilience registry so a persistent realtime node
        // attaches the SAME per-provider circuit breaker + reconnect governor the
        // HTTP `/realtime` path does (process-wide storm control / cross-session
        // FATAL tripping — W-D1/W-D2). Without it the session-long persistent
        // socket would auto-reconnect with no shared breaker (bug wc023gbbz #3).
        if let Some(resilience) = resilience {
            dag_context.set_resource(crate::dag::nodes::realtime_resilience_key(), resilience);
        }
    }

    {
        let message_tx = message_tx.clone();
        let livekit_client = livekit_client.cloned();
        let operation_queue = operation_queue.cloned();
        // D-G4: session-lifetime drain loop. Its `output_tx` clones live in
        // `state.dag_context` and the VoiceManager stt_callback, neither dropped
        // before teardown's audit, so the loop is stopped by the audit's abort
        // (cancels cleanly at the recv await point — not counted dangling).
        let task_tracker = state.read().await.task_tracker.clone();
        let drain = tokio::spawn(async move {
            while let Some(output) = output_rx.recv().await {
                match output {
                    DagOutput::Audio(audio) => {
                        // C-G5: client playback rate (passthrough when unset)
                        // — the SAME standardized seam as the simple path.
                        let data = match &egress_audio {
                            Some(e) => e.convert(
                                audio.data.to_vec(),
                                &audio.format,
                                audio.sample_rate,
                            ),
                            None => audio.data.to_vec(),
                        };
                        if data.is_empty() {
                            continue;
                        }
                        deliver_tts_audio(
                            data,
                            livekit_client.as_ref(),
                            operation_queue.as_ref(),
                            &message_tx,
                            None, // DAG sessions: chunking knob rides the conversation path only (carrier gap noted)
                        )
                        .await;
                    }
                    DagOutput::Text(text) => {
                        // Deliver DAG text output (e.g. an LLM response routed to a
                        // text_output node) as a message envelope.
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        let _ = message_tx
                            .send(MessageRoute::Outgoing(OutgoingMessage::Message {
                                message: UnifiedMessage {
                                    message: Some(text),
                                    data: None,
                                    identity: "dag".to_string(),
                                    topic: "dag_output".to_string(),
                                    room: String::new(),
                                    timestamp: now_ms,
                                },
                            }))
                            .await;
                    }
                }
            }
            debug!("DAG output drain task ended");
        });
        task_tracker.track("dag-output-drain", drain);
    }

    // Store DAG state in connection.
    {
        let mut state_guard = state.write().await;
        state_guard.compiled_dag = Some(compiled_dag.clone());
        state_guard.dag_executor = Some(executor.clone());
        state_guard.dag_context = Some(dag_context.clone());
        state_guard.set_dag_enabled(true);
    }

    // ── StreamDriver: run the executor once per finalized turn ───────────────────
    // Keep STT in the VoiceManager (already streaming, with turn detection). On each
    // finalized (speech_final) turn, inject the transcript at the post-STT node and
    // run the executor once. Output nodes deliver via `output_tx`. The transcript
    // itself still egresses via the standard STT callback (no double delivery: in DAG
    // mode the simple path never calls `speak()`).
    if let Some(vm) = voice_manager {
        register_dag_stream_driver(
            vm,
            compiled_dag,
            executor,
            dag_context,
            post_stt_node_id,
            message_tx,
            profiler,
        )
        .await;
    } else {
        warn!("DAG routing enabled but no voice manager; StreamDriver not started");
    }

    info!(stream_id = %stream_id, "DAG routing enabled");
    Ok(true)
}

/// Find the node directly downstream of the STT provider node (the "post-STT" node),
/// which is where the StreamDriver injects each finalized transcript.
///
/// Falls back to the entry node when the DAG has no STT provider node (e.g. a
/// text-only pipeline), so execution still starts somewhere sensible.
#[cfg(feature = "dag-routing")]
fn find_post_stt_node(dag: &crate::dag::compiler::CompiledDAG) -> String {
    // Locate the STT provider node, if any.
    for node_idx in &dag.topo_order {
        let node = &dag.graph[*node_idx];
        if node.node.node_type() == "stt_provider" {
            // The post-STT node is its first downstream successor.
            let outgoing = dag.outgoing_edges(*node_idx);
            if let Some((succ_idx, _edge)) = outgoing.into_iter().next() {
                return dag.graph[succ_idx].id.clone();
            }
            // STT node with no successor: inject at the STT node itself.
            return node.id.clone();
        }
    }
    // No STT node: start at the entry node.
    dag.graph[dag.entry].id.clone()
}

/// Register the StreamDriver: on each finalized STT turn, run the compiled DAG once
/// starting at the post-STT node, feeding it the transcript as an `STTResult`.
///
/// A fresh per-turn `DAGContext` is derived from the template (carrying the same
/// `stream_id`, auth, deadline and output sink) so per-turn node results don't leak
/// across turns, while the LLM node's per-`stream_id` conversation history is
/// preserved across turns.
#[cfg(feature = "dag-routing")]
#[allow(clippy::too_many_arguments)]
async fn register_dag_stream_driver(
    voice_manager: &Arc<VoiceManager>,
    compiled_dag: Arc<crate::dag::compiler::CompiledDAG>,
    executor: Arc<DAGExecutor>,
    ctx_template: DAGContext,
    post_stt_node_id: String,
    message_tx: &mpsc::Sender<MessageRoute>,
    profiler: Arc<crate::core::observability::LatencyProfiler>,
) {
    let message_tx = message_tx.clone();
    // A-G0: the DAG session drives turn policy through the same controller
    // spine as the conversation path (legacy set = previous inline gate).
    // The controller's turn_id keys the TurnTrace (A11 partial: one id per
    // turn across policy + profiling on this path).
    let turn_controller = Arc::new(crate::core::turn::TurnController::new(
        vec![Box::new(crate::core::turn::strategies::AnySpeechStart)],
        vec![Box::new(crate::core::turn::strategies::LegacySpeechFinalStop)],
        vec![],
    ));
    let result = voice_manager
        .on_stt_result(move |stt: STTResult| {
            let compiled_dag = compiled_dag.clone();
            let executor = executor.clone();
            let ctx_template = ctx_template.clone();
            let post_stt_node_id = post_stt_node_id.clone();
            let message_tx = message_tx.clone();
            let profiler = profiler.clone();
            let turn_controller = turn_controller.clone();

            Box::pin(async move {
                // Transcript egress: the DAG path REPLACES the simple-path STT callback
                // (VoiceManager stores a single STT callback), so we must still forward
                // the transcript to the client here — for both interim and final results,
                // matching the simple path. The pipeline (LLM→TTS→audio) only runs on a
                // finalized turn below; there is no double delivery because in DAG mode
                // the simple path never calls `speak()`.
                let _ = message_tx
                    .send(MessageRoute::Outgoing(OutgoingMessage::STTResult {
                        transcript: stt.transcript.clone(),
                        is_final: stt.is_final,
                        is_speech_final: stt.is_speech_final,
                        confidence: stt.confidence,
                        segment_transcript: stt.segment_transcript.clone(),
                    }))
                    .await;

                // Turn policy via the controller (legacy set ≡ the previous
                // inline `is_speech_final && !empty` gate); the DAG runs on
                // the Stopped event, keyed by the controller's turn_id.
                let signal = if stt.is_final || stt.is_speech_final {
                    crate::core::turn::ControllerSignal::SttFinal {
                        // Turn policy sees the FULL segment; client egress
                        // above kept the raw fragment.
                        text: stt.turn_transcript().to_string(),
                        is_speech_final: stt.is_speech_final,
                        is_finalized: stt.is_finalized,
                    }
                } else {
                    crate::core::turn::ControllerSignal::SttInterim {
                        text: stt.transcript.clone(),
                        confidence: stt.confidence,
                    }
                };
                let events = turn_controller.feed(&signal);
                let Some((turn_id, turn_transcript)) =
                    events.into_iter().find_map(|e| match e {
                        crate::core::turn::TurnEvent::Stopped { turn_id, transcript } => {
                            Some((turn_id, transcript))
                        }
                        // Started/barge-in has no DAG-side action today (the
                        // DAG has no cancellable bot turn at this layer).
                        _ => None,
                    })
                else {
                    return;
                };

                debug!(
                    transcript = %turn_transcript,
                    start_node = %post_stt_node_id,
                    "StreamDriver: running DAG for finalized turn"
                );

                // Fresh per-turn context (shares stream_id/auth/deadline/output sink).
                let mut ctx = ctx_template.clone_for_branch();

                use crate::core::observability::{TurnOutcome, TurnPath, TurnSink, TurnTrace};
                let mut trace = TurnTrace::open(
                    turn_id,
                    Arc::from(ctx.stream_id.as_str()),
                    TurnPath::Dag,
                    crate::core::observability::now_monotonic_ns(),
                );

                let input = crate::dag::nodes::DAGData::STTResult(STTResultData {
                    transcript: turn_transcript,
                    is_final: stt.is_final,
                    is_speech_final: stt.is_speech_final,
                    confidence: stt.confidence as f64,
                    speech_detected: true,
                    ..Default::default()
                });

                let exec_result = executor
                    .execute_from(&compiled_dag, &post_stt_node_id, input, &mut ctx)
                    .await;
                trace.audio_out_ns = crate::core::observability::now_monotonic_ns();
                trace.node_durations_us = ctx
                    .timing
                    .node_durations
                    .iter()
                    .map(|(node, dur)| (Arc::from(node.as_str()), dur.as_micros() as u64))
                    .collect();
                trace.outcome = if exec_result.is_ok() {
                    TurnOutcome::Completed
                } else {
                    TurnOutcome::Aborted
                };
                trace.bottleneck = trace.compute_bottleneck();
                profiler.record_turn(trace);
                match exec_result {
                    Ok(_) => {
                        debug!("StreamDriver: DAG turn completed");
                    }
                    Err(e) => {
                        warn!(error = %e, "StreamDriver: DAG turn failed");
                    }
                }
            })
        })
        .await;

    if let Err(e) = result {
        error!("Failed to register DAG StreamDriver STT callback: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_known_reasoning_model_matrix() {
        // Reasoning models (poor for the spoken path) → true.
        for m in [
            "o1-mini",
            "o3",
            "o4-mini",
            "deepseek-r1:1.5b",
            "gpt-5-thinking",
            "claude-sonnet-4-6-thinking",
            "DeepSeek-Reasoner",
            "qwq-32b",
        ] {
            assert!(is_known_reasoning_model(m), "{m} should be flagged reasoning");
        }
        // Fast / non-reasoning models → false.
        for m in [
            "gpt-4o-mini",
            "llama3.2:1b",
            "claude-haiku-4-5",
            "gemini-3-flash",
            "grok-4.1-fast",
        ] {
            assert!(!is_known_reasoning_model(m), "{m} should NOT be flagged");
        }
    }

    #[test]
    fn test_stream_id_generation_when_none() {
        let stream_id = resolve_stream_id(None);

        assert_eq!(stream_id.len(), 36);
        assert!(stream_id.contains('-'));
    }

    #[test]
    fn test_stream_id_uses_provided_value() {
        let stream_id = resolve_stream_id(Some("my-custom-stream-id".to_string()));

        assert_eq!(stream_id, "my-custom-stream-id");
    }

    #[test]
    fn test_stream_id_generation_uniqueness() {
        let id1 = Uuid::new_v4().to_string();
        let id2 = Uuid::new_v4().to_string();

        assert_ne!(id1, id2, "Generated UUIDs should be unique");
    }
}
