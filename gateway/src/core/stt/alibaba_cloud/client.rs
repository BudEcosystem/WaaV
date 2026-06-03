//! Alibaba Cloud DashScope STT WebSocket Client
//!
//! This module implements the `BaseSTT` trait for Alibaba Cloud's DashScope
//! Speech-to-Text WebSocket API.
//!
//! # Architecture
//!
//! The client supports two message formats:
//!
//! 1. **Qwen Format**: For Qwen3-ASR models using OpenAI-like realtime protocol
//! 2. **Inference Format**: For Paraformer models using DashScope inference protocol
//!
//! # WebSocket Message Flow (Qwen Format)
//!
//! ```text
//! Client                              Server
//!   |                                    |
//!   |------ Connect with Bearer -------->|
//!   |<----- HTTP 101 Upgrade ------------|
//!   |                                    |
//!   |------ session.update ------------->|
//!   |<----- session.created -------------|
//!   |                                    |
//!   |------ audio buffer append -------->|
//!   |<----- transcription results -------|
//!   |                                    |
//!   |------ session.finish ------------->|
//!   |<----- session.finished ------------|
//! ```

use bytes::Bytes;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{http::Request, protocol::Message},
};
use tracing::{debug, error, info, warn};

use super::config::{DashScopeSttConfig, TurnDetectionMode};
use super::messages::{
    ParaformerFinishTask, ParaformerResponse, ParaformerRunTask, QwenAudioBufferAppend,
    QwenServerMessage, QwenSessionFinish, QwenSessionUpdate,
};
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
const PROVIDER_INFO: &str = "Alibaba Cloud DashScope STT (阿里云)";

/// WebSocket connection timeout.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// WebSocket message timeout (idle detection).
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

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

/// The concrete WebSocket stream type DashScope dials.
type DashScopeWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

// =============================================================================
// Reconnect transport (W-D1 fleet adoption)
// =============================================================================

/// A [`WsTransport`] that adapts DashScope's streaming event loop to the generic
/// [`ReconnectableStream`] supervisor. One is built per (re)connect by the supervisor's
/// `connect` closure.
///
/// Like Azure (config carried in **post-handshake messages**, not the URL), DashScope opens its
/// featured session with a `session.update` (Qwen realtime) or `run-task` (Paraformer inference)
/// message after the handshake. So [`restore_session`](WsTransport::restore_session) re-sends that
/// message on the fresh socket — without it a reconnect would resume as a *bare* (un-featured)
/// session. [`run`](WsTransport::run) replaces the original split send/recv tasks with a single
/// `select!` loop that returns a [`ReconnectOutcome`] so a mid-stream transport drop reconnects
/// instead of silently ending the session.
struct DashScopeTransport {
    ws_sink: SplitSink<DashScopeWs, Message>,
    ws_stream: SplitStream<DashScopeWs>,
    /// Shared inbound audio receiver (single-consumer; locked for the duration of `run`).
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown signal (fires once; an intentional close must not reconnect).
    shutdown_rx: Arc<Mutex<oneshot::Receiver<()>>>,
    result_tx: mpsc::Sender<STTResult>,
    error_tx: mpsc::Sender<STTError>,
    /// Fires once after the featured session is (re)established, unblocking `connect`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    /// True for Qwen realtime models (session.update + JSON audio buffer append); false for
    /// Paraformer inference models (run-task + binary audio).
    is_qwen: bool,
    /// The post-handshake session-open message (Qwen `session.update` JSON or Paraformer
    /// `run-task` JSON) re-sent on every restore so reconnects keep the featured session.
    session_open_json: String,
    /// The Paraformer task id correlated with this connection's run-task; re-minted per restore
    /// (a fresh run-task carries a fresh task id). `None` for Qwen.
    task_id: Option<String>,
}

#[async_trait::async_trait]
impl WsTransport for DashScopeTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // Re-send the featured session-open message on this fresh socket (Qwen `session.update`
        // or Paraformer `run-task`). A reconnect must NOT resume as a bare session.
        self.ws_sink
            .send(Message::Text(self.session_open_json.clone().into()))
            .await
            .map_err(|e| {
                RestoreError::new(format!("failed to send DashScope session-open message: {e}"))
            })?;

        // The featured session is established: signal the waiting connect() exactly once.
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
                // Handle outgoing audio data
                Some(audio) = audio_rx.recv() => {
                    let msg = if self.is_qwen {
                        let audio_msg = QwenAudioBufferAppend::from_bytes(&audio);
                        Message::Text(audio_msg.to_json().unwrap_or_default().into())
                    } else {
                        // Paraformer expects binary audio.
                        Message::Binary(audio.to_vec().into())
                    };
                    if let Err(e) = self.ws_sink.send(msg).await {
                        let stt_error = STTError::NetworkError(format!(
                            "Failed to send audio to DashScope: {e}"
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
                            if self.is_qwen {
                                DashScopeStt::handle_qwen_response(
                                    &text,
                                    &self.result_tx,
                                    &self.error_tx,
                                );
                            } else {
                                DashScopeStt::handle_paraformer_response(
                                    &text,
                                    &self.result_tx,
                                    &self.error_tx,
                                );
                            }
                        }
                        Ok(Some(Ok(Message::Close(_)))) => {
                            // The provider signalled end-of-session — an intentional completion,
                            // NOT a transport drop.
                            debug!("DashScope WebSocket closed by server");
                            return ReconnectOutcome::Completed;
                        }
                        Ok(Some(Ok(Message::Ping(_)))) => {
                            // Pong handled automatically by tungstenite.
                        }
                        Ok(Some(Ok(_))) => {
                            // Binary/Pong/Frame — ignore.
                        }
                        Ok(Some(Err(e))) => {
                            let stt_error = STTError::ConnectionFailed(e.to_string());
                            error!("DashScope WebSocket error: {}", stt_error);
                            let _ = self.error_tx.try_send(stt_error);
                            return ReconnectOutcome::Reconnectable(StreamError::new("websocket error"));
                        }
                        Ok(None) => {
                            info!("DashScope WebSocket stream ended");
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            let stt_error = STTError::NetworkError(
                                "DashScope WebSocket idle timeout - no message for 60 seconds".into()
                            );
                            error!("{}", stt_error);
                            let _ = self.error_tx.try_send(stt_error);
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }

                // Handle shutdown signal (intentional close — must NOT reconnect)
                _ = &mut *shutdown_rx => {
                    debug!("Received shutdown signal for DashScope STT");
                    // Send the graceful finish message before closing.
                    let finish_msg = if self.is_qwen {
                        Some(Message::Text(
                            QwenSessionFinish::new().to_json().unwrap_or_default().into(),
                        ))
                    } else {
                        self.task_id.as_ref().map(|tid| {
                            Message::Text(
                                ParaformerFinishTask::new(tid).to_json().unwrap_or_default().into(),
                            )
                        })
                    };
                    if let Some(msg) = finish_msg {
                        let _ = self.ws_sink.send(msg).await;
                    }
                    return ReconnectOutcome::Completed;
                }
            }
        }
    }
}

// =============================================================================
// DashScope STT Client
// =============================================================================

/// Alibaba Cloud DashScope Speech-to-Text WebSocket client.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::alibaba_cloud::DashScopeStt;
///
/// let config = STTConfig {
///     api_key: "sk-xxxxxxxx".to_string(),
///     language: "zh".to_string(),
///     sample_rate: 16000,
///     ..Default::default()
/// };
///
/// let mut stt = DashScopeStt::new(config)?;
/// stt.connect().await?;
/// stt.send_audio(audio_data).await?;
/// stt.disconnect().await?;
/// ```
pub struct DashScopeStt {
    /// Base configuration for BaseSTT trait.
    base_config: STTConfig,

    /// DashScope-specific configuration.
    config: DashScopeSttConfig,

    /// Connection state.
    connected: Arc<AtomicBool>,

    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before firing `shutdown_tx`, so a client close racing a
    /// server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,

    /// State change notification.
    state_notify: Arc<Notify>,

    /// WebSocket sender for audio data.
    ws_sender: Option<mpsc::Sender<Bytes>>,

    /// Shutdown signal sender.
    shutdown_tx: Option<oneshot::Sender<()>>,

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

    /// Task ID for Paraformer format.
    task_id: Arc<Mutex<Option<String>>>,

    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`] supervisor. `None` before `set_resilience` (a direct
    /// unit-test construction) → the supervisor uses its own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl DashScopeStt {
    /// Create a new DashScope STT client.
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        let dashscope_config = DashScopeSttConfig::from_base(config.clone())?;
        dashscope_config.validate()?;

        Ok(Self {
            base_config: config,
            config: dashscope_config,
            connected: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            task_id: Arc::new(Mutex::new(None)),
            resilience: None,
        })
    }

    /// W1 keystone — construct directly from the standardized config so the advanced features
    /// DashScope can express (word timestamps, context-biasing keyterms, filler-word retention,
    /// endpointing window, automatic language detection) are honored END-TO-END. The flat
    /// `BaseSTT::new` path maps only the base config; this is the reachable standardized path.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "DashScope API key is required".to_string(),
            ));
        }
        let dashscope_config = DashScopeSttConfig::from_standard(std)?;
        dashscope_config.validate()?;

        Ok(Self {
            base_config: std.base.clone(),
            config: dashscope_config,
            connected: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            task_id: Arc::new(Mutex::new(None)),
            resilience: None,
        })
    }

    /// Build WebSocket request with authentication headers.
    fn build_request(&self) -> Result<Request<()>, STTError> {
        let url = self.config.get_websocket_url();

        let mut request = Request::builder()
            .uri(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("User-Agent", "WaaV-Gateway/1.0");

        // Add OpenAI-Beta header for Qwen models
        if self.config.model.is_qwen_model() {
            request = request.header("OpenAI-Beta", "realtime=v1");
        }

        request
            .body(())
            .map_err(|e| STTError::ConnectionFailed(format!("Failed to build request: {}", e)))
    }

    /// Create session update message for Qwen format.
    fn create_qwen_session_update(&self) -> String {
        let turn_detection_type = match self.config.turn_detection {
            TurnDetectionMode::ServerVad => "server_vad",
            TurnDetectionMode::Manual => "manual",
            TurnDetectionMode::None => "none",
        };

        let msg = QwenSessionUpdate::new(
            self.config.language.as_code(),
            self.config.sample_rate,
            self.config.audio_format.as_format_str(),
            self.config.silence_duration_ms,
            turn_detection_type,
            // Server-VAD speech-activation threshold (Qwen realtime), wired from the standardized
            // extras passthrough; `None` keeps the API default.
            self.config.turn_detection_threshold,
        );

        msg.to_json().unwrap_or_default()
    }

    /// Create run-task message for Paraformer format.
    fn create_paraformer_run_task(&self) -> (String, String) {
        let msg = ParaformerRunTask::new(
            self.config.model.as_model_id(),
            self.config.audio_format.as_format_str(),
            self.config.sample_rate,
            self.config.language.as_code(),
            self.config.disfluency_removal,
            self.config.punctuation,
            // VAD multi-threshold mode (Paraformer inference), wired from the standardized extras
            // passthrough; `None` omits the field (server default).
            self.config.multi_threshold_mode_enabled,
        );

        let task_id = msg.task_id().to_string();
        let json = msg.to_json().unwrap_or_default();
        (json, task_id)
    }

    /// Handle Qwen format response.
    fn handle_qwen_response(
        text: &str,
        result_tx: &mpsc::Sender<STTResult>,
        error_tx: &mpsc::Sender<STTError>,
    ) {
        match QwenServerMessage::from_json(text) {
            Ok(msg) => {
                if msg.is_error() {
                    if let Some(err) = &msg.error {
                        let _ =
                            error_tx.try_send(STTError::AudioProcessingError(err.message.clone()));
                    }
                } else if msg.is_transcription_completed() {
                    if let Some(transcript) = msg.get_transcript() {
                        let result = STTResult::new(transcript.to_string(), true, true, 1.0);
                        let _ = result_tx.try_send(result);
                    }
                } else if msg.is_session_created() || msg.is_session_updated() {
                    debug!("DashScope session event: {}", msg.msg_type);
                } else if msg.is_session_finished() {
                    debug!("DashScope session finished");
                }
            }
            Err(e) => {
                warn!("Failed to parse Qwen response: {}", e);
            }
        }
    }

    /// Handle Paraformer format response.
    fn handle_paraformer_response(
        text: &str,
        result_tx: &mpsc::Sender<STTResult>,
        error_tx: &mpsc::Sender<STTError>,
    ) {
        match ParaformerResponse::from_json(text) {
            Ok(msg) => {
                if msg.is_task_failed() {
                    if let Some((code, message)) = msg.get_error() {
                        let _ = error_tx.try_send(STTError::AudioProcessingError(format!(
                            "[{}] {}",
                            code, message
                        )));
                    }
                } else if msg.is_result_generated() {
                    if let Some(transcript) = msg.get_transcript() {
                        let result = STTResult::new(
                            transcript.to_string(),
                            msg.is_final(),
                            msg.is_final(),
                            1.0,
                        );
                        let _ = result_tx.try_send(result);
                    }
                } else if msg.is_task_started() {
                    debug!("DashScope Paraformer task started");
                } else if msg.is_task_finished() {
                    debug!("DashScope Paraformer task finished");
                }
            }
            Err(e) => {
                warn!("Failed to parse Paraformer response: {}", e);
            }
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for DashScopeStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        DashScopeStt::new(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        info!("Connecting to Alibaba Cloud DashScope STT...");

        // Create channels
        let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER);
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(RESULT_CHANNEL_BUFFER);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(ERROR_CHANNEL_BUFFER);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        self.ws_sender = Some(audio_tx);
        self.shutdown_tx = Some(shutdown_tx);

        // Build the featured session-open message once (re-sent verbatim on every restore by the
        // supervised transport). Qwen realtime models open with `session.update`; Paraformer
        // inference models open with `run-task` (whose task id is captured here so the matching
        // `finish-task` on shutdown references it). Re-sending the same run-task on reconnect is
        // the correct featured-session restore — same model, format, language, VAD parameters.
        let is_qwen = self.config.model.is_qwen_model();
        let (session_open_json, paraformer_task_id) = if is_qwen {
            (self.create_qwen_session_update(), None)
        } else {
            let (json, task_id) = self.create_paraformer_run_task();
            (json, Some(task_id))
        };
        if let Some(tid) = &paraformer_task_id {
            *self.task_id.lock().await = Some(tid.clone());
        }
        let url = self.config.get_websocket_url();

        // Shared state the supervised transport re-uses across reconnect attempts: a single-
        // consumer audio receiver + shutdown oneshot (locked per `run`) and the one-shot connected
        // signal that fires after the featured session is restored.
        let audio_rx = Arc::new(Mutex::new(audio_rx));
        let shutdown_rx = Arc::new(Mutex::new(shutdown_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));

        // Clone the connect-closure inputs (request must be rebuilt each attempt since
        // `http::Request` is not `Clone`).
        let api_key = self.config.api_key.clone();
        let is_qwen_model = is_qwen;

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor with
        // the shared process-global handles from CoreState (W-D1/W-D2 fleet adoption). When no
        // handles were injected (a direct unit-test construction), the supervisor uses its own
        // per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("alibaba_cloud", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => ReconnectableStream::new(ReconnectableStreamConfig::new(
                "alibaba_cloud",
                reconnection,
            )),
        }
        .with_disconnect_flag(disconnect_flag);

        // Set connected state (the BaseSTT contract: `connect()` returns once the session is
        // accepted; the supervisor owns the durable reconnect loop from here on).
        self.connected.store(true, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        // Start the connection task: the supervisor owns the outer reconnect loop; the `connect`
        // closure dials the featured URL with Bearer auth and hands back a transport whose
        // `restore_session` re-sends the `session.update`/`run-task` and whose `run()` is the
        // DashScope event loop.
        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let url = url.clone();
                    let api_key = api_key.clone();
                    let session_open_json = session_open_json.clone();
                    let paraformer_task_id = paraformer_task_id.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let shutdown_rx = Arc::clone(&shutdown_rx);
                    let connected_tx = Arc::clone(&connected_tx);
                    let result_tx = result_tx.clone();
                    let error_tx = error_tx.clone();
                    async move {
                        // Build a fresh request per attempt (Bearer auth in the headers).
                        let mut builder = Request::builder()
                            .uri(&url)
                            .header("Authorization", format!("Bearer {api_key}"))
                            .header("User-Agent", "WaaV-Gateway/1.0");
                        if is_qwen_model {
                            builder = builder.header("OpenAI-Beta", "realtime=v1");
                        }
                        let request = builder.body(()).map_err(|e| {
                            StreamError::new(format!("Failed to build request: {e}"))
                        })?;

                        let (ws_stream, _) =
                            match timeout(WS_CONNECT_TIMEOUT, connect_async(request)).await {
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
                        info!("Connected to DashScope: {}", url);
                        let (ws_sink, ws_stream) = ws_stream.split();

                        Ok(DashScopeTransport {
                            ws_sink,
                            ws_stream,
                            audio_rx,
                            shutdown_rx,
                            result_tx,
                            error_tx,
                            connected_tx,
                            is_qwen: is_qwen_model,
                            session_open_json,
                            task_id: paraformer_task_id,
                        })
                    }
                })
                .await;
            info!("DashScope WebSocket connection closed (supervisor exit: {exit:?})");
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
        match timeout(WS_CONNECT_TIMEOUT, connected_rx).await {
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

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Record the intent BEFORE the connected-guard so the supervisor sees it even if the
        // transport's run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Disconnecting from DashScope STT...");

        // Drop audio sender to signal end
        self.ws_sender.take();

        // Send shutdown signal
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        // Wait for connection task to complete
        if let Some(handle) = self.connection_handle.take() {
            let _ = timeout(Duration::from_secs(5), handle).await;
        }

        // Abort forwarding tasks
        if let Some(handle) = self.result_forward_handle.take() {
            handle.abort();
        }

        if let Some(handle) = self.error_forward_handle.take() {
            handle.abort();
        }

        // Clear task ID
        *self.task_id.lock().await = None;

        self.connected.store(false, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        info!("Disconnected from DashScope STT");
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
        let dashscope_config = DashScopeSttConfig::from_base(config.clone())?;
        self.config = dashscope_config;
        self.base_config = config;

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        PROVIDER_INFO
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `connect` drives the generic
        // ReconnectableStream supervisor with them — every DashScope session trips the same breaker
        // and shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
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
            api_key: "test_api_key".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "qwen3-asr-flash-realtime".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_new_client() {
        let config = create_test_config();
        let result = DashScopeStt::new(config);
        assert!(result.is_ok());
    }

    // W1 keystone: a standardized advanced feature DashScope supports (word timestamps +
    // context-biasing keyterms) survives through `new_standard` into the provider config.
    #[test]
    fn test_new_standard_unlocks_word_timestamps_and_keyterms() {
        use crate::core::stt::standard::{SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: create_test_config(),
            features: SttFeatures {
                word_timestamps: Some(true),
                keyterms: Some(vec!["WaaV".into(), "DashScope".into()]),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let stt = DashScopeStt::new_standard(&std).unwrap();
        assert!(stt.config.word_timestamps);
        assert_eq!(stt.config.context_text.as_deref(), Some("WaaV DashScope"));

        // Missing key is rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig {
            api_key: String::new(),
            ..create_test_config()
        });
        assert!(DashScopeStt::new_standard(&bad).is_err());
    }

    #[test]
    fn test_new_client_empty_api_key() {
        let config = STTConfig {
            api_key: "".to_string(),
            ..Default::default()
        };
        let result = DashScopeStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_not_connected_initially() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config).unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config).unwrap();
        let info = stt.get_provider_info();

        assert!(info.contains("DashScope") || info.contains("阿里云"));
    }

    #[test]
    fn test_config_access() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config.clone()).unwrap();
        assert_eq!(stt.get_config().unwrap().api_key, config.api_key);
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = DashScopeStt::new(config).unwrap();

        let result = stt.send_audio(Bytes::from_static(&[0u8; 1024])).await;
        assert!(matches!(result, Err(STTError::ConnectionFailed(_))));
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = DashScopeStt::new(config).unwrap();

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
        let mut stt = DashScopeStt::new(config).unwrap();
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
        let mut stt = DashScopeStt::new(config).unwrap();

        let result_cb: STTResultCallback = Arc::new(|_| Box::pin(async {}));
        let error_cb: STTErrorCallback = Arc::new(|_| Box::pin(async {}));

        assert!(stt.on_result(result_cb).await.is_ok());
        assert!(stt.on_error(error_cb).await.is_ok());
    }

    #[test]
    fn test_qwen_session_update_creation() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config).unwrap();

        let session_update = stt.create_qwen_session_update();
        assert!(session_update.contains("session.update"));
        assert!(session_update.contains("zh"));
    }

    #[test]
    fn test_paraformer_run_task_creation() {
        let mut config = create_test_config();
        config.model = "paraformer-realtime-v2".to_string();

        let stt = DashScopeStt::new(config).unwrap();
        let (json, task_id) = stt.create_paraformer_run_task();

        assert!(json.contains("run-task"));
        assert!(json.contains("paraformer-realtime-v2"));
        assert!(!task_id.is_empty());
    }

    // WIRE-LEVEL keystone: ProviderExtras -> from_standard -> config -> Paraformer run-task BODY.
    // The VAD multi-threshold knob must round-trip from the standardized extras all the way into
    // the serialized inference run-task `parameters` — proving it reaches the wire, not just the
    // config struct (the recurring bug class).
    #[test]
    fn test_from_standard_multi_threshold_reaches_paraformer_body() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig};
        let mut base = create_test_config();
        base.model = "paraformer-realtime-v2".to_string();
        let extras = ProviderExtras(
            serde_json::json!({ "multi_threshold_mode_enabled": true })
                .as_object()
                .unwrap()
                .clone(),
        );
        let std = StandardSTTConfig {
            base,
            features: Default::default(),
            extras,
        };
        let stt = DashScopeStt::new_standard(&std).unwrap();
        assert_eq!(stt.config.multi_threshold_mode_enabled, Some(true));
        let (json, _) = stt.create_paraformer_run_task();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v["payload"]["parameters"]["multi_threshold_mode_enabled"], true,
            "multi_threshold not on the run-task wire body: {json}"
        );
    }

    // WIRE-LEVEL keystone: ProviderExtras -> from_standard -> config -> Qwen session.update BODY.
    // The server-VAD speech-activation threshold must round-trip into turn_detection.threshold.
    #[test]
    fn test_from_standard_threshold_reaches_qwen_session_update() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig};
        let extras = ProviderExtras(
            serde_json::json!({ "turn_detection.threshold": 0.8 })
                .as_object()
                .unwrap()
                .clone(),
        );
        let std = StandardSTTConfig {
            base: create_test_config(), // qwen3-asr-flash-realtime
            features: Default::default(),
            extras,
        };
        let stt = DashScopeStt::new_standard(&std).unwrap();
        assert_eq!(stt.config.turn_detection_threshold, Some(0.8));
        let json = stt.create_qwen_session_update();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let thr = v["session"]["turn_detection"]["threshold"].as_f64().unwrap();
        assert!(
            (thr - 0.8).abs() < 1e-6,
            "threshold not on the session.update wire body: {json}"
        );
    }

    #[test]
    fn test_websocket_url_qwen() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config).unwrap();

        let url = stt.config.get_websocket_url();
        assert!(url.contains("realtime"));
        assert!(url.contains("qwen3-asr-flash-realtime"));
    }

    #[test]
    fn test_websocket_url_paraformer() {
        let mut config = create_test_config();
        config.model = "paraformer-realtime-v2".to_string();

        let stt = DashScopeStt::new(config).unwrap();
        let url = stt.config.get_websocket_url();
        assert!(url.contains("inference"));
    }

    #[test]
    fn test_build_request_qwen() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config).unwrap();

        let request = stt.build_request();
        assert!(request.is_ok());
    }

    #[test]
    fn test_handle_qwen_response_transcription() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "type": "conversation.item.input_audio_transcription.completed",
            "transcript": "你好世界"
        }"#;

        DashScopeStt::handle_qwen_response(json, &result_tx, &error_tx);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        assert_eq!(result.unwrap().transcript, "你好世界");
    }

    #[test]
    fn test_handle_qwen_response_error() {
        let (result_tx, _result_rx) = mpsc::channel(10);
        let (error_tx, mut error_rx) = mpsc::channel(10);

        let json = r#"{
            "type": "error",
            "error": {
                "type": "invalid_request",
                "code": "400",
                "message": "Invalid audio format"
            }
        }"#;

        DashScopeStt::handle_qwen_response(json, &result_tx, &error_tx);

        let error = error_rx.try_recv();
        assert!(error.is_ok());
    }

    #[test]
    fn test_handle_paraformer_response_result() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "header": {
                "task_id": "test-task-id",
                "event": "result-generated"
            },
            "payload": {
                "output": {
                    "sentence": {
                        "begin_time": 0,
                        "end_time": 1500,
                        "text": "你好世界",
                        "words": [],
                        "sentence_end": true
                    }
                }
            }
        }"#;

        DashScopeStt::handle_paraformer_response(json, &result_tx, &error_tx);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.transcript, "你好世界");
        assert!(result.is_final);
    }

    #[test]
    fn test_handle_paraformer_response_error() {
        let (result_tx, _result_rx) = mpsc::channel(10);
        let (error_tx, mut error_rx) = mpsc::channel(10);

        let json = r#"{
            "header": {
                "task_id": "test-task-id",
                "event": "task-failed",
                "error_code": "401",
                "error_message": "Unauthorized"
            }
        }"#;

        DashScopeStt::handle_paraformer_response(json, &result_tx, &error_tx);

        let error = error_rx.try_recv();
        assert!(error.is_ok());
    }
}
