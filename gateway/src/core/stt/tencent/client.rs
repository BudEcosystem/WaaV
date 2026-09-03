//! Tencent Cloud Speech Recognition WebSocket Client
//!
//! This module implements the `BaseSTT` trait for Tencent Cloud's
//! real-time Speech-to-Text service.
//!
//! # Architecture
//!
//! The client uses WebSocket streaming with HMAC-SHA1 signature authentication.
//! Audio is sent in 40ms chunks (1280 bytes at 16kHz).
//!
//! # WebSocket Message Flow
//!
//! ```text
//! Client                              Server
//!   |                                    |
//!   |------ Connect with signature ----->|
//!   |<----- HTTP 101 Upgrade ------------|
//!   |                                    |
//!   |------ Binary audio (40ms) -------->|
//!   |<----- JSON (slice_type=0) ---------|  (interim)
//!   |                                    |
//!   |------ Binary audio (40ms) -------->|
//!   |<----- JSON (slice_type=1) ---------|  (segment end)
//!   |                                    |
//!   |------ Close or timeout ----------->|
//!   |<----- JSON (final=1) --------------|  (complete)
//! ```
//!
//! # Authentication
//!
//! Signature parameters are embedded in the WebSocket URL:
//! `wss://asr.cloud.tencent.com/asr/v2/{app_id}?secretid=...&signature=...`

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::config::{TENCENT_ASR_WS_URL, TencentSttConfig};
use super::messages::TencentAsrResponse;
use super::signature::TencentSignatureBuilder;
use crate::core::resilience::connect::with_timeout;
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};
use futures::stream::{SplitSink, SplitStream};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

// =============================================================================
// Constants
// =============================================================================

/// Provider information string.
const PROVIDER_INFO: &str = "Tencent Cloud Speech (腾讯云语音)";

/// Per-message idle timeout for WebSocket message reception. Resets after each successful message;
/// catches stuck/dead connections so a silently-dropped socket reconnects instead of hanging.
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// The concrete WebSocket stream type Tencent dials.
type TencentWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A [`WsTransport`] that adapts Tencent ASR's streaming event loop to the generic
/// [`ReconnectableStream`] supervisor (W-D1 fleet adoption). One is built per (re)connect by the
/// supervisor's `connect` closure.
///
/// Like Cartesia, Tencent carries every feature (engine model, VAD, profanity/modal filtering,
/// word info, number conversion, hot words, …) in its **signed connect URL**, so
/// [`restore_session`](WsTransport::restore_session) is a no-op beyond signalling the waiting
/// `connect()` once — a fresh dial already restored the featured session. The original Tencent
/// client split the socket into independent send/recv tasks that `break` on the first transport
/// error (ending the session); [`run`](WsTransport::run) consolidates them into one `select!` loop
/// returning a [`ReconnectOutcome`] so a mid-stream drop reconnects instead.
struct TencentTransport {
    ws_sink: SplitSink<TencentWs, Message>,
    ws_stream: SplitStream<TencentWs>,
    /// Shared inbound audio receiver (single-consumer; locked for the duration of `run`).
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown token (fires once; an intentional close must not reconnect).
    shutdown_token: CancellationToken,
    result_tx: mpsc::Sender<STTResult>,
    error_tx: mpsc::Sender<STTError>,
    /// Fires once on the first successful connect, unblocking `connect()`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    /// Connection-ready flag mirrored from the client so `is_ready` flips false on a transport drop.
    connected_flag: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl WsTransport for TencentTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // Tencent puts every feature in the signed connect URL, so a fresh dial already restored the
        // featured session — nothing to re-send. Signal the waiting connect() exactly once.
        self.connected_flag.store(true, Ordering::SeqCst);
        if let Some(tx) = self.connected_tx.lock().await.take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        let mut audio_rx = self.audio_rx.lock().await;
        let shutdown_token = self.shutdown_token.clone();
        loop {
            if shutdown_token.is_cancelled() {
                debug!("Tencent shutdown signal received");
                let _ = self.ws_sink.send(Message::Close(None)).await;
                return ReconnectOutcome::Completed;
            }

            tokio::select! {
                // Handle outgoing audio data (raw binary frames)
                Some(audio) = audio_rx.recv() => {
                    if let Err(e) = self.ws_sink.send(Message::Binary(audio.to_vec().into())).await {
                        let stt_error = STTError::NetworkError(format!(
                            "Failed to send audio to Tencent: {e}"
                        ));
                        error!("{}", stt_error);
                        let _ = self.error_tx.try_send(stt_error);
                        // Transport-level send failure: reconnect to preserve the session.
                        return ReconnectOutcome::Reconnectable(StreamError::new("audio send failed"));
                    }
                }

                // Handle incoming messages with idle timeout
                message = timeout(WS_MESSAGE_TIMEOUT, self.ws_stream.next()) => {
                    match message {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            TencentStt::handle_response(&text, &self.result_tx, &self.error_tx);
                        }
                        Ok(Some(Ok(Message::Close(_)))) => {
                            debug!("Tencent WebSocket closed");
                            // Server-initiated close after the recognition is complete — an
                            // intentional end-of-session, NOT a transport drop.
                            return ReconnectOutcome::Completed;
                        }
                        Ok(Some(Ok(Message::Ping(data)))) => {
                            debug!("Received ping from Tencent, len={}", data.len());
                        }
                        Ok(Some(Ok(_))) => {
                            // Pong / other frames: ignore.
                        }
                        Ok(Some(Err(e))) => {
                            error!("Tencent WebSocket error: {}", e);
                            let _ = self.error_tx.try_send(STTError::ConnectionFailed(e.to_string()));
                            return ReconnectOutcome::Reconnectable(StreamError::new("websocket error"));
                        }
                        Ok(None) => {
                            info!("Tencent WebSocket stream ended");
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            let stt_error = STTError::NetworkError(
                                "Tencent WebSocket idle timeout - no message for 60 seconds".into()
                            );
                            error!("{}", stt_error);
                            let _ = self.error_tx.try_send(stt_error);
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }

                // Handle shutdown signal (intentional close — must NOT reconnect)
                _ = shutdown_token.cancelled() => {
                    debug!("Tencent shutdown signal received");
                    let _ = self.ws_sink.send(Message::Close(None)).await;
                    return ReconnectOutcome::Completed;
                }
            }
        }
    }
}

/// Channel buffer size for audio frames.
const AUDIO_CHANNEL_BUFFER: usize = 64;

/// Channel buffer size for results.
const RESULT_CHANNEL_BUFFER: usize = 256;

/// Channel buffer size for errors.
const ERROR_CHANNEL_BUFFER: usize = 64;

// =============================================================================
// Type Aliases
// =============================================================================

/// Async callback type for STT results.
type AsyncSTTCallback = Box<
    dyn Fn(STTResult) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Async callback type for errors.
type AsyncErrorCallback = Box<
    dyn Fn(STTError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

// =============================================================================
// Tencent STT Client
// =============================================================================

/// Tencent Cloud Speech-to-Text client.
///
/// Provides real-time speech recognition using WebSocket streaming.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::tencent::TencentStt;
///
/// let config = STTConfig {
///     // API key format: secret_id|secret_key|app_id
///     api_key: "your_secret_id|your_secret_key|your_app_id".to_string(),
///     language: "zh".to_string(),
///     sample_rate: 16000,
///     encoding: "pcm".to_string(),
///     model: "16k_zh".to_string(),
///     ..Default::default()
/// };
///
/// let mut stt = TencentStt::new(config)?;
/// stt.connect().await?;
/// stt.send_audio(audio_data).await?;
/// stt.disconnect().await?;
/// ```
pub struct TencentStt {
    /// Base configuration for BaseSTT trait.
    base_config: STTConfig,

    /// Tencent-specific configuration.
    config: TencentSttConfig,

    /// Connection state.
    connected: Arc<AtomicBool>,

    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before cancelling `shutdown_token`, so a client close racing a
    /// server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,

    /// State change notification.
    state_notify: Arc<Notify>,

    /// WebSocket sender for audio data.
    ws_sender: Option<mpsc::Sender<Bytes>>,

    /// Shutdown signal token.
    shutdown_token: Option<CancellationToken>,

    /// Connection task handle.
    connection_handle: Option<tokio::task::JoinHandle<()>>,

    /// Result forwarding task handle.
    result_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Error forwarding task handle.
    error_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Result callback storage.
    result_callback: Arc<Mutex<Option<AsyncSTTCallback>>>,

    /// Error callback storage.
    error_callback: Arc<Mutex<Option<AsyncErrorCallback>>>,

    /// Current voice ID for the session.
    voice_id: String,

    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`](crate::core::websocket::ReconnectableStream)
    /// supervisor. `None` before `set_resilience` (a direct unit-test construction) → the
    /// supervisor uses its own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl TencentStt {
    /// Create a new Tencent STT client.
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        let tencent_config = TencentSttConfig::from_base(config.clone())?;
        Self::from_tencent_config(config, tencent_config)
    }

    /// W1 keystone — construct directly from the standardized config so the advanced features
    /// Tencent ASR can express (word timestamps, profanity/modal filtering, VAD events,
    /// endpointing, hot words) are honored END-TO-END. The flat `BaseSTT::new` path uses
    /// `from_base`, which hardcodes those defaults; this is the reachable standardized path.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let tencent_config = TencentSttConfig::from_standard(std)?;
        Self::from_tencent_config(std.base.clone(), tencent_config)
    }

    /// Internal: construct the provider from an already-mapped Tencent config.
    fn from_tencent_config(
        base_config: STTConfig,
        tencent_config: TencentSttConfig,
    ) -> Result<Self, STTError> {
        tencent_config.validate()?;

        let voice_id = TencentSignatureBuilder::generate_voice_id();

        Ok(Self {
            base_config,
            config: tencent_config,
            connected: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_token: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            voice_id,
            resilience: None,
        })
    }

    /// Build the WebSocket URL with signature.
    fn build_ws_url(&self) -> Result<String, STTError> {
        let mut signature_builder =
            TencentSignatureBuilder::new(&self.config.secret_id, &self.config.secret_key)
                .with_engine_model(self.config.engine_model_type.as_str())
                .with_voice_format(self.config.voice_format.value())
                .with_voice_id(&self.voice_id)
                .with_needvad(self.config.needvad)
                .with_filter_dirty(self.config.filter_dirty.value() as u32)
                .with_filter_modal(self.config.filter_modal.value() as u32)
                .with_filter_punc(self.config.filter_punc)
                .with_word_info(self.config.word_info.value() as u32)
                .with_convert_num_mode(self.config.convert_num_mode.value() as u32);

        // Add reinforce_hotword if enabled
        if self.config.reinforce_hotword {
            signature_builder = signature_builder.with_reinforce_hotword(true);
        }

        // Add filter_empty_result if enabled
        if self.config.filter_empty_result {
            signature_builder = signature_builder.with_filter_empty_result(true);
        }

        // Add optional VAD silence time
        if let Some(vad_time) = self.config.vad_silence_time {
            signature_builder = signature_builder.with_vad_silence_time(vad_time);
        }

        // Add optional max speak time
        if let Some(max_speak) = self.config.max_speak_time {
            signature_builder = signature_builder.with_max_speak_time(max_speak);
        }

        // Add optional hotword ID
        if let Some(ref hotword_id) = self.config.hotword_id {
            signature_builder = signature_builder.with_hotword_id(hotword_id);
        }

        // Add optional hotword list
        if let Some(ref hotword_list) = self.config.hotword_list {
            signature_builder = signature_builder.with_hotword_list(hotword_list);
        }

        // Add optional customization ID
        if let Some(ref customization_id) = self.config.customization_id {
            signature_builder = signature_builder.with_customization_id(customization_id);
        }

        // Add optional noise filter threshold (extras passthrough)
        if let Some(noise_threshold) = self.config.noise_threshold {
            signature_builder = signature_builder.with_noise_threshold(noise_threshold);
        }

        // Add optional input sample rate (extras passthrough)
        if let Some(input_sample_rate) = self.config.input_sample_rate {
            signature_builder = signature_builder.with_input_sample_rate(input_sample_rate);
        }

        let url = signature_builder
            .build_url(TENCENT_ASR_WS_URL, &self.config.app_id)
            .map_err(|e| {
                STTError::ConfigurationError(format!("Failed to build signature: {}", e))
            })?;
        // Honor an `endpoint_override` for the in-repo mock/proxy: swap the dialed scheme://host while
        // keeping the signed `/asr/v2/{app_id}?...` path+query (the mock ignores the HMAC signature).
        if let Some(o) = self
            .config
            .endpoint_override
            .as_deref()
            .map(str::trim)
            .filter(|o| !o.is_empty())
            && let Some(idx) = url.find("/asr/v2")
        {
            return Ok(format!("{}{}", o.trim_end_matches('/'), &url[idx..]));
        }
        Ok(url)
    }

    /// Handle a WebSocket text message (JSON response).
    fn handle_response(
        text: &str,
        result_tx: &mpsc::Sender<STTResult>,
        error_tx: &mpsc::Sender<STTError>,
    ) {
        match TencentAsrResponse::from_json(text) {
            Ok(response) => {
                if response.is_error() {
                    if let Some(err_msg) = response.get_error_message() {
                        warn!("Tencent ASR error: {}", err_msg);
                        let _ = error_tx.try_send(STTError::ProviderError(err_msg));
                    }
                } else if let Some(transcript) = response.get_transcript() {
                    // Determine if this is a final result
                    let is_final = response.is_segment_final()
                        || response.is_session_final()
                        || response
                            .result
                            .as_ref()
                            .map(|r| r.is_recognition_final())
                            .unwrap_or(false);

                    let result = STTResult::new(
                        transcript.to_string(),
                        is_final,
                        response.is_session_final(),
                        1.0, // Tencent doesn't provide confidence scores
                    );

                    debug!(
                        "Tencent transcript ({}): {}",
                        if is_final { "final" } else { "interim" },
                        transcript
                    );

                    let _ = result_tx.try_send(result);
                }
            }
            Err(e) => {
                warn!("Failed to parse Tencent response: {} - raw: {}", e, text);
            }
        }
    }

    /// Get the recommended chunk size for audio streaming.
    pub fn get_chunk_size(&self) -> usize {
        self.config.get_chunk_size()
    }

    /// Get the current voice ID.
    pub fn voice_id(&self) -> &str {
        &self.voice_id
    }

    /// Generate a new voice ID for the next session.
    fn regenerate_voice_id(&mut self) {
        self.voice_id = TencentSignatureBuilder::generate_voice_id();
    }
}

#[async_trait::async_trait]
impl BaseSTT for TencentStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        TencentStt::new(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        // Build URL with signature (carries every feature — Cartesia-style features-in-URL).
        let url = self.build_ws_url()?;

        info!(
            "Connecting to Tencent ASR: {}",
            url.split('?').next().unwrap_or(&url)
        );

        // Create channels
        let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER);
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(RESULT_CHANNEL_BUFFER);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(ERROR_CHANNEL_BUFFER);
        let shutdown_token = CancellationToken::new();
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        self.ws_sender = Some(audio_tx);
        self.shutdown_token = Some(shutdown_token.clone());

        // Shared state the supervised transport re-uses across reconnect attempts: a single-
        // consumer audio receiver + shutdown token and the one-shot connected
        // signal that fires on the first successful connect.
        let audio_rx = Arc::new(Mutex::new(audio_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));
        let connected_flag = self.connected.clone();
        let state_notify = self.state_notify.clone();

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor (the
        // same one the chaos tests exercise) with the shared process-global handles from CoreState
        // (W-D1/W-D2 fleet adoption). When no handles were injected (a direct unit-test
        // construction), the supervisor uses its own per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("tencent", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => {
                ReconnectableStream::new(ReconnectableStreamConfig::new("tencent", reconnection))
            }
        }
        .with_disconnect_flag(disconnect_flag);

        // Spawn the supervised connection task: the supervisor owns the outer reconnect loop; the
        // `connect` closure dials the *featured* (signed) URL and hands back a transport whose
        // `run()` is the consolidated Tencent event loop.
        let connection_handle = tokio::spawn(async move {
            let exit =
                supervisor
                    .run(|| {
                        let url = url.clone();
                        let audio_rx = Arc::clone(&audio_rx);
                        let shutdown_token = shutdown_token.clone();
                        let connected_tx = Arc::clone(&connected_tx);
                        let connected_flag = connected_flag.clone();
                        let result_tx = result_tx.clone();
                        let error_tx = error_tx.clone();
                        async move {
                            // Deadline-bounded dial via the shared resilience helper. Tencent keeps
                            // its historical 10s bound (tighter than the canonical 15s
                            // `core::resilience::connect::WS_CONNECT_TIMEOUT`).
                            let (ws_stream, _) =
                                match with_timeout(Duration::from_secs(10), connect_async(&url))
                                    .await
                                {
                                    Ok(Ok(result)) => result,
                                    Ok(Err(e)) => {
                                        return Err(StreamError::new(format!(
                                            "WebSocket connection failed: {e}"
                                        )));
                                    }
                                    Err(_) => {
                                        return Err(StreamError::new("connection timeout"));
                                    }
                                };
                            info!("Connected to Tencent ASR");
                            let (ws_sink, ws_stream) = ws_stream.split();
                            Ok(TencentTransport {
                                ws_sink,
                                ws_stream,
                                audio_rx,
                                shutdown_token,
                                result_tx,
                                error_tx,
                                connected_tx,
                                connected_flag,
                            })
                        }
                    })
                    .await;
            connected_flag.store(false, Ordering::SeqCst);
            state_notify.notify_waiters();
            info!("Tencent STT WebSocket connection closed (supervisor exit: {exit:?})");
        });

        self.connection_handle = Some(connection_handle);

        // Spawn result forwarding task
        let result_callback = self.result_callback.clone();
        let result_forward_handle = tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                let callback = result_callback.lock().await;
                if let Some(cb) = callback.as_ref() {
                    cb(result).await;
                }
            }
        });

        self.result_forward_handle = Some(result_forward_handle);

        // Spawn error forwarding task
        let error_callback = self.error_callback.clone();
        let error_forward_handle = tokio::spawn(async move {
            while let Some(error) = error_rx.recv().await {
                let callback = error_callback.lock().await;
                if let Some(cb) = callback.as_ref() {
                    cb(error).await;
                }
            }
        });

        self.error_forward_handle = Some(error_forward_handle);

        // Wait for the first successful connect (the supervisor's restore signals `connected_tx`),
        // matching the other supervised providers — `connect()` returns ready only once the session
        // is live, and the `connected` flag drives `is_ready`.
        match timeout(Duration::from_secs(10), connected_rx).await {
            Ok(Ok(())) => {
                self.state_notify.notify_waiters();
                info!("Tencent STT connected successfully");
                Ok(())
            }
            Ok(Err(_)) => Err(STTError::ConnectionFailed(
                "Connection channel closed".to_string(),
            )),
            Err(_) => Err(STTError::ConnectionFailed("Connection timeout".to_string())),
        }
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Always regenerate voice ID for next session, even if not connected
        // This ensures fresh IDs after connection failures
        self.regenerate_voice_id();

        // Record the intent BEFORE the connected-guard so the supervisor sees it even if the
        // transport's run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        if !self.connected.load(Ordering::SeqCst) && self.connection_handle.is_none() {
            return Ok(());
        }

        info!("Disconnecting from Tencent STT...");

        // Drop audio sender to signal end
        self.ws_sender.take();

        // Send shutdown signal
        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }

        // Wait for connection task to complete
        if let Some(handle) = self.connection_handle.take() {
            crate::core::observability::await_task_shutdown(
                "tencent-stt-connection",
                handle,
                Duration::from_secs(5),
            )
            .await;
        }

        // Abort forwarding tasks
        if let Some(handle) = self.result_forward_handle.take() {
            crate::core::observability::abort_and_await_task(
                "tencent-stt-result-forwarder",
                handle,
            )
            .await;
        }

        if let Some(handle) = self.error_forward_handle.take() {
            crate::core::observability::abort_and_await_task("tencent-stt-error-forwarder", handle)
                .await;
        }

        self.connected.store(false, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        info!("Disconnected from Tencent STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn send_audio(&mut self, audio: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        if let Some(sender) = &self.ws_sender {
            sender.send(audio).await.map_err(|_| {
                STTError::ProviderError("Failed to send audio to channel".to_string())
            })?;
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        let async_callback: AsyncSTTCallback = Box::new(move |result| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(result).await;
            })
        });

        *self.result_callback.lock().await = Some(async_callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        let async_callback: AsyncErrorCallback = Box::new(move |error| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(error).await;
            })
        });

        *self.error_callback.lock().await = Some(async_callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        Some(&self.base_config)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        // Disconnect if connected
        if self.is_ready() {
            self.disconnect().await?;
        }

        // Update configs
        let tencent_config = TencentSttConfig::from_base(config.clone())?;
        tencent_config.validate()?;

        self.config = tencent_config;
        self.base_config = config;

        // Regenerate voice ID for new config
        self.regenerate_voice_id();

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        PROVIDER_INFO
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `connect` drives the generic
        // ReconnectableStream supervisor with them — every Tencent session trips the same breaker
        // and shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

impl TencentStt {
    /// The shared circuit breaker this session feeds into the generic supervisor, if the
    /// process-global resilience handles have been injected (W-D1/W-D2). Two `TencentStt` built
    /// from the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(&self) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }
}

impl Drop for TencentStt {
    fn drop(&mut self) {
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }
        if let Some(handle) = self.connection_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.result_forward_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.error_forward_handle.take() {
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

    fn create_test_config() -> STTConfig {
        STTConfig {
            api_key: "test_secret_id|test_secret_key|test_app_id".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "16k_zh".to_string(),
            ..Default::default()
        }
    }

    // =========================================================================
    // Client Creation Tests
    // =========================================================================

    #[test]
    fn test_new_client() {
        let config = create_test_config();
        let result = TencentStt::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_new_client_invalid_api_key_format() {
        let config = STTConfig {
            api_key: "only_one_part".to_string(),
            ..Default::default()
        };
        let result = TencentStt::new(config);
        assert!(result.is_err());
    }

    // W1 keystone: the advanced features Tencent can express (word timestamps, profanity filter,
    // key terms -> hot words) must survive THROUGH `new_standard` into the provider's Tencent
    // config — not just the config-level `from_standard`. The flat `new` path leaves them off.
    #[test]
    fn test_new_standard_unlocks_advanced_features() {
        use super::super::config::{TencentFilterDirtyMode, TencentWordInfo};
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "tencent".into(),
                api_key: "test_secret_id|test_secret_key|test_app_id".into(),
                language: "zh".into(),
                sample_rate: 16000,
                encoding: "pcm".into(),
                model: "16k_zh".into(),
                ..Default::default()
            },
            features: SttFeatures {
                word_timestamps: Some(true),
                profanity_filter: Some(true),
                keyterms: Some(vec!["WaaV".into(), "Tencent".into()]),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let stt = TencentStt::new_standard(&std).unwrap();
        assert_eq!(stt.config.word_info, TencentWordInfo::Basic); // word_timestamps
        assert_eq!(stt.config.filter_dirty, TencentFilterDirtyMode::Filter); // profanity_filter
        assert_eq!(
            stt.config.hotword_list,
            Some(vec!["WaaV".to_string(), "Tencent".to_string()])
        ); // keyterms -> hotword_list

        // Malformed credentials are rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig {
            api_key: "only_one_part".into(),
            ..Default::default()
        });
        assert!(TencentStt::new_standard(&bad).is_err());
    }

    #[test]
    fn test_new_standard_rejects_ssrf_endpoint_override() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};

        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let mk = |endpoint: &str| {
            StandardSTTConfig {
                base: STTConfig {
                    provider: "tencent".into(),
                    api_key: "test_secret_id|test_secret_key|test_app_id".into(),
                    language: "zh".into(),
                    sample_rate: 16000,
                    encoding: "pcm".into(),
                    model: "16k_zh".into(),
                    ..Default::default()
                },
                features: SttFeatures::default(),
                extras: ProviderExtras::default(),
                translation: None,
            }
            .with_endpoint_override(endpoint)
        };

        assert!(TencentStt::new_standard(&mk("wss://tencent-proxy.example.com")).is_ok());
        assert!(TencentStt::new_standard(&mk("ws://127.0.0.1:9000")).is_err());
        assert!(TencentStt::new_standard(&mk("file:///tmp/socket")).is_err());
        assert!(TencentStt::new_standard(&mk("https://tencent-proxy.example.com")).is_err());

        // SAFETY: restore the process env before releasing the test env lock.
        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous);
            } else {
                std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
            }
        }
    }

    #[test]
    fn test_new_client_missing_app_id() {
        let config = STTConfig {
            api_key: "id|key".to_string(), // Only 2 parts, needs 3
            ..Default::default()
        };
        let result = TencentStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_client_empty_credentials() {
        let config = STTConfig {
            api_key: "||".to_string(),
            ..Default::default()
        };
        let result = TencentStt::new(config);
        assert!(result.is_err());
    }

    // =========================================================================
    // State Tests
    // =========================================================================

    #[test]
    fn test_not_connected_initially() {
        let config = create_test_config();
        let stt = TencentStt::new(config).unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_voice_id_generated() {
        let config = create_test_config();
        let stt = TencentStt::new(config).unwrap();
        assert!(!stt.voice_id().is_empty());
        assert_eq!(stt.voice_id().len(), 16);
    }

    #[test]
    fn test_voice_id_unique() {
        let config1 = create_test_config();
        let config2 = create_test_config();
        let stt1 = TencentStt::new(config1).unwrap();
        let stt2 = TencentStt::new(config2).unwrap();

        // Voice IDs should be unique (very unlikely to be equal)
        assert_ne!(stt1.voice_id(), stt2.voice_id());
    }

    // =========================================================================
    // Provider Info Tests
    // =========================================================================

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let stt = TencentStt::new(config).unwrap();
        let info = stt.get_provider_info();

        assert!(info.contains("Tencent") || info.contains("腾讯"));
    }

    // =========================================================================
    // Config Tests
    // =========================================================================

    #[test]
    fn test_config_access() {
        let config = create_test_config();
        let stt = TencentStt::new(config.clone()).unwrap();

        assert!(stt.get_config().is_some());
        assert!(stt.get_config().unwrap().api_key.contains("test_secret_id"));
    }

    #[test]
    fn test_chunk_size() {
        let config = create_test_config();
        let stt = TencentStt::new(config).unwrap();

        // 40ms at 16kHz, 16-bit: 16000 * 2 * 40 / 1000 = 1280 bytes
        assert_eq!(stt.get_chunk_size(), 1280);
    }

    #[test]
    fn test_chunk_size_8k() {
        let mut config = create_test_config();
        config.model = "8k_zh".to_string();
        let stt = TencentStt::new(config).unwrap();

        // 40ms at 8kHz, 16-bit: 8000 * 2 * 40 / 1000 = 640 bytes
        assert_eq!(stt.get_chunk_size(), 640);
    }

    // =========================================================================
    // URL Building Tests
    // =========================================================================

    #[test]
    fn test_build_ws_url() {
        let config = create_test_config();
        let stt = TencentStt::new(config).unwrap();
        let url = stt.build_ws_url().unwrap();

        assert!(url.starts_with("wss://asr.cloud.tencent.com/asr/v2/test_app_id"));
        assert!(url.contains("secretid=test_secret_id"));
        assert!(url.contains("engine_model_type=16k_zh"));
        assert!(url.contains("signature="));
    }

    #[test]
    fn test_build_ws_url_with_english_model() {
        let mut config = create_test_config();
        config.model = "16k_en".to_string();
        let stt = TencentStt::new(config).unwrap();
        let url = stt.build_ws_url().unwrap();

        assert!(url.contains("engine_model_type=16k_en"));
    }

    // WIRE-LEVEL: the two Tencent Real-Time ASR knobs carried via the standardized `extras`
    // passthrough — `noise_threshold` (noise filter sensitivity, float -1.0..1.0) and
    // `input_sample_rate` (rate of the *input* audio when it differs from the engine's) — must
    // travel from `StandardSTTConfig::extras` all the way onto the signed WebSocket URL (they are
    // signed params, so they must appear in the query string the server receives). Guards the
    // recurring "set on the config struct but never emitted to the wire" gap class.
    #[test]
    fn extras_noise_threshold_and_input_sample_rate_reach_ws_url() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig};
        let mut extras = serde_json::Map::new();
        extras.insert("noise_threshold".into(), serde_json::json!(0.5));
        extras.insert("input_sample_rate".into(), serde_json::json!(8000));

        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "tencent".into(),
                api_key: "test_secret_id|test_secret_key|test_app_id".into(),
                language: "zh".into(),
                sample_rate: 16000,
                encoding: "pcm".into(),
                model: "16k_zh".into(),
                ..Default::default()
            },
            features: Default::default(),
            extras: ProviderExtras(extras),
            translation: None,
        };
        let stt = TencentStt::new_standard(&std).unwrap();
        let url = stt.build_ws_url().unwrap();

        assert!(
            url.contains("noise_threshold=0.5"),
            "noise_threshold missing from URL: {url}"
        );
        assert!(
            url.contains("input_sample_rate=8000"),
            "input_sample_rate missing from URL: {url}"
        );
    }

    // =========================================================================
    // Async Operation Tests
    // =========================================================================

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();

        let result = stt.send_audio(Bytes::from_static(&[0u8; 1280])).await;
        assert!(matches!(result, Err(STTError::ConnectionFailed(_))));
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();

        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    // W-D1: disconnect() must record intent on the supervisor-shared flag so a client close racing
    // a server-side close can never trigger a spurious reconnect (the supervisor's loop-top guard
    // observes this same `Arc<AtomicBool>`). Before this wiring the flag was the supervisor's own
    // and disconnect() never set it.
    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the supervisor-shared intentional-disconnect flag",
        );
    }

    #[tokio::test]
    async fn test_callback_registration() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();

        let result_cb: STTResultCallback = Arc::new(|_| Box::pin(async {}));
        let error_cb: STTErrorCallback = Arc::new(|_| Box::pin(async {}));

        assert!(stt.on_result(result_cb).await.is_ok());
        assert!(stt.on_error(error_cb).await.is_ok());
    }

    #[tokio::test]
    async fn test_update_config() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();

        let old_voice_id = stt.voice_id().to_string();

        let new_config = STTConfig {
            api_key: "new_id|new_key|new_app".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            encoding: "opus".to_string(),
            model: "16k_en".to_string(),
            ..Default::default()
        };

        let result = stt.update_config(new_config).await;
        assert!(result.is_ok());
        assert!(stt.get_config().unwrap().api_key.contains("new_id"));

        // Voice ID should be regenerated
        assert_ne!(stt.voice_id(), old_voice_id);
    }

    #[tokio::test]
    async fn test_update_config_invalid() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();

        let invalid_config = STTConfig {
            api_key: "invalid".to_string(),
            ..Default::default()
        };

        let result = stt.update_config(invalid_config).await;
        assert!(result.is_err());
    }

    // =========================================================================
    // Response Handling Tests
    // =========================================================================

    #[test]
    fn test_handle_response_success() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test_voice_id",
            "result": {
                "slice_type": 1,
                "index": 0,
                "start_time": 0,
                "end_time": 1500,
                "voice_text_str": "你好世界"
            },
            "final": 0
        }"#;

        TencentStt::handle_response(json, &result_tx, &error_tx);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.transcript, "你好世界");
        assert!(result.is_final);
    }

    #[test]
    fn test_handle_response_interim() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test_voice_id",
            "result": {
                "slice_type": 0,
                "index": 0,
                "start_time": 0,
                "end_time": 500,
                "voice_text_str": "你好"
            }
        }"#;

        TencentStt::handle_response(json, &result_tx, &error_tx);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.transcript, "你好");
        assert!(!result.is_final); // slice_type=0 is interim
    }

    #[test]
    fn test_handle_response_session_final() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test_voice_id",
            "result": {
                "slice_type": 2,
                "index": 5,
                "start_time": 0,
                "end_time": 10000,
                "voice_text_str": "完整识别结果"
            },
            "final": 1
        }"#;

        TencentStt::handle_response(json, &result_tx, &error_tx);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.is_final);
        assert!(result.is_speech_final);
    }

    #[test]
    fn test_handle_response_error() {
        let (result_tx, _result_rx) = mpsc::channel(10);
        let (error_tx, mut error_rx) = mpsc::channel(10);

        let json = r#"{
            "code": 4002,
            "message": "Authentication failed",
            "voice_id": "test_voice_id"
        }"#;

        TencentStt::handle_response(json, &result_tx, &error_tx);

        let error = error_rx.try_recv();
        assert!(error.is_ok());
    }

    #[test]
    fn test_handle_response_invalid_json() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, mut error_rx) = mpsc::channel(10);

        TencentStt::handle_response("not valid json", &result_tx, &error_tx);

        // Should not produce result or error (just log warning)
        assert!(result_rx.try_recv().is_err());
        assert!(error_rx.try_recv().is_err());
    }

    #[test]
    fn test_handle_response_empty_transcript() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test_voice_id"
        }"#;

        TencentStt::handle_response(json, &result_tx, &error_tx);

        // No result because no transcript
        assert!(result_rx.try_recv().is_err());
    }

    // =========================================================================
    // Voice ID Regeneration Tests
    // =========================================================================

    #[tokio::test]
    async fn test_voice_id_regenerated_on_disconnect() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();

        let old_voice_id = stt.voice_id().to_string();

        // Disconnect (even when not connected) should regenerate
        stt.disconnect().await.unwrap();

        assert_ne!(stt.voice_id(), old_voice_id);
    }
}
