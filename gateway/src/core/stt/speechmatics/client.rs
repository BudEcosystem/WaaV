//! Speechmatics STT WebSocket Client
//!
//! Real-time speech-to-text using Speechmatics WebSocket streaming API.

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info};

use super::config::SpeechmaticsSTTConfig;
use super::messages::{
    AddPartialTranscriptMessage, AddTranscriptMessage, AdditionalVocabWord, AudioFormat,
    EndOfStreamMessage, ErrorMessage, PunctuationOverrides, Replacement, SpeakerDiarizationConfig,
    StartRecognitionMessage, TranscriptFilteringConfig, TranscriptionConfig,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;
type WsReadStream = futures_util::stream::SplitStream<WsStream>;

/// Per-message idle timeout for WebSocket message reception.
/// Resets after each successful message. Catches stuck/dead connections.
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

// =============================================================================
// Supervised transport (W-D1 production adoption)
// =============================================================================

/// A [`WsTransport`] that adapts Speechmatics' streaming event loop to the generic
/// [`ReconnectableStream`] supervisor (W-D1 fleet adoption). One is built per (re)connect by the
/// supervisor's `connect` closure.
///
/// Unlike Cartesia/Rev AI (all features in the URL → no-op restore), Speechmatics carries its
/// featured session in a **post-handshake `StartRecognition` message** (audio format + the full
/// `transcription_config`: diarization, partials, vocabulary, punctuation, EoU, …). So
/// [`restore_session`](WsTransport::restore_session) re-sends that message on the fresh socket —
/// without it a reconnect would resume as a *bare* (un-featured) session, exactly the failure mode
/// the supervisor doc warns about. [`run`](WsTransport::run) IS the original receiver loop, now
/// returning a [`ReconnectOutcome`] so a transport drop reconnects instead of ending the session.
struct SpeechmaticsTransport {
    ws_sink: WsSink,
    ws_stream: WsReadStream,
    /// Shared inbound audio receiver (single-consumer; locked for the duration of `run`).
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown signal (fires once; an intentional close must not reconnect).
    shutdown_rx: Arc<Mutex<oneshot::Receiver<()>>>,
    /// The featured `StartRecognition` JSON, re-sent verbatim on every restore.
    start_recognition_json: String,
    /// Set true once `RecognitionStarted` arrives (drives `is_ready`); cleared per (re)connect.
    is_session_started: Arc<AtomicBool>,
    /// Audio sequence number (Speechmatics `EndOfStream` carries the last seq_no).
    seq_no: Arc<AtomicU64>,
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,
    /// Fires once after `StartRecognition` is (re)sent, unblocking `connect`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

#[async_trait]
impl WsTransport for SpeechmaticsTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // Speechmatics: re-send the featured `StartRecognition` (audio format +
        // transcription_config) on the fresh socket. A reconnect must NOT resume as a bare session.
        // A fresh connection has not yet observed `RecognitionStarted`, so clear the flag.
        self.is_session_started.store(false, Ordering::SeqCst);
        self.ws_sink
            .send(Message::Text(self.start_recognition_json.clone().into()))
            .await
            .map_err(|e| {
                RestoreError::new(format!("failed to send Speechmatics StartRecognition: {e}"))
            })?;

        // The featured session has been (re)requested: signal the waiting connect() exactly once.
        // (The original client returned readiness right after sending StartRecognition, before
        // RecognitionStarted; we preserve that to keep the happy path identical.)
        if let Some(tx) = self.connected_tx.lock().await.take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        let mut audio_rx = self.audio_rx.lock().await;
        let mut shutdown_rx = self.shutdown_rx.lock().await;
        loop {
            tokio::select! {
                // Handle outgoing audio data (raw binary frames).
                Some(audio_data) = audio_rx.recv() => {
                    if let Err(e) = self
                        .ws_sink
                        .send(Message::Binary(audio_data.to_vec().into()))
                        .await
                    {
                        let stt_error = STTError::NetworkError(format!("Failed to send audio: {e}"));
                        error!("{}", stt_error);
                        if let Some(ref cb) = *self.error_callback.read().await {
                            cb(stt_error).await;
                        }
                        return ReconnectOutcome::Reconnectable(StreamError::new("audio send failed"));
                    }
                    self.seq_no.fetch_add(1, Ordering::SeqCst);
                }

                // Handle incoming messages with idle timeout.
                message = timeout(WS_MESSAGE_TIMEOUT, self.ws_stream.next()) => {
                    match message {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            if let Some(outcome) = self.handle_text_message(&text).await {
                                return outcome;
                            }
                        }
                        Ok(Some(Ok(Message::Binary(_)))) => {
                            debug!("Received unexpected binary message from server");
                        }
                        Ok(Some(Ok(Message::Ping(data)))) => {
                            let _ = self.ws_sink.send(Message::Pong(data)).await;
                        }
                        Ok(Some(Ok(Message::Pong(_)))) => {}
                        Ok(Some(Ok(Message::Close(_)))) => {
                            info!("WebSocket closed by server");
                            self.is_session_started.store(false, Ordering::SeqCst);
                            return ReconnectOutcome::Reconnectable(StreamError::new("server close"));
                        }
                        Ok(Some(Ok(Message::Frame(_)))) => {}
                        Ok(Some(Err(e))) => {
                            error!("WebSocket error: {}", e);
                            self.is_session_started.store(false, Ordering::SeqCst);
                            if let Some(ref cb) = *self.error_callback.read().await {
                                cb(STTError::ConnectionFailed(e.to_string())).await;
                            }
                            return ReconnectOutcome::Reconnectable(StreamError::new("websocket error"));
                        }
                        Ok(None) => {
                            info!("WebSocket stream ended");
                            self.is_session_started.store(false, Ordering::SeqCst);
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            let stt_error = STTError::NetworkError(
                                "Speechmatics WebSocket idle timeout - no message for 60 seconds".into(),
                            );
                            error!("Speechmatics STT idle timeout: {}", stt_error);
                            self.is_session_started.store(false, Ordering::SeqCst);
                            if let Some(ref cb) = *self.error_callback.read().await {
                                cb(stt_error).await;
                            }
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }

                // Handle shutdown signal (intentional close — must NOT reconnect).
                _ = &mut *shutdown_rx => {
                    info!("Speechmatics: Received shutdown signal");
                    let end_msg = EndOfStreamMessage::new(self.seq_no.load(Ordering::SeqCst));
                    if let Ok(json) = serde_json::to_string(&end_msg) {
                        let _ = self.ws_sink.send(Message::Text(json.into())).await;
                    }
                    let _ = self.ws_sink.close().await;
                    return ReconnectOutcome::Completed;
                }
            }
        }
    }
}

impl SpeechmaticsTransport {
    /// Parse and route a Speechmatics text message. Returns `Some(outcome)` when the loop must
    /// exit (provider `Error` → Fatal), `None` to keep running.
    async fn handle_text_message(&self, text: &str) -> Option<ReconnectOutcome> {
        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return None,
        };
        let message_type = value.get("message").and_then(|v| v.as_str()).unwrap_or("");

        match message_type {
            "RecognitionStarted" => {
                info!("Speechmatics session started");
                self.is_session_started.store(true, Ordering::SeqCst);
            }
            "AddPartialTranscript" => {
                if let Ok(msg) = serde_json::from_str::<AddPartialTranscriptMessage>(text) {
                    let transcript = msg.transcript();
                    if !transcript.is_empty()
                        && let Some(callback) = self.result_callback.read().await.as_ref()
                    {
                        let result = STTResult::new(transcript.to_string(), false, false, 0.0);
                        callback(result).await;
                    }
                }
            }
            "AddTranscript" => {
                if let Ok(msg) = serde_json::from_str::<AddTranscriptMessage>(text) {
                    let transcript = msg.transcript();
                    if !transcript.is_empty() {
                        let words: Vec<_> = msg.words().collect();
                        let confidence = if !words.is_empty() {
                            words.iter().map(|w| w.confidence() as f32).sum::<f32>()
                                / words.len() as f32
                        } else {
                            0.9
                        };

                        if let Some(callback) = self.result_callback.read().await.as_ref() {
                            let result =
                                STTResult::new(transcript.to_string(), true, false, confidence);
                            callback(result).await;
                        }
                    }
                }
            }
            "EndOfTranscript" => {
                info!("Speechmatics session ended");
                self.is_session_started.store(false, Ordering::SeqCst);
            }
            "EndOfUtterance" => {
                if let Some(callback) = self.result_callback.read().await.as_ref() {
                    let result = STTResult::new(String::new(), true, true, 1.0);
                    callback(result).await;
                }
            }
            "Error" => {
                if let Ok(msg) = serde_json::from_str::<ErrorMessage>(text) {
                    error!("Speechmatics error: {}", msg);
                    if let Some(callback) = self.error_callback.read().await.as_ref() {
                        callback(STTError::ProviderError(msg.to_string())).await;
                    }
                }
                // A provider error frame is typically fatal (bad config) — don't hammer it with
                // reconnects.
                return Some(ReconnectOutcome::Fatal(StreamError::new("provider error frame")));
            }
            _ => {}
        }
        None
    }
}

/// Speechmatics STT WebSocket client
pub struct SpeechmaticsSTT {
    /// Speechmatics-specific configuration
    config: SpeechmaticsSTTConfig,
    /// Base STT configuration
    base_config: Option<STTConfig>,
    /// Audio sender (bounded channel for backpressure); the supervised transport drains it.
    ws_sender: Option<mpsc::Sender<Bytes>>,
    /// Shutdown signal sender.
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Connection task handle (the supervisor's outer reconnect loop).
    connection_handle: Option<tokio::task::JoinHandle<()>>,
    /// Connection state
    is_connected: Arc<AtomicBool>,
    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before firing `shutdown_tx`, so a client close racing a
    /// server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,
    /// Session started flag
    is_session_started: Arc<AtomicBool>,
    /// Audio sequence number
    seq_no: Arc<AtomicU64>,
    /// Result callback
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,
    /// Error callback
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,
    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`](crate::core::websocket::ReconnectableStream)
    /// supervisor. `None` before `set_resilience` (a direct unit-test construction) → the
    /// supervisor uses its own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl SpeechmaticsSTT {
    /// W1 keystone — construct directly from the standardized config so Speechmatics' rich feature
    /// surface (diarization, interim partials, entity detection, custom vocabulary) is honored
    /// END-TO-END. The flat `BaseSTT::new` path uses `from_base`, which hardcodes those off; this
    /// is the reachable standardized path.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let speechmatics_config = SpeechmaticsSTTConfig::from_standard(std)?;
        speechmatics_config.validate()?;

        info!(
            "Creating Speechmatics STT client (standardized): region={}, language={}, operating_point={}",
            speechmatics_config.region,
            speechmatics_config.language,
            speechmatics_config.operating_point
        );

        Ok(Self {
            config: speechmatics_config,
            base_config: Some(std.base.clone()),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            is_connected: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            is_session_started: Arc::new(AtomicBool::new(false)),
            seq_no: Arc::new(AtomicU64::new(0)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            resilience: None,
        })
    }

    /// The shared circuit breaker this session feeds into the generic supervisor, if the
    /// process-global resilience handles have been injected (W-D1/W-D2). Two `SpeechmaticsSTT`
    /// built from the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(&self) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }

    /// Build the WebSocket URL with authentication
    fn build_ws_url(&self) -> String {
        self.config.ws_url().to_string()
    }

    /// Build the StartRecognition message
    fn build_start_recognition(&self) -> StartRecognitionMessage {
        let audio_format = AudioFormat::raw(self.config.encoding, self.config.sample_rate);

        let mut transcription_config = TranscriptionConfig::new(self.config.language)
            .with_operating_point(self.config.operating_point)
            .with_partials(self.config.enable_partials)
            .with_max_delay(self.config.max_delay);

        // Diarization: an explicit channel/speaker mode override (from the standardized
        // `multichannel`/`diarization` features) takes precedence over the legacy speaker-only flag.
        match self.config.diarization_mode.as_deref() {
            Some(mode) => {
                transcription_config = transcription_config.with_diarization_mode(mode);
            }
            None if self.config.enable_diarization => {
                transcription_config =
                    transcription_config.with_diarization(self.config.max_speakers);
            }
            None => {}
        }

        // Speaker diarization sub-config (sensitivity / prefer-current-speaker / max-speakers).
        if self.config.speaker_sensitivity.is_some()
            || self.config.prefer_current_speaker.is_some()
            || (self.config.diarization_mode.is_some() && self.config.max_speakers.is_some())
        {
            transcription_config =
                transcription_config.with_speaker_diarization_config(SpeakerDiarizationConfig {
                    max_speakers: self.config.max_speakers,
                    speaker_sensitivity: self.config.speaker_sensitivity,
                    prefer_current_speaker: self.config.prefer_current_speaker,
                });
        }

        // Custom vocabulary: fold per-word phonetic hints (`sounds_like`) in where present.
        if !self.config.additional_vocab.is_empty() {
            let words: Vec<AdditionalVocabWord> = self
                .config
                .additional_vocab
                .iter()
                .map(|w| match self.config.vocab_sounds_like.get(w) {
                    Some(hints) => AdditionalVocabWord::with_sounds_like(w.clone(), hints.clone()),
                    None => AdditionalVocabWord::new(w.clone()),
                })
                .collect();
            transcription_config = transcription_config.with_vocab_words(words);
        }

        // End-of-utterance silence trigger (turn detection).
        if let Some(secs) = self.config.end_of_utterance_silence_trigger {
            transcription_config =
                transcription_config.with_end_of_utterance_silence_trigger(secs);
        }

        // Transcript filtering: disfluency removal + find-and-replace.
        if self.config.remove_disfluencies.is_some() || self.config.replacements.is_some() {
            transcription_config =
                transcription_config.with_transcript_filtering_config(TranscriptFilteringConfig {
                    remove_disfluencies: self.config.remove_disfluencies,
                    replacements: self.config.replacements.as_ref().map(|reps| {
                        reps.iter()
                            .map(|(from, to)| Replacement {
                                from: from.clone(),
                                to: to.clone(),
                            })
                            .collect()
                    }),
                });
        }

        // Punctuation overrides: permitted marks + sensitivity.
        if self.config.permitted_marks.is_some() || self.config.punctuation_sensitivity.is_some() {
            transcription_config =
                transcription_config.with_punctuation_overrides(PunctuationOverrides {
                    permitted_marks: self.config.permitted_marks.clone(),
                    sensitivity: self.config.punctuation_sensitivity,
                });
        }

        if let Some(ref locale) = self.config.output_locale {
            transcription_config = transcription_config.with_output_locale(locale.clone());
        }
        if let Some(ref domain) = self.config.domain {
            transcription_config = transcription_config.with_domain(domain.clone());
        }
        if let Some(ref mode) = self.config.max_delay_mode {
            transcription_config = transcription_config.with_max_delay_mode(mode.clone());
        }

        StartRecognitionMessage::with_config(audio_format, transcription_config)
    }
}

#[async_trait]
impl BaseSTT for SpeechmaticsSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        let speechmatics_config = SpeechmaticsSTTConfig::from_base(&config)?;
        speechmatics_config.validate()?;

        info!(
            "Creating Speechmatics STT client: region={}, language={}, operating_point={}",
            speechmatics_config.region,
            speechmatics_config.language,
            speechmatics_config.operating_point
        );

        Ok(Self {
            config: speechmatics_config,
            base_config: Some(config),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            is_connected: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            is_session_started: Arc::new(AtomicBool::new(false)),
            seq_no: Arc::new(AtomicU64::new(0)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            resilience: None,
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.is_connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        let ws_url = self.build_ws_url();
        info!("Connecting to Speechmatics: {}", ws_url);

        let api_key = self.config.api_key.clone();
        // Serialize the featured StartRecognition once; the supervised transport re-sends it
        // verbatim on every (re)connect.
        let start_msg = self.build_start_recognition();
        let start_recognition_json = serde_json::to_string(&start_msg)
            .map_err(|e| STTError::ProviderError(format!("Failed to serialize: {}", e)))?;

        // Create channels for communication (bounded for backpressure on audio).
        let (ws_tx, ws_rx) = mpsc::channel::<Bytes>(32);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        self.ws_sender = Some(ws_tx);
        self.shutdown_tx = Some(shutdown_tx);
        self.seq_no.store(0, Ordering::SeqCst);

        // Shared state the supervised transport re-uses across reconnect attempts.
        let audio_rx = Arc::new(Mutex::new(ws_rx));
        let shutdown_rx = Arc::new(Mutex::new(shutdown_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));

        let result_callback = Arc::clone(&self.result_callback);
        let error_callback = Arc::clone(&self.error_callback);
        let is_session_started = Arc::clone(&self.is_session_started);
        let seq_no = Arc::clone(&self.seq_no);

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor with
        // the shared process-global handles from CoreState (W-D1/W-D2 fleet adoption). When no
        // handles were injected, the supervisor uses its own per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("speechmatics", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => ReconnectableStream::new(ReconnectableStreamConfig::new(
                "speechmatics",
                reconnection,
            )),
        }
        .with_disconnect_flag(disconnect_flag);

        // Start the connection task: the supervisor owns the outer reconnect loop; the `connect`
        // closure dials with the Authorization header and hands back a transport whose
        // `restore_session` re-sends StartRecognition and whose `run()` is the original receiver
        // loop.
        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let ws_url = ws_url.clone();
                    let api_key = api_key.clone();
                    let start_recognition_json = start_recognition_json.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let shutdown_rx = Arc::clone(&shutdown_rx);
                    let connected_tx = Arc::clone(&connected_tx);
                    let result_callback = Arc::clone(&result_callback);
                    let error_callback = Arc::clone(&error_callback);
                    let is_session_started = Arc::clone(&is_session_started);
                    let seq_no = Arc::clone(&seq_no);
                    async move {
                        // Derive the Host header from the ACTUAL dial URL — hardcoding the EU host
                        // broke every US-region session (dialing us.rt.speechmatics.com while
                        // sending `Host: eu.rt.speechmatics.com` is a Host/SNI mismatch the server
                        // rejects). tokio-tungstenite only auto-derives Host for a URL string, not a
                        // hand-built Request, so we must set it ourselves to match the region.
                        let ws_host = ws_url
                            .split("://")
                            .nth(1)
                            .and_then(|rest| rest.split(['/', '?']).next())
                            .unwrap_or("eu.rt.speechmatics.com");
                        let request = http::Request::builder()
                            .uri(&ws_url)
                            .header("Authorization", format!("Bearer {}", api_key))
                            .header("Sec-WebSocket-Protocol", "json")
                            .header("Host", ws_host)
                            .header("Connection", "Upgrade")
                            .header("Upgrade", "websocket")
                            .header("Sec-WebSocket-Version", "13")
                            .header(
                                "Sec-WebSocket-Key",
                                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
                            )
                            .body(())
                            .map_err(|e| {
                                StreamError::new(format!("Failed to build request: {e}"))
                            })?;

                        let (ws_stream, _) = tokio_tungstenite::connect_async(request)
                            .await
                            .map_err(|e| {
                                StreamError::new(format!("WebSocket connect failed: {e}"))
                            })?;
                        info!("Speechmatics connected");
                        let (ws_sink, ws_stream) = ws_stream.split();
                        Ok(SpeechmaticsTransport {
                            ws_sink,
                            ws_stream,
                            audio_rx,
                            shutdown_rx,
                            start_recognition_json,
                            is_session_started,
                            seq_no,
                            result_callback,
                            error_callback,
                            connected_tx,
                        })
                    }
                })
                .await;
            info!("Speechmatics STT WebSocket connection closed (supervisor exit: {exit:?})");
        });

        self.connection_handle = Some(connection_handle);

        // Wait for the first successful connect (restore_session fires the connected signal right
        // after StartRecognition is sent — matching the original "ready after StartRecognition"
        // semantics, which did not block on RecognitionStarted).
        match timeout(Duration::from_secs(10), connected_rx).await {
            Ok(Ok(())) => {
                self.is_connected.store(true, Ordering::SeqCst);
                info!("Speechmatics connected, sent StartRecognition");
                Ok(())
            }
            Ok(Err(_)) => Err(STTError::ConnectionFailed(
                "Connection channel closed before Speechmatics session started".to_string(),
            )),
            Err(_) => Err(STTError::ConnectionFailed(
                "Connection timeout waiting for Speechmatics session".to_string(),
            )),
        }
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Record the intent BEFORE the connected-guard so the supervisor sees it even if the
        // transport's run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        if !self.is_connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        // Signal the supervised transport to send EndOfStream + close intentionally (no reconnect).
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        if let Some(handle) = self.connection_handle.take() {
            let _ = timeout(Duration::from_secs(5), handle).await;
        }

        self.ws_sender = None;
        self.is_connected.store(false, Ordering::SeqCst);
        self.is_session_started.store(false, Ordering::SeqCst);

        info!("Speechmatics disconnected");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_connected.load(Ordering::SeqCst) && self.is_session_started.load(Ordering::SeqCst)
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_connected.load(Ordering::SeqCst) {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        if let Some(ws_sender) = &self.ws_sender {
            ws_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio: {}", e)))?;
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.result_callback.write().await = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.error_callback.write().await = Some(callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        self.base_config.as_ref()
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        let was_connected = self.is_connected.load(Ordering::SeqCst);

        if was_connected {
            self.disconnect().await?;
        }

        self.config = SpeechmaticsSTTConfig::from_base(&config)?;
        self.config.validate()?;
        self.base_config = Some(config);

        if was_connected {
            self.connect().await?;
        }

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Speechmatics Real-time STT (55+ languages, WebSocket streaming)"
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `connect` drives the generic
        // ReconnectableStream supervisor with them — every Speechmatics session trips the same
        // breaker and shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

impl Drop for SpeechmaticsSTT {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(handle) = self.connection_handle.take() {
            handle.abort();
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speechmatics_stt_creation() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            encoding: "pcm_s16le".to_string(),
            ..Default::default()
        };

        let stt = SpeechmaticsSTT::new(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_speechmatics_stt_requires_api_key() {
        let config = STTConfig::default();
        let result = SpeechmaticsSTT::new(config);
        assert!(result.is_err());
    }

    // W-D1: disconnect() must record intent on the supervisor-shared flag so a client close racing
    // a server-side close can never trigger a spurious reconnect (the supervisor's loop-top guard
    // observes this same `Arc<AtomicBool>`). Before this wiring the flag was the supervisor's own
    // and disconnect() never set it.
    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            encoding: "pcm_s16le".to_string(),
            ..Default::default()
        };
        let mut stt = SpeechmaticsSTT::new(config).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the supervisor-shared intentional-disconnect flag",
        );
    }

    // W1 keystone: Speechmatics' rich advanced features (diarization, interim partials, entity
    // detection, custom vocabulary) must survive THROUGH `new_standard` into the provider's
    // config — not just the config-level `from_standard`. The flat `new` path leaves them off.
    #[test]
    fn test_new_standard_unlocks_advanced_features() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "speechmatics".into(),
                api_key: "test-api-key".into(),
                language: "en".into(),
                sample_rate: 16000,
                encoding: "pcm_s16le".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                interim_results: Some(true),
                entity_detection: Some(true),
                keyterms: Some(vec!["WaaV".into(), "Speechmatics".into()]),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let stt = SpeechmaticsSTT::new_standard(&std).unwrap();
        assert!(stt.config.enable_diarization);
        assert!(stt.config.enable_partials);
        assert!(stt.config.enable_entities);
        assert_eq!(stt.config.additional_vocab, vec!["WaaV", "Speechmatics"]);

        // Missing api_key is rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig::default());
        assert!(SpeechmaticsSTT::new_standard(&bad).is_err());
    }

    #[test]
    fn test_build_start_recognition() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "fr".to_string(),
            sample_rate: 44100,
            encoding: "pcm_f32le".to_string(),
            ..Default::default()
        };

        let stt = SpeechmaticsSTT::new(config).unwrap();
        let msg = stt.build_start_recognition();

        assert_eq!(msg.message, "StartRecognition");
        assert_eq!(msg.audio_format.format_type, "raw");
        assert_eq!(msg.audio_format.sample_rate, Some(44100));
        assert_eq!(msg.transcription_config.language, "fr");
    }

    // WIRE-LEVEL: every standardized Speechmatics knob must travel from the standardized config
    // into the SERIALIZED `transcription_config` JSON of the StartRecognition message — the bytes
    // that actually reach `wss://{eu,us}.rt.speechmatics.com/v2`, not merely the config struct.
    // Guards the recurring "set on the struct but never emitted to the wire" gap class.
    #[test]
    fn standardized_features_reach_start_recognition_json() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let mut extras = serde_json::Map::new();
        extras.insert("speaker_sensitivity".into(), serde_json::json!(0.7));
        extras.insert("prefer_current_speaker".into(), serde_json::json!(true));
        extras.insert(
            "permitted_marks".into(),
            serde_json::json!([".", ",", "?"]),
        );
        extras.insert("punctuation_sensitivity".into(), serde_json::json!(0.4));
        extras.insert(
            "replacements".into(),
            serde_json::json!([{"from": "gonna", "to": "going to"}]),
        );
        extras.insert("output_locale".into(), serde_json::json!("en-US"));
        extras.insert("domain".into(), serde_json::json!("finance"));
        extras.insert("max_delay_mode".into(), serde_json::json!("flexible"));
        extras.insert(
            "additional_vocab".into(),
            serde_json::json!([{"content": "Speechmatics", "sounds_like": ["speech matics"]}]),
        );

        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "speechmatics".into(),
                api_key: "test-api-key".into(),
                language: "en".into(),
                sample_rate: 16000,
                encoding: "pcm_s16le".into(),
                ..Default::default()
            },
            features: SttFeatures {
                // channel diarization mode: multichannel + speaker -> "channel_and_speaker"
                multichannel: Some(true),
                diarization: Some(true),
                // end-of-utterance silence trigger: 750ms -> 0.75s
                utterance_end_ms: Some(750),
                // disfluency removal: filler_words=false -> remove_disfluencies=true
                filler_words: Some(false),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };

        let stt = SpeechmaticsSTT::new_standard(&std).unwrap();
        let msg = stt.build_start_recognition();
        let json = serde_json::to_string(&msg).unwrap();

        // -- typed features --
        assert!(
            json.contains("\"diarization\":\"channel_and_speaker\""),
            "channel diarization mode missing: {json}"
        );
        assert!(
            json.contains("\"end_of_utterance_silence_trigger\":0.75"),
            "end-of-utterance silence trigger missing: {json}"
        );
        assert!(
            json.contains("\"conversation_config\""),
            "conversation_config missing: {json}"
        );
        assert!(
            json.contains("\"remove_disfluencies\":true"),
            "remove_disfluencies missing: {json}"
        );
        assert!(
            json.contains("\"transcript_filtering_config\""),
            "transcript_filtering_config missing: {json}"
        );

        // -- extras passthrough --
        assert!(
            json.contains("\"speaker_sensitivity\":0.7"),
            "speaker_sensitivity missing: {json}"
        );
        assert!(
            json.contains("\"prefer_current_speaker\":true"),
            "prefer_current_speaker missing: {json}"
        );
        assert!(
            json.contains("\"punctuation_overrides\""),
            "punctuation_overrides missing: {json}"
        );
        assert!(
            json.contains("\"permitted_marks\":[\".\",\",\",\"?\"]"),
            "permitted_marks missing: {json}"
        );
        assert!(
            json.contains("\"sensitivity\":0.4"),
            "punctuation sensitivity missing: {json}"
        );
        assert!(
            json.contains("\"from\":\"gonna\"") && json.contains("\"to\":\"going to\""),
            "replacements missing: {json}"
        );
        assert!(
            json.contains("\"output_locale\":\"en-US\""),
            "output_locale missing: {json}"
        );
        assert!(json.contains("\"domain\":\"finance\""), "domain missing: {json}");
        assert!(
            json.contains("\"max_delay_mode\":\"flexible\""),
            "max_delay_mode missing: {json}"
        );
        // additional_vocab carries the phonetic hints on the word.
        assert!(
            json.contains("\"content\":\"Speechmatics\""),
            "vocab content missing: {json}"
        );
        assert!(
            json.contains("\"sounds_like\":[\"speech matics\"]"),
            "vocab sounds_like hint missing: {json}"
        );
    }

    #[test]
    fn test_build_ws_url() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            ..Default::default()
        };

        let stt = SpeechmaticsSTT::new(config).unwrap();
        let url = stt.build_ws_url();

        assert!(url.starts_with("wss://"));
        assert!(url.contains("speechmatics.com"));
    }

    #[tokio::test]
    async fn test_speechmatics_stt_send_audio_not_connected() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            ..Default::default()
        };

        let mut stt = SpeechmaticsSTT::new(config).unwrap();
        let result = stt.send_audio(Bytes::from_static(b"test")).await;

        assert!(result.is_err());
    }

    #[test]
    fn test_get_provider_info() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            ..Default::default()
        };

        let stt = SpeechmaticsSTT::new(config).unwrap();
        let info = stt.get_provider_info();

        assert!(info.contains("Speechmatics"));
    }

    #[test]
    fn test_get_config() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            ..Default::default()
        };

        let stt = SpeechmaticsSTT::new(config.clone()).unwrap();
        let retrieved_config = stt.get_config();

        assert!(retrieved_config.is_some());
        assert_eq!(retrieved_config.unwrap().language, "en");
    }
}
