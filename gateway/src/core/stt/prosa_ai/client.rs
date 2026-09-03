//! Prosa.ai STT client implementation.
//!
//! This module implements the BaseSTT trait for Prosa.ai's
//! Speech-to-Text service.

use super::config::{
    MIN_AUDIO_BUFFER_SIZE, PROSA_STT_BASE_URL, PROSA_STT_WS_ENDPOINT, ProsaAudioFormat,
    ProsaSttAudioConfig, ProsaSttConfig, ProsaSttModel, ProsaSttRequest, ProsaSttRequestConfig,
    ProsaSttRequestData, ProsaSttResponse, ProsaSttStreamConfig, ProsaSttWsMessage,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use crate::core::stt::wav as stt_wav;

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};
use url::form_urlencoded;

use crate::core::resilience::connect::{WS_CONNECT_TIMEOUT, with_timeout};
use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};
use futures_util::stream::{SplitSink, SplitStream};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Per-message idle timeout for WebSocket message reception.
/// Resets after each successful message. Catches stuck/dead connections.
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// The concrete WebSocket stream type Prosa.ai dials.
type ProsaWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A [`WsTransport`] that adapts Prosa.ai's streaming event loop to the generic
/// [`ReconnectableStream`] supervisor (W-D1 fleet adoption). One is built per (re)connect by the
/// supervisor's `connect` closure.
///
/// Like Azure (and unlike URL-carries-features providers such as ElevenLabs/Cartesia), Prosa.ai
/// carries its featured session in a **post-handshake `config` frame** sent as the first WebSocket
/// message after connect. So [`restore_session`](WsTransport::restore_session) re-sends that
/// `build_stream_config` frame on the fresh socket — without it a reconnect would resume as a
/// *bare* (un-featured) session. [`run`](WsTransport::run) IS the original streaming event loop,
/// now returning a [`ReconnectOutcome`] so a mid-stream transport drop reconnects instead of ending
/// the session.
struct ProsaTransport {
    ws_sink: SplitSink<ProsaWs, Message>,
    ws_stream: SplitStream<ProsaWs>,
    /// Shared inbound audio receiver (single-consumer; locked for the duration of `run`).
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown token (fires once; an intentional close must not reconnect).
    shutdown_token: CancellationToken,
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,
    current_job_id: Arc<RwLock<Option<String>>>,
    /// Fires once after the featured session is (re)established, unblocking `start_streaming`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    /// The serialized streaming `config` frame (the featured session), re-sent on every restore.
    config_json: String,
}

impl ProsaTransport {
    async fn send_end_and_close(ws_sink: &mut SplitSink<ProsaWs, Message>) {
        let _ = ws_sink.send(Message::Binary(Bytes::new())).await;
        let _ = ws_sink.send(Message::Close(None)).await;
    }
}

#[async_trait]
impl WsTransport for ProsaTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // Prosa.ai sends its featured session as a post-handshake `config` frame (the streaming
        // analogue of the REST `config`). Re-send it on this fresh socket so the restored session
        // is identical — same model, label, include_partial, include_filler, audio params.
        self.ws_sink
            .send(Message::Text(self.config_json.clone().into()))
            .await
            .map_err(|e| RestoreError::new(format!("failed to send Prosa.ai config: {e}")))?;

        // The featured session is established: signal the waiting start_streaming() exactly once.
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
                info!("Received shutdown signal for Prosa.ai STT");
                Self::send_end_and_close(&mut self.ws_sink).await;
                return ReconnectOutcome::Completed;
            }

            tokio::select! {
                // Handle outgoing audio data (raw binary chunks).
                Some(audio_data) = audio_rx.recv() => {
                    if let Err(e) = self.ws_sink.send(Message::Binary(audio_data)).await {
                        let stt_error = STTError::ProviderError(format!("Failed to send audio: {e}"));
                        error!("{}", stt_error);
                        if let Some(cb) = self.error_callback.read().await.as_ref() {
                            cb(stt_error).await;
                        }
                        // Transport-level send failure: reconnect to preserve the session.
                        return ReconnectOutcome::Reconnectable(StreamError::new("audio send failed"));
                    }
                }

                // Handle incoming messages with idle timeout.
                message = timeout(WS_MESSAGE_TIMEOUT, self.ws_stream.next()) => {
                    match message {
                        Ok(Some(Ok(msg))) => {
                            match ProsaStt::handle_ws_message(
                                msg,
                                &self.result_callback,
                                &self.error_callback,
                                &self.current_job_id,
                            ).await {
                                ProsaWsAction::Continue => {}
                                ProsaWsAction::ServerClosed => {
                                    info!("Prosa.ai WebSocket closed by server");
                                    return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                                }
                            }
                        }
                        Ok(Some(Err(e))) => {
                            let stt_error = STTError::NetworkError(format!("WebSocket error: {e}"));
                            error!("{}", stt_error);
                            if let Some(cb) = self.error_callback.read().await.as_ref() {
                                cb(stt_error).await;
                            }
                            return ReconnectOutcome::Reconnectable(StreamError::new("websocket error"));
                        }
                        Ok(None) => {
                            info!("Prosa.ai WebSocket stream ended");
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            let stt_error = STTError::NetworkError(
                                "WebSocket idle timeout - no message for 60 seconds".into()
                            );
                            error!("Prosa.ai STT idle timeout: {}", stt_error);
                            if let Some(cb) = self.error_callback.read().await.as_ref() {
                                cb(stt_error).await;
                            }
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }

                // Handle shutdown signal (intentional close — must NOT reconnect).
                _ = shutdown_token.cancelled() => {
                    info!("Received shutdown signal for Prosa.ai STT");
                    Self::send_end_and_close(&mut self.ws_sink).await;
                    return ReconnectOutcome::Completed;
                }
            }
        }
    }
}

/// What the inbound-message handler decided the run loop should do next.
enum ProsaWsAction {
    /// Keep streaming.
    Continue,
    /// The server closed the socket (reconnect-eligible).
    ServerClosed,
}

// =============================================================================
// Prosa STT Client
// =============================================================================

/// Prosa.ai STT client.
pub struct ProsaStt {
    /// Provider-specific configuration.
    config: ProsaSttConfig,

    /// Original base configuration.
    base_config: Option<STTConfig>,

    /// HTTP client for REST API.
    http_client: Option<Client>,

    /// Outbound audio channel to the supervised streaming transport (bounded for backpressure).
    /// `None` until the streaming session is started.
    audio_tx: Arc<RwLock<Option<mpsc::Sender<Bytes>>>>,

    /// Shutdown signal token for the supervised streaming task.
    shutdown_token: Arc<RwLock<Option<CancellationToken>>>,

    /// Supervised streaming connection task handle.
    connection_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,

    /// Audio buffer for batching (REST/batch mode).
    audio_buffer: Arc<RwLock<Vec<u8>>>,

    /// Connection state flag.
    is_connected: Arc<AtomicBool>,

    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before cancelling `shutdown_token`, so a client close racing a
    /// server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,

    /// Result callback.
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,

    /// Error callback.
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,

    /// Current job ID.
    current_job_id: Arc<RwLock<Option<String>>>,

    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`](crate::core::websocket::ReconnectableStream)
    /// supervisor. `None` before `set_resilience` (a direct unit-test construction) → the
    /// supervisor uses its own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

fn prosa_stt_http_client(timeout_secs: u64) -> Result<Client, reqwest::Error> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
        .timeout(Duration::from_secs(timeout_secs))
        .build()
}

fn default_prosa_stt_http_client() -> Option<Client> {
    match prosa_stt_http_client(super::config::DEFAULT_REQUEST_TIMEOUT) {
        Ok(client) => Some(client),
        Err(err) => {
            error!(
                error = %err,
                "failed to create default Prosa STT HTTP client; default instance is inert until rebuilt with a valid config"
            );
            None
        }
    }
}

impl ProsaStt {
    /// Create a new Prosa.ai STT client.
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        let prosa_config = ProsaSttConfig::from_base(&config)?;
        Self::from_prosa_config(prosa_config, Some(config))
    }

    /// Internal: construct the provider from an already-mapped Prosa config (and the optional
    /// originating base config). Centralizes HTTP-client construction so both the flat `new` and
    /// the standardized `new_standard` paths share one builder.
    fn from_prosa_config(
        prosa_config: ProsaSttConfig,
        base_config: Option<STTConfig>,
    ) -> Result<Self, STTError> {
        let http_client =
            prosa_stt_http_client(prosa_config.request_timeout_secs).map_err(|e| {
                STTError::ConnectionFailed(format!("Failed to create HTTP client: {}", e))
            })?;

        Ok(Self {
            config: prosa_config,
            base_config,
            http_client: Some(http_client),
            audio_tx: Arc::new(RwLock::new(None)),
            shutdown_token: Arc::new(RwLock::new(None)),
            connection_handle: Arc::new(RwLock::new(None)),
            audio_buffer: Arc::new(RwLock::new(Vec::new())),
            is_connected: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            current_job_id: Arc::new(RwLock::new(None)),
            resilience: None,
        })
    }

    /// W1 keystone — construct directly from the standardized config so the advanced features
    /// Prosa.ai can express (interim results, filler words, smart formatting) are honored
    /// END-TO-END. Mirrors `DeepgramSTT::new_standard`: validate the api_key, then build the
    /// provider from the standardized->provider config mapping (`ProsaSttConfig::from_standard`).
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Prosa.ai API key is required".to_string(),
            ));
        }
        let prosa_config = ProsaSttConfig::from_standard(std)?;
        Self::from_prosa_config(prosa_config, Some(std.base.clone()))
    }

    /// Get the Prosa.ai specific configuration.
    pub fn get_prosa_config(&self) -> &ProsaSttConfig {
        &self.config
    }

    /// Set the STT model.
    pub fn set_model(&mut self, model: ProsaSttModel) {
        self.config.model = model;
    }

    /// Set the audio format.
    pub fn set_audio_format(&mut self, format: ProsaAudioFormat) {
        self.config.audio_format = format;
    }

    /// Enable or disable partial results.
    pub fn set_include_partial(&mut self, include: bool) {
        self.config.include_partial = include;
    }

    /// Set speaker count for diarization.
    pub fn set_speaker_count(&mut self, count: u32) {
        self.config.speaker_count = count;
    }

    fn build_websocket_url(&self) -> String {
        // Honor an `endpoint_override` (scheme://host[:port]) for the in-repo mock/proxy: swap the
        // dialed base while re-appending Prosa.ai's WS path (a path-less URL fails the handshake),
        // keeping the `?x-api-key=` query. `None` uses the production endpoint.
        let base = match self
            .config
            .endpoint_override
            .as_deref()
            .filter(|o| !o.is_empty())
        {
            Some(o) => format!("{}/v2/speech/stt", o.trim_end_matches('/')),
            None => PROSA_STT_WS_ENDPOINT.to_string(),
        };
        let encoded_key: String =
            form_urlencoded::byte_serialize(self.config.api_key.as_bytes()).collect();
        format!("{base}?x-api-key={encoded_key}")
    }

    /// Process audio using REST API (synchronous mode).
    async fn process_audio_rest(&self, audio_data: &[u8]) -> Result<String, STTError> {
        let url = crate::core::tts::standard::override_rest_endpoint(
            PROSA_STT_BASE_URL,
            self.config.endpoint_override.as_deref(),
        );

        // Encode audio as base64
        let encoded_audio = BASE64.encode(audio_data);

        // Build request
        let request = ProsaSttRequest {
            config: ProsaSttRequestConfig {
                engine: self.config.model.as_str().to_string(),
                wait: Some(self.config.wait),
                speaker_count: if self.config.speaker_count > 0 {
                    Some(self.config.speaker_count)
                } else {
                    None
                },
                include_filler: Some(self.config.include_filler),
                auto_punctuation: Some(self.config.auto_punctuation),
                enable_spoken_numerals: Some(self.config.enable_spoken_numerals),
            },
            request: ProsaSttRequestData {
                label: self.config.label.clone(),
                data: Some(encoded_audio),
                uri: None,
            },
        };

        debug!("Sending STT request to Prosa.ai REST API");

        let http_client = self.http_client.as_ref().ok_or_else(|| {
            STTError::ConfigurationError(
                "Prosa STT default HTTP client is unavailable; construct with ProsaStt::new or new_standard".to_string(),
            )
        })?;

        let response = http_client
            .post(url)
            .header("x-api-key", &self.config.api_key)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| STTError::ConnectionFailed(format!("Failed to send request: {}", e)))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| STTError::ProviderError(format!("Failed to read response: {}", e)))?;

        debug!(
            "Received response: status={}, body_len={}",
            status,
            body.len()
        );

        if !status.is_success() {
            return Err(self.parse_error_response(status.as_u16(), &body));
        }

        let prosa_response: ProsaSttResponse = serde_json::from_str(&body)
            .map_err(|e| STTError::ProviderError(format!("Failed to parse response: {}", e)))?;

        // Store job ID
        if !prosa_response.job_id.is_empty() {
            let mut job_id = self.current_job_id.write().await;
            *job_id = Some(prosa_response.job_id.clone());
        }

        if prosa_response.has_error() {
            return Err(STTError::ProviderError(
                prosa_response.status_message().to_string(),
            ));
        }

        // If wait=false, we need to poll for results
        if !self.config.wait && prosa_response.is_in_progress() {
            return self.poll_job_result(&prosa_response.job_id).await;
        }

        prosa_response
            .transcription()
            .ok_or_else(|| STTError::ProviderError("No transcription in response".to_string()))
    }

    /// Poll for job result (async mode).
    async fn poll_job_result(&self, job_id: &str) -> Result<String, STTError> {
        let base = crate::core::tts::standard::override_rest_endpoint(
            PROSA_STT_BASE_URL,
            self.config.endpoint_override.as_deref(),
        );
        let url = format!("{}/{}", base.trim_end_matches('/'), job_id);
        let max_attempts = 60; // Poll for up to 60 seconds
        let poll_interval = std::time::Duration::from_secs(1);
        let http_client = self.http_client.as_ref().ok_or_else(|| {
            STTError::ConfigurationError(
                "Prosa STT default HTTP client is unavailable; construct with ProsaStt::new or new_standard".to_string(),
            )
        })?;

        for attempt in 0..max_attempts {
            debug!("Polling job {} (attempt {})", job_id, attempt + 1);

            let response = http_client
                .get(&url)
                .header("x-api-key", &self.config.api_key)
                .send()
                .await
                .map_err(|e| STTError::ConnectionFailed(format!("Failed to poll job: {}", e)))?;

            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|e| STTError::ProviderError(format!("Failed to read response: {}", e)))?;

            if !status.is_success() {
                return Err(self.parse_error_response(status.as_u16(), &body));
            }

            let prosa_response: ProsaSttResponse = serde_json::from_str(&body)
                .map_err(|e| STTError::ProviderError(format!("Failed to parse response: {}", e)))?;

            if prosa_response.is_complete() {
                return prosa_response.transcription().ok_or_else(|| {
                    STTError::ProviderError("No transcription in response".to_string())
                });
            }

            if prosa_response.has_error() {
                return Err(STTError::ProviderError(
                    prosa_response.status_message().to_string(),
                ));
            }

            tokio::time::sleep(poll_interval).await;
        }

        Err(STTError::ProviderError("Job polling timeout".to_string()))
    }

    /// Parse error response.
    fn parse_error_response(&self, status: u16, body: &str) -> STTError {
        // Try to parse as JSON error
        if let Ok(response) = serde_json::from_str::<ProsaSttResponse>(body)
            && let Some(err) = response.error
        {
            return match err.code.as_str() {
                "auth_invalid_api_key" | "auth_unauthorized" => {
                    STTError::AuthenticationFailed(err.message)
                }
                "forbidden" => STTError::AuthenticationFailed("Access forbidden".to_string()),
                "quota_insufficient" | "quota_empty" => STTError::ProviderError(err.message),
                _ => STTError::ProviderError(err.message),
            };
        }

        // Fallback to status code based error
        match status {
            401 => STTError::AuthenticationFailed("Invalid API key".to_string()),
            403 => STTError::AuthenticationFailed("Access forbidden".to_string()),
            400 => STTError::ProviderError("Bad request".to_string()),
            404 => STTError::ProviderError("Not found".to_string()),
            422 => STTError::ProviderError("Validation error".to_string()),
            429 => STTError::ProviderError("Rate limited".to_string()),
            500..=599 => STTError::ProviderError("Server error".to_string()),
            _ => STTError::ProviderError(format!("HTTP error: {}", status)),
        }
    }

    /// Build the streaming session `config` message sent on the WebSocket as the first frame.
    ///
    /// Wire-builder for the streaming path: every recognition toggle that must reach the Prosa.ai
    /// streaming endpoint is materialized here (mirroring `process_audio_rest`'s REST `config`).
    /// `include_filler` is wired here so the standardized `filler_words` feature reaches the
    /// streaming wire, not just the REST request body (the prior recurring "set on the struct but
    /// never serialized to the wire" gap).
    fn build_stream_config(&self) -> ProsaSttStreamConfig {
        ProsaSttStreamConfig {
            model: self.config.model.as_str().to_string(),
            label: self.config.label.clone(),
            include_partial: Some(self.config.include_partial),
            include_filler: Some(self.config.include_filler),
            audio: Some(ProsaSttAudioConfig {
                format: self.config.audio_format.as_str().to_string(),
                channels: Some(self.config.channels),
                sample_rate: Some(self.config.sample_rate),
            }),
        }
    }

    /// Handle one inbound WebSocket message: route transcripts/errors to the registered callbacks
    /// and tell the run loop whether to keep going. Shared by the supervised transport's `run`.
    async fn handle_ws_message(
        msg: Message,
        result_callback: &Arc<RwLock<Option<STTResultCallback>>>,
        error_callback: &Arc<RwLock<Option<STTErrorCallback>>>,
        job_id: &Arc<RwLock<Option<String>>>,
    ) -> ProsaWsAction {
        match msg {
            Message::Text(text) => {
                if let Ok(ws_msg) = serde_json::from_str::<ProsaSttWsMessage>(&text) {
                    match ws_msg {
                        ProsaSttWsMessage::Created { id } => {
                            debug!("Streaming session created: {}", id);
                            let mut job = job_id.write().await;
                            *job = Some(id);
                        }
                        ProsaSttWsMessage::Partial { transcript } => {
                            debug!("Partial transcript: {}", transcript);
                            let callback = result_callback.read().await;
                            if let Some(ref cb) = *callback {
                                let result = STTResult::new(
                                    transcript.clone(),
                                    false, // is_final
                                    false, // is_speech_final
                                    0.0,   // confidence (not provided by partial)
                                );
                                cb(result).await;
                            }
                        }
                        ProsaSttWsMessage::Result {
                            transcript,
                            time_start,
                            time_end,
                        } => {
                            debug!(
                                "Final transcript: {} ({:.2}s - {:.2}s)",
                                transcript, time_start, time_end
                            );
                            let callback = result_callback.read().await;
                            if let Some(ref cb) = *callback {
                                let result = STTResult::new(
                                    transcript.clone(),
                                    true, // is_final
                                    true, // is_speech_final (end of segment)
                                    1.0,  // confidence (Prosa doesn't provide, assume high)
                                );
                                cb(result).await;
                            }
                        }
                        ProsaSttWsMessage::Status { status } => {
                            debug!("Status update: {}", status);
                        }
                        ProsaSttWsMessage::Metadata {
                            duration,
                            quota_used,
                        } => {
                            debug!(
                                "Session metadata: duration={:?}, quota_used={:?}",
                                duration, quota_used
                            );
                        }
                        ProsaSttWsMessage::Error { message } => {
                            error!("WebSocket error: {}", message);
                            let callback = error_callback.read().await;
                            if let Some(ref cb) = *callback {
                                cb(STTError::ProviderError(message)).await;
                            }
                        }
                    }
                }
                ProsaWsAction::Continue
            }
            Message::Close(_) => ProsaWsAction::ServerClosed,
            _ => ProsaWsAction::Continue,
        }
    }

    /// Start the supervised WebSocket streaming session.
    ///
    /// The generic [`ReconnectableStream`] supervisor owns the outer reconnect loop: the `connect`
    /// closure dials the Prosa.ai endpoint and hands back a [`ProsaTransport`] whose
    /// `restore_session` re-sends the featured `config` frame and whose `run` is the streaming event
    /// loop. A mid-stream transport drop reconnects instead of ending the session.
    async fn connect_websocket(&self) -> Result<(), STTError> {
        if !self.config.model.supports_streaming() {
            return Err(STTError::ConnectionFailed(
                "Selected model does not support streaming. Use stt-general-online for streaming."
                    .to_string(),
            ));
        }

        let url = self.build_websocket_url();

        debug!(
            "Connecting to Prosa.ai WebSocket: {}",
            PROSA_STT_WS_ENDPOINT
        );

        // Serialize the featured `config` frame once; the transport re-sends it on every restore.
        let config_json = serde_json::to_string(&self.build_stream_config()).map_err(|e| {
            STTError::ConnectionFailed(format!("Failed to serialize config: {}", e))
        })?;

        // Outbound audio channel + shutdown token + connected oneshot, mirroring the templates.
        let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(32);
        let shutdown_token = CancellationToken::new();
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        {
            let mut tx = self.audio_tx.write().await;
            *tx = Some(audio_tx);
        }
        {
            let mut token = self.shutdown_token.write().await;
            *token = Some(shutdown_token.clone());
        }

        // Shared state the supervised transport re-uses across reconnect attempts.
        let audio_rx = Arc::new(Mutex::new(audio_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));
        let result_callback = self.result_callback.clone();
        let error_callback = self.error_callback.clone();
        let current_job_id = self.current_job_id.clone();

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor with
        // the shared process-global handles from CoreState (W-D1/W-D2 fleet adoption). When no
        // handles were injected (a direct unit-test construction), the supervisor uses its own
        // per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("prosa_ai", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => {
                ReconnectableStream::new(ReconnectableStreamConfig::new("prosa_ai", reconnection))
            }
        }
        .with_disconnect_flag(disconnect_flag);

        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let url = url.clone();
                    let config_json = config_json.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let shutdown_token = shutdown_token.clone();
                    let connected_tx = Arc::clone(&connected_tx);
                    let result_callback = result_callback.clone();
                    let error_callback = error_callback.clone();
                    let current_job_id = current_job_id.clone();
                    async move {
                        let (ws_stream, _) = with_timeout(WS_CONNECT_TIMEOUT, connect_async(&url))
                            .await
                            .map_err(|_| {
                                StreamError::new(format!(
                                    "connect to Prosa.ai timed out after {}s",
                                    WS_CONNECT_TIMEOUT.as_secs()
                                ))
                            })?
                            .map_err(|e| {
                                StreamError::new(format!("WebSocket connection failed: {e}"))
                            })?;
                        info!("Connected to Prosa.ai STT WebSocket");
                        let (ws_sink, ws_stream) = ws_stream.split();
                        Ok(ProsaTransport {
                            ws_sink,
                            ws_stream,
                            audio_rx,
                            shutdown_token,
                            result_callback,
                            error_callback,
                            current_job_id,
                            connected_tx,
                            config_json,
                        })
                    }
                })
                .await;
            info!("Prosa.ai STT WebSocket connection closed (supervisor exit: {exit:?})");
        });

        {
            let mut handle = self.connection_handle.write().await;
            *handle = Some(connection_handle);
        }

        // Wait for the featured session to be established (config frame sent), with a timeout.
        match timeout(Duration::from_secs(10), connected_rx).await {
            Ok(Ok(())) => {
                info!("Connected to Prosa.ai STT WebSocket");
                Ok(())
            }
            Ok(Err(_)) => Err(STTError::ConnectionFailed(
                "Connection channel closed before session started".to_string(),
            )),
            Err(_) => Err(STTError::ConnectionFailed(
                "Connection timeout waiting for Prosa.ai streaming session".to_string(),
            )),
        }
    }

    /// Send audio chunk via the supervised streaming transport (queued on the outbound channel).
    async fn send_audio_ws(&self, audio: &[u8]) -> Result<(), STTError> {
        let tx = self.audio_tx.read().await;
        if let Some(ref sender) = *tx {
            sender
                .send(Bytes::copy_from_slice(audio))
                .await
                .map_err(|e| STTError::ProviderError(format!("Failed to send audio: {}", e)))?;
        } else {
            return Err(STTError::ConnectionFailed(
                "WebSocket not connected".to_string(),
            ));
        }
        Ok(())
    }

    /// Signal end of audio stream (an empty binary frame), queued on the outbound channel.
    async fn end_audio_stream(&self) -> Result<(), STTError> {
        let tx = self.audio_tx.read().await;
        if let Some(ref sender) = *tx {
            sender.send(Bytes::new()).await.map_err(|e| {
                STTError::ProviderError(format!("Failed to send end signal: {}", e))
            })?;
        }
        Ok(())
    }

    /// Flush the audio buffer and process the accumulated audio.
    /// For batch mode, this sends the buffered audio to the REST API.
    /// For streaming mode, this signals the end of the audio stream.
    pub async fn flush(&mut self) -> Result<String, STTError> {
        let audio_data = {
            let mut buffer = self.audio_buffer.write().await;

            std::mem::take(&mut *buffer)
        };

        if audio_data.is_empty() {
            return Ok(String::new());
        }

        if audio_data.len() < MIN_AUDIO_BUFFER_SIZE {
            debug!(
                "Audio buffer too small ({} bytes), skipping transcription",
                audio_data.len()
            );
            return Ok(String::new());
        }

        if self.config.model.supports_streaming() {
            // For streaming, signal end of stream
            self.end_audio_stream().await?;
            Ok(String::new()) // Results come via callback
        } else {
            // For batch mode, send to REST API
            let wav_data = self.wrap_in_wav(&audio_data)?;
            self.process_audio_rest(&wav_data).await
        }
    }

    /// Get the current buffer size.
    pub async fn buffer_size(&self) -> usize {
        let buffer = self.audio_buffer.read().await;
        buffer.len()
    }

    /// Wrap raw PCM audio in WAV header.
    fn wrap_in_wav(&self, pcm_data: &[u8]) -> Result<Vec<u8>, STTError> {
        stt_wav::encode_pcm16_wav(pcm_data, self.config.sample_rate, self.config.channels)
            .map_err(|e| STTError::AudioProcessingError(format!("Invalid WAV parameters: {e}")))
    }
}

impl Default for ProsaStt {
    fn default() -> Self {
        Self {
            config: ProsaSttConfig::default(),
            base_config: None,
            http_client: default_prosa_stt_http_client(),
            audio_tx: Arc::new(RwLock::new(None)),
            shutdown_token: Arc::new(RwLock::new(None)),
            connection_handle: Arc::new(RwLock::new(None)),
            audio_buffer: Arc::new(RwLock::new(Vec::new())),
            is_connected: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            current_job_id: Arc::new(RwLock::new(None)),
            resilience: None,
        }
    }
}

#[async_trait]
impl BaseSTT for ProsaStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        ProsaStt::new(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Prosa.ai API key is required".to_string(),
            ));
        }

        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        // For streaming model, connect to WebSocket
        if self.config.model.supports_streaming() {
            self.connect_websocket().await?;
        }

        // Clear buffer
        {
            let mut buffer = self.audio_buffer.write().await;
            buffer.clear();
        }

        self.is_connected.store(true, Ordering::SeqCst);
        info!("Prosa.ai STT connected (model: {})", self.config.model);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Record the intent BEFORE the streaming-guard so the supervisor sees it even if the
        // transport's run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);

        // Send end signal for streaming, then ask the supervisor to stop (intentional close — must
        // NOT reconnect).
        if self.config.model.supports_streaming() {
            let _ = self.end_audio_stream().await;

            if let Some(shutdown_token) = self.shutdown_token.write().await.take() {
                shutdown_token.cancel();
            }

            // Drop the outbound audio sender so the transport's recv() also unblocks.
            *self.audio_tx.write().await = None;

            // Wait for the supervised connection task to wind down, with a timeout.
            if let Some(handle) = self.connection_handle.write().await.take() {
                crate::core::observability::await_task_shutdown(
                    "prosa-ai-stt-connection",
                    handle,
                    Duration::from_secs(5),
                )
                .await;
            }
        }

        // Clear buffer
        {
            let mut buffer = self.audio_buffer.write().await;
            buffer.clear();
        }

        // Clear job ID
        {
            let mut job_id = self.current_job_id.write().await;
            *job_id = None;
        }

        self.is_connected.store(false, Ordering::SeqCst);
        info!("Prosa.ai STT disconnected");
        Ok(())
    }

    async fn send_audio(&mut self, audio: Bytes) -> Result<(), STTError> {
        if audio.is_empty() {
            return Ok(());
        }

        // Auto-connect if not connected
        if !self.is_ready() {
            self.connect().await?;
        }

        if self.config.model.supports_streaming() {
            // For streaming, send audio chunks directly
            self.send_audio_ws(&audio).await?;
        } else {
            // For batch mode, buffer the audio
            let mut buffer = self.audio_buffer.write().await;
            buffer.extend_from_slice(&audio);
        }

        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_connected.load(Ordering::SeqCst)
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        let mut cb = self.result_callback.write().await;
        *cb = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        let mut cb = self.error_callback.write().await;
        *cb = Some(callback);
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Prosa.ai STT (Indonesian NLP)"
    }

    fn get_config(&self) -> Option<&STTConfig> {
        self.base_config.as_ref()
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        let new_config = ProsaSttConfig::from_base(&config)?;
        self.config = new_config;
        self.base_config = Some(config);
        Ok(())
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `connect_websocket` drives the generic
        // ReconnectableStream supervisor with them — every Prosa.ai streaming session trips the
        // same breaker and shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

impl ProsaStt {
    /// The shared circuit breaker this session feeds into the generic supervisor, if the
    /// process-global resilience handles have been injected (W-D1/W-D2). Two `ProsaStt` built from
    /// the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(&self) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }
}

impl Drop for ProsaStt {
    fn drop(&mut self) {
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        if let Ok(mut shutdown_token) = self.shutdown_token.try_write()
            && let Some(token) = shutdown_token.take()
        {
            token.cancel();
        }
        if let Ok(mut audio_tx) = self.audio_tx.try_write() {
            *audio_tx = None;
        }
        if let Ok(mut handle) = self.connection_handle.try_write()
            && let Some(handle) = handle.take()
        {
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
    use std::sync::atomic::AtomicUsize;

    fn make_test_config() -> STTConfig {
        STTConfig {
            provider: "prosa-ai".to_string(),
            api_key: "test_key".to_string(),
            language: "id".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        }
    }

    #[test]
    fn test_new_valid_config() {
        let config = make_test_config();
        let result = ProsaStt::new(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn websocket_url_encodes_api_key_query_value() {
        let mut config = make_test_config();
        config.api_key = "key&session=evil".to_string();
        let stt = ProsaStt::new(config).unwrap();

        let url = stt.build_websocket_url();
        let parsed = url::Url::parse(&url).unwrap();
        let pairs: Vec<_> = parsed.query_pairs().collect();
        assert!(pairs.contains(&("x-api-key".into(), "key&session=evil".into())));
        assert!(
            !pairs.iter().any(|(key, _)| key == "session"),
            "api key must not create sibling query params: {url}"
        );
        assert!(
            !url.contains("x-api-key=key&session=evil"),
            "api key must be encoded as one query value: {url}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn prosa_stt_redirect_policy_rejects_private_hop() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local redirect test server");
        let addr = listener.local_addr().expect("local addr");

        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: http://127.0.0.1:9/metadata\r\n",
                "Content-Length: 0\r\n",
                "\r\n"
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
        });

        let stt = ProsaStt::new(make_test_config()).expect("construct Prosa STT");
        let err = stt
            .http_client
            .as_ref()
            .expect("strict constructor builds an HTTP client")
            .get(format!("http://{addr}/start"))
            .send()
            .await
            .expect_err("private redirect target must be rejected");
        let mut error_chain = err.to_string();
        let mut source = std::error::Error::source(&err);
        while let Some(err) = source {
            error_chain.push_str(": ");
            error_chain.push_str(&err.to_string());
            source = err.source();
        }
        assert!(error_chain.contains("SSRF protection"), "{error_chain}");
    }

    #[tokio::test]
    async fn default_without_http_client_returns_typed_error_without_panic() {
        let mut stt = ProsaStt::default();
        stt.http_client = None;

        let err = stt
            .process_audio_rest(&[0u8; MIN_AUDIO_BUFFER_SIZE])
            .await
            .expect_err("inert default client must fail with a typed error");

        match err {
            STTError::ConfigurationError(msg) => {
                assert!(msg.contains("default HTTP client"), "{msg}");
            }
            other => panic!("expected ConfigurationError, got {other:?}"),
        }
    }

    // WIRE-LEVEL (streaming): the standardized `filler_words` feature must reach the streaming WS
    // session-config FRAME that `connect_websocket` sends, not just the REST request body. This
    // serializes exactly what goes on the wire (`build_stream_config`) and asserts the
    // `include_filler` param is present with the requested value — guarding the recurring
    // "mapped onto the config struct but never serialized to the streaming wire" gap class.
    #[test]
    fn test_filler_words_reaches_streaming_ws_config_frame() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};

        // filler_words = true  =>  include_filler:true on the streaming frame.
        let std_on = StandardSTTConfig {
            base: STTConfig {
                provider: "prosa-ai".into(),
                api_key: "test_key".into(),
                model: "stt-general-online".into(), // streaming model
                ..Default::default()
            },
            features: SttFeatures {
                filler_words: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let stt = ProsaStt::new_standard(&std_on).unwrap();
        assert!(stt.config.model.supports_streaming());
        let frame = serde_json::to_string(&stt.build_stream_config()).unwrap();
        assert!(
            frame.contains("\"include_filler\":true"),
            "streaming WS config frame must carry include_filler=true; got: {frame}"
        );

        // filler_words = false  =>  include_filler:false on the streaming frame.
        let std_off = StandardSTTConfig {
            features: SttFeatures {
                filler_words: Some(false),
                ..Default::default()
            },
            ..std_on.clone()
        };
        let stt_off = ProsaStt::new_standard(&std_off).unwrap();
        let frame_off = serde_json::to_string(&stt_off.build_stream_config()).unwrap();
        assert!(
            frame_off.contains("\"include_filler\":false"),
            "streaming WS config frame must carry include_filler=false; got: {frame_off}"
        );
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = STTConfig {
            provider: "prosa-ai".to_string(),
            api_key: String::new(),
            ..Default::default()
        };

        let result = ProsaStt::new(config);
        assert!(result.is_err());

        match result {
            Err(STTError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("API key"));
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[test]
    fn test_provider_info() {
        let stt = ProsaStt::new(make_test_config()).unwrap();
        let info = stt.get_provider_info();

        assert!(info.contains("Prosa.ai"));
        assert!(info.contains("Indonesian"));
    }

    #[test]
    fn test_default_state() {
        let stt = ProsaStt::default();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_get_config() {
        let config = make_test_config();
        let stt = ProsaStt::new(config.clone()).unwrap();

        let retrieved_config = stt.get_config();
        assert!(retrieved_config.is_some());
        assert_eq!(retrieved_config.unwrap().api_key, config.api_key);
    }

    #[test]
    fn test_get_prosa_config() {
        let stt = ProsaStt::new(make_test_config()).unwrap();
        let config = stt.get_prosa_config();

        assert_eq!(config.api_key, "test_key");
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.channels, 1);
        assert_eq!(config.model, ProsaSttModel::General);
    }

    #[test]
    fn test_set_model() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        assert_eq!(stt.config.model, ProsaSttModel::General);

        stt.set_model(ProsaSttModel::GeneralOnline);
        assert_eq!(stt.config.model, ProsaSttModel::GeneralOnline);
    }

    #[test]
    fn test_set_audio_format() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        assert_eq!(stt.config.audio_format, ProsaAudioFormat::Wav);

        stt.set_audio_format(ProsaAudioFormat::Mp3);
        assert_eq!(stt.config.audio_format, ProsaAudioFormat::Mp3);
    }

    #[test]
    fn test_set_include_partial() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        assert!(stt.config.include_partial);

        stt.set_include_partial(false);
        assert!(!stt.config.include_partial);
    }

    #[test]
    fn test_set_speaker_count() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        assert_eq!(stt.config.speaker_count, 0);

        stt.set_speaker_count(2);
        assert_eq!(stt.config.speaker_count, 2);
    }

    #[tokio::test]
    async fn test_connect_success() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        // REST mode doesn't need actual connection
        let result = stt.connect().await;

        assert!(result.is_ok());
        assert!(stt.is_ready());
    }

    #[tokio::test]
    async fn test_connect_empty_api_key() {
        let mut stt = ProsaStt::default();
        let result = stt.connect().await;

        assert!(result.is_err());
        match result {
            Err(STTError::AuthenticationFailed(_)) => {}
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_disconnect() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        let result = stt.disconnect().await;
        assert!(result.is_ok());
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the supervisor-shared intentional-disconnect flag",
        );
    }

    #[tokio::test]
    async fn test_send_audio_empty() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        let result = stt.send_audio(Bytes::new()).await;
        assert!(result.is_ok());
        assert_eq!(stt.buffer_size().await, 0);
    }

    #[tokio::test]
    async fn test_send_audio_buffers() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        let audio_data = Bytes::from(vec![0u8; 1000]);
        let result = stt.send_audio(audio_data).await;

        assert!(result.is_ok());
        assert_eq!(stt.buffer_size().await, 1000);
    }

    #[tokio::test]
    async fn test_send_audio_multiple() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
        stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
        stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();

        assert_eq!(stt.buffer_size().await, 1500);
    }

    #[tokio::test]
    async fn test_flush_empty_buffer() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        let result = stt.flush().await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_disconnect_clears_buffer() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        stt.send_audio(Bytes::from(vec![0u8; 1000])).await.unwrap();
        assert!(stt.buffer_size().await > 0);

        stt.disconnect().await.unwrap();
        assert_eq!(stt.buffer_size().await, 0);
    }

    #[tokio::test]
    async fn test_on_result_callback() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let callback: STTResultCallback = Arc::new(move |_result: STTResult| {
            callback_count_clone.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        });

        let result = stt.on_result(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_on_error_callback() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let callback: STTErrorCallback = Arc::new(move |_error: STTError| {
            callback_count_clone.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        });

        let result = stt.on_error(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_update_config() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();

        let new_config = STTConfig {
            provider: "prosa-ai".to_string(),
            api_key: "new_key".to_string(),
            sample_rate: 8000,
            channels: 2,
            ..Default::default()
        };

        let result = stt.update_config(new_config).await;
        assert!(result.is_ok());

        assert_eq!(stt.get_prosa_config().sample_rate, 8000);
        assert_eq!(stt.get_prosa_config().channels, 2);
        assert_eq!(stt.get_prosa_config().api_key, "new_key");
    }

    #[tokio::test]
    async fn test_auto_connect_on_send_audio() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        assert!(!stt.is_ready());

        // Should auto-connect
        let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_ok());
        assert!(stt.is_ready());
    }

    #[tokio::test]
    async fn test_buffer_size() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        assert_eq!(stt.buffer_size().await, 0);

        stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
        assert_eq!(stt.buffer_size().await, 500);

        stt.send_audio(Bytes::from(vec![0u8; 300])).await.unwrap();
        assert_eq!(stt.buffer_size().await, 800);
    }

    #[tokio::test]
    async fn test_connect_clears_previous_buffer() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        // Add some audio
        stt.send_audio(Bytes::from(vec![0u8; 1000])).await.unwrap();
        assert_eq!(stt.buffer_size().await, 1000);

        // Disconnect
        stt.disconnect().await.unwrap();
        assert_eq!(stt.buffer_size().await, 0);

        // Reconnect - buffer should be clear
        stt.connect().await.unwrap();
        assert_eq!(stt.buffer_size().await, 0);
    }

    #[test]
    fn test_wrap_in_wav() {
        let stt = ProsaStt::new(make_test_config()).unwrap();
        let pcm_data = vec![0u8; 1000];

        let wav_data = stt.wrap_in_wav(&pcm_data).expect("valid WAV header");

        // WAV header is 44 bytes
        assert_eq!(wav_data.len(), 44 + 1000);

        // Check RIFF header
        assert_eq!(&wav_data[0..4], b"RIFF");
        assert_eq!(&wav_data[8..12], b"WAVE");
        assert_eq!(&wav_data[12..16], b"fmt ");
        assert_eq!(&wav_data[36..40], b"data");
    }
}
