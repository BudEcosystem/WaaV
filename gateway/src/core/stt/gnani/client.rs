//! Gnani.ai STT Client Implementation
//!
//! Implements the BaseSTT trait for Gnani's Speech-to-Text service using
//! gRPC bidirectional streaming for real-time transcription.
//!
//! ## Architecture
//!
//! The client uses Gnani's `Listener.DoSpeechToText` gRPC service which accepts
//! a stream of `SpeechChunk` messages and returns a stream of `TranscriptChunk`
//! responses.
//!
//! ```text
//! Audio Input → SpeechChunk stream → gRPC → TranscriptChunk stream → Callbacks
//! ```

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};

use super::config::GnaniSTTConfig;
use super::grpc::{GnaniGrpcClient, classify_grpc_outcome, create_gnani_channel};
use super::messages::TranscriptChunk;

/// Per-message idle timeout for the gRPC response stream — resets after each successful message.
/// Catches stuck/dead connections while allowing active streams to continue.
const GRPC_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// A [`WsTransport`] (the trait is transport-agnostic despite the `Ws` name) that adapts Gnani's
/// gRPC **bidirectional** `DoSpeechToText` streaming to the generic [`ReconnectableStream`]
/// supervisor (W-D1 fleet adoption). One is built per (re)connect by the supervisor's `connect`
/// closure.
///
/// Gnani is gRPC, not WebSocket: the featured session (auth token/access_key + lang/encoding
/// metadata, and the auth-bearing first `SpeechChunk`) lives in the **head of the bidi stream**.
/// So [`run`](WsTransport::run) opens a fresh `DoSpeechToText` call (via
/// [`GnaniGrpcClient::open_stream`]) whose request stream re-yields that featured head — the
/// featured-session restore is intrinsic to opening a new stream — then drains responses until a
/// [`ReconnectOutcome`]. A transport drop (`Unavailable`/idle timeout/stream end) becomes a
/// reconnect; a clean shutdown or a fatal gRPC error (auth/invalid-arg) does not. Hence
/// [`restore_session`](WsTransport::restore_session) only unblocks the waiting connect().
struct GnaniTransport {
    /// The gRPC client over the (per-connect) channel; `open_stream` opens a fresh bidi call.
    grpc_client: GnaniGrpcClient,
    /// Durable inbound audio receiver, shared across reconnect attempts (locked for the duration of
    /// `run`). Each attempt forwards from it into a fresh per-stream channel, so queued audio
    /// survives a reconnect instead of being lost with the old gRPC stream.
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown signal (fires once; an intentional close must not reconnect).
    shutdown_rx: Arc<Mutex<oneshot::Receiver<()>>>,
    /// Decoded transcript chunks are forwarded here to the dedupe/callback task.
    result_tx: mpsc::Sender<Result<TranscriptChunk, STTError>>,
    /// Fires once on the first successful stream open, unblocking `start_streaming_session`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

#[async_trait]
impl WsTransport for GnaniTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // Gnani's featured session (auth metadata + auth-bearing first chunk) is the head of the
        // bidi stream, re-opened at the start of every `run()` — so there is nothing to do here
        // beyond unblocking the waiting connect() exactly once.
        if let Some(tx) = self.connected_tx.lock().await.take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        use futures::StreamExt;

        let mut audio_rx = self.audio_rx.lock().await;
        let mut shutdown_rx = self.shutdown_rx.lock().await;

        // Per-attempt inner channel handed to the fresh gRPC stream. We forward the durable audio
        // receiver into it inside the select! below, so a reconnect re-uses the same durable
        // receiver (and any audio that arrives during the gap). `inner_tx` is held in an Option so
        // we can DROP it once the outer audio channel closes — that completes the gRPC request
        // stream while we keep draining responses.
        let (inner_tx, inner_rx) = mpsc::channel::<Bytes>(100);
        let mut inner_tx = Some(inner_tx);

        let mut response_stream = match self.grpc_client.open_stream(inner_rx).await {
            Ok(s) => s,
            Err(status) => {
                let stt_error = super::grpc::grpc_status_to_stt_error(status.clone());
                error!("Failed to start Gnani streaming recognition: {}", stt_error);
                let _ = self.result_tx.try_send(Err(stt_error));
                return classify_grpc_outcome(status);
            }
        };

        loop {
            tokio::select! {
                biased;

                // Intentional shutdown — must NOT reconnect.
                _ = &mut *shutdown_rx => {
                    info!("Shutdown signal received for Gnani STT");
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
                            debug!("Gnani outer audio channel closed");
                            inner_tx = None;
                        }
                    }
                }

                message = tokio::time::timeout(GRPC_MESSAGE_TIMEOUT, response_stream.next()) => {
                    match message {
                        Ok(Some(Ok(data))) => {
                            match TranscriptChunk::decode(&data) {
                                Ok(chunk) => {
                                    if chunk.has_content()
                                        && self.result_tx.send(Ok(chunk)).await.is_err()
                                    {
                                        // Receiver dropped (session torn down) — stop cleanly.
                                        return ReconnectOutcome::Completed;
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "Failed to decode Gnani transcript chunk");
                                }
                            }
                        }
                        Ok(Some(Err(status))) => {
                            let stt_error = super::grpc::grpc_status_to_stt_error(status.clone());
                            error!("Streaming error from Gnani STT: {}", stt_error);
                            let _ = self.result_tx.try_send(Err(stt_error));
                            return classify_grpc_outcome(status);
                        }
                        Ok(None) => {
                            info!("Gnani STT stream ended");
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            let stt_error = STTError::NetworkError(
                                "gRPC idle timeout - no message received for 60 seconds".into(),
                            );
                            error!("Gnani STT gRPC idle timeout: {}", stt_error);
                            let _ = self.result_tx.try_send(Err(stt_error));
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }
            }
        }
    }
}

/// Gnani Speech-to-Text client
///
/// Implements streaming STT using Gnani's gRPC bidirectional streaming API.
/// The client maintains a persistent connection and streams audio chunks
/// to the server while receiving transcription results asynchronously.
pub struct GnaniSTT {
    /// Provider-specific configuration
    config: Option<GnaniSTTConfig>,

    /// gRPC channel
    grpc_channel: Option<Channel>,

    /// Connection state (lock-free for performance)
    is_connected: Arc<AtomicBool>,

    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before firing `shutdown_tx`, so a client close racing a
    /// server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,

    /// Result callback
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,

    /// Error callback
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,

    /// Audio sender for the streaming session
    audio_sender: Option<mpsc::Sender<Bytes>>,

    /// Shutdown signal for the supervised streaming task (intentional close → no reconnect).
    shutdown_tx: Option<oneshot::Sender<()>>,

    /// Supervised streaming connection task handle (owns the ReconnectableStream supervisor).
    connection_handle: Option<tokio::task::JoinHandle<()>>,

    /// Handle to the result processing task
    result_task: Option<tokio::task::JoinHandle<()>>,

    /// Last transcript to detect changes (for interim results)
    last_transcript: Arc<RwLock<String>>,

    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`](crate::core::websocket::ReconnectableStream)
    /// supervisor. `None` before `set_resilience` (a direct unit-test construction) → the
    /// supervisor uses its own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl Default for GnaniSTT {
    fn default() -> Self {
        Self {
            config: None,
            grpc_channel: None,
            is_connected: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            audio_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_task: None,
            last_transcript: Arc::new(RwLock::new(String::new())),
            resilience: None,
        }
    }
}

impl GnaniSTT {
    /// W1 keystone — construct directly from the standardized config so Gnani's only mappable
    /// feature (interim/partial results) and its non-standard credentials (`token`, `access_key`
    /// via `extras`) are honored END-TO-END. The remaining advanced features are capability gaps
    /// Gnani cannot express and stay at default. Mirrors `DeepgramSTT::new_standard`; credential
    /// validation happens at `connect()` time (parity with `BaseSTT::new`), since Gnani
    /// authenticates with mTLS token/access_key rather than an api_key.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let gnani_config =
            GnaniSTTConfig::from_standard(std).map_err(STTError::ConfigurationError)?;
        Ok(Self {
            config: Some(gnani_config),
            ..Default::default()
        })
    }

    /// Create a new Gnani STT instance
    pub fn create(config: STTConfig) -> Result<Self, STTError> {
        // Convert base config to Gnani-specific config
        let gnani_config =
            GnaniSTTConfig::from_base(config).map_err(STTError::ConfigurationError)?;

        Ok(Self {
            config: Some(gnani_config),
            ..Default::default()
        })
    }

    /// Start the supervised gRPC streaming session.
    ///
    /// The generic [`ReconnectableStream`] supervisor owns the outer reconnect loop: the `connect`
    /// closure builds a [`GnaniGrpcClient`] over the channel and hands back a [`GnaniTransport`]
    /// whose `run()` opens a fresh `DoSpeechToText` bidi stream (restoring the featured session) and
    /// drains responses. A mid-stream transport drop reconnects instead of ending the session.
    async fn start_streaming_session(&mut self) -> Result<(), STTError> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| STTError::ConfigurationError("No configuration set".to_string()))?
            .clone();

        let channel = self
            .grpc_channel
            .as_ref()
            .ok_or_else(|| STTError::ConnectionFailed("Not connected".to_string()))?
            .clone();

        // Outbound audio channel + shutdown + decoded-result channel + connected signal.
        let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(100);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (result_tx, mut result_rx) =
            mpsc::channel::<Result<TranscriptChunk, STTError>>(100);
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        self.audio_sender = Some(audio_tx);
        self.shutdown_tx = Some(shutdown_tx);

        // Shared state the supervised transport re-uses across reconnect attempts.
        let audio_rx = Arc::new(Mutex::new(audio_rx));
        let shutdown_rx = Arc::new(Mutex::new(shutdown_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor with
        // the shared process-global handles from CoreState (W-D1/W-D2 fleet adoption). When no
        // handles were injected (a direct unit-test construction), the supervisor uses its own
        // per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        // Share the client-owned intentional-disconnect flag INTO the supervisor (W-D1): the
        // supervisor checks it at its loop top + after run(), so a client `disconnect()` racing a
        // server-side close can never trigger a spurious reconnect. Captured here while `self` is
        // still borrowable, before the supervisor is moved into the spawned task below.
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("gnani", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => {
                ReconnectableStream::new(ReconnectableStreamConfig::new("gnani", reconnection))
            }
        }
        .with_disconnect_flag(disconnect_flag);

        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let channel = channel.clone();
                    let config = config.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let shutdown_rx = Arc::clone(&shutdown_rx);
                    let connected_tx = Arc::clone(&connected_tx);
                    let result_tx = result_tx.clone();
                    async move {
                        // The channel was already established by `connect()`; building the client is
                        // infallible. (A future enhancement could re-dial the channel here to
                        // recover from a fully-dropped TCP connection.)
                        let grpc_client = GnaniGrpcClient::new(channel, config);
                        Ok(GnaniTransport {
                            grpc_client,
                            audio_rx,
                            shutdown_rx,
                            result_tx,
                            connected_tx,
                        })
                    }
                })
                .await;
            info!("Gnani STT connection closed (supervisor exit: {exit:?})");
        });

        self.connection_handle = Some(connection_handle);

        // Wait for the first successful stream open before reporting connected.
        match tokio::time::timeout(Duration::from_secs(30), connected_rx).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                return Err(STTError::ConnectionFailed(
                    "Connection channel closed before stream opened".to_string(),
                ));
            }
            Err(_) => {
                return Err(STTError::ConnectionFailed(
                    "Connection timeout waiting for Gnani stream".to_string(),
                ));
            }
        }

        // Clone refs for the result processing task
        let result_callback = self.result_callback.clone();
        let error_callback = self.error_callback.clone();
        let last_transcript = self.last_transcript.clone();
        let is_connected = self.is_connected.clone();

        // Spawn task to process results
        let handle = tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                match result {
                    Ok(chunk) => {
                        // Create STT result from transcript chunk
                        let stt_result = STTResult::new(
                            chunk.best_transcript().to_string(),
                            chunk.is_final,
                            chunk.is_final,
                            chunk.confidence,
                        );

                        // Only send if transcript changed (avoid duplicate interim results)
                        let should_send = {
                            let last = last_transcript.read().await;
                            *last != stt_result.transcript
                        };

                        if should_send && !stt_result.transcript.is_empty() {
                            {
                                let mut last = last_transcript.write().await;
                                *last = stt_result.transcript.clone();
                            }

                            if let Some(callback) = result_callback.read().await.as_ref() {
                                callback(stt_result).await;
                            }
                        }

                        // Clear last transcript on final result
                        if chunk.is_final {
                            let mut last = last_transcript.write().await;
                            last.clear();
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Gnani STT streaming error");
                        if let Some(callback) = error_callback.read().await.as_ref() {
                            callback(e).await;
                        }
                    }
                }
            }

            debug!("Gnani STT result processing task ended");
            is_connected.store(false, Ordering::Release);
        });

        self.result_task = Some(handle);

        Ok(())
    }
}

#[async_trait]
impl BaseSTT for GnaniSTT {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        GnaniSTT::create(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| STTError::ConfigurationError("No configuration set".to_string()))?;

        // Validate configuration
        config.validate().map_err(STTError::ConfigurationError)?;

        info!(
            language = %config.base.language,
            "Connecting to Gnani STT via gRPC"
        );

        // Create gRPC channel with mTLS
        let channel = create_gnani_channel(config).await?;
        self.grpc_channel = Some(channel);

        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        // Start streaming session
        self.start_streaming_session().await?;

        self.is_connected.store(true, Ordering::Release);
        info!("Connected to Gnani STT");

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        info!("Disconnecting from Gnani STT");

        // Record the intent BEFORE firing shutdown_tx so the supervisor sees it even if the
        // transport's run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);

        // Ask the supervisor to stop (intentional close — must NOT reconnect).
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Drop audio sender to signal end of stream
        self.audio_sender = None;

        // Wait for the supervised connection task to wind down, with a timeout.
        if let Some(handle) = self.connection_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }

        // Wait for result task to complete
        if let Some(task) = self.result_task.take() {
            // Give it a moment to finish processing
            tokio::time::timeout(Duration::from_secs(2), task).await.ok();
        }

        self.grpc_channel = None;
        self.is_connected.store(false, Ordering::Release);

        // Clear state
        {
            let mut last = self.last_transcript.write().await;
            last.clear();
        }

        info!("Disconnected from Gnani STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_connected.load(Ordering::Acquire) && self.audio_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        // Send audio to the streaming channel
        if let Some(ref sender) = self.audio_sender {
            sender.send(audio_data).await.map_err(|_| {
                self.is_connected.store(false, Ordering::Release);
                STTError::AudioProcessingError("Audio channel closed".to_string())
            })?;
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
        self.config.as_ref().map(|c| &c.base)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        let gnani_config =
            GnaniSTTConfig::from_base(config).map_err(STTError::ConfigurationError)?;
        self.config = Some(gnani_config);
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Gnani.ai Vachana STT - Indic Speech-to-Text with 14 language support via gRPC streaming"
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `start_streaming_session` drives the generic
        // ReconnectableStream supervisor with them — every Gnani session trips the same breaker and
        // shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

impl GnaniSTT {
    /// The shared circuit breaker this session feeds into the generic supervisor, if the
    /// process-global resilience handles have been injected (W-D1/W-D2). Two `GnaniSTT` built from
    /// the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(
        &self,
    ) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> STTConfig {
        STTConfig {
            provider: "gnani".to_string(),
            api_key: String::new(),
            language: "hi-IN".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm16".to_string(),
            model: "default".to_string(),
        }
    }

    #[test]
    fn test_gnani_stt_creation() {
        let config = create_test_config();
        let result = GnaniSTT::create(config);
        assert!(result.is_ok());
    }

    // W1 keystone: Gnani's mappable feature (interim/partial results) plus its non-standard
    // credentials (via extras) must survive through `new_standard` into the provider config
    // (previously dropped by the flat factory).
    #[test]
    fn new_standard_propagates_interim_results_and_creds() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "gnani".into(),
                api_key: String::new(),
                language: "hi-IN".into(),
                ..Default::default()
            },
            features: SttFeatures {
                interim_results: Some(false),
                ..Default::default()
            },
            extras: ProviderExtras(
                serde_json::json!({ "access_key": "ak-123" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            translation: None,
        };
        let stt = GnaniSTT::new_standard(&std).expect("new_standard should succeed");
        let cfg = stt.config.expect("config should be set");
        assert!(!cfg.interim_results);
        assert_eq!(cfg.access_key, "ak-123");
    }

    #[test]
    fn test_gnani_stt_not_connected_initially() {
        let config = create_test_config();
        let stt = GnaniSTT::create(config).unwrap();
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_gnani_stt_send_audio_requires_connection() {
        let config = create_test_config();
        let mut stt = GnaniSTT::create(config).unwrap();

        let result = stt.send_audio(Bytes::from_static(b"test")).await;
        assert!(result.is_err());
        match result {
            Err(STTError::ConnectionFailed(msg)) => {
                assert!(msg.contains("Not connected"));
            }
            _ => panic!("Expected ConnectionFailed error"),
        }
    }

    #[test]
    fn test_gnani_stt_provider_info() {
        let config = create_test_config();
        let stt = GnaniSTT::create(config).unwrap();
        assert!(stt.get_provider_info().contains("Gnani"));
        assert!(stt.get_provider_info().contains("gRPC"));
    }

    #[tokio::test]
    async fn test_gnani_stt_callback_registration() {
        let config = create_test_config();
        let mut stt = GnaniSTT::create(config).unwrap();

        let callback = Arc::new(|_result: STTResult| {
            Box::pin(async move {})
                as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        let result = stt.on_result(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_gnani_stt_connect_requires_credentials() {
        let config = create_test_config();
        let mut stt = GnaniSTT::create(config).unwrap();

        // Should fail because no credentials are set
        let result = stt.connect().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_gnani_stt_default() {
        let stt = GnaniSTT::default();
        assert!(!stt.is_ready());
        assert!(stt.config.is_none());
    }

    #[tokio::test]
    async fn test_gnani_stt_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = GnaniSTT::create(config).unwrap();

        // Should not error when disconnecting without being connected
        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
        let config = create_test_config();
        let mut stt = GnaniSTT::create(config).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the supervisor-shared intentional-disconnect flag",
        );
    }
}
