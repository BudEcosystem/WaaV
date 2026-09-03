//! Tinkoff VoiceKit STT Provider Implementation
//!
//! Implements the BaseSTT trait for Tinkoff's gRPC-based Speech-to-Text service.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::{Mutex, Notify, RwLock, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::config::TinkoffSttConfig;
use super::grpc::{TinkoffGrpcClient, classify_grpc_outcome, create_tinkoff_channel};
use super::messages::{RecognitionConfig, StreamingRecognitionConfig, StreamingRecognizeResponse};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};

/// Per-message idle timeout for the gRPC response stream — resets after each successful message.
/// Catches stuck/dead connections while allowing active streams to continue.
const GRPC_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// A [`WsTransport`] (the trait is transport-agnostic despite the `Ws` name) that adapts Tinkoff
/// VoiceKit's gRPC **bidirectional** `StreamingRecognize` to the generic [`ReconnectableStream`]
/// supervisor (W-D1 fleet adoption). One is built per (re)connect by the supervisor's `connect`
/// closure.
///
/// Tinkoff is gRPC, not WebSocket: the featured session lives in the **first request** of the bidi
/// stream (the `StreamingRecognitionConfig` carrying encoding/language/VAD/diarization/profanity/
/// speech-contexts/interim-results). So [`run`](WsTransport::run) opens a fresh `StreamingRecognize`
/// call (via [`TinkoffGrpcClient::open_stream`]) whose request stream yields that featured config
/// first — the featured-session restore is intrinsic to opening a new stream — then forwards audio
/// and drains responses until a [`ReconnectOutcome`]. A transport drop
/// (`Unavailable`/idle timeout/stream end) becomes a reconnect; a clean shutdown or a fatal gRPC
/// error (auth/invalid-arg) does not. Hence [`restore_session`](WsTransport::restore_session) only
/// unblocks the waiting connect().
struct TinkoffTransport {
    /// The gRPC client over the (per-connect) channel; `open_stream` opens a fresh bidi call.
    grpc_client: TinkoffGrpcClient,
    /// The featured first request (recognition config) — re-yielded as the head of every fresh bidi
    /// stream so a reconnect restores the *featured* session, not a bare one.
    streaming_config: StreamingRecognitionConfig,
    /// Durable inbound audio receiver, shared across reconnect attempts (locked for the duration of
    /// `run`). Each attempt forwards from it into a fresh per-stream channel, so queued audio
    /// survives a reconnect instead of being lost with the old gRPC stream.
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown signal (cloneable across reconnect attempts; intentional close must not reconnect).
    shutdown_token: CancellationToken,
    /// Decoded responses are forwarded here to the callback task.
    result_tx: mpsc::Sender<Result<StreamingRecognizeResponse, STTError>>,
    /// Fires once on the first successful stream open, unblocking `start_connection`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

#[async_trait::async_trait]
impl WsTransport for TinkoffTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // Tinkoff's featured session (recognition config) is the FIRST request of the bidi stream,
        // re-sent at the head of every `run()` — so there is nothing to do here beyond unblocking
        // the waiting connect() exactly once.
        if let Some(tx) = self.connected_tx.lock().await.take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        use futures::StreamExt;

        let mut audio_rx = self.audio_rx.lock().await;
        let shutdown_token = self.shutdown_token.clone();

        if shutdown_token.is_cancelled() {
            info!("Shutdown signal already received for Tinkoff STT");
            return ReconnectOutcome::Completed;
        }

        // Per-attempt inner channel handed to the fresh gRPC stream. We forward the durable audio
        // receiver into it inside the select! below, so a reconnect re-uses the same durable
        // receiver (and any audio that arrives during the gap). `inner_tx` is held in an Option so
        // we can DROP it once the outer audio channel closes — that completes the gRPC request
        // stream while we keep draining responses.
        let (inner_tx, inner_rx) = mpsc::channel::<Bytes>(100);
        let mut inner_tx = Some(inner_tx);

        let mut response_stream = match self
            .grpc_client
            .open_stream(inner_rx, self.streaming_config.clone())
            .await
        {
            Ok(s) => s,
            Err(status) => {
                let stt_error = super::grpc::grpc_status_to_stt_error(status.clone());
                error!(
                    "Failed to start Tinkoff streaming recognition: {}",
                    stt_error
                );
                let _ = self.result_tx.try_send(Err(stt_error));
                return classify_grpc_outcome(status);
            }
        };

        loop {
            tokio::select! {
                biased;

                // Intentional shutdown — must NOT reconnect.
                _ = shutdown_token.cancelled() => {
                    info!("Shutdown signal received for Tinkoff STT");
                    return ReconnectOutcome::Completed;
                }

                // Forward durable audio into the per-stream channel. Disabled once the outer audio
                // channel has closed (so a closed channel can't busy-spin returning None).
                audio_opt = audio_rx.recv(), if inner_tx.is_some() => {
                    match audio_opt {
                        Some(audio) => {
                            if let Some(tx) = inner_tx.as_ref()
                                && tx.send(audio).await.is_err()
                            {
                                // The gRPC request stream closed its end; treat as a drop so the
                                // session reconnects rather than silently stalling.
                                return ReconnectOutcome::Reconnectable(StreamError::new("request stream closed"));
                            }
                        }
                        None => {
                            // Outer audio channel closed (disconnect dropped the sender) — end of
                            // input. Drop inner_tx so the gRPC request stream completes; keep
                            // draining responses until the server closes.
                            debug!("Tinkoff outer audio channel closed");
                            inner_tx = None;
                        }
                    }
                }

                message = tokio::time::timeout(GRPC_MESSAGE_TIMEOUT, response_stream.next()) => {
                    match message {
                        Ok(Some(Ok(data))) => {
                            match StreamingRecognizeResponse::decode(&data) {
                                Ok(response) => {
                                    if !response.results.is_empty()
                                        && self.result_tx.send(Ok(response)).await.is_err()
                                    {
                                        // Receiver dropped (session torn down) — stop cleanly.
                                        return ReconnectOutcome::Completed;
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "Failed to decode Tinkoff response");
                                }
                            }
                        }
                        Ok(Some(Err(status))) => {
                            let stt_error = super::grpc::grpc_status_to_stt_error(status.clone());
                            error!("Streaming error from Tinkoff STT: {}", stt_error);
                            let _ = self.result_tx.try_send(Err(stt_error));
                            return classify_grpc_outcome(status);
                        }
                        Ok(None) => {
                            info!("Tinkoff STT stream ended");
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            let stt_error = STTError::NetworkError(
                                "gRPC idle timeout - no message received for 60 seconds".into(),
                            );
                            error!("Tinkoff STT gRPC idle timeout: {}", stt_error);
                            let _ = self.result_tx.try_send(Err(stt_error));
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }
            }
        }
    }
}

type AsyncSTTCallback = Box<
    dyn Fn(STTResult) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

type AsyncErrorCallback = Box<
    dyn Fn(STTError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

#[derive(Debug, Clone)]
pub(super) enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    #[allow(dead_code)]
    Error(String),
}

/// Audio channel buffer size
const AUDIO_CHANNEL_BUFFER_SIZE: usize = 32;

/// Tinkoff VoiceKit STT provider
pub struct TinkoffStt {
    pub(super) config: Option<TinkoffSttConfig>,
    pub(super) state: ConnectionState,
    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before cancelling the shutdown token, so a client close racing a
    /// server-side close can never trigger a spurious reconnect.
    pub(super) intentional_disconnect: Arc<AtomicBool>,
    pub(super) state_notify: Arc<Notify>,
    pub(super) audio_sender: Option<mpsc::Sender<Bytes>>,
    pub(super) shutdown_token: Option<CancellationToken>,
    pub(super) connection_handle: Option<tokio::task::JoinHandle<()>>,
    pub(super) result_forward_handle: Option<tokio::task::JoinHandle<()>>,
    pub(super) error_forward_handle: Option<tokio::task::JoinHandle<()>>,
    pub(super) result_callback: Arc<RwLock<Option<AsyncSTTCallback>>>,
    pub(super) error_callback: Arc<RwLock<Option<AsyncErrorCallback>>>,
    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven by
    /// the generic [`ReconnectableStream`](crate::core::websocket::ReconnectableStream) supervisor.
    /// `None` before `set_resilience` (a direct unit-test construction) → the supervisor uses its
    /// own per-session governor/breaker default.
    pub(super) resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl Default for TinkoffStt {
    fn default() -> Self {
        Self {
            config: None,
            state: ConnectionState::Disconnected,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            audio_sender: None,
            shutdown_token: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            resilience: None,
        }
    }
}

impl TinkoffStt {
    /// W1 keystone — construct directly from the standardized config so the advanced features
    /// Tinkoff can express (interim/partial results) are honored END-TO-END. The flat
    /// `BaseSTT::new` path resets those to provider defaults; this is the reachable
    /// standardized path. Mirrors `DeepgramSTT::new_standard`.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }
        let tinkoff_config =
            TinkoffSttConfig::from_standard(std).map_err(STTError::ConfigurationError)?;
        Ok(Self {
            config: Some(tinkoff_config),
            state: ConnectionState::Disconnected,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            audio_sender: None,
            shutdown_token: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            resilience: None,
        })
    }

    /// Create streaming recognition config from provider config
    fn create_streaming_config(config: &TinkoffSttConfig) -> StreamingRecognitionConfig {
        // Map provider-side speech contexts to wire messages (phrase boosting, field 6).
        let speech_contexts = config
            .speech_contexts
            .iter()
            .map(|ctx| super::messages::SpeechContext {
                phrases: ctx
                    .phrases
                    .iter()
                    .map(|p| super::messages::SpeechContextPhrase {
                        text: p.text.clone(),
                        score: p.score,
                    })
                    .collect(),
                speech_context_dictionary_id: ctx.speech_context_dictionary_id.clone(),
            })
            .collect();

        StreamingRecognitionConfig {
            config: RecognitionConfig {
                encoding: config.encoding,
                sample_rate_hertz: config.base.sample_rate,
                language_code: config.base.language.clone(),
                max_alternatives: config.max_alternatives,
                profanity_filter: config.profanity_filter,
                speech_contexts,
                enable_automatic_punctuation: config.enable_punctuation,
                num_channels: config.base.channels as u32,
                // oneof vad: `do_not_perform_vad` (field 13) takes precedence over `vad_config`
                // (field 14); never emit both.
                do_not_perform_vad: config.do_not_perform_vad,
                vad: if config.do_not_perform_vad {
                    None
                } else {
                    config.vad_config.clone()
                },
                enable_denormalization: config.enable_denormalization,
                enable_gender_identification: config.enable_gender_identification,
            },
            interim_results: config.interim_results,
            single_utterance: config.single_utterance,
            interim_results_interval: config.interim_results_interval,
        }
    }

    /// Start the supervised gRPC streaming session.
    ///
    /// The generic [`ReconnectableStream`] supervisor owns the outer reconnect loop: the `connect`
    /// closure builds a [`TinkoffGrpcClient`] over the channel and hands back a [`TinkoffTransport`]
    /// whose `run()` opens a fresh `StreamingRecognize` bidi stream (whose first request restores
    /// the featured session) and drains responses. A mid-stream transport drop reconnects instead
    /// of ending the session.
    async fn start_connection(&mut self, config: TinkoffSttConfig) -> Result<(), STTError> {
        // Validate configuration
        config.validate().map_err(STTError::ConfigurationError)?;

        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        // Establish the gRPC channel up front so connect/auth errors surface synchronously.
        let channel = create_tinkoff_channel(&config).await?;

        // Channels
        let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER_SIZE);
        let shutdown_token = CancellationToken::new();
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<StreamingRecognizeResponse, STTError>>(100);
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        self.audio_sender = Some(audio_tx);
        self.shutdown_token = Some(shutdown_token.clone());

        let streaming_config = Self::create_streaming_config(&config);

        // Shared state the supervised transport re-uses across reconnect attempts.
        let audio_rx = Arc::new(Mutex::new(audio_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor with
        // the shared process-global handles from CoreState (W-D1/W-D2 fleet adoption). When no
        // handles were injected (a direct unit-test construction), the supervisor uses its own
        // per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("tinkoff", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => {
                ReconnectableStream::new(ReconnectableStreamConfig::new("tinkoff", reconnection))
            }
        }
        .with_disconnect_flag(disconnect_flag);

        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let channel = channel.clone();
                    let config = config.clone();
                    let streaming_config = streaming_config.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let shutdown_token = shutdown_token.clone();
                    let connected_tx = Arc::clone(&connected_tx);
                    let result_tx = result_tx.clone();
                    async move {
                        // The channel was already established by `start_connection`; building the
                        // client is infallible.
                        let grpc_client = TinkoffGrpcClient::new(channel, config);
                        Ok(TinkoffTransport {
                            grpc_client,
                            streaming_config,
                            audio_rx,
                            shutdown_token,
                            result_tx,
                            connected_tx,
                        })
                    }
                })
                .await;
            info!("Tinkoff VoiceKit STT connection closed (supervisor exit: {exit:?})");
        });

        self.connection_handle = Some(connection_handle);

        // Result/error forwarding task: convert decoded responses into STTResult and invoke the
        // registered callbacks (preserving the original per-alternative fan-out).
        let callback_ref = self.result_callback.clone();
        let error_callback_ref = self.error_callback.clone();
        let result_forward_handle = tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                match result {
                    Ok(response) => {
                        for result in response.results {
                            if let Some(alt) = result.alternatives.first() {
                                let stt_result = STTResult::new(
                                    alt.transcript.clone(),
                                    result.is_final,
                                    result.is_final,
                                    alt.confidence,
                                );

                                let callback_opt = {
                                    let guard = callback_ref.read().await;
                                    guard.as_ref().map(|cb| cb(stt_result.clone()))
                                };

                                if let Some(future) = callback_opt {
                                    future.await;
                                } else {
                                    debug!(
                                        "Received STT result but no callback: {} (confidence: {})",
                                        stt_result.transcript, stt_result.confidence
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Tinkoff streaming error: {}", e);
                        let callback_opt = {
                            let guard = error_callback_ref.read().await;
                            guard.as_ref().map(|cb| cb(e.clone()))
                        };

                        if let Some(future) = callback_opt {
                            future.await;
                        } else {
                            error!("STT streaming error but no error callback: {}", e);
                        }
                    }
                }
            }
        });

        self.result_forward_handle = Some(result_forward_handle);
        self.state = ConnectionState::Connecting;

        // Wait for connection to be established
        match tokio::time::timeout(Duration::from_secs(30), connected_rx).await {
            Ok(Ok(())) => {
                self.state = ConnectionState::Connected;
                self.state_notify.notify_waiters();
                info!("Successfully connected to Tinkoff VoiceKit STT");
                Ok(())
            }
            Ok(Err(_)) => {
                let error_msg = "Connection channel closed unexpectedly".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
            Err(_) => {
                let error_msg = "Connection timeout (30s)".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for TinkoffStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        let tinkoff_config =
            TinkoffSttConfig::from_base(config).map_err(STTError::ConfigurationError)?;

        Ok(Self {
            config: Some(tinkoff_config),
            state: ConnectionState::Disconnected,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            audio_sender: None,
            shutdown_token: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            resilience: None,
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        let config = self.config.clone().ok_or_else(|| {
            STTError::ConfigurationError("No configuration available".to_string())
        })?;

        self.start_connection(config).await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Record the intent BEFORE anything else so the supervisor sees it even if the transport's
        // run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);

        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }

        // Drop the outbound audio sender so the transport's recv() also unblocks.
        self.audio_sender = None;

        if let Some(handle) = self.connection_handle.take() {
            crate::core::observability::await_task_shutdown(
                "tinkoff-stt-connection",
                handle,
                Duration::from_secs(5),
            )
            .await;
        }

        if let Some(handle) = self.result_forward_handle.take() {
            crate::core::observability::abort_and_await_task(
                "tinkoff-stt-result-forwarder",
                handle,
            )
            .await;
        }

        if let Some(handle) = self.error_forward_handle.take() {
            crate::core::observability::abort_and_await_task("tinkoff-stt-error-forwarder", handle)
                .await;
        }

        self.audio_sender = None;
        *self.result_callback.write().await = None;
        *self.error_callback.write().await = None;

        self.state = ConnectionState::Disconnected;
        self.state_notify.notify_waiters();

        info!("Disconnected from Tinkoff VoiceKit STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        matches!(self.state, ConnectionState::Connected) && self.audio_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to Tinkoff VoiceKit STT".to_string(),
            ));
        }

        if let Some(audio_sender) = &self.audio_sender {
            let data_len = audio_data.len();

            audio_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio data: {}", e)))?;

            debug!("Sent {} bytes of audio data to Tinkoff STT", data_len);
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.result_callback.write().await = Some(Box::new(move |result| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(result).await;
            })
        }));
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.error_callback.write().await = Some(Box::new(move |error| {
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
        if self.is_ready() {
            self.disconnect().await?;
        }

        let tinkoff_config =
            TinkoffSttConfig::from_base(config).map_err(STTError::ConfigurationError)?;

        self.config = Some(tinkoff_config);
        self.connect().await?;
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Tinkoff VoiceKit Speech-to-Text"
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `start_connection` drives the generic
        // ReconnectableStream supervisor with them — every Tinkoff session trips the same breaker
        // and shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

impl TinkoffStt {
    /// The shared circuit breaker this session feeds into the generic supervisor, if the
    /// process-global resilience handles have been injected (W-D1/W-D2). Two `TinkoffStt` built from
    /// the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(&self) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }
}

impl Drop for TinkoffStt {
    fn drop(&mut self) {
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> STTConfig {
        STTConfig {
            provider: "tinkoff".to_string(),
            api_key: "test-api-key".to_string(),
            language: "ru-RU".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "default".to_string(),
        }
    }

    // W1 keystone: an advanced feature Tinkoff supports (interim/partial results) must survive
    // through `new_standard` into the provider-specific config, instead of being reset to the
    // provider default by the flat path. RED until `new_standard` maps it.
    #[test]
    fn test_tinkoff_new_standard_unlocks_advanced_features() {
        use crate::core::stt::standard::{StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "tinkoff".into(),
                api_key: "test-api-key".into(),
                language: "ru-RU".into(),
                ..Default::default()
            },
            features: SttFeatures {
                interim_results: Some(false),
                ..Default::default()
            },
            ..StandardSTTConfig::from_base(STTConfig::default())
        };
        let stt = TinkoffStt::new_standard(&std).unwrap();
        let cfg = stt.config.as_ref().expect("config present");
        // interim_results survived from the standardized feature (provider default is true).
        assert!(!cfg.interim_results);
        assert_eq!(cfg.api_key, "test-api-key");
    }

    #[test]
    fn test_tinkoff_new_standard_requires_api_key() {
        use crate::core::stt::standard::StandardSTTConfig;
        let std = StandardSTTConfig::from_base(STTConfig {
            provider: "tinkoff".into(),
            api_key: String::new(),
            ..Default::default()
        });
        assert!(TinkoffStt::new_standard(&std).is_err());
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
                    provider: "tinkoff".into(),
                    api_key: "test-key|test-secret".into(),
                    language: "ru-RU".into(),
                    sample_rate: 16000,
                    encoding: "linear16".into(),
                    model: "default".into(),
                    ..Default::default()
                },
                features: SttFeatures::default(),
                extras: ProviderExtras::default(),
                translation: None,
            }
            .with_endpoint_override(endpoint)
        };

        assert!(TinkoffStt::new_standard(&mk("https://tinkoff-proxy.example.com")).is_ok());
        assert!(TinkoffStt::new_standard(&mk("http://127.0.0.1:9000")).is_err());
        assert!(TinkoffStt::new_standard(&mk("file:///tmp/socket")).is_err());
        assert!(TinkoffStt::new_standard(&mk("grpc://tinkoff-proxy.example.com")).is_err());

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
    fn test_tinkoff_stt_new() {
        let config = create_test_config();
        let stt = TinkoffStt::new(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
        assert!(stt.config.is_some());
    }

    #[test]
    fn test_tinkoff_stt_default() {
        let stt = TinkoffStt::default();
        assert!(!stt.is_ready());
        assert!(stt.config.is_none());
    }

    #[test]
    fn test_tinkoff_stt_get_provider_info() {
        let config = create_test_config();
        let stt = TinkoffStt::new(config).unwrap();
        assert_eq!(stt.get_provider_info(), "Tinkoff VoiceKit Speech-to-Text");
    }

    #[test]
    fn test_tinkoff_stt_get_config() {
        let config = create_test_config();
        let stt = TinkoffStt::new(config).unwrap();

        let retrieved_config = stt.get_config();
        assert!(retrieved_config.is_some());

        let config = retrieved_config.unwrap();
        assert_eq!(config.language, "ru-RU");
        assert_eq!(config.sample_rate, 16000);
    }

    #[tokio::test]
    async fn test_tinkoff_stt_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = TinkoffStt::new(config).unwrap();

        let result = stt.send_audio(Bytes::from_static(&[0x01, 0x02])).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), STTError::ConnectionFailed(_)));
    }

    #[tokio::test]
    async fn test_tinkoff_stt_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = TinkoffStt::new(config).unwrap();

        // Disconnecting when not connected should succeed
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
        let mut stt = TinkoffStt::new(config).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the supervisor-shared intentional-disconnect flag",
        );
    }

    #[test]
    fn test_create_streaming_config() {
        let mut tinkoff_config = TinkoffSttConfig::default();
        tinkoff_config.base.sample_rate = 16000;
        tinkoff_config.base.language = "ru-RU".to_string();
        tinkoff_config.interim_results = true;
        tinkoff_config.single_utterance = false;

        let streaming_config = TinkoffStt::create_streaming_config(&tinkoff_config);

        assert_eq!(streaming_config.config.sample_rate_hertz, 16000);
        assert_eq!(streaming_config.config.language_code, "ru-RU");
        assert!(streaming_config.interim_results);
        assert!(!streaming_config.single_utterance);
    }

    // WIRE-LEVEL end-to-end: the standardized features must reach the ENCODED protobuf bytes of the
    // first streaming request — `StandardSTTConfig` → `from_standard` → `create_streaming_config`
    // → `StreamingRecognizeRequest::config(..).encode()` (the exact bytes the gRPC stream sends).
    // This guards the recurring "set on config struct, never serialized to the wire" bug class.
    #[test]
    fn test_streaming_features_reach_encoded_request_e2e() {
        use super::super::messages::StreamingRecognizeRequest;
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};

        let mut extras = serde_json::Map::new();
        extras.insert(
            "interim_results_config.interval".into(),
            serde_json::json!(0.25),
        );
        extras.insert(
            "enable_gender_identification".into(),
            serde_json::json!(true),
        );
        extras.insert("vad_config.silence_max".into(), serde_json::json!(1.5));
        extras.insert("vad_config.silence_min".into(), serde_json::json!(0.3));

        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "tinkoff".into(),
                api_key: "test-api-key".into(),
                language: "ru-RU".into(),
                sample_rate: 16000,
                ..Default::default()
            },
            features: SttFeatures {
                interim_results: Some(true),
                profanity_filter: Some(true),
                numerals: Some(true),
                keyterms: Some(vec!["Тинькофф".into()]),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
            translation: None,
        };

        let cfg = TinkoffSttConfig::from_standard(&std).unwrap();
        let streaming_config = TinkoffStt::create_streaming_config(&cfg);
        // Encode the actual first-message bytes the AudioChunkStream emits onto the gRPC stream.
        let bytes = StreamingRecognizeRequest::config(streaming_config).encode();

        let contains = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);

        // profanity_filter (field 5) bytes 0x28 0x01 must be present inside the embedded config.
        assert!(
            contains(&[0x28, 0x01]),
            "profanity_filter missing from encoded request"
        );
        // speech_contexts phrase text bytes must be present.
        assert!(
            contains("Тинькофф".as_bytes()),
            "speech context phrase missing from encoded request"
        );
        // enable_denormalization (field 16) bytes 0x80 0x01 0x01.
        assert!(
            contains(&[0x80, 0x01, 0x01]),
            "enable_denormalization missing from encoded request"
        );
        // enable_gender_identification (field 18) bytes 0x90 0x01 0x01.
        assert!(
            contains(&[0x90, 0x01, 0x01]),
            "enable_gender_identification missing from encoded request"
        );
        // VAD silence_max (1.5) and silence_min (0.3) float bytes.
        assert!(
            contains(&1.5f32.to_le_bytes()),
            "vad silence_max missing from encoded request"
        );
        assert!(
            contains(&0.3f32.to_le_bytes()),
            "vad silence_min missing from encoded request"
        );
        // interim_results_config.interval (0.25) float bytes.
        assert!(
            contains(&0.25f32.to_le_bytes()),
            "interim interval missing from encoded request"
        );
    }
}
