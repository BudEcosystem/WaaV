//! iFlytek STT WebSocket Client
//!
//! This module implements the `BaseSTT` trait for iFlytek's Speech-to-Text WebSocket API.
//!
//! # Architecture
//!
//! The client uses a WebSocket connection for real-time streaming:
//!
//! 1. Connect with HMAC-SHA256 signed URL
//! 2. Send first frame with app_id and business parameters
//! 3. Stream audio frames (1280 bytes @ 40ms intervals)
//! 4. Receive partial results with dynamic correction
//! 5. Send last frame and receive final result
//!
//! # WebSocket Message Flow
//!
//! ```text
//! Client                              Server
//!   |                                    |
//!   |------ Connect (with auth) -------->|
//!   |<----- HTTP 101 Upgrade ------------|
//!   |                                    |
//!   |------ First frame (status=0) ----->|
//!   |------ Audio frames (status=1) ---->|
//!   |------ Last frame (status=2) ------>|
//!   |                                    |
//!   |<----- Partial results -------------|
//!   |<----- Final result (status=2) -----|
//!   |                                    |
//!   |<----- Server closes connection ----|
//! ```

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::time::{interval, timeout};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::super::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use super::config::IFlytekSttConfig;
use super::messages::{SttRequest, SttResponse};
use crate::core::resilience::connect::with_timeout;
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
const PROVIDER_INFO: &str = "iFlytek STT WebSocket v2.0 (科大讯飞)";

/// WebSocket message timeout (idle detection).
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// Audio frame interval for streaming.
const FRAME_INTERVAL: Duration = Duration::from_millis(40);

/// Default audio frame size (1280 bytes @ 16kHz, 40ms).
const DEFAULT_FRAME_SIZE: usize = 1280;

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

/// The concrete WebSocket stream type iFlytek dials.
type IFlytekWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// The featured first-frame parameters, cloned once per session and re-applied on every
/// (re)connect. iFlytek carries its entire featured session in the **first frame** (status=0):
/// language, domain, accent, VAD endpointing, dynamic correction, punctuation, number conversion,
/// audio format/encoding, and business extras. On a fresh connect the transport's `run()` re-sends
/// this first frame, so a reconnect restores the *featured* session rather than a bare one.
#[derive(Clone)]
struct IFlytekFeatures {
    app_id: String,
    language: String,
    domain: String,
    accent: String,
    vad_eos_ms: u32,
    dynamic_correction: bool,
    punctuation: bool,
    convert_numbers: bool,
    audio_format: String,
    encoding: String,
    business_extras: super::messages::IFlytekBusinessExtras,
}

/// A [`WsTransport`] that adapts iFlytek's frame-paced streaming event loop to the generic
/// [`ReconnectableStream`] supervisor (W-D1 fleet adoption). One is built per (re)connect by the
/// supervisor's `connect` closure.
///
/// iFlytek's signed connect URL carries only auth — the entire featured session rides the **first
/// audio frame** (status=0). Like Cartesia (features-in-the-handshake), the featured session is
/// re-established *inside* `run()` on every fresh connect: the loop resets `is_first_frame = true`
/// and re-emits the featured first frame from the cloned [`IFlytekFeatures`]. So
/// [`restore_session`](WsTransport::restore_session) is a no-op beyond signalling the waiting
/// `connect()` once. [`run`](WsTransport::run) IS the original `select!` loop, now returning a
/// [`ReconnectOutcome`] so a mid-stream transport drop reconnects instead of ending the session.
struct IFlytekTransport {
    ws_sink: SplitSink<IFlytekWs, Message>,
    ws_stream: SplitStream<IFlytekWs>,
    /// Shared inbound audio receiver (single-consumer; locked for the duration of `run`).
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown signal (cloneable across reconnect attempts; intentional close must not reconnect).
    shutdown_token: CancellationToken,
    result_tx: mpsc::Sender<STTResult>,
    error_tx: mpsc::Sender<STTError>,
    /// Fires once on the first successful connect, unblocking `start_connection`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    /// Featured first-frame parameters, re-applied on every (re)connect.
    features: IFlytekFeatures,
    /// Frame counter for status/telemetry (shared with the client).
    frame_count: Arc<std::sync::atomic::AtomicU64>,
}

#[async_trait::async_trait]
impl WsTransport for IFlytekTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // iFlytek's featured session rides the first audio frame, which `run()` re-sends on every
        // fresh connect (is_first_frame resets there). Nothing to re-send here — just signal the
        // waiting connect() exactly once.
        if let Some(tx) = self.connected_tx.lock().await.take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        let mut audio_rx = self.audio_rx.lock().await;
        let shutdown_token = self.shutdown_token.clone();
        let f = &self.features;

        // Per-connect framing state: a fresh connection must re-send the featured FIRST frame, so
        // these reset every time `run()` is (re)entered.
        let mut frame_timer = interval(FRAME_INTERVAL);
        let mut is_first_frame = true;
        let mut audio_buffer: Vec<u8> = Vec::with_capacity(DEFAULT_FRAME_SIZE * 2);
        let mut session_ended = false;

        if shutdown_token.is_cancelled() {
            info!("iFlytek STT shutdown signal already received");
            let _ = self.ws_sink.close().await;
            return ReconnectOutcome::Completed;
        }

        loop {
            tokio::select! {
                // Handle incoming audio data
                Some(audio_data) = audio_rx.recv() => {
                    audio_buffer.extend_from_slice(&audio_data);
                }

                // Send frames at regular intervals
                _ = frame_timer.tick() => {
                    if session_ended {
                        // The provider signalled end-of-session (final result) — an intentional
                        // completion, NOT a transport drop.
                        return ReconnectOutcome::Completed;
                    }

                    // Check if we have enough data to send
                    if audio_buffer.len() >= DEFAULT_FRAME_SIZE || !is_first_frame {
                        let frame_size = audio_buffer.len().min(DEFAULT_FRAME_SIZE);
                        let frame_data: Vec<u8> = audio_buffer.drain(..frame_size).collect();

                        let request = if is_first_frame {
                            is_first_frame = false;
                            SttRequest::first_frame_with_extras(
                                &f.app_id,
                                &f.language,
                                &f.domain,
                                Some(&f.accent),
                                f.vad_eos_ms,
                                f.dynamic_correction,
                                f.punctuation,
                                f.convert_numbers,
                                &f.audio_format,
                                &f.encoding,
                                &frame_data,
                                &f.business_extras,
                            )
                        } else {
                            SttRequest::continue_frame(
                                &f.app_id,
                                &f.audio_format,
                                &f.encoding,
                                &frame_data,
                            )
                        };

                        let json = match request.to_json() {
                            Ok(j) => j,
                            Err(e) => {
                                error!("Failed to serialize request: {}", e);
                                continue;
                            }
                        };

                        if let Err(e) = self.ws_sink.send(Message::Text(json.into())).await {
                            let err = STTError::NetworkError(format!("Failed to send frame: {}", e));
                            error!("{}", err);
                            let _ = self.error_tx.try_send(err);
                            // Transport-level send failure: reconnect to preserve the session.
                            return ReconnectOutcome::Reconnectable(StreamError::new("frame send failed"));
                        }

                        self.frame_count.fetch_add(1, Ordering::Relaxed);
                        debug!("Sent iFlytek frame #{}", self.frame_count.load(Ordering::Relaxed));
                    }
                }

                // Handle incoming messages with timeout
                message = timeout(WS_MESSAGE_TIMEOUT, self.ws_stream.next()) => {
                    match message {
                        Ok(Some(Ok(msg))) => {
                            match IFlytekStt::handle_websocket_message(msg, &self.result_tx, &self.error_tx) {
                                Ok(is_final) => {
                                    if is_final {
                                        info!("iFlytek STT session complete");
                                        session_ended = true;
                                    }
                                }
                                Err(e) => {
                                    error!("iFlytek message handling error: {}", e);
                                    // A non-retryable provider error frame is fatal (bad config /
                                    // auth) — don't hammer it with reconnects.
                                    return ReconnectOutcome::Fatal(StreamError::new("provider error frame"));
                                }
                            }
                        }
                        Ok(Some(Err(e))) => {
                            let err = STTError::NetworkError(format!("WebSocket error: {}", e));
                            error!("{}", err);
                            let _ = self.error_tx.try_send(err);
                            return ReconnectOutcome::Reconnectable(StreamError::new("websocket error"));
                        }
                        Ok(None) => {
                            info!("iFlytek WebSocket stream ended");
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_) => {
                            let err = STTError::NetworkError(
                                "iFlytek WebSocket idle timeout".to_string()
                            );
                            error!("{}", err);
                            let _ = self.error_tx.try_send(err);
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }

                // Handle shutdown signal (intentional close — must NOT reconnect)
                _ = shutdown_token.cancelled() => {
                    info!("iFlytek STT shutdown signal received");

                    // Send remaining buffer as last frame
                    if !audio_buffer.is_empty() || !is_first_frame {
                        let request = SttRequest::last_frame(
                            &f.app_id,
                            &f.audio_format,
                            &f.encoding,
                            &audio_buffer,
                        );

                        if let Ok(json) = request.to_json() {
                            let _ = self.ws_sink.send(Message::Text(json.into())).await;
                            debug!("Sent iFlytek last frame");
                        }
                    }

                    let _ = self.ws_sink.close().await;
                    return ReconnectOutcome::Completed;
                }
            }
        }
    }
}

// =============================================================================
// iFlytek STT Client
// =============================================================================

/// iFlytek Speech-to-Text WebSocket client.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::iflytek::IFlytekStt;
///
/// let config = STTConfig {
///     api_key: "app_id|api_key|api_secret".to_string(),
///     language: "zh_cn".to_string(),
///     sample_rate: 16000,
///     ..Default::default()
/// };
///
/// let mut stt = IFlytekStt::new(config)?;
/// stt.connect().await?;
/// stt.send_audio(audio_data).await?;
/// stt.disconnect().await?;
/// ```
pub struct IFlytekStt {
    /// Base configuration for BaseSTT trait.
    base_config: STTConfig,

    /// iFlytek-specific configuration.
    config: IFlytekSttConfig,

    /// Connection state.
    connected: AtomicBool,

    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before cancelling the shutdown token, so a client close racing a
    /// server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,

    /// State change notification.
    state_notify: Arc<Notify>,

    /// WebSocket sender for audio data.
    ws_sender: Option<mpsc::Sender<Bytes>>,

    /// Shutdown signal for the supervised connection task.
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

    /// Frame counter for status tracking.
    frame_count: Arc<std::sync::atomic::AtomicU64>,

    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`](crate::core::websocket::ReconnectableStream)
    /// supervisor. `None` before `set_resilience` (a direct unit-test construction) → the
    /// supervisor uses its own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl IFlytekStt {
    /// Create a new iFlytek STT client.
    fn create_internal(config: STTConfig) -> Result<Self, STTError> {
        let iflytek_config = IFlytekSttConfig::from_base(config.clone())?;

        // Validate configuration
        iflytek_config.validate()?;

        Ok(Self {
            base_config: config,
            config: iflytek_config,
            connected: AtomicBool::new(false),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_token: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            frame_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            resilience: None,
        })
    }

    /// Public constructor.
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        Self::create_internal(config)
    }

    /// Internal: construct the provider from an already-mapped iFlytek config.
    fn from_iflytek_config(base_config: STTConfig, iflytek_config: IFlytekSttConfig) -> Self {
        Self {
            base_config,
            config: iflytek_config,
            connected: AtomicBool::new(false),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_token: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            frame_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            resilience: None,
        }
    }

    /// W1 keystone — construct directly from the standardized config so the advanced features
    /// iFlytek can express (smart_format → convert_numbers, endpointing_ms → vad_eos_ms) are
    /// honored END-TO-END. The flat `BaseSTT::new` path can only see the base config; this is the
    /// reachable standardized path. Features iFlytek's API lacks remain capability gaps at default.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }
        let iflytek_config = IFlytekSttConfig::from_standard(std)?;
        iflytek_config.validate()?;
        Ok(Self::from_iflytek_config(std.base.clone(), iflytek_config))
    }

    /// Handle incoming WebSocket message.
    fn handle_websocket_message(
        message: Message,
        result_tx: &mpsc::Sender<STTResult>,
        error_tx: &mpsc::Sender<STTError>,
    ) -> Result<bool, STTError> {
        match message {
            Message::Text(text) => {
                debug!("iFlytek STT received: {}", text);

                let response = SttResponse::from_json(&text).map_err(|e| {
                    STTError::ProviderError(format!("Failed to parse response: {}", e))
                })?;

                // Check for errors
                if !response.is_success() {
                    let error_code = response.error_code();
                    let error = STTError::ProviderError(format!("iFlytek error: {}", error_code));

                    // Send error through channel
                    if let Err(e) = error_tx.try_send(error.clone()) {
                        warn!("Failed to send error: {:?}", e);
                    }

                    // Retryable errors don't terminate the connection
                    if !error_code.is_retryable() {
                        return Err(error);
                    }
                    return Ok(false);
                }

                // Extract transcript
                if let Some(transcript) = response.transcript()
                    && !transcript.is_empty()
                {
                    let is_final = response.is_final();
                    let is_replacement = response.is_replacement();

                    let result = STTResult::new(
                        transcript,
                        is_final,
                        is_final, // speech_final matches is_final for iFlytek
                        response.confidence(),
                    );

                    // Log replacement events for debugging
                    if is_replacement {
                        debug!(
                            "iFlytek dynamic correction: sn={:?}",
                            response.sentence_number()
                        );
                    }

                    // Send result through channel
                    if let Err(e) = result_tx.try_send(result) {
                        match e {
                            mpsc::error::TrySendError::Full(_) => {
                                warn!("iFlytek result channel full - dropping result");
                            }
                            mpsc::error::TrySendError::Closed(_) => {
                                warn!("iFlytek result channel closed");
                            }
                        }
                    }
                }

                // Return true if this is the final response
                Ok(response.is_final())
            }
            Message::Close(frame) => {
                info!("iFlytek WebSocket closed: {:?}", frame);
                Ok(true)
            }
            Message::Ping(_) => {
                debug!("iFlytek received ping");
                Ok(false)
            }
            Message::Pong(_) => {
                debug!("iFlytek received pong");
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    /// Start the WebSocket connection task.
    async fn start_connection(&mut self) -> Result<(), STTError> {
        // Build signed WebSocket URL
        let signed_url = self
            .config
            .auth
            .build_signed_url(self.config.host(), self.config.path())
            .map_err(|e| {
                STTError::ConnectionFailed(format!("Failed to build signed URL: {}", e))
            })?;

        // Honor an `endpoint_override` for the in-repo mock/proxy: swap the dialed scheme://host while
        // keeping the signed `/v2/{iat,ist}?authorization=...` path+query (the mock ignores the HMAC
        // signature). `/v2/` is the stable path marker shared by IAT and IST and precedes the query.
        let ws_url = match self
            .config
            .endpoint_override
            .as_deref()
            .map(str::trim)
            .filter(|o| !o.is_empty())
            .and_then(|o| signed_url.find("/v2/").map(|idx| (o, idx)))
        {
            Some((o, idx)) => format!("{}{}", o.trim_end_matches('/'), &signed_url[idx..]),
            None => signed_url,
        };

        debug!("Connecting to iFlytek: {}", ws_url);

        // Create channels
        let (ws_tx, ws_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER);
        let shutdown_token = CancellationToken::new();
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(RESULT_CHANNEL_BUFFER);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(ERROR_CHANNEL_BUFFER);
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        // Store channels
        self.ws_sender = Some(ws_tx);
        self.shutdown_token = Some(shutdown_token.clone());

        // The featured first-frame parameters, cloned once and re-applied on every (re)connect.
        let features = IFlytekFeatures {
            app_id: self.config.auth.app_id.clone(),
            language: self.config.language.as_code().to_string(),
            domain: self.config.domain.as_str().to_string(),
            accent: self.config.accent.clone(),
            vad_eos_ms: self.config.vad_eos_ms,
            dynamic_correction: self.config.dynamic_correction,
            punctuation: self.config.punctuation,
            convert_numbers: self.config.convert_numbers,
            audio_format: self.config.audio_format_string(),
            encoding: self.config.encoding.as_str().to_string(),
            business_extras: self.config.business_extras.clone(),
        };
        let frame_count = self.frame_count.clone();

        // Shared state the supervised transport re-uses across reconnect attempts: a single-
        // consumer audio receiver + shutdown token and the one-shot connected
        // signal that fires on the first successful connect.
        let audio_rx = Arc::new(Mutex::new(ws_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor (the
        // same one the chaos tests exercise) with the shared process-global handles from CoreState
        // (W-D1/W-D2 fleet adoption). When no handles were injected (a direct unit-test
        // construction), the supervisor uses its own per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("iflytek", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => {
                ReconnectableStream::new(ReconnectableStreamConfig::new("iflytek", reconnection))
            }
        }
        .with_disconnect_flag(disconnect_flag);

        // Start connection task: the supervisor owns the outer reconnect loop; the `connect`
        // closure dials the (signed) URL and hands back a transport whose `run()` re-emits the
        // featured first frame and is the original iFlytek event loop.
        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let ws_url = ws_url.clone();
                    let features = features.clone();
                    let frame_count = frame_count.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let shutdown_token = shutdown_token.clone();
                    let connected_tx = Arc::clone(&connected_tx);
                    let result_tx = result_tx.clone();
                    let error_tx = error_tx.clone();
                    async move {
                        // Deadline-bounded dial via the shared resilience helper. iFlytek keeps
                        // its historical 10s bound (tighter than the canonical 15s
                        // `core::resilience::connect::WS_CONNECT_TIMEOUT`).
                        let ws_stream =
                            match with_timeout(Duration::from_secs(10), connect_async(&ws_url))
                                .await
                            {
                                Ok(Ok((stream, _))) => stream,
                                Ok(Err(e)) => {
                                    let err = STTError::ConnectionFailed(format!(
                                        "WebSocket connection failed: {}",
                                        e
                                    ));
                                    error!("{}", err);
                                    let _ = error_tx.try_send(err);
                                    return Err(StreamError::new(format!(
                                        "WebSocket connection failed: {e}"
                                    )));
                                }
                                Err(_) => {
                                    let err = STTError::ConnectionFailed(
                                        "Connection timeout".to_string(),
                                    );
                                    error!("{}", err);
                                    let _ = error_tx.try_send(err);
                                    return Err(StreamError::new("connection timeout"));
                                }
                            };

                        info!("Connected to iFlytek STT WebSocket");
                        let (ws_sink, ws_stream) = ws_stream.split();
                        Ok(IFlytekTransport {
                            ws_sink,
                            ws_stream,
                            audio_rx,
                            shutdown_token,
                            result_tx,
                            error_tx,
                            connected_tx,
                            features,
                            frame_count,
                        })
                    }
                })
                .await;
            info!("iFlytek STT WebSocket connection closed (supervisor exit: {exit:?})");
        });

        self.connection_handle = Some(connection_handle);

        // Start result forwarding task
        let callback_ref = self.result_callback.clone();
        let result_forward_handle = tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                if let Some(callback) = callback_ref.lock().await.as_ref() {
                    callback(result).await;
                } else {
                    debug!(
                        "iFlytek STT result (no callback): {} (confidence: {})",
                        result.transcript, result.confidence
                    );
                }
            }
        });
        self.result_forward_handle = Some(result_forward_handle);

        // Start error forwarding task
        let error_callback_ref = self.error_callback.clone();
        let error_forward_handle = tokio::spawn(async move {
            while let Some(error) = error_rx.recv().await {
                if let Some(callback) = error_callback_ref.lock().await.as_ref() {
                    callback(error).await;
                } else {
                    error!("iFlytek STT error (no callback): {}", error);
                }
            }
        });
        self.error_forward_handle = Some(error_forward_handle);

        // Wait for connection to be established
        match timeout(Duration::from_secs(10), connected_rx).await {
            Ok(Ok(())) => {
                self.connected.store(true, Ordering::SeqCst);
                self.state_notify.notify_waiters();
                info!("iFlytek STT connected successfully");
                Ok(())
            }
            Ok(Err(_)) => Err(STTError::ConnectionFailed(
                "Connection channel closed".to_string(),
            )),
            Err(_) => Err(STTError::ConnectionFailed("Connection timeout".to_string())),
        }
    }
}

impl Default for IFlytekStt {
    fn default() -> Self {
        Self {
            base_config: STTConfig::default(),
            config: IFlytekSttConfig::default(),
            connected: AtomicBool::new(false),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_token: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            frame_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            resilience: None,
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for IFlytekStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        Self::create_internal(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        info!(
            "Connecting to iFlytek STT (language: {}, mode: {:?})",
            self.config.language.display_name(),
            self.config.mode
        );

        self.frame_count.store(0, Ordering::Relaxed);
        self.start_connection().await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Record the intent BEFORE the connected-guard so the supervisor sees it even if the
        // transport's run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        info!("Disconnecting from iFlytek STT");

        // Send shutdown signal
        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }

        // Wait for connection task to finish
        if let Some(handle) = self.connection_handle.take() {
            crate::core::observability::await_task_shutdown(
                "iflytek-stt-connection",
                handle,
                Duration::from_secs(5),
            )
            .await;
        }

        // Clean up forwarding tasks
        if let Some(handle) = self.result_forward_handle.take() {
            crate::core::observability::abort_and_await_task(
                "iflytek-stt-result-forwarder",
                handle,
            )
            .await;
        }
        if let Some(handle) = self.error_forward_handle.take() {
            crate::core::observability::abort_and_await_task("iflytek-stt-error-forwarder", handle)
                .await;
        }

        // Clear state
        self.ws_sender = None;
        *self.result_callback.lock().await = None;
        *self.error_callback.lock().await = None;
        self.connected.store(false, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        info!(
            "Disconnected from iFlytek STT (frames sent: {})",
            self.frame_count.load(Ordering::Relaxed)
        );
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst) && self.ws_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to iFlytek STT".to_string(),
            ));
        }

        if let Some(ws_sender) = &self.ws_sender {
            let data_len = audio_data.len();

            ws_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to queue audio: {}", e)))?;

            debug!("Queued {} bytes of audio for iFlytek", data_len);
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.result_callback.lock().await = Some(Box::new(move |result| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(result).await;
            })
        }));
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.error_callback.lock().await = Some(Box::new(move |error| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(error).await;
            })
        }));
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        Some(&self.base_config)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        // Need to reconnect to update config
        if self.is_ready() {
            self.disconnect().await?;
        }

        // Parse new config
        let iflytek_config = IFlytekSttConfig::from_base(config.clone())?;
        iflytek_config.validate()?;

        self.base_config = config;
        self.config = iflytek_config;

        self.connect().await
    }

    fn get_provider_info(&self) -> &'static str {
        PROVIDER_INFO
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `start_connection` drives the generic
        // ReconnectableStream supervisor with them — every iFlytek session trips the same breaker
        // and shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

impl IFlytekStt {
    /// The shared circuit breaker this session feeds into the generic supervisor, if the
    /// process-global resilience handles have been injected (W-D1/W-D2). Two `IFlytekStt` built
    /// from the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(&self) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }
}

impl Drop for IFlytekStt {
    fn drop(&mut self) {
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_api_key() -> String {
        "test_app_id|test_api_key_xxxxx|test_api_secret_xx".to_string()
    }

    fn create_test_config() -> STTConfig {
        STTConfig {
            api_key: create_test_api_key(),
            language: "zh_cn".to_string(),
            sample_rate: 16000,
            encoding: "raw".to_string(),
            punctuation: true,
            ..Default::default()
        }
    }

    // W-D1: disconnect() must record intent on the supervisor-shared flag so a client close racing
    // a server-side close can never trigger a spurious reconnect (the supervisor's loop-top guard
    // observes this same `Arc<AtomicBool>`). Before this wiring the flag was the supervisor's own
    // and disconnect() never set it.
    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
        let config = create_test_config();
        let mut stt = IFlytekStt::new(config).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the supervisor-shared intentional-disconnect flag",
        );
    }

    #[test]
    fn test_iflytek_stt_creation() {
        let config = create_test_config();
        let result = IFlytekStt::new(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), PROVIDER_INFO);
    }

    // W1 keystone: a standardized advanced feature iFlytek can express (endpointing_ms →
    // vad_eos_ms, smart_format → convert_numbers) survives through `new_standard` onto the
    // provider's own config — proving the standardized path doesn't drop it.
    #[test]
    fn test_iflytek_new_standard_unlocks_features() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "iflytek".into(),
                api_key: create_test_api_key(),
                ..create_test_config()
            },
            features: SttFeatures {
                smart_format: Some(false),
                endpointing_ms: Some(1500),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let stt = IFlytekStt::new_standard(&std).unwrap();
        assert_eq!(stt.config.vad_eos_ms, 1500); // endpointing_ms survived to provider config
        assert!(!stt.config.convert_numbers); // smart_format survived

        // Missing key is rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig {
            api_key: String::new(),
            ..Default::default()
        });
        assert!(IFlytekStt::new_standard(&bad).is_err());
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
                    provider: "iflytek".into(),
                    api_key: create_test_api_key(),
                    language: "zh_cn".into(),
                    sample_rate: 16000,
                    encoding: "raw".into(),
                    ..Default::default()
                },
                features: SttFeatures::default(),
                extras: ProviderExtras::default(),
                translation: None,
            }
            .with_endpoint_override(endpoint)
        };

        assert!(IFlytekStt::new_standard(&mk("wss://iflytek-proxy.example.com")).is_ok());
        assert!(IFlytekStt::new_standard(&mk("ws://127.0.0.1:9000")).is_err());
        assert!(IFlytekStt::new_standard(&mk("file:///tmp/socket")).is_err());
        assert!(IFlytekStt::new_standard(&mk("https://iflytek-proxy.example.com")).is_err());

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
    fn test_iflytek_stt_invalid_api_key() {
        let config = STTConfig {
            api_key: "invalid_format".to_string(),
            ..Default::default()
        };
        let result = IFlytekStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_iflytek_stt_get_config() {
        let config = create_test_config();
        let stt = IFlytekStt::new(config).unwrap();

        let stored = stt.get_config();
        assert!(stored.is_some());
        assert_eq!(stored.unwrap().language, "zh_cn");
    }

    #[test]
    fn test_iflytek_stt_initial_state() {
        let config = create_test_config();
        let stt = IFlytekStt::new(config).unwrap();

        assert!(!stt.is_ready());
        assert_eq!(stt.frame_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = IFlytekStt::new(config).unwrap();

        let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_err());

        if let Err(STTError::ConnectionFailed(_)) = result {
            // Expected
        } else {
            panic!("Expected ConnectionFailed error");
        }
    }

    #[test]
    fn test_iflytek_stt_default() {
        let stt = IFlytekStt::default();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let stt = IFlytekStt::new(config).unwrap();

        let info = stt.get_provider_info();
        assert!(info.contains("iFlytek"));
        assert!(info.contains("科大讯飞"));
    }

    #[test]
    fn test_message_handling_success() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "result": {
                    "ws": [{"bg": 0, "cw": [{"w": "你好", "sc": 0.95}]}],
                    "sn": 1,
                    "ls": false
                },
                "status": 1
            }
        }"#;

        let (result_tx, mut result_rx) = mpsc::channel(256);
        let (error_tx, _error_rx) = mpsc::channel(64);

        let message = Message::Text(json.to_string().into());
        let result = IFlytekStt::handle_websocket_message(message, &result_tx, &error_tx);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Not final

        // Check result was sent
        let received = result_rx.try_recv();
        assert!(received.is_ok());
        assert_eq!(received.unwrap().transcript, "你好");
    }

    #[test]
    fn test_message_handling_error() {
        let json = r#"{
            "code": 10005,
            "message": "authorization failure",
            "sid": "test_sid"
        }"#;

        let (result_tx, _result_rx) = mpsc::channel(256);
        let (error_tx, mut error_rx) = mpsc::channel(64);

        let message = Message::Text(json.to_string().into());
        let result = IFlytekStt::handle_websocket_message(message, &result_tx, &error_tx);
        assert!(result.is_err());

        // Check error was sent
        let error = error_rx.try_recv();
        assert!(error.is_ok());
    }

    #[test]
    fn test_message_handling_final() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "result": {
                    "ws": [{"bg": 0, "cw": [{"w": "完成"}]}],
                    "sn": 1,
                    "ls": true
                },
                "status": 2
            }
        }"#;

        let (result_tx, _) = mpsc::channel(256);
        let (error_tx, _) = mpsc::channel(64);

        let message = Message::Text(json.to_string().into());
        let result = IFlytekStt::handle_websocket_message(message, &result_tx, &error_tx);
        assert!(result.is_ok());
        assert!(result.unwrap()); // Final
    }

    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_FRAME_SIZE, 1280);
        assert_eq!(FRAME_INTERVAL, Duration::from_millis(40));
        assert_eq!(WS_MESSAGE_TIMEOUT, Duration::from_secs(60));
    }
}
