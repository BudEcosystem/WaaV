use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use google_api_proto::google::cloud::speech::v2::StreamingRecognizeRequest;
use google_api_proto::google::cloud::speech::v2::speech_client::SpeechClient;
use tokio::sync::{Mutex, Notify, RwLock, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use crate::core::providers::google::{
    CredentialSource, GOOGLE_CLOUD_PLATFORM_SCOPE, GOOGLE_SPEECH_ENDPOINT, GoogleAuthClient,
    GoogleError, TokenProvider, create_authenticated_channel,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};

use super::config::GoogleSTTConfig;
use super::streaming::{
    KEEPALIVE_INTERVAL_SECS, KeepaliveTracker, build_audio_request, build_config_request,
    chunk_audio, handle_grpc_error, handle_streaming_response, validate_keepalive_audio_geometry,
};

/// Per-message idle timeout for the gRPC response stream — resets after each successful message.
/// Catches stuck/dead connections while allowing active streams to continue.
const GRPC_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) fn google_speech_grpc_endpoint(config: &GoogleSTTConfig) -> String {
    config
        .endpoint_override
        .as_deref()
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| GOOGLE_SPEECH_ENDPOINT.to_string())
}

/// A [`WsTransport`] (the trait is transport-agnostic despite the `Ws` name) that adapts Google
/// STT's gRPC **bidirectional** streaming to the generic [`ReconnectableStream`] supervisor (W-D1
/// fleet adoption).
///
/// Google is gRPC, not WebSocket: the featured session lives in the **first request** of the
/// bidi stream (`build_config_request`, carrying the recognizer + recognition config). So
/// [`run`](WsTransport::run) opens a fresh `streaming_recognize` call whose request stream yields
/// that config request first (the featured-session restore is intrinsic to opening a new stream),
/// then forwards audio + keep-alive, and drains responses until a [`ReconnectOutcome`]. A
/// transport drop (`Unavailable`/idle timeout/stream end) becomes a reconnect; a clean shutdown or
/// a fatal gRPC error (auth/invalid-arg) does not.
struct GoogleTransport {
    /// A ready-to-use Speech client over the authenticated channel (the auth interceptor is baked
    /// in at construction). Cheap to hold; the channel is cloned per connect by the closure.
    client: SpeechClient<
        tonic::service::interceptor::InterceptedService<
            tonic::transport::Channel,
            Box<dyn FnMut(tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> + Send>,
        >,
    >,
    /// The featured first request (recognizer + recognition config) — re-yielded as the head of
    /// every fresh bidi stream so a reconnect restores the *featured* session, not a bare one.
    initial_config: StreamingRecognizeRequest,
    recognizer_path: String,
    sample_rate: u32,
    channels: u32,
    /// Shared inbound audio receiver (single-consumer; locked for the duration of `run`).
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown token (fires once; an intentional close must not reconnect).
    shutdown_token: CancellationToken,
    result_tx: mpsc::Sender<STTResult>,
    error_tx: mpsc::Sender<STTError>,
    /// Fires once on the first successful stream open, unblocking `start_connection`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    /// Set true once a clean shutdown was requested mid-stream so `run` can report Completed.
    shutdown_seen: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl WsTransport for GoogleTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // Google's featured session (recognizer + recognition config) is the FIRST request of the
        // bidi stream, re-sent at the head of every `run()` — so there is nothing to do here beyond
        // unblocking the waiting connect() exactly once.
        if let Some(tx) = self.connected_tx.lock().await.take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        // Use OWNED guards (lock_owned) so the request-stream generator can move them in and remain
        // `'static`, which `streaming_recognize` requires. The guards are released when the stream
        // is dropped at the end of this attempt, so the next reconnect re-locks the same receivers.
        let mut audio_rx = Arc::clone(&self.audio_rx).lock_owned().await;
        let initial_config = self.initial_config.clone();
        let recognizer_for_stream = self.recognizer_path.clone();
        let sample_rate = self.sample_rate;
        let channels = self.channels;
        let shutdown_token = self.shutdown_token.clone();
        let shutdown_seen = Arc::clone(&self.shutdown_seen);
        if let Err(e) = validate_keepalive_audio_geometry(sample_rate, channels) {
            error!("Invalid Google STT keepalive audio geometry: {}", e);
            let _ = self.error_tx.try_send(e);
            return ReconnectOutcome::Fatal(StreamError::new("invalid keepalive audio geometry"));
        }
        let error_tx = self.error_tx.clone();

        // Build the request stream for THIS attempt. It re-yields the featured config first, then
        // forwards audio + keep-alive. The owned guards are moved into the generator so it is
        // `'static`; they release when the stream drops at the end of the attempt.
        let request_stream = async_stream::stream! {
            debug!("Sending initial streaming configuration (Google bidi)");
            yield initial_config;

            let mut keepalive_timer =
                tokio::time::interval(Duration::from_secs(KEEPALIVE_INTERVAL_SECS));
            let mut keepalive_tracker = KeepaliveTracker::new(sample_rate, channels);

            loop {
                if shutdown_token.is_cancelled() {
                    info!("Shutdown signal received, ending Google request stream");
                    shutdown_seen.store(true, std::sync::atomic::Ordering::Release);
                    break;
                }

                tokio::select! {
                    biased;

                    audio_opt = audio_rx.recv() => {
                        match audio_opt {
                            Some(audio_data) => {
                                keepalive_tracker.touch();
                                for chunk in chunk_audio(audio_data) {
                                    yield build_audio_request(chunk, recognizer_for_stream.clone());
                                }
                            }
                            None => {
                                debug!("Audio channel closed, ending Google request stream");
                                break;
                            }
                        }
                    }

                    _ = keepalive_timer.tick() => {
                        if keepalive_tracker.needs_keepalive() {
                            let silence = match keepalive_tracker.generate_keepalive() {
                                Ok(silence) => silence,
                                Err(e) => {
                                    error!("Failed to generate Google STT keepalive audio: {}", e);
                                    let _ = error_tx.try_send(e);
                                    shutdown_seen.store(true, std::sync::atomic::Ordering::Release);
                                    break;
                                }
                            };
                            yield build_audio_request(silence, recognizer_for_stream.clone());
                            keepalive_tracker.touch();
                        }
                    }

                    _ = shutdown_token.cancelled() => {
                        info!("Shutdown signal received, ending Google request stream");
                        shutdown_seen.store(true, std::sync::atomic::Ordering::Release);
                        break;
                    }
                }
            }
        };

        let response = match self.client.streaming_recognize(request_stream).await {
            Ok(r) => r,
            Err(e) => {
                let stt_error = handle_grpc_error(e.clone());
                error!(
                    "Failed to start Google streaming recognition: {}",
                    stt_error
                );
                let _ = self.error_tx.try_send(stt_error);
                // Connection-level gRPC errors (Unavailable, etc.) are reconnectable; auth/config
                // are fatal. Reuse the status-code classification.
                return classify_grpc_outcome(e);
            }
        };

        let mut response_stream = response.into_inner();
        loop {
            // If a shutdown was requested while draining, stop cleanly.
            if self
                .shutdown_seen
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return ReconnectOutcome::Completed;
            }
            match tokio::time::timeout(GRPC_MESSAGE_TIMEOUT, response_stream.message()).await {
                Ok(Ok(Some(msg))) => {
                    if let Err(e) = handle_streaming_response(msg, &self.result_tx) {
                        error!("Error handling Google streaming response: {}", e);
                        let _ = self.error_tx.try_send(e);
                        return ReconnectOutcome::Fatal(StreamError::new(
                            "provider response error",
                        ));
                    }
                }
                Ok(Ok(None)) => {
                    info!("Google Speech-to-Text stream ended");
                    if self
                        .shutdown_seen
                        .load(std::sync::atomic::Ordering::Acquire)
                    {
                        return ReconnectOutcome::Completed;
                    }
                    return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                }
                Ok(Err(status)) => {
                    let stt_error = handle_grpc_error(status.clone());
                    error!("Streaming error from Google STT: {}", stt_error);
                    let _ = self.error_tx.try_send(stt_error);
                    return classify_grpc_outcome(status);
                }
                Err(_elapsed) => {
                    let stt_error = STTError::NetworkError(
                        "gRPC idle timeout - no message received for 60 seconds".into(),
                    );
                    error!("Google STT gRPC idle timeout: {}", stt_error);
                    let _ = self.error_tx.try_send(stt_error);
                    return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                }
            }
        }
    }
}

/// Map a gRPC status to a [`ReconnectOutcome`]: transient transport failures (Unavailable,
/// DeadlineExceeded, Internal, Cancelled, Aborted) are reconnectable; auth/permission/argument
/// errors are fatal (retrying would fail identically).
fn classify_grpc_outcome(status: tonic::Status) -> ReconnectOutcome {
    use tonic::Code;
    match status.code() {
        Code::Unavailable
        | Code::DeadlineExceeded
        | Code::Internal
        | Code::Cancelled
        | Code::Aborted
        | Code::ResourceExhausted => {
            ReconnectOutcome::Reconnectable(StreamError::new(format!("grpc {}", status.code())))
        }
        _ => ReconnectOutcome::Fatal(StreamError::new(format!("grpc {}", status.code()))),
    }
}

/// Converts a GoogleError to an STTError.
pub(crate) fn google_error_to_stt(e: GoogleError) -> STTError {
    match e {
        GoogleError::AuthenticationFailed(msg) => STTError::AuthenticationFailed(msg),
        GoogleError::ConfigurationError(msg) => STTError::ConfigurationError(msg),
        GoogleError::ConnectionFailed(msg) => STTError::ConnectionFailed(msg),
        GoogleError::NetworkError(msg) => STTError::NetworkError(msg),
        GoogleError::ApiError(msg) => STTError::ProviderError(msg),
        GoogleError::GrpcError { code, message } => {
            STTError::ProviderError(format!("gRPC error ({code}): {message}"))
        }
    }
}

/// STT-specific wrapper for GoogleAuthClient.
///
/// Validates credentials and creates an auth client with the Speech API scope.
#[derive(Debug)]
pub struct STTGoogleAuthClient {
    inner: GoogleAuthClient,
}

impl STTGoogleAuthClient {
    /// Creates a new STT auth client from a credential source.
    pub fn new(credential_source: CredentialSource) -> Result<Self, STTError> {
        credential_source.validate().map_err(google_error_to_stt)?;

        let inner = GoogleAuthClient::new(credential_source, &[GOOGLE_CLOUD_PLATFORM_SCOPE])
            .map_err(google_error_to_stt)?;

        Ok(Self { inner })
    }

    /// Creates a new auth client from an API key string.
    pub fn from_api_key(api_key: &str) -> Result<Self, STTError> {
        let source = CredentialSource::from_api_key(api_key);
        Self::new(source)
    }
}

#[async_trait::async_trait]
impl TokenProvider for STTGoogleAuthClient {
    async fn get_token(&self) -> Result<String, GoogleError> {
        self.inner.get_token().await
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

/// Audio channel buffer size - matches Deepgram's buffer for consistent behavior.
/// 32 provides good balance between latency and burst handling.
const AUDIO_CHANNEL_BUFFER_SIZE: usize = 32;

pub struct GoogleSTT {
    pub(super) config: Option<GoogleSTTConfig>,
    pub(super) state: ConnectionState,
    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before cancelling `shutdown_token`, so a client close racing a
    /// server-side close can never trigger a spurious reconnect.
    pub(super) intentional_disconnect: Arc<AtomicBool>,
    pub(super) state_notify: Arc<Notify>,
    /// Audio sender uses Bytes for zero-copy transfer
    pub(super) audio_sender: Option<mpsc::Sender<Bytes>>,
    pub(super) shutdown_token: Option<CancellationToken>,
    pub(super) result_tx: Option<mpsc::Sender<STTResult>>,
    /// Channel for propagating streaming errors to the client
    pub(super) error_tx: Option<mpsc::Sender<STTError>>,
    pub(super) connection_handle: Option<tokio::task::JoinHandle<()>>,
    pub(super) result_forward_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle for error forwarding task
    pub(super) error_forward_handle: Option<tokio::task::JoinHandle<()>>,
    /// Using RwLock instead of Mutex to reduce contention - callbacks are read-heavy
    pub(super) result_callback: Arc<RwLock<Option<AsyncSTTCallback>>>,
    /// Error callback for streaming errors
    pub(super) error_callback: Arc<RwLock<Option<AsyncErrorCallback>>>,
    pub(super) auth_client: Option<Arc<dyn TokenProvider>>,
    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`](crate::core::websocket::ReconnectableStream)
    /// supervisor. `None` before `set_resilience` → per-session governor/breaker default.
    pub(super) resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl Default for GoogleSTT {
    fn default() -> Self {
        Self {
            config: None,
            state: ConnectionState::Disconnected,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            audio_sender: None,
            shutdown_token: None,
            result_tx: None,
            error_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            auth_client: None,
            resilience: None,
        }
    }
}

impl GoogleSTT {
    /// W1 keystone — construct directly from the standardized config so Google's mappable
    /// features (interim results, voice-activity events) and its non-standard `project_id`
    /// (read from `extras`) are honored END-TO-END. Mirrors `DeepgramSTT::new_standard`: the
    /// credential is `std.base.api_key` (a Google credential source), used to build the auth
    /// client exactly as `BaseSTT::new` does. Features Google can't express stay at default.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        // A caller-supplied static access token (bring-your-own-token / mock path) makes the
        // service-account credential optional, since the OAuth fetch is bypassed.
        let has_static_token = std
            .extras
            .0
            .get("access_token")
            .and_then(|v| v.as_str())
            .is_some_and(|t| !t.is_empty());
        if std.base.api_key.is_empty() && !has_static_token {
            return Err(STTError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }
        let google_config = GoogleSTTConfig::from_standard(std);
        google_config
            .validate_endpoint_override()
            .map_err(STTError::ConfigurationError)?;
        let auth_client = STTGoogleAuthClient::from_api_key(&std.base.api_key)?;
        Ok(Self {
            config: Some(google_config),
            state: ConnectionState::Disconnected,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            audio_sender: None,
            shutdown_token: None,
            result_tx: None,
            error_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            auth_client: Some(Arc::new(auth_client)),
            resilience: None,
        })
    }

    pub(super) fn create_google_config(config: STTConfig, project_id: String) -> GoogleSTTConfig {
        GoogleSTTConfig {
            base: config,
            project_id,
            location: "global".to_string(),
            recognizer_id: None,
            interim_results: true,
            enable_voice_activity_events: true,
            speech_start_timeout: None,
            speech_end_timeout: None,
            single_utterance: false,
            ..GoogleSTTConfig::default()
        }
    }

    async fn start_connection(&mut self, config: GoogleSTTConfig) -> Result<(), STTError> {
        let auth_client = self.auth_client.clone().ok_or_else(|| {
            STTError::AuthenticationFailed("Auth client not initialized".to_string())
        })?;

        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        // Use smaller buffer for lower latency - Bytes enables zero-copy
        let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER_SIZE);
        let shutdown_token = CancellationToken::new();
        // Bounded channels for backpressure - 256 should handle bursts while preventing memory exhaustion
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(256);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(64);
        // Connection result channel - sends () on success when gRPC channel is established
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        self.audio_sender = Some(audio_tx);
        self.shutdown_token = Some(shutdown_token.clone());
        self.result_tx = Some(result_tx.clone());
        self.error_tx = Some(error_tx.clone());

        // Pre-compute recognizer path once - avoid repeated allocations in hot path
        let recognizer_path = config.recognizer_path();
        let initial_config = build_config_request(&config);

        // Endpoint + auth overrides (mirrors GoogleTTS): an `endpoint_override` (e.g. an `http://`
        // localhost tonic mock) replaces the production Speech endpoint, and a `static_access_token`
        // (a pre-minted bearer) bypasses the network OAuth fetch — together letting a mock e2e test
        // point this gRPC channel at a PLAINTEXT mock with no Google network round-trip.
        let grpc_endpoint = google_speech_grpc_endpoint(&config);
        let static_token = config.static_access_token.clone();

        // Get sample rate and channels for keep-alive audio generation
        let sample_rate = config.base.sample_rate;
        let channels = config.base.channels as u32;

        // Shared state the supervised transport re-uses across reconnect attempts: a single-
        // consumer audio receiver + shutdown token, the one-shot connected
        // signal, and a flag that records whether a clean shutdown was requested mid-stream.
        let audio_rx = Arc::new(Mutex::new(audio_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));
        let shutdown_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor with
        // the shared process-global handles from CoreState (W-D1/W-D2 fleet adoption). When no
        // handles were injected (a direct unit-test construction), the supervisor uses its own
        // per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("google", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => {
                ReconnectableStream::new(ReconnectableStreamConfig::new("google", reconnection))
            }
        }
        .with_disconnect_flag(disconnect_flag);

        // The supervisor owns the outer reconnect loop. The `connect` closure (re)establishes the
        // authenticated gRPC channel with a FRESH auth token (so a long-lived session that
        // reconnects after token expiry re-authenticates), and hands back a transport whose `run()`
        // opens a fresh bidi `streaming_recognize` whose first request restores the featured
        // session.
        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let auth_client = auth_client.clone();
                    let initial_config = initial_config.clone();
                    let recognizer_path = recognizer_path.clone();
                    let grpc_endpoint = grpc_endpoint.clone();
                    let static_token = static_token.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let shutdown_token = shutdown_token.clone();
                    let connected_tx = Arc::clone(&connected_tx);
                    let shutdown_seen = Arc::clone(&shutdown_seen);
                    let result_tx = result_tx.clone();
                    let error_tx = error_tx.clone();
                    async move {
                        let authenticated_channel =
                            create_authenticated_channel(&grpc_endpoint, auth_client.clone())
                                .await
                                .map_err(|e| {
                                    StreamError::new(google_error_to_stt(e).to_string())
                                })?;

                        // A pre-minted static token authenticates with NO network OAuth fetch (mock
                        // e2e); otherwise fetch a fresh bearer so a long-lived reconnect re-auths.
                        let auth_header = match &static_token {
                            Some(t) => format!("Bearer {t}"),
                            None => authenticated_channel
                                .get_authorization_header()
                                .await
                                .map_err(|e| {
                                    StreamError::new(google_error_to_stt(e).to_string())
                                })?,
                        };

                        let auth_metadata_value: tonic::metadata::MetadataValue<_> =
                            auth_header.parse().map_err(|_| {
                                StreamError::new("Failed to parse authorization header".to_string())
                            })?;

                        let channel = authenticated_channel.clone_channel();
                        // Box the interceptor closure so the client type is nameable on the struct.
                        let interceptor: Box<
                            dyn FnMut(
                                    tonic::Request<()>,
                                )
                                    -> Result<tonic::Request<()>, tonic::Status>
                                + Send,
                        > = Box::new(move |mut req: tonic::Request<()>| {
                            req.metadata_mut()
                                .insert("authorization", auth_metadata_value.clone());
                            Ok(req)
                        });
                        let client = SpeechClient::with_interceptor(channel, interceptor);

                        info!(
                            "Connected to Google Speech-to-Text API, recognizer: {}",
                            recognizer_path
                        );

                        Ok(GoogleTransport {
                            client,
                            initial_config,
                            recognizer_path,
                            sample_rate,
                            channels,
                            audio_rx,
                            shutdown_token,
                            result_tx,
                            error_tx,
                            connected_tx,
                            shutdown_seen,
                        })
                    }
                })
                .await;
            info!("Google Speech-to-Text connection closed (supervisor exit: {exit:?})");
        });

        self.connection_handle = Some(connection_handle);

        // Callback forwarding task - acquires lock once per result
        let callback_ref = self.result_callback.clone();
        let result_forward_handle = tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                // Single lock acquisition - clone the callback if present to release lock before await
                let callback_opt = {
                    let guard = callback_ref.read().await;
                    guard.as_ref().map(|cb| {
                        // Create the future while holding the lock, but don't await it
                        cb(result.clone())
                    })
                };

                if let Some(future) = callback_opt {
                    // Execute the callback future without holding any lock
                    future.await;
                } else {
                    debug!(
                        "Received STT result but no callback registered: {} (confidence: {})",
                        result.transcript, result.confidence
                    );
                }
            }
        });

        self.result_forward_handle = Some(result_forward_handle);

        // Error forwarding task - propagates streaming errors to registered callback
        let error_callback_ref = self.error_callback.clone();
        let error_forward_handle = tokio::spawn(async move {
            while let Some(error) = error_rx.recv().await {
                // Single lock acquisition - clone the callback if present to release lock before await
                let callback_opt = {
                    let guard = error_callback_ref.read().await;
                    guard.as_ref().map(|cb| cb(error.clone()))
                };

                if let Some(future) = callback_opt {
                    // Execute the callback future without holding any lock
                    future.await;
                } else {
                    error!(
                        "STT streaming error but no error callback registered: {}",
                        error
                    );
                }
            }
        });

        self.error_forward_handle = Some(error_forward_handle);

        self.state = ConnectionState::Connecting;

        // Wait for gRPC channel to be established (like Deepgram waits for WebSocket handshake)
        // Any permission/auth errors will come through the error_tx channel asynchronously
        match tokio::time::timeout(Duration::from_secs(30), connected_rx).await {
            Ok(Ok(())) => {
                // gRPC channel established - stream is ready to receive audio
                self.state = ConnectionState::Connected;
                self.state_notify.notify_waiters();
                info!("Successfully connected to Google Speech-to-Text");
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
impl BaseSTT for GoogleSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        // First, determine the credential source so we can extract project_id from it
        let credential_source = CredentialSource::from_api_key(&config.api_key);

        let (project_id, model_name) = if config.model.contains(':') {
            // Project ID explicitly provided in model field: "project_id:model_name"
            let parts: Vec<&str> = config.model.splitn(2, ':').collect();
            (parts[0].to_string(), parts[1].to_string())
        } else {
            // Try to extract project_id from credentials (service account JSON has project_id field)
            let project_id = credential_source.extract_project_id().unwrap_or_default();
            (project_id, config.model.clone())
        };

        if project_id.is_empty() {
            return Err(STTError::ConfigurationError(
                "Google Cloud project_id is required. Provide it either:\n\
                 1. In the model field as 'project_id:model_name'\n\
                 2. In the service account credentials JSON (project_id field)"
                    .to_string(),
            ));
        }

        let auth_client = STTGoogleAuthClient::new(credential_source)?;

        let mut updated_config = config;
        updated_config.model = model_name;
        let google_config = Self::create_google_config(updated_config, project_id);

        Ok(Self {
            config: Some(google_config),
            state: ConnectionState::Disconnected,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            audio_sender: None,
            shutdown_token: None,
            result_tx: None,
            error_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            auth_client: Some(Arc::new(auth_client)),
            resilience: None,
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        let config = self.config.as_ref().ok_or_else(|| {
            STTError::ConfigurationError("No configuration available".to_string())
        })?;
        config
            .validate_endpoint_override()
            .map_err(STTError::ConfigurationError)?;

        self.start_connection(config.clone()).await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Record the intent BEFORE any teardown so the supervisor sees it even if the transport's
        // run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);

        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }

        if let Some(handle) = self.connection_handle.take() {
            crate::core::observability::await_task_shutdown(
                "google-stt-connection",
                handle,
                Duration::from_secs(5),
            )
            .await;
        }

        if let Some(handle) = self.result_forward_handle.take() {
            crate::core::observability::abort_and_await_task("google-stt-result-forwarder", handle)
                .await;
        }

        if let Some(handle) = self.error_forward_handle.take() {
            crate::core::observability::abort_and_await_task("google-stt-error-forwarder", handle)
                .await;
        }

        self.audio_sender = None;
        self.result_tx = None;
        self.error_tx = None;
        *self.result_callback.write().await = None;
        *self.error_callback.write().await = None;

        self.state = ConnectionState::Disconnected;
        self.state_notify.notify_waiters();

        info!("Disconnected from Google Speech-to-Text");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        matches!(self.state, ConnectionState::Connected) && self.audio_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to Google Speech-to-Text".to_string(),
            ));
        }

        if let Some(audio_sender) = &self.audio_sender {
            let data_len = audio_data.len();

            // Zero-copy - Bytes passed directly to channel
            audio_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio data: {e}")))?;

            debug!("Sent {} bytes of audio data to Google STT", data_len);
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        // Use write lock since we're modifying the callback
        *self.result_callback.write().await = Some(Box::new(move |result| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(result).await;
            })
        }));
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        // Use write lock since we're modifying the callback
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

        // First, determine the credential source so we can extract project_id from it
        let credential_source = CredentialSource::from_api_key(&config.api_key);

        let (project_id, model_name) = if config.model.contains(':') {
            // Project ID explicitly provided in model field: "project_id:model_name"
            let parts: Vec<&str> = config.model.splitn(2, ':').collect();
            (parts[0].to_string(), parts[1].to_string())
        } else {
            // Try to extract project_id from credentials (service account JSON has project_id field)
            let project_id = credential_source.extract_project_id().unwrap_or_default();
            (project_id, config.model.clone())
        };

        if project_id.is_empty() {
            return Err(STTError::ConfigurationError(
                "Google Cloud project_id is required. Provide it either:\n\
                 1. In the model field as 'project_id:model_name'\n\
                 2. In the service account credentials JSON (project_id field)"
                    .to_string(),
            ));
        }

        let auth_client = STTGoogleAuthClient::new(credential_source)?;
        self.auth_client = Some(Arc::new(auth_client));

        let mut updated_config = config;
        updated_config.model = model_name;
        self.config = Some(Self::create_google_config(updated_config, project_id));

        self.connect().await?;
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Google Cloud Speech-to-Text v2"
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `start_connection` drives the generic
        // ReconnectableStream supervisor with them — every Google session trips the same breaker
        // and shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

impl GoogleSTT {
    /// The shared circuit breaker this session feeds into the generic supervisor, if the
    /// process-global resilience handles have been injected (W-D1/W-D2). Two `GoogleSTT` built from
    /// the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(&self) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }
}

impl Drop for GoogleSTT {
    fn drop(&mut self) {
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }
    }
}
