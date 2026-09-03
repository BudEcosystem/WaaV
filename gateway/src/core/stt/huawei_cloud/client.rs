//! Huawei Cloud Speech Interaction Service (SIS) STT Client
//!
//! This module implements the `BaseSTT` trait for Huawei Cloud's Speech-to-Text
//! service, supporting both WebSocket real-time streaming and REST API short
//! sentence recognition.
//!
//! # Architecture
//!
//! The client supports three operation modes:
//!
//! 1. **Short Sentence (REST)**: For audio files up to 30 seconds
//! 2. **Streaming (WebSocket)**: For real-time audio up to 1 minute
//! 3. **Continuous (WebSocket)**: For long-running audio up to 5 hours
//!
//! # WebSocket Message Flow
//!
//! ```text
//! Client                              Server
//!   |                                    |
//!   |------ Connect with IAM token ----->|
//!   |<----- HTTP 101 Upgrade ------------|
//!   |                                    |
//!   |------ START command (JSON) ------->|
//!   |<----- STARTED confirmation --------|
//!   |                                    |
//!   |------ Audio frames (binary) ------>|
//!   |<----- RESULT (interim) ------------|
//!   |<----- END (final) -----------------|
//!   |                                    |
//!   |------ END command (JSON) --------->|
//!   |<----- ENDED confirmation ----------|
//! ```
//!
//! # Authentication
//!
//! Uses IAM token authentication:
//! 1. Exchange username/password for IAM token
//! 2. Token valid for 24 hours (auto-refreshed)
//! 3. Include X-Auth-Token header in WebSocket handshake

use bytes::Bytes;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{client::IntoClientRequest, protocol::Message},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::auth::HuaweiTokenManager;
use super::config::{HuaweiCloudAsrMode, HuaweiCloudSttConfig};
use super::messages::{
    HuaweiEndFrame, HuaweiRealtimeResponse, HuaweiShortAsrRequest, HuaweiShortAsrResponse,
    HuaweiStartFrame,
};
use crate::core::resilience::connect::{WS_CONNECT_TIMEOUT, with_timeout};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};

// =============================================================================
// Constants
// =============================================================================

/// Provider information string.
const PROVIDER_INFO: &str = "Huawei Cloud Speech (华为云语音)";

/// Per-message idle timeout for WebSocket message reception. Resets after each successful
/// message; catches stuck/dead connections so the supervisor can reconnect.
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// HTTP request timeout for REST API.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

fn huawei_stt_http_client() -> Result<reqwest::Client, reqwest::Error> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
        .timeout(HTTP_TIMEOUT)
        .build()
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

/// The concrete WebSocket stream type Huawei dials.
type HuaweiWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

// =============================================================================
// Reconnect transport (W-D1 fleet adoption)
// =============================================================================

/// A [`WsTransport`] that adapts Huawei Cloud RASR's event loop to the generic
/// [`ReconnectableStream`] supervisor. One is built per (re)connect by the supervisor's `connect`
/// closure. Only the WebSocket modes (Streaming/Continuous) flow through here; the ShortSentence
/// REST mode has no persistent stream and is unaffected.
///
/// Like Azure (config carried in a **post-handshake message**, not the URL), Huawei opens its
/// featured session with a `START` command (model, audio format, word-info, punctuation, …) sent
/// after the handshake. So [`restore_session`](WsTransport::restore_session) re-sends that command
/// on the fresh socket — without it a reconnect would resume as a *bare* session. [`run`](WsTransport::run)
/// replaces the original split send/recv tasks with a single `select!` loop that returns a
/// [`ReconnectOutcome`] so a mid-stream transport drop reconnects instead of silently ending the
/// session.
struct HuaweiTransport {
    ws_sink: SplitSink<HuaweiWs, Message>,
    ws_stream: SplitStream<HuaweiWs>,
    /// Shared inbound audio receiver (single-consumer; locked for the duration of `run`).
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown token (an intentional close must not reconnect).
    shutdown_token: CancellationToken,
    result_tx: mpsc::Sender<STTResult>,
    error_tx: mpsc::Sender<STTError>,
    /// Fires once after the featured session is (re)established, unblocking `connect`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    /// Shared session-ready flag (set on the `STARTED` response, cleared on `ENDED`). Reset on
    /// every restore so a reconnect re-waits for the fresh session's `STARTED`.
    session_ready: Arc<AtomicBool>,
    /// The pre-serialized `START` command JSON, re-sent on every restore so reconnects keep the
    /// featured session (model, format, word-info, punctuation, digit-norm, vocabulary).
    start_frame_json: String,
}

impl HuaweiTransport {
    async fn shutdown_gracefully(ws_sink: &mut SplitSink<HuaweiWs, Message>) -> ReconnectOutcome {
        if let Ok(end_json) = HuaweiEndFrame::new().to_json() {
            let _ = ws_sink.send(Message::Text(end_json.into())).await;
        }
        ReconnectOutcome::Completed
    }
}

#[async_trait::async_trait]
impl WsTransport for HuaweiTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // A reconnect starts a fresh recognition turn: clear session-ready until this socket's
        // `STARTED` arrives.
        self.session_ready.store(false, Ordering::SeqCst);

        // Re-send the featured `START` command on this fresh socket. A reconnect must NOT resume
        // as a bare session.
        self.ws_sink
            .send(Message::Text(self.start_frame_json.clone().into()))
            .await
            .map_err(|e| RestoreError::new(format!("failed to send Huawei START frame: {e}")))?;

        // The featured session is established on the wire: signal the waiting connect() exactly
        // once (the `STARTED` confirmation then flips `session_ready`).
        if let Some(tx) = self.connected_tx.lock().await.take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        let shutdown_token = self.shutdown_token.clone();
        if shutdown_token.is_cancelled() {
            debug!("Received shutdown signal for Huawei Cloud STT");
            return Self::shutdown_gracefully(&mut self.ws_sink).await;
        }
        let mut audio_rx = self.audio_rx.lock().await;
        loop {
            tokio::select! {
                // Handle outgoing audio data (raw binary frames)
                Some(audio) = audio_rx.recv() => {
                    if let Err(e) = self.ws_sink.send(Message::Binary(audio.to_vec().into())).await {
                        let stt_error = STTError::NetworkError(format!(
                            "Failed to send audio to Huawei Cloud: {e}"
                        ));
                        error!("{}", stt_error);
                        let _ = self.error_tx.try_send(stt_error);
                        return ReconnectOutcome::Reconnectable(StreamError::new("audio send failed"));
                    }
                }

                // Handle incoming messages with idle timeout
                message = timeout(WS_MESSAGE_TIMEOUT, self.ws_stream.next()) => {
                    match message {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            HuaweiCloudStt::handle_realtime_response(
                                &text,
                                &self.result_tx,
                                &self.error_tx,
                                &self.session_ready,
                            );
                        }
                        Ok(Some(Ok(Message::Close(_)))) => {
                            // The provider signalled end-of-session — an intentional completion,
                            // NOT a transport drop.
                            debug!("Huawei Cloud WebSocket closed by server");
                            return ReconnectOutcome::Completed;
                        }
                        Ok(Some(Ok(Message::Ping(_)))) => {
                            debug!("Received ping from Huawei Cloud");
                        }
                        Ok(Some(Ok(_))) => {
                            // Binary/Pong/Frame — ignore.
                        }
                        Ok(Some(Err(e))) => {
                            let stt_error = STTError::ConnectionFailed(e.to_string());
                            error!("Huawei Cloud WebSocket error: {}", stt_error);
                            let _ = self.error_tx.try_send(stt_error);
                            return ReconnectOutcome::Reconnectable(StreamError::new("websocket error"));
                        }
                        Ok(None) => {
                            info!("Huawei Cloud WebSocket stream ended");
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            let stt_error = STTError::NetworkError(
                                "Huawei Cloud WebSocket idle timeout - no message for 60 seconds".into()
                            );
                            error!("{}", stt_error);
                            let _ = self.error_tx.try_send(stt_error);
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }

                // Handle shutdown signal (intentional close — must NOT reconnect)
                _ = shutdown_token.cancelled() => {
                    debug!("Received shutdown signal for Huawei Cloud STT");
                    return Self::shutdown_gracefully(&mut self.ws_sink).await;
                }
            }
        }
    }
}

// =============================================================================
// Huawei Cloud STT Client
// =============================================================================

/// Huawei Cloud Speech Interaction Service (SIS) STT client.
///
/// Supports real-time WebSocket streaming and REST API for short audio.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::huawei_cloud::HuaweiCloudStt;
///
/// let config = STTConfig {
///     // API key format: username|password|domain_name|project_id
///     api_key: "user|pass|domain|project123".to_string(),
///     language: "zh".to_string(),
///     sample_rate: 16000,
///     encoding: "pcm16k16bit".to_string(),
///     model: "chinese_16k_general".to_string(),
///     ..Default::default()
/// };
///
/// let mut stt = HuaweiCloudStt::new(config)?;
/// stt.connect().await?;
/// stt.send_audio(audio_data).await?;
/// stt.disconnect().await?;
/// ```
pub struct HuaweiCloudStt {
    /// Base configuration for BaseSTT trait.
    base_config: STTConfig,

    /// Huawei Cloud-specific configuration.
    config: HuaweiCloudSttConfig,

    /// IAM token manager.
    token_manager: Arc<HuaweiTokenManager>,

    /// Connection state.
    connected: Arc<AtomicBool>,

    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before cancelling the shutdown token, so a client close
    /// racing a server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,

    /// State change notification.
    state_notify: Arc<Notify>,

    /// WebSocket sender for audio data.
    ws_sender: Option<mpsc::Sender<Bytes>>,

    /// Shutdown token shared with supervised transports.
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

    /// HTTP client for REST API.
    http_client: reqwest::Client,

    /// Audio buffer for REST API mode.
    audio_buffer: Arc<Mutex<Vec<u8>>>,

    /// Session ready flag.
    session_ready: Arc<AtomicBool>,

    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`] supervisor. `None` before `set_resilience` (a direct
    /// unit-test construction) → the supervisor uses its own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl HuaweiCloudStt {
    /// W1 keystone — construct directly from the standardized config so Huawei's mappable
    /// recognition knobs (word-level timing `need_word_info`, smart formatting `add_punctuation`)
    /// are honored END-TO-END. Mirrors `DeepgramSTT::new_standard`: the provider config is built
    /// from `HuaweiCloudSttConfig::from_standard` (which parses+validates the pipe-separated
    /// credential in `api_key`); features Huawei can't express stay at default.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let huawei_config = HuaweiCloudSttConfig::from_standard(std)?;
        huawei_config.validate()?;
        let http_client = huawei_stt_http_client().map_err(|e| {
            STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
        })?;

        Ok(Self {
            base_config: std.base.clone(),
            config: huawei_config,
            token_manager: Arc::new(HuaweiTokenManager::with_client(http_client.clone())),
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
            http_client,
            audio_buffer: Arc::new(Mutex::new(Vec::new())),
            session_ready: Arc::new(AtomicBool::new(false)),
            resilience: None,
        })
    }

    /// Create a new Huawei Cloud STT client.
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        let huawei_config = HuaweiCloudSttConfig::from_base(config.clone())?;
        huawei_config.validate()?;
        let http_client = huawei_stt_http_client().map_err(|e| {
            STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
        })?;

        Ok(Self {
            base_config: config,
            config: huawei_config,
            token_manager: Arc::new(HuaweiTokenManager::with_client(http_client.clone())),
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
            http_client,
            audio_buffer: Arc::new(Mutex::new(Vec::new())),
            session_ready: Arc::new(AtomicBool::new(false)),
            resilience: None,
        })
    }

    /// Get the IAM token, fetching if necessary.
    ///
    /// Passes `endpoint_override` so the credential-free mock harness can redirect the IAM token
    /// POST (scheme+host) to a localhost mock; `None` in production hits the real IAM endpoint.
    async fn get_token(&self) -> Result<String, STTError> {
        self.token_manager
            .get_token_with_override(
                &self.config.username,
                &self.config.password,
                &self.config.domain_name,
                self.config.region,
                self.config.endpoint_override.as_deref(),
            )
            .await
    }

    /// Connect using WebSocket real-time mode.
    async fn connect_realtime(&mut self) -> Result<(), STTError> {
        // Get WebSocket URL
        let url = self.config.get_realtime_url().ok_or_else(|| {
            STTError::ConfigurationError("WebSocket URL not available for this mode".to_string())
        })?;

        info!("Connecting to Huawei Cloud RASR: {}", url);

        // Validate the IAM token can be obtained up-front so `connect()` fails fast on bad
        // credentials (the supervisor's connect closure re-fetches per attempt, which is cheap
        // since the token manager caches — and a reconnect after token expiry re-fetches).
        let _ = self.get_token().await?;

        // Create channels
        let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER);
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(RESULT_CHANNEL_BUFFER);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(ERROR_CHANNEL_BUFFER);
        let shutdown_token = CancellationToken::new();
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        self.ws_sender = Some(audio_tx);
        self.shutdown_token = Some(shutdown_token.clone());

        // Build the featured `START` command once (re-sent verbatim on every restore by the
        // supervised transport). A reconnect must restore the featured session, not a bare one.
        let start_frame = HuaweiStartFrame::from_config(&self.config);
        let start_frame_json = start_frame.to_json().map_err(|e| {
            STTError::ConnectionFailed(format!("Failed to serialize START frame: {}", e))
        })?;

        // Shared state the supervised transport re-uses across reconnect attempts.
        let audio_rx = Arc::new(Mutex::new(audio_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));

        // Per-attempt dial inputs (the IAM token is re-fetched inside the closure so a reconnect
        // after a 24h token expiry transparently re-authenticates).
        let token_manager = self.token_manager.clone();
        let username = self.config.username.clone();
        let password = self.config.password.clone();
        let domain_name = self.config.domain_name.clone();
        let region = self.config.region;
        let host = self.config.region.sis_endpoint();
        let iam_override = self.config.endpoint_override.clone();
        let session_ready = self.session_ready.clone();

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor with
        // the shared process-global handles from CoreState (W-D1/W-D2 fleet adoption). When no
        // handles were injected (a direct unit-test construction), the supervisor uses its own
        // per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("huawei_cloud", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => ReconnectableStream::new(ReconnectableStreamConfig::new(
                "huawei_cloud",
                reconnection,
            )),
        }
        .with_disconnect_flag(disconnect_flag);

        // Set connected state (the BaseSTT contract: `connect()` returns once the session is
        // accepted; the supervisor owns the durable reconnect loop from here on).
        self.connected.store(true, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        // Start the connection task: the supervisor owns the outer reconnect loop; the `connect`
        // closure re-fetches the IAM token, dials with the `X-Auth-Token` header, and hands back a
        // transport whose `restore_session` re-sends the `START` command and whose `run()` is the
        // Huawei event loop.
        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let url = url.clone();
                    let host = host.clone();
                    let token_manager = token_manager.clone();
                    let username = username.clone();
                    let password = password.clone();
                    let domain_name = domain_name.clone();
                    let iam_override = iam_override.clone();
                    let start_frame_json = start_frame_json.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let shutdown_token = shutdown_token.clone();
                    let connected_tx = Arc::clone(&connected_tx);
                    let session_ready = Arc::clone(&session_ready);
                    let result_tx = result_tx.clone();
                    let error_tx = error_tx.clone();
                    async move {
                        // Re-fetch the IAM token (cached; re-authenticates on expiry). Passes
                        // `endpoint_override` so a reconnect's token POST honors the mock redirect.
                        let token = token_manager
                            .get_token_with_override(
                                &username,
                                &password,
                                &domain_name,
                                region,
                                iam_override.as_deref(),
                            )
                            .await
                            .map_err(|e| StreamError::new(format!("IAM token error: {e}")))?;

                        // Build the upgrade request via `into_client_request` (repo convention):
                        // it derives the 5 mandatory WS handshake headers (`Host`, `Connection`,
                        // `Upgrade`, `Sec-WebSocket-Version`, `Sec-WebSocket-Key`) from the dial
                        // URL; only Huawei's `X-Auth-Token` rides on top. The explicit `Host` is
                        // then pinned to the regional SIS endpoint: identical to the URL-derived
                        // value in production, and it preserves the historical behavior of keeping
                        // the production Host when an `endpoint_override` redirects the dial to a
                        // local mock.
                        let mut request = url.as_str().into_client_request().map_err(|e| {
                            StreamError::new(format!("Failed to build request: {e}"))
                        })?;
                        let headers = request.headers_mut();
                        headers.insert(
                            "X-Auth-Token",
                            token.parse().map_err(|e| {
                                StreamError::new(format!("Failed to build request: {e}"))
                            })?,
                        );
                        headers.insert(
                            "Host",
                            host.parse().map_err(|e| {
                                StreamError::new(format!("Failed to build request: {e}"))
                            })?,
                        );

                        // Deadline-bounded dial via the shared resilience helper (canonical 15s).
                        let (ws_stream, _) = match with_timeout(
                            WS_CONNECT_TIMEOUT,
                            connect_async_with_config(request, None, false),
                        )
                        .await
                        {
                            Ok(Ok(s)) => s,
                            Ok(Err(e)) => {
                                return Err(StreamError::new(format!(
                                    "WebSocket connection failed: {e}"
                                )));
                            }
                            Err(_) => {
                                return Err(StreamError::new("Connection timeout".to_string()));
                            }
                        };
                        info!("Connected to Huawei Cloud RASR");
                        let (ws_sink, ws_stream) = ws_stream.split();
                        Ok(HuaweiTransport {
                            ws_sink,
                            ws_stream,
                            audio_rx,
                            shutdown_token,
                            result_tx,
                            error_tx,
                            connected_tx,
                            session_ready,
                            start_frame_json,
                        })
                    }
                })
                .await;
            info!("Huawei Cloud WebSocket connection closed (supervisor exit: {exit:?})");
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

        // Wait for the featured session to be established (first restore) with a timeout.
        match timeout(Duration::from_secs(15), connected_rx).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => {
                self.connected.store(false, Ordering::SeqCst);
                Err(STTError::ConnectionFailed(
                    "Connection channel closed before confirmation".to_string(),
                ))
            }
            Err(_) => {
                self.connected.store(false, Ordering::SeqCst);
                Err(STTError::ConnectionFailed("Connection timeout".to_string()))
            }
        }
    }

    /// Handle real-time WebSocket response.
    fn handle_realtime_response(
        text: &str,
        result_tx: &mpsc::Sender<STTResult>,
        error_tx: &mpsc::Sender<STTError>,
        session_ready: &AtomicBool,
    ) {
        match HuaweiRealtimeResponse::from_json(text) {
            Ok(response) => {
                // Handle session started
                if response.is_started() {
                    session_ready.store(true, Ordering::SeqCst);
                    debug!("Huawei Cloud ASR session started");
                    return;
                }

                // Handle session ended
                if response.is_ended() {
                    session_ready.store(false, Ordering::SeqCst);
                    debug!("Huawei Cloud ASR session ended");
                    return;
                }

                // Handle errors
                if response.is_error() {
                    if let Some(err_msg) = response.get_error() {
                        warn!("Huawei Cloud ASR error: {}", err_msg);
                        let _ = error_tx.try_send(STTError::AudioProcessingError(err_msg));
                    }
                    return;
                }

                // Handle recognition results
                if let Some(transcript) = response.get_transcript() {
                    let result = STTResult::new(
                        transcript.to_string(),
                        response.is_final(),
                        response.is_final(),
                        response.get_confidence().unwrap_or(1.0),
                    );

                    debug!(
                        "Huawei Cloud transcript ({}): {}",
                        if response.is_final() {
                            "final"
                        } else {
                            "interim"
                        },
                        transcript
                    );

                    let _ = result_tx.try_send(result);
                }
            }
            Err(e) => {
                warn!(
                    "Failed to parse Huawei Cloud response: {} - raw: {}",
                    e, text
                );
            }
        }
    }

    /// Recognize short audio using REST API.
    pub async fn recognize_short_audio(&self, audio_data: &[u8]) -> Result<String, STTError> {
        // Get IAM token
        let token = self.get_token().await?;

        // Build request
        let request = HuaweiShortAsrRequest::new(
            audio_data,
            self.config.audio_format.as_str(),
            self.config.model.as_str(),
            self.config.add_punctuation,
            self.config.digit_norm,
            self.config.vocabulary_id.as_deref(),
            self.config.need_word_info,
        );

        let request_json = request.to_json().map_err(|e| {
            STTError::AudioProcessingError(format!("Failed to serialize request: {}", e))
        })?;

        let url = self.config.get_short_asr_url();

        debug!(
            "Sending short audio request to {}, {} bytes",
            url,
            audio_data.len()
        );

        let response = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Auth-Token", &token)
            .body(request_json)
            .send()
            .await
            .map_err(|e| STTError::NetworkError(format!("REST API request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(STTError::ProviderError(format!(
                "Huawei Cloud API error {}: {}",
                status, body
            )));
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to read response: {}", e)))?;

        let asr_response = HuaweiShortAsrResponse::from_json(&response_text)
            .map_err(|e| STTError::ProviderError(format!("Failed to parse response: {}", e)))?;

        if !asr_response.is_success() {
            if let Some(error) = asr_response.get_error() {
                return Err(STTError::ProviderError(error));
            }
            return Err(STTError::ProviderError("Unknown error".to_string()));
        }

        asr_response
            .get_transcript()
            .map(|s| s.to_string())
            .ok_or_else(|| STTError::ProviderError("No transcript in response".to_string()))
    }

    /// Get the recommended chunk size for audio streaming.
    pub fn get_chunk_size(&self) -> usize {
        self.config.get_chunk_size()
    }

    /// Check if using WebSocket mode.
    pub fn is_websocket_mode(&self) -> bool {
        self.config.mode.is_websocket()
    }

    /// Check if session is ready for audio.
    pub fn is_session_ready(&self) -> bool {
        self.session_ready.load(Ordering::SeqCst)
    }

    /// Get the operation mode.
    pub fn get_mode(&self) -> HuaweiCloudAsrMode {
        self.config.mode
    }
}

#[async_trait::async_trait]
impl BaseSTT for HuaweiCloudStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        HuaweiCloudStt::new(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        match self.config.mode {
            HuaweiCloudAsrMode::ShortSentence => {
                // REST API mode: just mark as ready (no persistent connection)
                // Validate token works
                let _ = self.get_token().await?;
                self.connected.store(true, Ordering::SeqCst);
                self.state_notify.notify_waiters();
                info!("Huawei Cloud STT ready in REST API mode");
                Ok(())
            }
            HuaweiCloudAsrMode::Streaming | HuaweiCloudAsrMode::Continuous => {
                self.connect_realtime().await
            }
        }
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Record the intent BEFORE the connected-guard so the supervisor sees it even if the
        // transport's run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);

        if self.connected.load(Ordering::SeqCst) {
            info!("Disconnecting from Huawei Cloud STT...");
        }

        // Drop audio sender to signal end
        self.ws_sender.take();

        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }

        // Wait for connection task to complete
        if let Some(handle) = self.connection_handle.take() {
            crate::core::observability::await_task_shutdown(
                "huawei-cloud-stt-connection",
                handle,
                Duration::from_secs(5),
            )
            .await;
        }

        // Abort forwarding tasks
        if let Some(handle) = self.result_forward_handle.take() {
            crate::core::observability::abort_and_await_task(
                "huawei-cloud-stt-result-forwarder",
                handle,
            )
            .await;
        }

        if let Some(handle) = self.error_forward_handle.take() {
            crate::core::observability::abort_and_await_task(
                "huawei-cloud-stt-error-forwarder",
                handle,
            )
            .await;
        }

        // Clear audio buffer
        self.audio_buffer.lock().await.clear();

        // Reset session state
        self.session_ready.store(false, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        info!("Disconnected from Huawei Cloud STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn send_audio(&mut self, audio: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        match self.config.mode {
            HuaweiCloudAsrMode::ShortSentence => {
                // REST API mode: accumulate in buffer
                self.audio_buffer.lock().await.extend_from_slice(&audio);
            }
            HuaweiCloudAsrMode::Streaming | HuaweiCloudAsrMode::Continuous => {
                // WebSocket mode: send to channel
                if let Some(sender) = &self.ws_sender {
                    sender.send(audio).await.map_err(|_| {
                        STTError::ProviderError("Failed to send audio to channel".to_string())
                    })?;
                }
            }
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
        let huawei_config = HuaweiCloudSttConfig::from_base(config.clone())?;
        huawei_config.validate()?;

        self.config = huawei_config;
        self.base_config = config;

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        PROVIDER_INFO
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `connect_realtime` drives the generic
        // ReconnectableStream supervisor with them — every Huawei RASR session trips the same
        // breaker and shares the one process-wide reconnect cap (W-D2). The ShortSentence REST
        // mode has no persistent stream and is unaffected.
        self.resilience = Some(resilience);
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    fn create_test_config() -> STTConfig {
        STTConfig {
            api_key: "test_user|test_pass|test_domain|test_project".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm16k16bit".to_string(),
            model: "chinese_16k_general".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_new_client() {
        let config = create_test_config();
        let result = HuaweiCloudStt::new(config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn huawei_cloud_stt_redirect_policy_rejects_private_hop() {
        let _env = crate::core::net::ssrf_env_lock();
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(err) => {
                if err.kind() == ErrorKind::PermissionDenied {
                    eprintln!(
                        "Skipping huawei_cloud_stt_redirect_policy_rejects_private_hop: {err}"
                    );
                    return;
                }
                panic!("Failed to bind redirect test server listener: {err}");
            }
        };
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: http://127.0.0.1:9/metadata\r\n",
                "Content-Length: 0\r\n",
                "\r\n"
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });

        let err = huawei_stt_http_client()
            .unwrap()
            .get(format!("http://{addr}/start"))
            .send()
            .await
            .expect_err("private redirect target must be rejected");
        let mut error_chain = err.to_string();
        let mut source = std::error::Error::source(&err);
        while let Some(error) = source {
            error_chain.push_str(": ");
            error_chain.push_str(&error.to_string());
            source = error.source();
        }
        assert!(
            error_chain.contains("redirect URL rejected"),
            "unexpected redirect error: {error_chain}"
        );
    }

    #[test]
    fn test_new_client_invalid_api_key_format() {
        let config = STTConfig {
            api_key: "invalid_format".to_string(),
            ..Default::default()
        };
        let result = HuaweiCloudStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_client_empty_credentials() {
        let config = STTConfig {
            api_key: "|||".to_string(),
            ..Default::default()
        };
        let result = HuaweiCloudStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_not_connected_initially() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();
        let info = stt.get_provider_info();

        assert!(info.contains("Huawei") || info.contains("华为"));
    }

    #[test]
    fn test_config_access() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config.clone()).unwrap();
        assert!(stt.get_config().is_some());
        assert!(stt.get_config().unwrap().api_key.contains("test_user"));
    }

    #[test]
    fn test_chunk_size() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();

        // 200ms at 16kHz, 16-bit: 16000 * 2 * 200 / 1000 = 6400 bytes
        assert_eq!(stt.get_chunk_size(), 6400);
    }

    #[test]
    fn test_is_websocket_mode() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();
        assert!(stt.is_websocket_mode()); // Default is streaming mode
    }

    #[test]
    fn test_get_mode() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();
        assert_eq!(stt.get_mode(), HuaweiCloudAsrMode::Streaming);
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = HuaweiCloudStt::new(config).unwrap();

        let result = stt.send_audio(Bytes::from_static(&[0u8; 1024])).await;
        assert!(matches!(result, Err(STTError::ConnectionFailed(_))));
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = HuaweiCloudStt::new(config).unwrap();

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
        let mut stt = HuaweiCloudStt::new(config).unwrap();
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
        let mut stt = HuaweiCloudStt::new(config).unwrap();

        let result_cb: STTResultCallback = Arc::new(|_| Box::pin(async {}));
        let error_cb: STTErrorCallback = Arc::new(|_| Box::pin(async {}));

        assert!(stt.on_result(result_cb).await.is_ok());
        assert!(stt.on_error(error_cb).await.is_ok());
    }

    #[tokio::test]
    async fn test_update_config() {
        let config = create_test_config();
        let mut stt = HuaweiCloudStt::new(config).unwrap();

        // Use 8k model with 8kHz sample rate (consistent configuration)
        let new_config = STTConfig {
            api_key: "new_user|new_pass|new_domain|new_project".to_string(),
            language: "zh".to_string(),
            sample_rate: 8000,
            encoding: "pcm8k16bit".to_string(),
            model: "chinese_8k_general".to_string(),
            ..Default::default()
        };

        let result = stt.update_config(new_config).await;
        assert!(result.is_ok());
        assert!(stt.get_config().unwrap().api_key.contains("new_user"));
    }

    #[test]
    fn test_handle_realtime_response_final() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);
        let session_ready = AtomicBool::new(true);

        let json = r#"{
            "resp_type": "END",
            "error_code": 0,
            "result": {
                "text": "你好世界",
                "score": 0.95,
                "is_final": true
            }
        }"#;

        HuaweiCloudStt::handle_realtime_response(json, &result_tx, &error_tx, &session_ready);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.transcript, "你好世界");
        assert!(result.is_final);
        assert_eq!(result.confidence, 0.95);
    }

    #[test]
    fn test_handle_realtime_response_interim() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);
        let session_ready = AtomicBool::new(true);

        let json = r#"{
            "resp_type": "RESULT",
            "error_code": 0,
            "result": {
                "text": "你好",
                "is_final": false
            }
        }"#;

        HuaweiCloudStt::handle_realtime_response(json, &result_tx, &error_tx, &session_ready);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.transcript, "你好");
        assert!(!result.is_final);
    }

    #[test]
    fn test_handle_realtime_response_error() {
        let (result_tx, _result_rx) = mpsc::channel(10);
        let (error_tx, mut error_rx) = mpsc::channel(10);
        let session_ready = AtomicBool::new(true);

        let json = r#"{
            "resp_type": "ERROR",
            "error_code": 3,
            "error_msg": "Invalid parameter"
        }"#;

        HuaweiCloudStt::handle_realtime_response(json, &result_tx, &error_tx, &session_ready);

        let error = error_rx.try_recv();
        assert!(error.is_ok());
    }

    #[test]
    fn test_handle_realtime_response_started() {
        let (result_tx, _result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);
        let session_ready = AtomicBool::new(false);

        let json = r#"{
            "resp_type": "STARTED",
            "error_code": 0
        }"#;

        HuaweiCloudStt::handle_realtime_response(json, &result_tx, &error_tx, &session_ready);

        assert!(session_ready.load(Ordering::SeqCst));
    }

    #[test]
    fn test_handle_realtime_response_ended() {
        let (result_tx, _result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);
        let session_ready = AtomicBool::new(true);

        let json = r#"{
            "resp_type": "ENDED",
            "error_code": 0
        }"#;

        HuaweiCloudStt::handle_realtime_response(json, &result_tx, &error_tx, &session_ready);

        assert!(!session_ready.load(Ordering::SeqCst));
    }

    #[test]
    fn test_session_ready() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();
        assert!(!stt.is_session_ready());
    }
}
