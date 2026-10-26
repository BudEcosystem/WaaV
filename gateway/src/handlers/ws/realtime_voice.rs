//! A `/ws` session whose voice is a speech-to-speech provider (`interaction.speech.realtime`):
//! regimes B and C in a LiveKit room, and so on a SIP call. The session has no VoiceManager:
//! the room's caller (`CallerTap`) is heard by the provider, the provider's audio is published
//! in the room, and the interaction session of `/realtime` (`handlers/realtime/interaction.rs`:
//! the regime resolver, the verb executor, the provider session) runs between them. Its events
//! go to the socket and to the room's `waav.interaction` data topic.
//!
//! Turns are the provider's (its own turn detection), unless the voice says
//! `turn_detection: {"mode": "manual"}`: then the gateway's analyser hears the caller, commits
//! each caller turn to the provider at 700 ms of silence, and cuts the provider when the
//! caller talks over it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::{RwLock, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::core::audio::PumpControl;
use crate::core::interaction::WireEvent;
use crate::core::interaction::actor::{self, Events, InteractionHandle, Msg};
use crate::core::interaction::analyser::{self, TapFrame};
use crate::core::interaction::config::DuplexConfig;
use crate::core::interaction::executor::Verb;
use crate::core::interaction::provider::AudioOut;
use crate::core::interaction::types::DuplexEvent;
use crate::core::realtime::{
    BaseRealtime, RealtimeAudioData, RealtimeError, ReconnectionEvent, SpeechEvent,
    TranscriptResult, get_supported_realtime_providers,
};
use crate::handlers::realtime::interaction::{self as rt, Tap, WS_SURFACE};
use crate::handlers::realtime::messages::{RealtimeSessionConfig, TurnDetectionConfig};
use crate::livekit::{CallerTap, LiveKitOperation, OperationQueue};
use crate::state::AppState;

use super::config::{ConversationWebSocketConfig, LiveKitWebSocketConfig, TTSWebSocketConfig};
use super::config_handler::{
    initialize_livekit_client, resolve_stream_id, send_config_warning, send_critical,
};
use super::messages::{MessageClass, MessageRoute, OutgoingMessage};
use super::state::ConnectionState;

/// The caller's frames that may wait for the provider; a frame that finds the queue full is
/// dropped.
const FRAMES: usize = 64;
/// Silence after the caller's speech at which a manual turn is committed (an analyser mark).
const COMMIT_SILENCE_MS: u32 = 700;
/// Voiced audio shorter than this is a cough or a click, not a turn.
const MIN_TURN_MS: u32 = 250;
/// How long the end of the session waits for the provider to close.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// The session's `interaction.speech.realtime`, when the session sent one itself.
fn requested(request: &Value) -> Option<&Value> {
    request.pointer("/speech/realtime")
}

/// The block without `speech.realtime`, which only this module reads.
fn without_voice(request: &Value) -> Value {
    let mut block = request.clone();
    if let Some(speech) = block.get_mut("speech").and_then(Value::as_object_mut) {
        speech.remove("realtime");
    }
    block
}

/// The provider's settings from `speech.realtime` (the `/realtime` config's keys), and the
/// block the interaction session parses: the rest of the session's block, with the session's
/// `conversation_config` as `tasks.llm` when the block names no LLM. Functions of the session
/// are not read: a call nobody can answer would hold the provider.
fn voice_config(
    voice: &Value,
    request: &Value,
    conversation: Option<&ConversationWebSocketConfig>,
) -> Result<RealtimeSessionConfig, String> {
    let mut config: RealtimeSessionConfig =
        serde_json::from_value(voice.clone()).map_err(|e| e.to_string())?;
    let mut block = without_voice(request);
    if block.pointer("/tasks/llm").is_none()
        && let (Some(conversation), Some(object)) = (conversation, block.as_object_mut())
    {
        let llm = serde_json::to_value(conversation).map_err(|e| e.to_string())?;
        let tasks = object.entry("tasks").or_insert_with(|| json!({}));
        if let Some(tasks) = tasks.as_object_mut() {
            tasks.insert("llm".to_owned(), llm);
        }
    }
    config.tools = None;
    config.alias = None;
    config.interaction = Some(block);
    Ok(config)
}

async fn refuse(message_tx: &mpsc::Sender<MessageRoute>, code: &str, message: String) {
    warn!(code, "{message}");
    let error = WireEvent::Error {
        message,
        code: code.to_owned(),
    };
    if let Ok(json) = serde_json::to_string(&error) {
        send_critical(message_tx, MessageRoute::Json(json)).await;
    }
}

/// Take the session when its own `interaction` block names `speech.realtime` and it has a
/// `livekit` block. `Some(done)`: the session was handled (ready, or refused with `error`).
/// `None`: it is configured as any `/ws` session, and the key is ignored with a warning.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle(
    stream_id: Option<String>,
    livekit: Option<&LiveKitWebSocketConfig>,
    conversation: Option<&ConversationWebSocketConfig>,
    agent: bool,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
    app_state: &Arc<AppState>,
) -> Option<bool> {
    let (request, asked) = state.read().await.interaction_request.clone()?;
    let voice = requested(&request).filter(|_| asked)?.clone();
    // Without a room the parser says why the key is ignored.
    let livekit = livekit?;
    let reason = if app_state.bud_mode.is_some() {
        Some("bud_mode")
    } else if agent {
        Some("agent")
    } else {
        None
    };
    if let Some(reason) = reason {
        let message = "interaction.speech.realtime is not available on this session (a Bud deployment or a voice agent); it is ignored.";
        let detail =
            json!({"path": "interaction.speech.realtime", "surface": WS_SURFACE, "reason": reason});
        send_config_warning(
            message_tx,
            "interaction_surface_unavailable",
            message,
            Some(detail),
        )
        .await;
        state.write().await.interaction_request = Some((without_voice(&request), asked));
        return None;
    }
    Some(
        start(
            &request,
            &voice,
            stream_id,
            livekit,
            conversation,
            state,
            message_tx,
            app_state,
        )
        .await,
    )
}

#[allow(clippy::too_many_arguments)]
async fn start(
    request: &Value,
    voice: &Value,
    stream_id: Option<String>,
    livekit: &LiveKitWebSocketConfig,
    conversation: Option<&ConversationWebSocketConfig>,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
    app_state: &Arc<AppState>,
) -> bool {
    let mut config = match voice_config(voice, request, conversation) {
        Ok(config) => config,
        Err(error) => {
            let message = format!("interaction.speech.realtime is not a realtime voice: {error}");
            refuse(message_tx, "invalid_realtime_voice", message).await;
            return true;
        }
    };
    let provider_name = config
        .provider
        .clone()
        .unwrap_or_else(|| "openai".to_owned());
    let provider_name = provider_name.as_str();
    let supported = get_supported_realtime_providers();
    if !supported
        .iter()
        .any(|p| p.eq_ignore_ascii_case(provider_name))
    {
        let message = format!("Unsupported provider: {provider_name}. Supported: {supported:?}");
        refuse(message_tx, "invalid_provider", message).await;
        return true;
    }
    let model = config.model.clone();
    if let Some(notice) =
        crate::handlers::realtime::lifecycle_notice(provider_name, model.as_deref(), app_state)
    {
        if notice.fatal {
            refuse(message_tx, notice.code, notice.message).await;
            return true;
        }
        send_config_warning(message_tx, notice.code, notice.message, Some(notice.detail)).await;
    }
    let (prepared, findings) = rt::decide(&mut config, provider_name, WS_SURFACE);
    for (code, message, detail) in findings {
        send_config_warning(message_tx, code, message, Some(detail)).await;
    }
    let Some(prepared) = prepared else {
        let message = "interaction.speech.realtime runs only with interaction mode, which this session cannot have (see the warnings).".to_owned();
        refuse(message_tx, "interaction_realtime_unavailable", message).await;
        return true;
    };
    let mut provider =
        match crate::handlers::realtime::provider_for(&config, provider_name, app_state, "") {
            Ok(provider) => provider,
            Err((code, message)) => {
                refuse(message_tx, code, message).await;
                return true;
            }
        };
    let stream_id = resolve_stream_id(stream_id);
    {
        let mut guard = state.write().await;
        guard.set_audio_enabled(true);
        guard.stream_id = Some(stream_id.clone());
    }
    let (in_rate, out_rate) = provider.audio_rates().unwrap_or((24_000, 24_000));
    let manual = matches!(config.turn_detection, Some(TurnDetectionConfig::Manual));

    // The caller's frames, ahead of the room's noise filter, at the provider's input rate.
    let ctl = Arc::new(PumpControl::new(mpsc::unbounded_channel().0, 0));
    let (frame_tx, frames) = mpsc::channel::<Bytes>(FRAMES);
    let line = ctl.clone();
    let id = stream_id.clone();
    // The keys the caller presses on a phone line answer the session's questions
    // (`hitl.channels: dtmf`).
    let (key_tx, mut keys) = mpsc::unbounded_channel::<char>();
    let caller = CallerTap::new(
        move |frame: &[u8]| {
            if frame_tx.try_send(Bytes::copy_from_slice(frame)).is_err() {
                crate::core::metrics::bridge::record_degraded("interaction", "voice_frames_full");
            }
        },
        move |phone| {
            info!(stream_id = %id, phone, "the realtime voice found the room's caller");
            if phone {
                line.set_phone_leg();
            }
        },
    )
    .on_keys(move |key| {
        let _ = key_tx.send(key);
    });
    // The room publishes what the provider says at the provider's output rate.
    let egress: TTSWebSocketConfig =
        match serde_json::from_value(json!({ "sample_rate": out_rate })) {
            Ok(egress) => egress,
            Err(error) => {
                refuse(message_tx, "internal_error", error.to_string()).await;
                return true;
            }
        };
    let (auth_id, tracker) = {
        let guard = state.read().await;
        (guard.auth.id.clone(), guard.task_tracker.clone())
    };
    let mut livekit = livekit.clone();
    livekit.room_name = state
        .read()
        .await
        .auth
        .normalize_room_name(&livekit.room_name);
    let Some((client, queue, room_name, egress_id, identity, name)) = initialize_livekit_client(
        livekit,
        Some(&egress),
        in_rate,
        &app_state.config.livekit_url,
        None,
        message_tx,
        app_state.livekit_room_handler.as_ref(),
        &stream_id,
        auth_id.as_deref(),
        &tracker,
        Some(caller),
        // The room's `waav.interaction` topic: its controls and `task_seen` are the session's.
        Some(state.clone()),
    )
    .await
    else {
        return true;
    };
    {
        let mut guard = state.write().await;
        guard.livekit_client = Some(client);
        guard.livekit_operation_queue = queue.clone();
        guard.livekit_room_name = Some(room_name.clone());
        guard.livekit_local_identity = Some(identity.clone());
        guard.recording_egress_id = egress_id;
    }
    let Some(queue) = queue else {
        refuse(
            message_tx,
            "livekit_unavailable",
            "The room has no operation queue.".to_owned(),
        )
        .await;
        return true;
    };

    let (events, _forwarder) = actor::forward(
        message_tx.clone(),
        Some(queue.clone()),
        prepared.inbox.downgrade(),
    );
    let room = Room::new(queue, ctl.clone(), manual, events.clone());
    let tap = rt::tap(&prepared);
    register(&mut *provider, &tap, &room, manual, message_tx);
    info!(provider = provider_name, room = %room_name, "Connecting the room's realtime voice");
    if let Err(error) = provider.connect().await {
        refuse(
            message_tx,
            "connection_error",
            format!("Failed to connect: {error}"),
        )
        .await;
        return true;
    }

    let owner = crate::handlers::tasks::owner(&state.read().await.auth).to_owned();
    let echo = prepared.planned.echo.clone();
    let duplex = prepared.planned.cfg.duplex.clone();
    let session = prepared.inbox.clone();
    // After `session_failed` (the provider stayed down, chapter 10) the socket is closed as a
    // cascade session's is, once the event has gone out.
    let closer = message_tx.clone();
    let held = room.clone();
    let out = Box::new(move |json: String| {
        let failed = rt::is_session_failed(&json);
        // The session stopped the speech (a claim caught as it was said, a client's stop): what
        // the room still holds of it is dropped too.
        if stops_speech(&json) {
            let room = held.clone();
            tokio::spawn(async move {
                room.drop_held().await;
            });
        }
        let _ = events.send((json, MessageClass::Critical, None));
        if failed {
            let closer = closer.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let close = MessageRoute::CloseWith {
                    code: 1011,
                    reason: "session_failed".to_owned(),
                };
                send_critical(&closer, close).await;
            });
        }
    });
    let lines = room.clone();
    let audio: AudioOut = Box::new(move |pcm| lines.play_line(pcm, out_rate));
    let voice = rt::voice_of(&prepared.planned.cfg, app_state);
    let steered = rt::start_with(
        prepared,
        provider,
        &stream_id,
        &owner,
        out,
        Some(audio),
        voice,
        Some(ctl.clone()),
    );

    // The session's inbox: the socket's messages and, with manual turns, the analyser's.
    let (inbox_tx, inbox) = mpsc::channel(256);
    let stopped = CancellationToken::new();
    let handle = InteractionHandle::new(inbox_tx.clone(), echo.clone(), stopped.clone());
    let key_inbox = inbox_tx.clone();
    tracker.track(
        "interaction-voice-keys",
        tokio::spawn(async move {
            while let Some(key) = keys.recv().await {
                if key_inbox.send(Msg::Dtmf(key)).await.is_err() {
                    break;
                }
            }
        }),
    );
    let mut models = None;
    let analysed = manual.then(|| {
        let (tap_tx, tap_rx) = mpsc::channel::<TapFrame>(FRAMES);
        let (analyser, loading) =
            analyser::spawn_loading(tap_rx, in_rate, ctl.clone(), inbox_tx, duplex.clone(), None);
        tracker.track("interaction-voice-analyser", analyser);
        models = Some(loading);
        tap_tx
    });
    let voice = Voice {
        steered,
        session,
        tap: tap.clone(),
        room,
        analysed,
        duplex,
        onset: None,
        spoke: false,
    };
    // Not on the session's tracker: the end of the session reaches it through `shutdown`.
    tokio::spawn(voice.run(frames, inbox, stopped));
    state.write().await.interaction = Some(handle);
    // With manual turns `ready` waits for the analyser's models, at most `MODELS_WAIT`.
    if let Some(models) = models {
        analyser::models_loaded(models).await;
    }

    send_critical(
        message_tx,
        MessageRoute::Outgoing(OutgoingMessage::Ready {
            protocol_version: super::messages::INTERACTION_PROTOCOL_VERSION.to_string(),
            stream_id: stream_id.clone(),
            livekit_room_name: Some(room_name),
            livekit_url: Some(app_state.config.livekit_public_url.clone()),
            waav_participant_identity: Some(identity),
            waav_participant_name: Some(name),
            resolved_alias: None,
            audio_in_codec: None,
            audio_out_codec: None,
            interaction: Some(echo),
        }),
    )
    .await;
    info!(%stream_id, provider = provider_name, manual, "Realtime voice session ready");
    true
}

/// What the room still plays of the provider, on the analyser's clock.
#[derive(Default)]
struct Playout {
    /// When the audio sent to the room so far ends.
    until_us: u64,
    /// A response of the provider sends audio.
    responding: bool,
    /// The caller cut in: the rest of that response is dropped.
    muted: bool,
}

/// The room's side of the provider. Its audio goes to the room on a task of its own, paced
/// [`LEAD_US`] ahead of playout as the metered pump does: the room's operations run in order
/// and each frame waits for its playout, so a cut (and every data event) would otherwise wait
/// behind the whole response. The provider's callback never waits.
struct Room {
    ctl: Arc<PumpControl>,
    playout: Arc<Mutex<Playout>>,
    /// The provider's audio for the room, with its rate and the cut it belongs after.
    audio: mpsc::UnboundedSender<(u64, Bytes, u32)>,
    cuts: Arc<AtomicU64>,
    queue: OperationQueue,
    events: Events,
}

/// How far ahead of playout the provider's audio is handed to the room, in 20 ms frames.
const LEAD_US: u64 = 80_000;

impl Room {
    /// `reference`: the analyser's echo gate reads what was played.
    fn new(
        queue: OperationQueue,
        ctl: Arc<PumpControl>,
        reference: bool,
        events: Events,
    ) -> Arc<Self> {
        let (audio, queued) = mpsc::unbounded_channel();
        let room = Arc::new(Self {
            ctl,
            playout: Arc::default(),
            audio,
            cuts: Arc::default(),
            queue,
            events,
        });
        tokio::spawn(Self::publish(
            queued,
            room.queue.clone(),
            room.ctl.clone(),
            room.playout.clone(),
            room.cuts.clone(),
            reference,
        ));
        room
    }

    async fn publish(
        mut queued: mpsc::UnboundedReceiver<(u64, Bytes, u32)>,
        queue: OperationQueue,
        ctl: Arc<PumpControl>,
        playout: Arc<Mutex<Playout>>,
        cuts: Arc<AtomicU64>,
        reference: bool,
    ) {
        while let Some((cut, audio, rate)) = queued.recv().await {
            // 20 ms at a time: a frame holds the room's queue until it has played.
            let frame = (rate as usize / 50 * 2).max(2);
            for start in (0..audio.len()).step_by(frame) {
                let audio = audio.slice(start..(start + frame).min(audio.len()));
                let due = playout.lock().until_us.saturating_sub(LEAD_US);
                let now = ctl.now_us();
                if due > now {
                    tokio::time::sleep(Duration::from_micros(due - now)).await;
                }
                let at = {
                    let mut playout = playout.lock();
                    if cut != cuts.load(Ordering::Acquire) {
                        break;
                    }
                    let at = playout.until_us.max(ctl.now_us());
                    let samples = audio.len() as u64 / 2;
                    playout.until_us = at + samples * 1_000_000 / u64::from(rate.max(1));
                    at
                };
                if reference {
                    ctl.record_reference(at, &audio, rate);
                }
                let send = LiveKitOperation::SendAudio {
                    audio_data: audio.to_vec(),
                    response_tx: oneshot::channel().0,
                };
                if queue.queue(send).await.is_err() {
                    return;
                }
            }
        }
    }

    fn audible(&self) -> bool {
        self.playout.lock().until_us > self.ctl.now_us()
    }

    /// One of the session's own lines (chapter 10's failure lines) for the room, after what
    /// it holds of the provider's audio.
    fn play_line(&self, pcm: Bytes, rate: u32) {
        let cut = self.cuts.load(Ordering::Acquire);
        let _ = self.audio.send((cut, pcm, rate));
    }

    /// One chunk of the provider's audio for the room.
    fn play(&self, audio: &RealtimeAudioData) {
        let mut playout = self.playout.lock();
        if playout.muted {
            return;
        }
        playout.responding = true;
        let cut = self.cuts.load(Ordering::Acquire);
        let _ = self
            .audio
            .send((cut, audio.data.clone(), audio.sample_rate));
    }

    fn response_done(&self) {
        let mut playout = self.playout.lock();
        playout.responding = false;
        playout.muted = false;
    }

    /// What the room still holds is dropped, and the rest of the provider's response in
    /// progress. `false`: nothing was playing.
    async fn drop_held(&self) -> bool {
        {
            let mut playout = self.playout.lock();
            let now = self.ctl.now_us();
            if playout.until_us <= now {
                return false;
            }
            playout.until_us = now;
            playout.muted = playout.responding;
            self.cuts.fetch_add(1, Ordering::AcqRel);
        }
        let clear = LiveKitOperation::ClearAudio {
            response_tx: oneshot::channel().0,
        };
        let _ = self.queue.queue(clear).await;
        true
    }

    /// The caller talks over the provider: what the room still holds of it is dropped, and
    /// the rest of the response in progress. `false`: nothing was playing.
    async fn cut(&self) -> bool {
        if !self.drop_held().await {
            return false;
        }
        let stopped = WireEvent::AgentAudioStopped {
            utterance_ids: Vec::new(),
            reason: "barge_in",
            delivered_ms: Vec::new(),
            t_ms: self.ctl.now_ms(),
        };
        if let Ok(json) = serde_json::to_string(&stopped) {
            let _ = self.events.send((json, MessageClass::Critical, None));
        }
        true
    }
}

/// Whether a session event stops the speech the room holds: `agent_audio_stopped` that the
/// session decided (a claim caught as it was said, a client's stop), not the caller's barge-in
/// the room cut itself.
fn stops_speech(json: &str) -> bool {
    json.starts_with(r#"{"type":"agent_audio_stopped""#)
        && (json.contains(r#""reason":"policy""#) || json.contains(r#""reason":"client_clear""#))
}

/// A final transcript of the provider's as the socket's message: the caller's words as
/// `stt_result`, the provider's as `assistant_transcript` (source `provider`; `true`: an event
/// the room's topic gets too).
fn transcript_json(result: &TranscriptResult, turns: &AtomicU64) -> Option<(String, bool)> {
    if !result.is_final || result.text.trim().is_empty() {
        return None;
    }
    let json = match result.role {
        crate::core::realtime::TranscriptRole::User => {
            turns.fetch_add(1, Ordering::AcqRel);
            let json = serde_json::to_string(&OutgoingMessage::STTResult {
                transcript: result.text.clone(),
                is_final: true,
                is_speech_final: true,
                confidence: 1.0,
                segment_transcript: None,
                translations: Vec::new(),
                detected_language: None,
            })
            .ok()?;
            (json, false)
        }
        crate::core::realtime::TranscriptRole::Assistant => {
            let json = json!({
                "type": "assistant_transcript",
                "turn_index": turns.load(Ordering::Acquire),
                "delta": result.text,
                "source": "provider",
            });
            (json.to_string(), true)
        }
    };
    Some(json)
}

/// The provider's callbacks: its words and calls to the interaction session, its audio to the
/// room, its failures to the socket.
fn register(
    provider: &mut dyn BaseRealtime,
    tap: &Arc<Tap>,
    room: &Arc<Room>,
    manual: bool,
    message_tx: &mpsc::Sender<MessageRoute>,
) {
    let (t, tx, r) = (tap.clone(), message_tx.clone(), room.clone());
    let turns = Arc::new(AtomicU64::new(0));
    let _ = provider.on_transcript(Arc::new(move |result: TranscriptResult| {
        let (tap, tx, room, turns) = (t.clone(), tx.clone(), r.clone(), turns.clone());
        Box::pin(async move {
            tap.transcript(&result.role, &result.text, result.is_final)
                .await;
            // The final words of both sides go to the socket, as a plain session's do; the
            // provider's also to the room's topic, as the cascade's agent lines do.
            match transcript_json(&result, &turns) {
                Some((json, true)) => {
                    let _ = room.events.send((json, MessageClass::Critical, None));
                }
                Some((json, false)) => send_critical(&tx, MessageRoute::Json(json)).await,
                None => {}
            }
        })
    }));
    let (t, r) = (tap.clone(), room.clone());
    let _ = provider.on_audio(Arc::new(move |audio: RealtimeAudioData| {
        let (tap, room) = (t.clone(), r.clone());
        Box::pin(async move {
            if tap.audio(audio.data.len()) {
                room.play(&audio);
            }
        })
    }));
    let t = tap.clone();
    let _ = provider.on_function_call(Arc::new(
        move |call: crate::core::realtime::FunctionCallRequest| {
            let tap = t.clone();
            Box::pin(async move {
                if let Some(call) = tap.call(call).await {
                    warn!(name = %call.name, "a function the realtime voice does not have was called");
                }
            })
        },
    ));
    let (t, r) = (tap.clone(), room.clone());
    let _ = provider.on_response_done(Arc::new(move |_id: String| {
        let (tap, room) = (t.clone(), r.clone());
        Box::pin(async move {
            room.response_done();
            tap.response_done().await;
        })
    }));
    // A provider that ends the turns stops itself when the caller cuts in: the room drops
    // what it still holds of it.
    if !manual {
        let (t, r) = (tap.clone(), room.clone());
        let _ = provider.on_speech_event(Arc::new(move |event: SpeechEvent| {
            let (tap, room) = (t.clone(), r.clone());
            Box::pin(async move {
                let started = matches!(event, SpeechEvent::Started { .. });
                if started {
                    room.cut().await;
                }
                tap.caller(started).await;
            })
        }));
    }
    let (t, tx) = (tap.clone(), message_tx.clone());
    let _ = provider.on_error(Arc::new(move |error: RealtimeError| {
        let (tap, tx) = (t.clone(), tx.clone());
        Box::pin(async move {
            tap.failed(&error).await;
            refuse(&tx, "provider_error", error.to_string()).await;
        })
    }));
    // A provider lost for good: the session says the exit line and ends with
    // `session_failed`, then the socket closes (chapter 10).
    let t = tap.clone();
    let _ = provider.on_reconnection(Arc::new(move |event: ReconnectionEvent| {
        let tap = t.clone();
        Box::pin(async move {
            tap.reconnection(event.success, event.error).await;
        })
    }));
}

/// The session's loop: the caller's frames to the provider (and the analyser), the socket's
/// messages to the interaction session, and with manual turns the analyser's events.
struct Voice {
    steered: Box<dyn BaseRealtime>,
    /// The interaction session's inbox.
    session: mpsc::Sender<Msg>,
    tap: Arc<Tap>,
    room: Arc<Room>,
    /// The analyser's audio, with manual turns.
    analysed: Option<mpsc::Sender<TapFrame>>,
    duplex: DuplexConfig,
    /// When the caller's current stretch of speech began.
    onset: Option<u64>,
    /// The caller spoke since the last commit.
    spoke: bool,
}

impl Voice {
    async fn run(
        mut self,
        mut frames: mpsc::Receiver<Bytes>,
        mut inbox: mpsc::Receiver<Msg>,
        stopped: CancellationToken,
    ) {
        let clean = loop {
            tokio::select! {
                frame = frames.recv() => {
                    let Some(frame) = frame else { break false };
                    if let Some(analysed) = &self.analysed {
                        let _ = analysed.try_send((frame.clone(), false));
                    }
                    if let Err(error) = self.steered.send_audio(frame).await {
                        debug!(%error, "caller audio not sent to the realtime voice");
                    }
                }
                msg = inbox.recv() => match msg {
                    None => break false,
                    Some(Msg::Shutdown { clean }) => break clean,
                    Some(Msg::Duplex(event)) => self.on_duplex(event).await,
                    Some(other) => {
                        let _ = self.session.send(other).await;
                    }
                },
            }
        };
        let _ = self.session.send(Msg::Shutdown { clean }).await;
        self.analysed = None;
        match tokio::time::timeout(CLOSE_WAIT, self.steered.disconnect()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!(%error, "the realtime voice did not close cleanly"),
            Err(_) => warn!("the realtime voice did not close in time"),
        }
        stopped.cancel();
    }

    /// Manual turns: the gateway ends the caller's turns and hears the caller cut in.
    async fn on_duplex(&mut self, event: DuplexEvent) {
        match event {
            DuplexEvent::Onset { at, .. } => {
                self.onset = Some(at);
                self.tap.caller(true).await;
            }
            DuplexEvent::Level { .. } => {
                let now = self.room.ctl.now_ms();
                let long = self.onset.is_some_and(|at| {
                    now.saturating_sub(at) >= u64::from(self.duplex.pause_after_ms)
                });
                if self.duplex.barge_in && long && self.room.audible() && self.room.cut().await {
                    info!("the caller cut in over the realtime voice");
                    self.tap.exec.send(Verb::StopOutput);
                }
            }
            DuplexEvent::Offset { voiced_ms, .. } => {
                self.onset = None;
                self.spoke |= voiced_ms >= MIN_TURN_MS;
            }
            DuplexEvent::Silence { ms, .. } if ms >= COMMIT_SILENCE_MS && self.spoke => {
                self.spoke = false;
                if let Err(error) = self.steered.commit_audio_buffer().await {
                    debug!(%error, "the caller's turn was not committed");
                }
                // The caller's turn ends here: the provider owes an answer.
                self.tap.caller(false).await;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_voice_is_the_realtime_config_and_the_rest_is_the_interaction_block() {
        let request = json!({
            "speech": {"realtime": {"provider": "openai", "voice": "verse",
                                    "instructions": "Be brief.", "turn_detection": {"mode": "manual"},
                                    "tools": [{"type": "function", "function": {"name": "x"}}]},
                       "claim_policy": "correct"},
            "regime": "delegating",
        });
        let voice = requested(&request).unwrap().clone();
        let conversation: ConversationWebSocketConfig =
            serde_json::from_value(json!({"model": "m", "base_url": "http://127.0.0.1:1/v1"}))
                .unwrap();
        let config = voice_config(&voice, &request, Some(&conversation)).unwrap();
        assert_eq!(config.provider.as_deref(), Some("openai"));
        assert_eq!(config.instructions.as_deref(), Some("Be brief."));
        assert!(matches!(
            config.turn_detection,
            Some(TurnDetectionConfig::Manual)
        ));
        assert!(
            config.tools.is_none(),
            "a session's own functions are not read"
        );
        let block = config.interaction.unwrap();
        assert_eq!(block["speech"], json!({"claim_policy": "correct"}));
        assert_eq!(block["regime"], "delegating");
        assert_eq!(
            block["tasks"]["llm"]["model"], "m",
            "the conversation is the background"
        );

        // A block that names its own LLM keeps it.
        let own = json!({"speech": {"realtime": {}}, "tasks": {"llm": {"model": "own"}}});
        let config = voice_config(&json!({}), &own, Some(&conversation)).unwrap();
        assert_eq!(config.interaction.unwrap()["tasks"]["llm"]["model"], "own");
        assert!(voice_config(&json!({"provider": 3}), &own, None).is_err());
    }

    use crate::core::interaction::executor;
    use crate::core::interaction::provider::ProviderEvent;
    use crate::core::realtime::{
        AudioOutputCallback, ConnectionState as Link, FunctionCallCallback, RealtimeConfig,
        RealtimeErrorCallback, RealtimeResult, ReconnectionCallback, ResponseDoneCallback,
        SpeechEventCallback, TranscriptCallback,
    };
    use crate::livekit::operations::QueuedOperation;

    /// A provider that keeps its audio and response callbacks and logs what it is asked.
    #[derive(Clone, Default)]
    struct Provider {
        log: Arc<Mutex<Vec<&'static str>>>,
        audio: Arc<Mutex<Option<AudioOutputCallback>>>,
        done: Arc<Mutex<Option<ResponseDoneCallback>>>,
    }

    impl Provider {
        fn note(&self, what: &'static str) -> RealtimeResult<()> {
            self.log.lock().push(what);
            Ok(())
        }

        /// One second of the provider's audio, as its callback gets it.
        async fn speak(&self) {
            let audio = self.audio.lock().clone().unwrap();
            audio(RealtimeAudioData {
                data: Bytes::from(vec![0u8; 48_000]),
                sample_rate: 24_000,
                item_id: None,
                response_id: None,
            })
            .await;
        }

        async fn response_done(&self) {
            let done = self.done.lock().clone().unwrap();
            done("r1".to_owned()).await;
        }
    }

    #[async_trait::async_trait]
    impl BaseRealtime for Provider {
        fn new(_c: RealtimeConfig) -> RealtimeResult<Self> {
            unreachable!()
        }
        async fn connect(&mut self) -> RealtimeResult<()> {
            Ok(())
        }
        async fn disconnect(&mut self) -> RealtimeResult<()> {
            self.note("disconnect")
        }
        fn is_ready(&self) -> bool {
            true
        }
        fn get_connection_state(&self) -> Link {
            Link::Connected
        }
        async fn send_audio(&mut self, _a: Bytes) -> RealtimeResult<()> {
            self.note("audio")
        }
        async fn send_text(&mut self, _t: &str) -> RealtimeResult<()> {
            Ok(())
        }
        async fn create_response(&mut self) -> RealtimeResult<()> {
            Ok(())
        }
        async fn cancel_response(&mut self) -> RealtimeResult<()> {
            Ok(())
        }
        async fn commit_audio_buffer(&mut self) -> RealtimeResult<()> {
            self.note("commit")
        }
        async fn clear_audio_buffer(&mut self) -> RealtimeResult<()> {
            Ok(())
        }
        fn on_transcript(&mut self, _c: TranscriptCallback) -> RealtimeResult<()> {
            Ok(())
        }
        fn on_audio(&mut self, c: AudioOutputCallback) -> RealtimeResult<()> {
            *self.audio.lock() = Some(c);
            Ok(())
        }
        fn on_error(&mut self, _c: RealtimeErrorCallback) -> RealtimeResult<()> {
            Ok(())
        }
        fn on_function_call(&mut self, _c: FunctionCallCallback) -> RealtimeResult<()> {
            Ok(())
        }
        fn on_speech_event(&mut self, _c: SpeechEventCallback) -> RealtimeResult<()> {
            self.note("speech_events")
        }
        fn on_response_done(&mut self, c: ResponseDoneCallback) -> RealtimeResult<()> {
            *self.done.lock() = Some(c);
            Ok(())
        }
        fn on_reconnection(&mut self, _c: ReconnectionCallback) -> RealtimeResult<()> {
            Ok(())
        }
        async fn update_session(&mut self, _c: RealtimeConfig) -> RealtimeResult<()> {
            Ok(())
        }
        async fn submit_function_result(&mut self, _id: &str, _r: &str) -> RealtimeResult<()> {
            Ok(())
        }
        fn get_provider_info(&self) -> Value {
            json!({})
        }
    }

    async fn next_op(ops: &mut mpsc::Receiver<QueuedOperation>) -> &'static str {
        let op = tokio::time::timeout(Duration::from_secs(2), ops.recv()).await;
        match op.ok().flatten().map(|op| op.operation) {
            Some(LiveKitOperation::SendAudio { .. }) => "send_audio",
            Some(LiveKitOperation::ClearAudio { .. }) => "clear_audio",
            Some(_) => "other",
            None => "none",
        }
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    /// Manual turns in a room: the gateway commits each caller turn after its silence, cuts
    /// the provider's audio in the room when the caller talks over it (and drops the rest of
    /// that response), passes the socket's messages on, and closes the provider at the end.
    #[tokio::test]
    async fn a_room_voice_commits_turns_and_cuts_the_provider_when_the_caller_talks_over_it() {
        let (queue, mut ops) = OperationQueue::new(64);
        let ctl = Arc::new(PumpControl::new(mpsc::unbounded_channel().0, 0));
        let (events, mut sent) = mpsc::unbounded_channel();
        let room = Room::new(queue, ctl.clone(), true, events);
        let (exec, _pending) = executor::handle();
        let (session, mut heard) = mpsc::channel(16);
        let tap = Arc::new(Tap {
            inbox: session.clone(),
            exec,
            mute: false,
        });
        let mut provider = Provider::default();
        let (socket, _client) = mpsc::channel(16);
        register(&mut provider, &tap, &room, true, &socket);
        assert!(
            !provider.log.lock().contains(&"speech_events"),
            "manual turns are the gateway's"
        );

        provider.speak().await;
        assert_eq!(next_op(&mut ops).await, "send_audio");
        assert!(room.audible());

        let duplex = DuplexConfig {
            pause_after_ms: 100,
            ..DuplexConfig::default()
        };
        let voice = Voice {
            steered: Box::new(provider.clone()),
            session: session.clone(),
            tap: tap.clone(),
            room: room.clone(),
            analysed: None,
            duplex,
            onset: None,
            spoke: false,
        };
        let (frames_tx, frames) = mpsc::channel(8);
        let (inbox, rx) = mpsc::channel(16);
        let stopped = CancellationToken::new();
        let running = tokio::spawn(voice.run(frames, rx, stopped.clone()));
        frames_tx.send(Bytes::from_static(&[0; 480])).await.unwrap();

        // Speech over the provider: shorter than `pause_after_ms` cuts nothing.
        let onset = DuplexEvent::Onset {
            at: ctl.now_ms(),
            level_db: -20.0,
            reflex_ducked: false,
        };
        inbox.send(Msg::Duplex(onset)).await.unwrap();
        let level = || Msg::Duplex(DuplexEvent::Level { level_db: -20.0 });
        inbox.send(level()).await.unwrap();
        settle().await;
        assert!(room.audible(), "too short to be a barge-in");
        tokio::time::sleep(Duration::from_millis(120)).await;
        inbox.send(level()).await.unwrap();
        // The frames sent before the cut, then the clear.
        let mut before = 0;
        let mut op = next_op(&mut ops).await;
        while op == "send_audio" {
            before += 1;
            op = next_op(&mut ops).await;
        }
        assert_eq!(op, "clear_audio");
        assert!(
            before < 20,
            "paced 20 ms frames, not the whole second: {before}"
        );
        let (json, ..) = sent.recv().await.unwrap();
        let stopped_event: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(stopped_event["type"], "agent_audio_stopped");
        assert_eq!(stopped_event["reason"], "barge_in");
        assert!(!room.audible());

        // The rest of the cut response is not played; the next one is.
        provider.speak().await;
        settle().await;
        assert!(ops.try_recv().is_err(), "the cut response stays quiet");
        provider.response_done().await;
        let mut provider_event = async || {
            heard.recv().await.and_then(|m| match m {
                Msg::Provider(event) => Some(event),
                _ => None,
            })
        };
        // The caller's speech began (the health watcher's turn, chapter 10), then the response
        // ended.
        assert_eq!(
            provider_event().await,
            Some(ProviderEvent::CallerSpeaking(true))
        );
        assert_eq!(provider_event().await, Some(ProviderEvent::ResponseDone));
        provider.speak().await;
        assert_eq!(next_op(&mut ops).await, "send_audio");

        // A turn is committed once, at 700 ms of silence; a click is no turn.
        let silence = |ms| Msg::Duplex(DuplexEvent::Silence { at: 0, ms });
        let offset = |voiced_ms| Msg::Duplex(DuplexEvent::Offset { at: 0, voiced_ms });
        for msg in [offset(800), silence(200), silence(700), silence(1500)] {
            inbox.send(msg).await.unwrap();
        }
        for msg in [offset(100), silence(700)] {
            inbox.send(msg).await.unwrap();
        }
        // The committed turn ends the caller's speech for the health watcher; the click does not.
        assert!(matches!(
            heard.recv().await,
            Some(Msg::Provider(ProviderEvent::CallerSpeaking(false)))
        ));
        // The socket's messages are the interaction session's.
        let cancel = Msg::TaskCancel {
            task_id: "t1".to_owned(),
        };
        inbox.send(cancel).await.unwrap();
        assert!(matches!(heard.recv().await, Some(Msg::TaskCancel { .. })));

        inbox.send(Msg::Shutdown { clean: true }).await.unwrap();
        assert!(matches!(
            heard.recv().await,
            Some(Msg::Shutdown { clean: true })
        ));
        tokio::time::timeout(Duration::from_secs(2), stopped.cancelled())
            .await
            .unwrap();
        running.await.unwrap();
        let log = provider.log.lock().clone();
        assert_eq!(log.iter().filter(|w| **w == "commit").count(), 1, "{log:?}");
        assert!(
            log.contains(&"audio"),
            "the caller's frame reached the provider"
        );
        assert_eq!(log.last(), Some(&"disconnect"));
    }

    /// A provider that ends the turns cuts itself; the room drops what it still holds.
    #[tokio::test]
    async fn a_provider_with_its_own_turns_gets_the_speech_events() {
        let (queue, _ops) = OperationQueue::new(8);
        let ctl = Arc::new(PumpControl::new(mpsc::unbounded_channel().0, 0));
        let room = Room::new(queue, ctl, false, mpsc::unbounded_channel().0);
        let (exec, _pending) = executor::handle();
        let tap = Arc::new(Tap {
            inbox: mpsc::channel(4).0,
            exec,
            mute: false,
        });
        let mut provider = Provider::default();
        register(&mut provider, &tap, &room, false, &mpsc::channel(4).0);
        assert!(provider.log.lock().contains(&"speech_events"));
    }

    #[test]
    fn egress_takes_the_providers_rate() {
        let egress: TTSWebSocketConfig =
            serde_json::from_value(json!({ "sample_rate": 16_000 })).unwrap();
        assert_eq!(egress.sample_rate, Some(16_000));
    }

    /// The provider's final words reach the socket as a plain session's would: the caller's
    /// as `stt_result`, the provider's as `assistant_transcript` with the caller's turn; a
    /// stop the session decided drops what the room holds, a barge-in the room cut does not.
    #[test]
    fn the_providers_words_reach_the_socket_and_a_stop_drops_the_rooms_audio() {
        let turns = AtomicU64::new(0);
        let said = |role, text: &str, is_final| TranscriptResult {
            text: text.to_owned(),
            role,
            is_final,
            item_id: None,
        };
        use crate::core::realtime::TranscriptRole::{Assistant, User};
        assert!(transcript_json(&said(User, "When do you", false), &turns).is_none());
        let (heard, to_room) =
            transcript_json(&said(User, "When do you open?", true), &turns).unwrap();
        assert!(!to_room, "the socket's own message");
        let heard: Value = serde_json::from_str(&heard).unwrap();
        assert_eq!(heard["type"], "stt_result");
        assert_eq!(heard["transcript"], "When do you open?");
        let (answer, to_room) =
            transcript_json(&said(Assistant, "At nine.", true), &turns).unwrap();
        assert!(to_room, "an agent line, also on the room's topic");
        let answer: Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(
            answer,
            json!({"type": "assistant_transcript", "turn_index": 1, "delta": "At nine.", "source": "provider"})
        );
        assert!(stops_speech(
            r#"{"type":"agent_audio_stopped","utterance_ids":[],"reason":"policy"}"#
        ));
        assert!(stops_speech(
            r#"{"type":"agent_audio_stopped","utterance_ids":[],"reason":"client_clear"}"#
        ));
        assert!(!stops_speech(
            r#"{"type":"agent_audio_stopped","utterance_ids":[],"reason":"barge_in"}"#
        ));
    }
}
