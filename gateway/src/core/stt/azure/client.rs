//! Azure STT WebSocket client implementation.
//!
//! This module contains the main `AzureSTT` struct that implements the
//! `BaseSTT` trait for real-time speech-to-text streaming using Microsoft
//! Azure Cognitive Services Speech-to-Text API.
//!
//! # Architecture
//!
//! The implementation follows the same patterns as DeepgramSTT and ElevenLabsSTT:
//!
//! ```text
//! ┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
//! │   send_audio()  │────▶│  ws_sender (mpsc)│────▶│  WebSocket Task │
//! └─────────────────┘     └──────────────────┘     └────────┬────────┘
//!                                                           │
//!                         ┌──────────────────┐              │
//!                         │  result_tx (mpsc)│◀─────────────┘
//!                         └────────┬─────────┘
//!                                  │
//!                         ┌────────▼─────────┐
//!                         │ Result Forward   │────▶ User Callback
//!                         │      Task        │
//!                         └──────────────────┘
//! ```
//!
//! # Azure-Specific Details
//!
//! - Uses `Ocp-Apim-Subscription-Key` header for authentication
//! - Includes `X-ConnectionId` header for debugging
//! - Content-Type specifies PCM audio format
//! - Messages may have header prefixes before JSON content

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::time::{Instant, interval, timeout};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, warn};

use super::config::AzureSTTConfig;
use super::messages::{AzureMessage, RecognitionStatus};
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

/// Timeout for receiving WebSocket messages (60 seconds)
/// This prevents hung connections when Azure stops responding
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

// =============================================================================
// Type Aliases
// =============================================================================

/// Type alias for the async result callback function.
type AsyncSTTCallback = Box<
    dyn Fn(STTResult) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Type alias for the async error callback function.
type AsyncErrorCallback = Box<
    dyn Fn(STTError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// The concrete WebSocket stream type Azure dials.
type AzureWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A [`WsTransport`] that adapts Azure's USP streaming event loop to the generic
/// [`ReconnectableStream`] supervisor (W-D1 fleet adoption). One is built per (re)connect by the
/// supervisor's `connect` closure.
///
/// Unlike ElevenLabs (all features in the URL → no-op restore), Azure carries its featured
/// session in **post-handshake USP messages** (`speech.config` + the advanced-feature
/// `speech.context`). So [`restore_session`](WsTransport::restore_session) re-sends those on the
/// fresh socket — without it a reconnect would resume as a *bare* (un-featured) session, exactly
/// the failure mode the supervisor doc warns about. [`run`](WsTransport::run) IS the original
/// `select!` loop, now returning a [`ReconnectOutcome`] so a transport drop reconnects instead of
/// ending the session.
struct AzureTransport {
    ws_sink: SplitSink<AzureWs, Message>,
    ws_stream: SplitStream<AzureWs>,
    /// Shared inbound audio receiver (single-consumer; locked for the duration of `run`).
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown signal (fires once; an intentional close must not reconnect).
    shutdown_rx: Arc<Mutex<oneshot::Receiver<()>>>,
    result_tx: mpsc::Sender<STTResult>,
    error_tx: mpsc::Sender<STTError>,
    /// Fires once after the featured session is (re)established, unblocking `start_connection`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    /// A fresh USP request id per connection (Azure correlates `speech.config`/`speech.context`/
    /// audio under one X-RequestId for a recognition turn).
    request_id: String,
    content_type: String,
    interim_results_enabled: bool,
    /// The advanced-feature `speech.context` body (None → Azure defaults), re-sent on every
    /// restore so reconnects keep diarization/languageId/phrase-list biasing.
    speech_context_body: Option<String>,
    /// Wall clock of the last audio chunk, for the keep-alive silence frames.
    last_audio_time: Instant,
}

#[async_trait::async_trait]
impl WsTransport for AzureTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // Azure USP: re-send the mandatory `speech.config`, then (if any advanced feature is
        // configured) the `speech.context`, under a fresh request id for this connection. This is
        // the featured-session restore — a reconnect must NOT resume as a bare session.
        self.request_id = AzureSTT::new_request_id();
        let speech_config = AzureSTT::usp_text_message(
            "speech.config",
            &self.request_id,
            &AzureSTT::iso8601_now(),
            "application/json",
            &AzureSTT::speech_config_body(),
        );
        self.ws_sink
            .send(Message::Text(speech_config.into()))
            .await
            .map_err(|e| RestoreError::new(format!("failed to send Azure speech.config: {e}")))?;

        if let Some(ref ctx_body) = self.speech_context_body {
            let speech_context = AzureSTT::usp_text_message(
                "speech.context",
                &self.request_id,
                &AzureSTT::iso8601_now(),
                "application/json",
                ctx_body,
            );
            self.ws_sink
                .send(Message::Text(speech_context.into()))
                .await
                .map_err(|e| {
                    RestoreError::new(format!("failed to send Azure speech.context: {e}"))
                })?;
        }

        // The featured session is established: signal the waiting connect() exactly once.
        if let Some(tx) = self.connected_tx.lock().await.take() {
            let _ = tx.send(());
        }
        self.last_audio_time = Instant::now();
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        let mut audio_rx = self.audio_rx.lock().await;
        let mut shutdown_rx = self.shutdown_rx.lock().await;
        let mut keepalive_timer = interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                // Prioritize audio sending for lowest latency
                biased;

                // Handle outgoing audio data
                Some(audio_data) = audio_rx.recv() => {
                    let framed = AzureSTT::usp_audio_frame(
                        &self.request_id,
                        &AzureSTT::iso8601_now(),
                        &self.content_type,
                        &audio_data,
                    );
                    if let Err(e) = self.ws_sink.send(Message::Binary(framed.into())).await {
                        let stt_error = STTError::NetworkError(format!(
                            "Failed to send audio to Azure: {e}"
                        ));
                        error!("{}", stt_error);
                        let _ = self.error_tx.try_send(stt_error);
                        // Transport-level send failure: reconnect to preserve the session.
                        return ReconnectOutcome::Reconnectable(StreamError::new("audio send failed"));
                    }
                    self.last_audio_time = Instant::now();
                }

                // Handle incoming messages with timeout to detect hung connections
                message = timeout(WS_MESSAGE_TIMEOUT, self.ws_stream.next()) => {
                    match message {
                        Ok(Some(Ok(msg))) => {
                            if let Err(e) = AzureSTT::handle_websocket_message(
                                msg,
                                &self.result_tx,
                                self.interim_results_enabled,
                            ) {
                                error!("Azure streaming error: {}", e);
                                let _ = self.error_tx.try_send(e);
                                // A provider recognition error is typically fatal (bad config) —
                                // don't hammer it with reconnects.
                                return ReconnectOutcome::Fatal(StreamError::new("provider error frame"));
                            }
                        }
                        Ok(Some(Err(e))) => {
                            let stt_error = STTError::NetworkError(format!("WebSocket error: {e}"));
                            error!("{}", stt_error);
                            let _ = self.error_tx.try_send(stt_error);
                            return ReconnectOutcome::Reconnectable(StreamError::new("websocket error"));
                        }
                        Ok(None) => {
                            info!("Azure WebSocket stream ended");
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            let stt_error = STTError::NetworkError(
                                "Azure WebSocket timeout - no message received within 60 seconds".to_string()
                            );
                            error!("{}", stt_error);
                            let _ = self.error_tx.try_send(stt_error);
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }

                // Handle keep-alive timer
                _ = keepalive_timer.tick() => {
                    if self.last_audio_time.elapsed() >= Duration::from_secs(5) {
                        let silence_frame = vec![0u8; 64];
                        let framed = AzureSTT::usp_audio_frame(
                            &self.request_id,
                            &AzureSTT::iso8601_now(),
                            &self.content_type,
                            &silence_frame,
                        );
                        if let Err(e) = self.ws_sink.send(Message::Binary(framed.into())).await {
                            let stt_error = STTError::NetworkError(format!(
                                "Failed to send keep-alive: {e}"
                            ));
                            error!("{}", stt_error);
                            let _ = self.error_tx.try_send(stt_error);
                            return ReconnectOutcome::Reconnectable(StreamError::new("keepalive send failed"));
                        }
                        debug!("Sent keep-alive silence frame to Azure");
                        self.last_audio_time = Instant::now();
                    }
                }

                // Handle shutdown signal (intentional close — must NOT reconnect)
                _ = &mut *shutdown_rx => {
                    info!("Received shutdown signal for Azure STT");
                    let _ = self.ws_sink.send(Message::Close(None)).await;
                    return ReconnectOutcome::Completed;
                }
            }
        }
    }
}

// =============================================================================
// Connection State
// =============================================================================

/// Connection state for the WebSocket client.
#[derive(Debug, Clone)]
enum ConnectionState {
    /// Not connected to Azure.
    Disconnected,
    /// In the process of establishing connection.
    Connecting,
    /// Connected and ready to receive audio.
    Connected,
    /// An error occurred (stores error message for debugging).
    #[allow(dead_code)]
    Error(String),
}

// =============================================================================
// AzureSTT Client
// =============================================================================

/// Microsoft Azure Speech-to-Text WebSocket client.
///
/// This struct implements real-time speech-to-text using the Azure Cognitive
/// Services Speech-to-Text WebSocket API. It manages:
///
/// - WebSocket connection lifecycle
/// - Audio data streaming to Azure
/// - Transcription result callbacks (both interim and final)
/// - Error handling and recovery
/// - Keep-alive mechanism to prevent timeout during silence
///
/// # Thread Safety
///
/// All shared state is protected by:
/// - `tokio::sync::Mutex` for async-safe access to callbacks
/// - `Arc<Notify>` for state change notifications
/// - Bounded `mpsc` channels for backpressure control
///
/// # Example
///
/// ```rust,no_run
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::azure::AzureSTT;
/// use std::sync::Arc;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = STTConfig {
///         api_key: "your-azure-subscription-key".to_string(),
///         language: "en-US".to_string(),
///         sample_rate: 16000,
///         ..Default::default()
///     };
///
///     let mut stt = AzureSTT::new(config)?;
///
///     // Register result callback
///     stt.on_result(Arc::new(|result| {
///         Box::pin(async move {
///             println!("Transcript: {}", result.transcript);
///         })
///     })).await?;
///
///     // Connect to Azure
///     stt.connect().await?;
///
///     // Send audio data
///     let audio_data = vec![0u8; 1024]; // Your PCM audio data
///     stt.send_audio(audio_data.into()).await?;
///
///     // Disconnect when done
///     stt.disconnect().await?;
///
///     Ok(())
/// }
/// ```
pub struct AzureSTT {
    /// Configuration for the STT client.
    config: Option<AzureSTTConfig>,

    /// Current connection state.
    state: ConnectionState,

    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before firing `shutdown_tx`, so a client close racing a
    /// server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,

    /// State change notification.
    state_notify: Arc<Notify>,

    /// WebSocket sender for audio data.
    /// Uses bounded channel (32 items) to provide backpressure.
    ws_sender: Option<mpsc::Sender<Bytes>>,

    /// Shutdown signal sender.
    shutdown_tx: Option<oneshot::Sender<()>>,

    /// Result channel sender.
    result_tx: Option<mpsc::Sender<STTResult>>,

    /// Error channel sender.
    error_tx: Option<mpsc::Sender<STTError>>,

    /// Connection task handle.
    connection_handle: Option<tokio::task::JoinHandle<()>>,

    /// Result forwarding task handle.
    result_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Error forwarding task handle.
    error_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Shared callback storage for async access.
    result_callback: Arc<Mutex<Option<AsyncSTTCallback>>>,

    /// Error callback storage.
    error_callback: Arc<Mutex<Option<AsyncErrorCallback>>>,

    /// Connection ID for debugging (sent to Azure in headers).
    connection_id: String,

    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`](crate::core::websocket::ReconnectableStream)
    /// supervisor. `None` before `set_resilience` (a direct unit-test construction) → the
    /// supervisor uses its own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl Default for AzureSTT {
    fn default() -> Self {
        Self {
            config: None,
            state: ConnectionState::Disconnected,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            result_tx: None,
            error_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            connection_id: generate_key(),
            resilience: None,
        }
    }
}

impl AzureSTT {
    /// W1 keystone — construct directly from the standardized config so the advanced features
    /// Azure can express (interim results, word-level timing, profanity handling) are honored
    /// END-TO-END. The flat `BaseSTT::new` path uses `from_base` (defaults only); this is the
    /// reachable standardized path. Capabilities Azure has no field for stay at their defaults.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Azure subscription key is required".to_string(),
            ));
        }
        // `Self` implements `Drop`, so the struct-update (`..Default::default()`) move is illegal;
        // start from the Default value and overwrite only the config.
        let mut stt = Self::default();
        stt.config = Some(AzureSTTConfig::from_standard(std));
        Ok(stt)
    }

    /// Build the Content-Type header value for audio format.
    ///
    /// Azure expects a specific format like:
    /// `audio/wav; codecs=audio/pcm; samplerate=16000`
    fn build_content_type(config: &AzureSTTConfig) -> String {
        format!(
            "audio/wav; codecs=audio/pcm; samplerate={}",
            config.base.sample_rate
        )
    }

    /// A fresh 32-hex-char request id (GUID without dashes), as Azure's USP protocol expects.
    fn new_request_id() -> String {
        uuid::Uuid::new_v4().simple().to_string()
    }

    /// Current UTC time formatted as ISO-8601 (Azure `X-Timestamp`). Built from `time`
    /// accessors so it needs no extra crate feature.
    fn iso8601_now() -> String {
        let n = time::OffsetDateTime::now_utc();
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            n.year(),
            n.month() as u8,
            n.day(),
            n.hour(),
            n.minute(),
            n.second(),
            n.millisecond()
        )
    }

    /// Build an Azure USP **text** message: a CRLF header block (`Path`, `X-RequestId`,
    /// `X-Timestamp`, `Content-Type`) + a blank line + the body. Sent as a WebSocket Text frame.
    fn usp_text_message(
        path: &str,
        request_id: &str,
        timestamp: &str,
        content_type: &str,
        body: &str,
    ) -> String {
        format!(
            "Path: {path}\r\nX-RequestId: {request_id}\r\nX-Timestamp: {timestamp}\r\nContent-Type: {content_type}\r\n\r\n{body}"
        )
    }

    /// Build an Azure USP **binary** (audio) message: a 2-byte big-endian header length, the
    /// CRLF header block, then the raw audio payload. Sending unframed binary (the previous
    /// behaviour) is silently ignored by Azure — this is the protocol the service requires.
    fn usp_audio_frame(
        request_id: &str,
        timestamp: &str,
        content_type: &str,
        payload: &[u8],
    ) -> Vec<u8> {
        let header = format!(
            "Path: audio\r\nX-RequestId: {request_id}\r\nX-Timestamp: {timestamp}\r\nContent-Type: {content_type}\r\n"
        );
        let hb = header.as_bytes();
        let mut out = Vec::with_capacity(2 + hb.len() + payload.len());
        out.extend_from_slice(&(hb.len() as u16).to_be_bytes());
        out.extend_from_slice(hb);
        out.extend_from_slice(payload);
        out
    }

    /// The `speech.config` JSON body Azure requires once after the handshake (system context).
    fn speech_config_body() -> String {
        serde_json::json!({
            "context": {
                "system": { "name": "WaaV-Gateway", "version": "1.0.0" },
                "os": { "platform": "Linux", "name": "WaaV", "version": "1.0" },
                "device": { "manufacturer": "WaaV", "model": "Gateway", "version": "1.0" }
            }
        })
        .to_string()
    }

    /// Handle incoming WebSocket messages from Azure.
    ///
    /// This method parses Azure messages and routes them appropriately:
    /// - SpeechHypothesis -> interim results (if enabled)
    /// - SpeechPhrase -> final results
    /// - Errors -> error channel
    fn handle_websocket_message(
        message: Message,
        result_tx: &mpsc::Sender<STTResult>,
        interim_results_enabled: bool,
    ) -> Result<(), STTError> {
        match message {
            Message::Text(text) => {
                debug!("Received Azure message: {}", text);

                match AzureMessage::parse(&text) {
                    Ok(parsed_msg) => match parsed_msg {
                        AzureMessage::SpeechStartDetected(start) => {
                            debug!("Speech started at offset: {}s", start.offset_seconds());
                        }

                        AzureMessage::SpeechHypothesis(hypothesis) => {
                            if interim_results_enabled && !hypothesis.text.is_empty() {
                                let stt_result = hypothesis.to_stt_result();
                                if result_tx.try_send(stt_result).is_err() {
                                    warn!("Failed to send hypothesis result - channel closed");
                                }
                            }
                        }

                        AzureMessage::SpeechPhrase(phrase) => {
                            // Check recognition status
                            if phrase.recognition_status.is_error() {
                                let error_msg = format!(
                                    "Azure recognition error: {:?}",
                                    phrase.recognition_status
                                );
                                return Err(STTError::ProviderError(error_msg));
                            }

                            // Only send results for successful recognition
                            if let Some(stt_result) = phrase.to_stt_result() {
                                if result_tx.try_send(stt_result).is_err() {
                                    warn!("Failed to send phrase result - channel closed");
                                }
                            } else if phrase.recognition_status == RecognitionStatus::NoMatch {
                                debug!("No speech detected in audio segment");
                            }
                        }

                        AzureMessage::SpeechEndDetected(end) => {
                            debug!("Speech ended at offset: {}s", end.offset_seconds());
                        }

                        AzureMessage::TurnStart => {
                            debug!("Azure recognition turn started");
                        }

                        AzureMessage::TurnEnd => {
                            debug!("Azure recognition turn ended");
                        }

                        AzureMessage::Unknown(raw) => {
                            debug!("Received unknown Azure message: {}", raw);
                        }
                    },
                    Err(e) => {
                        warn!("Failed to parse Azure message: {}", e);
                    }
                }
            }

            Message::Binary(data) => {
                // Azure may send binary data for certain responses
                debug!("Received binary message from Azure: {} bytes", data.len());
            }

            Message::Close(close_frame) => {
                info!("Azure WebSocket closed: {:?}", close_frame);
            }

            Message::Ping(_) => {
                debug!("Received ping from Azure");
            }

            Message::Pong(_) => {
                debug!("Received pong from Azure");
            }

            _ => {
                debug!("Received unexpected message type from Azure");
            }
        }

        Ok(())
    }

    /// Start the WebSocket connection to Azure Speech Services.
    async fn start_connection(&mut self, config: AzureSTTConfig) -> Result<(), STTError> {
        let ws_url = config.build_websocket_url();

        // Create channels for communication
        let (ws_tx, ws_rx) = mpsc::channel::<Bytes>(32);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        // Bounded channels for backpressure - 256 should handle bursts while preventing memory exhaustion
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(256);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(64);
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        // Store channels
        self.ws_sender = Some(ws_tx);
        self.shutdown_tx = Some(shutdown_tx);
        self.result_tx = Some(result_tx.clone());
        self.error_tx = Some(error_tx.clone());

        // Clone necessary data for the connection task
        let api_key = config.base.api_key.clone();
        let host = config.region.stt_hostname();
        let content_type = Self::build_content_type(&config);
        let connection_id = self.connection_id.clone();
        let interim_results_enabled = config.interim_results;
        // Advanced recognition features ride a USP `speech.context` message (built from the
        // standardized features). `None` → no advanced feature requested, so the message is
        // skipped and the wire stays at Azure defaults. The supervised transport re-sends this on
        // every restore so reconnects keep the featured session.
        let speech_context_body = config.build_speech_context_body();

        // Shared state the supervised transport re-uses across reconnect attempts: a single-
        // consumer audio receiver + shutdown oneshot (locked per `run`) and the one-shot connected
        // signal that fires after the featured session is restored.
        let audio_rx = Arc::new(Mutex::new(ws_rx));
        let shutdown_rx = Arc::new(Mutex::new(shutdown_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor (the
        // same one the chaos tests exercise) with the shared process-global handles from CoreState
        // (W-D1/W-D2 fleet adoption). When no handles were injected (a direct unit-test
        // construction), the supervisor uses its own per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        // Share the client-owned intentional-disconnect flag INTO the supervisor (W-D1): a client
        // close racing a server-side close must never trigger a spurious reconnect. Captured here
        // while `self` is still borrowable, before the supervisor is moved into `tokio::spawn`.
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("azure", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => ReconnectableStream::new(ReconnectableStreamConfig::new("azure", reconnection)),
        }
        .with_disconnect_flag(disconnect_flag);

        // Start the connection task: the supervisor owns the outer reconnect loop; the `connect`
        // closure dials with Azure auth headers and hands back a transport whose `restore_session`
        // re-sends the USP `speech.config`/`speech.context` and whose `run()` is the original
        // Azure event loop.
        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let ws_url = ws_url.clone();
                    let host = host.clone();
                    let api_key = api_key.clone();
                    let connection_id = connection_id.clone();
                    let content_type = content_type.clone();
                    let speech_context_body = speech_context_body.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let shutdown_rx = Arc::clone(&shutdown_rx);
                    let connected_tx = Arc::clone(&connected_tx);
                    let result_tx = result_tx.clone();
                    let error_tx = error_tx.clone();
                    async move {
                        let request = tokio_tungstenite::tungstenite::http::Request::builder()
                            .method("GET")
                            .uri(&ws_url)
                            .header("Host", &host)
                            .header("Upgrade", "websocket")
                            .header("Connection", "upgrade")
                            .header("Sec-WebSocket-Key", generate_key())
                            .header("Sec-WebSocket-Version", "13")
                            .header("Ocp-Apim-Subscription-Key", &api_key)
                            .header("X-ConnectionId", &connection_id)
                            .header("Content-Type", &content_type)
                            .body(())
                            .map_err(|e| {
                                StreamError::new(format!("Failed to create WebSocket request: {e}"))
                            })?;

                        let (ws_stream, _response) =
                            match timeout(Duration::from_secs(30), connect_async(request)).await {
                                Ok(Ok(s)) => s,
                                Ok(Err(e)) => {
                                    return Err(StreamError::new(format!(
                                        "Failed to connect to Azure: {e}"
                                    )));
                                }
                                Err(_) => {
                                    return Err(StreamError::new(
                                        "Connection to Azure timed out after 30 seconds".to_string(),
                                    ));
                                }
                            };
                        info!(
                            "Connected to Azure Speech-to-Text WebSocket (connection_id: {})",
                            connection_id
                        );
                        let (ws_sink, ws_stream) = ws_stream.split();
                        Ok(AzureTransport {
                            ws_sink,
                            ws_stream,
                            audio_rx,
                            shutdown_rx,
                            result_tx,
                            error_tx,
                            connected_tx,
                            request_id: AzureSTT::new_request_id(),
                            content_type,
                            interim_results_enabled,
                            speech_context_body,
                            last_audio_time: Instant::now(),
                        })
                    }
                })
                .await;
            info!("Azure STT WebSocket connection closed (supervisor exit: {exit:?})");
        });

        self.connection_handle = Some(connection_handle);

        // Start result forwarding task
        let callback_ref = self.result_callback.clone();
        let result_forwarding_handle = tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                if let Some(callback) = callback_ref.lock().await.as_ref() {
                    callback(result).await;
                } else {
                    debug!(
                        "Azure STT result (no callback): {} (final: {}, confidence: {})",
                        result.transcript, result.is_final, result.confidence
                    );
                }
            }
        });

        self.result_forward_handle = Some(result_forwarding_handle);

        // Start error forwarding task
        let error_callback_ref = self.error_callback.clone();
        let error_forwarding_handle = tokio::spawn(async move {
            while let Some(error) = error_rx.recv().await {
                if let Some(callback) = error_callback_ref.lock().await.as_ref() {
                    callback(error).await;
                } else {
                    error!("Azure STT error (no callback registered): {}", error);
                }
            }
        });

        self.error_forward_handle = Some(error_forwarding_handle);

        // Update state and wait for connection
        self.state = ConnectionState::Connecting;

        // Wait for connection with timeout
        match timeout(Duration::from_secs(30), connected_rx).await {
            Ok(Ok(())) => {
                self.state = ConnectionState::Connected;
                self.state_notify.notify_waiters();
                info!("Successfully connected to Azure Speech-to-Text");
                Ok(())
            }
            Ok(Err(_)) => {
                let error_msg = "Connection channel closed before confirmation".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
            Err(_) => {
                let error_msg = "Connection timeout waiting for Azure".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
        }
    }

    /// Get the connection ID for debugging.
    ///
    /// This ID is sent to Azure in the `X-ConnectionId` header and can be
    /// used to correlate logs between client and server.
    pub fn get_connection_id(&self) -> &str {
        &self.connection_id
    }

    /// The shared circuit breaker this session feeds into the generic supervisor, if the
    /// process-global resilience handles have been injected (W-D1/W-D2). Two `AzureSTT` built from
    /// the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(
        &self,
    ) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }

    /// Get the Azure-specific configuration.
    ///
    /// Returns `None` if the client was not properly initialized.
    pub fn get_azure_config(&self) -> Option<&AzureSTTConfig> {
        self.config.as_ref()
    }
}

impl Drop for AzureSTT {
    fn drop(&mut self) {
        // Send shutdown signal if still connected
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
}

// =============================================================================
// BaseSTT Trait Implementation
// =============================================================================

#[async_trait::async_trait]
impl BaseSTT for AzureSTT {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        // Validate API key (subscription key)
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Azure subscription key is required".to_string(),
            ));
        }

        // Create Azure-specific configuration with defaults
        let azure_config = AzureSTTConfig::from_base(config);

        Ok(Self {
            config: Some(azure_config),
            state: ConnectionState::Disconnected,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            result_tx: None,
            error_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            connection_id: generate_key(),
            resilience: None,
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        // Check if already connected
        if matches!(self.state, ConnectionState::Connected) {
            return Err(STTError::ConnectionFailed(
                "Already connected to Azure".to_string(),
            ));
        }

        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        let config = self.config.as_ref().ok_or_else(|| {
            STTError::ConfigurationError("No configuration available".to_string())
        })?;

        // Generate new connection ID for this connection attempt
        self.connection_id = generate_key();

        self.start_connection(config.clone()).await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Record the intent BEFORE firing the shutdown signal so the supervisor sees it even if the
        // transport's run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);

        // Send shutdown signal
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Wait for connection task to finish with timeout
        if let Some(handle) = self.connection_handle.take() {
            let _ = timeout(Duration::from_secs(5), handle).await;
        }

        // Clean up result forwarding task
        if let Some(handle) = self.result_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        // Clean up error forwarding task
        if let Some(handle) = self.error_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        // Clear all channels
        self.ws_sender = None;
        self.result_tx = None;
        self.error_tx = None;

        // Clear callbacks
        *self.result_callback.lock().await = None;
        *self.error_callback.lock().await = None;

        // Update state
        self.state = ConnectionState::Disconnected;
        self.state_notify.notify_waiters();

        info!("Disconnected from Azure Speech-to-Text");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        matches!(self.state, ConnectionState::Connected) && self.ws_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to Azure Speech-to-Text".to_string(),
            ));
        }

        if let Some(ws_sender) = &self.ws_sender {
            let data_len = audio_data.len();

            // Zero-copy - Bytes passed directly to WebSocket
            ws_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio data: {e}")))?;

            debug!("Queued {} bytes of audio for Azure STT", data_len);
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
        self.config.as_ref().map(|c| &c.base)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        // Disconnect if currently connected
        if self.is_ready() {
            self.disconnect().await?;
        }

        // Preserve Azure-specific settings from existing config
        let existing = self.config.take();

        let azure_config = AzureSTTConfig {
            base: config,
            region: existing
                .as_ref()
                .map(|c| c.region.clone())
                .unwrap_or_default(),
            output_format: existing
                .as_ref()
                .map(|c| c.output_format)
                .unwrap_or_default(),
            profanity: existing.as_ref().map(|c| c.profanity).unwrap_or_default(),
            interim_results: existing.as_ref().map(|c| c.interim_results).unwrap_or(true),
            word_level_timing: existing.as_ref().is_some_and(|c| c.word_level_timing),
            endpoint_id: existing.as_ref().and_then(|c| c.endpoint_id.clone()),
            auto_detect_languages: existing
                .as_ref()
                .and_then(|c| c.auto_detect_languages.clone()),
            // Preserve the advanced speech.context features across a config update (set from the
            // standardized features at session start; must survive a mid-session base swap).
            speaker_diarization: existing.as_ref().is_some_and(|c| c.speaker_diarization),
            segmentation_silence_timeout_ms: existing
                .as_ref()
                .and_then(|c| c.segmentation_silence_timeout_ms),
            language_id_continuous: existing.as_ref().is_some_and(|c| c.language_id_continuous),
            nbest_count: existing.as_ref().and_then(|c| c.nbest_count),
            phrase_list: existing.as_ref().map(|c| c.phrase_list.clone()).unwrap_or_default(),
            phrase_output_options: existing
                .as_ref()
                .map(|c| c.phrase_output_options.clone())
                .unwrap_or_default(),
            dictation_mode: existing.as_ref().is_some_and(|c| c.dictation_mode),
            sentiment_analysis: existing.as_ref().is_some_and(|c| c.sentiment_analysis),
            endpoint_override: existing.as_ref().and_then(|c| c.endpoint_override.clone()),
        };

        self.config = Some(azure_config);

        // Reconnect with new configuration
        self.connect().await
    }

    fn get_provider_info(&self) -> &'static str {
        "Microsoft Azure Speech-to-Text"
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `start_connection` drives the generic
        // ReconnectableStream supervisor with them — every Azure session trips the same breaker
        // and shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

// =============================================================================
// Azure-Specific Helper Methods
// =============================================================================

impl AzureSTT {
    /// Update Azure-specific settings.
    ///
    /// This allows updating Azure-specific parameters without affecting
    /// the base STT configuration. Requires reconnection.
    ///
    /// # Arguments
    ///
    /// * `region` - Optional new Azure region
    /// * `output_format` - Optional new output format (Simple or Detailed)
    /// * `profanity` - Optional profanity handling setting
    /// * `interim_results` - Optional interim results toggle
    pub async fn update_azure_settings(
        &mut self,
        region: Option<super::config::AzureRegion>,
        output_format: Option<super::config::AzureOutputFormat>,
        profanity: Option<super::config::AzureProfanityOption>,
        interim_results: Option<bool>,
    ) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }

        if let Some(config) = &mut self.config {
            if let Some(r) = region {
                config.region = r;
            }
            if let Some(f) = output_format {
                config.output_format = f;
            }
            if let Some(p) = profanity {
                config.profanity = p;
            }
            if let Some(ir) = interim_results {
                config.interim_results = ir;
            }
        }

        self.connect().await
    }

    /// Set a Custom Speech endpoint.
    ///
    /// Use this to specify a Custom Speech model trained on your specific
    /// domain or vocabulary.
    ///
    /// # Arguments
    ///
    /// * `endpoint_id` - The Custom Speech endpoint ID, or None to use default
    pub async fn set_custom_endpoint(
        &mut self,
        endpoint_id: Option<String>,
    ) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }

        if let Some(config) = &mut self.config {
            config.endpoint_id = endpoint_id;
        }

        self.connect().await
    }

    /// Enable automatic language detection.
    ///
    /// When enabled, Azure will automatically detect which of the specified
    /// languages is being spoken.
    ///
    /// # Arguments
    ///
    /// * `languages` - List of BCP-47 language codes to detect, or None to disable
    pub async fn set_auto_detect_languages(
        &mut self,
        languages: Option<Vec<String>>,
    ) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }

        if let Some(config) = &mut self.config {
            config.auto_detect_languages = languages;
        }

        self.connect().await
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: a standardized advanced feature Azure supports (word-level timing) survives
    // through new_standard onto the stored provider config — the flat factory path drops it.
    #[test]
    fn new_standard_carries_word_timestamps_to_config() {
        use crate::core::stt::standard::{SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "azure".into(),
                api_key: "subscription-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
            translation: None,
        };
        let stt = AzureSTT::new_standard(&std).unwrap();
        assert!(stt.config.as_ref().unwrap().word_level_timing);

        // Missing key is rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig {
            api_key: String::new(),
            ..Default::default()
        });
        assert!(AzureSTT::new_standard(&bad).is_err());
    }

    // Azure USP framing — converts the provider from BROKEN (unframed binary, no speech.config)
    // to the documented Universal Speech Protocol. Byte-exact acceptance is gated on the live
    // CI smoke test; these assert the framing STRUCTURE the protocol requires.
    #[test]
    fn usp_audio_frame_has_2byte_header_len_then_header_then_payload() {
        let payload = [1u8, 2, 3, 4];
        let frame = AzureSTT::usp_audio_frame("rid", "2024-01-01T00:00:00.000Z", "audio/x-wav", &payload);
        // First 2 bytes = big-endian header length.
        let hdr_len = u16::from_be_bytes([frame[0], frame[1]]) as usize;
        let header = std::str::from_utf8(&frame[2..2 + hdr_len]).unwrap();
        assert!(header.starts_with("Path: audio\r\n"));
        assert!(header.contains("X-RequestId: rid\r\n"));
        assert!(header.contains("X-Timestamp: 2024-01-01T00:00:00.000Z\r\n"));
        assert!(header.contains("Content-Type: audio/x-wav\r\n"));
        // Payload is appended verbatim after the header block.
        assert_eq!(&frame[2 + hdr_len..], &payload);
    }

    #[test]
    fn usp_text_message_has_headers_blank_line_then_body() {
        let body = AzureSTT::speech_config_body();
        let msg = AzureSTT::usp_text_message(
            "speech.config", "rid", "2024-01-01T00:00:00.000Z", "application/json", &body,
        );
        assert!(msg.starts_with("Path: speech.config\r\n"));
        assert!(msg.contains("Content-Type: application/json\r\n\r\n"));
        // Body is valid JSON with the required `context` block.
        let json_part = msg.split("\r\n\r\n").nth(1).unwrap();
        let v: serde_json::Value = serde_json::from_str(json_part).unwrap();
        assert!(v["context"]["system"]["name"].is_string());
    }

    #[test]
    fn iso8601_now_is_well_formed() {
        let ts = AzureSTT::iso8601_now();
        // YYYY-MM-DDTHH:MM:SS.mmmZ
        assert_eq!(ts.len(), 24);
        assert!(ts.ends_with('Z') && ts.contains('T'));
    }

    // W-D1: disconnect() must record intent on the supervisor-shared flag so a client close racing
    // a server-side close can never trigger a spurious reconnect (the supervisor's loop-top guard
    // observes this same `Arc<AtomicBool>`). Before this wiring the flag was the supervisor's own
    // and disconnect() never set it.
    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
        let config = STTConfig {
            provider: "azure".to_string(),
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };
        let mut stt = <AzureSTT as BaseSTT>::new(config).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the supervisor-shared intentional-disconnect flag",
        );
    }

    #[test]
    fn test_azure_stt_creation() {
        let config = STTConfig {
            provider: "azure".to_string(),
            api_key: "test_subscription_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "default".to_string(),
        };

        let stt = <AzureSTT as BaseSTT>::new(config).unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), "Microsoft Azure Speech-to-Text");
    }

    #[test]
    fn test_azure_stt_empty_api_key_error() {
        let config = STTConfig {
            provider: "azure".to_string(),
            api_key: String::new(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "default".to_string(),
        };

        let result = <AzureSTT as BaseSTT>::new(config);
        assert!(result.is_err());
        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("subscription key"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[test]
    fn test_azure_stt_config_access() {
        let config = STTConfig {
            provider: "azure".to_string(),
            api_key: "test_key".to_string(),
            language: "de-DE".to_string(),
            sample_rate: 8000,
            channels: 1,
            punctuation: false,
            encoding: "linear16".to_string(),
            model: "default".to_string(),
        };

        let stt = <AzureSTT as BaseSTT>::new(config).unwrap();

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.api_key, "test_key");
        assert_eq!(stored_config.language, "de-DE");
        assert_eq!(stored_config.sample_rate, 8000);
    }

    #[test]
    fn test_azure_stt_azure_config_access() {
        let config = STTConfig {
            provider: "azure".to_string(),
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let stt = <AzureSTT as BaseSTT>::new(config).unwrap();

        let azure_config = stt.get_azure_config().unwrap();
        // Default region should be EastUS
        assert_eq!(
            azure_config.region,
            super::super::config::AzureRegion::EastUS
        );
        // Default interim_results should be true
        assert!(azure_config.interim_results);
    }

    #[test]
    fn test_azure_stt_connection_id() {
        let config = STTConfig {
            provider: "azure".to_string(),
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let stt = <AzureSTT as BaseSTT>::new(config).unwrap();

        // Connection ID should be a non-empty string (base64-encoded random bytes)
        let conn_id = stt.get_connection_id();
        assert!(!conn_id.is_empty());
        // The key is base64-encoded, so it should contain only valid base64 characters
        assert!(
            conn_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
        );
    }

    #[test]
    fn test_content_type_building() {
        let config = AzureSTTConfig {
            base: STTConfig {
                sample_rate: 16000,
                ..Default::default()
            },
            ..Default::default()
        };

        let content_type = AzureSTT::build_content_type(&config);
        assert_eq!(
            content_type,
            "audio/wav; codecs=audio/pcm; samplerate=16000"
        );

        let config_8k = AzureSTTConfig {
            base: STTConfig {
                sample_rate: 8000,
                ..Default::default()
            },
            ..Default::default()
        };

        let content_type_8k = AzureSTT::build_content_type(&config_8k);
        assert_eq!(
            content_type_8k,
            "audio/wav; codecs=audio/pcm; samplerate=8000"
        );
    }

    #[test]
    fn test_default_state() {
        let stt = AzureSTT::default();
        assert!(!stt.is_ready());
        assert!(stt.config.is_none());
    }

    #[tokio::test]
    async fn test_send_audio_not_connected_error() {
        let config = STTConfig {
            provider: "azure".to_string(),
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let mut stt = <AzureSTT as BaseSTT>::new(config).unwrap();

        let result = stt.send_audio(vec![0u8; 1024].into()).await;
        assert!(result.is_err());
        if let Err(STTError::ConnectionFailed(msg)) = result {
            assert!(msg.contains("Not connected"));
        } else {
            panic!("Expected ConnectionFailed error");
        }
    }

    #[tokio::test]
    async fn test_callback_registration() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let config = STTConfig {
            provider: "azure".to_string(),
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let mut stt = <AzureSTT as BaseSTT>::new(config).unwrap();

        let callback_registered = Arc::new(AtomicBool::new(false));
        let callback_flag = callback_registered.clone();

        let callback: STTResultCallback = Arc::new(move |_result| {
            callback_flag.store(true, Ordering::SeqCst);
            Box::pin(async {})
        });

        stt.on_result(callback).await.unwrap();

        // Callback should be stored (we can't easily test invocation without a real connection)
        assert!(stt.result_callback.lock().await.is_some());
    }

    #[tokio::test]
    async fn test_error_callback_registration() {
        let config = STTConfig {
            provider: "azure".to_string(),
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let mut stt = <AzureSTT as BaseSTT>::new(config).unwrap();

        let callback: STTErrorCallback = Arc::new(move |_error| Box::pin(async {}));

        stt.on_error(callback).await.unwrap();

        // Error callback should be stored
        assert!(stt.error_callback.lock().await.is_some());
    }

    #[tokio::test]
    async fn test_connect_already_connected_error() {
        // This test verifies the error message for double-connect
        // We can't actually connect in unit tests without Azure credentials
        let config = STTConfig {
            provider: "azure".to_string(),
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let mut stt = <AzureSTT as BaseSTT>::new(config).unwrap();

        // Manually set state to Connected to simulate
        stt.state = ConnectionState::Connected;
        stt.ws_sender = Some(mpsc::channel(1).0); // Dummy sender

        let result = stt.connect().await;
        assert!(result.is_err());
        if let Err(STTError::ConnectionFailed(msg)) = result {
            assert!(msg.contains("Already connected"));
        } else {
            panic!("Expected ConnectionFailed error");
        }
    }
}
