//! IBM Watson STT WebSocket client implementation.
//!
//! This module contains the main `IbmWatsonSTT` struct that implements the
//! `BaseSTT` trait for real-time speech-to-text streaming using IBM Watson
//! Speech-to-Text API.
//!
//! # Architecture
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
//! # IBM Watson-Specific Details
//!
//! - Uses IAM bearer token authentication (requires token refresh)
//! - WebSocket URL includes `access_token` query parameter
//! - Start message configures recognition parameters
//! - Stop message signals end of audio stream
//! - Supports interim results, word timestamps, speaker labels

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, RwLock, mpsc, oneshot};
use tokio::time::{Instant, interval, timeout};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, warn};
use url::form_urlencoded;

use super::config::{IBM_IAM_URL, IbmWatsonSTTConfig};
use super::messages::{IbmWatsonMessage, StopMessage};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

// =============================================================================
// Constants
// =============================================================================

/// Per-message idle timeout for WebSocket message reception.
/// Resets after each successful message. Catches stuck/dead connections.
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

// =============================================================================
// IAM Token Management
// =============================================================================

/// IAM access token with expiration tracking.
#[derive(Debug, Clone)]
struct IamToken {
    /// The bearer token string.
    access_token: String,
    /// When the token expires (Unix timestamp).
    expires_at: std::time::Instant,
}

impl IamToken {
    /// Check if the token is expired or about to expire (within 60 seconds).
    fn is_expired(&self) -> bool {
        self.expires_at <= std::time::Instant::now() + Duration::from_secs(60)
    }
}

/// IAM token response from IBM Cloud.
#[derive(Debug, serde::Deserialize)]
struct IamTokenResponse {
    access_token: String,
    /// Token lifetime in seconds.
    expires_in: u64,
}

/// Fetch IAM access token from IBM Cloud using API key.
async fn fetch_iam_token(api_key: &str) -> Result<IamToken, STTError> {
    // Create client with explicit timeouts to prevent indefinite hangs
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(4) // Limit connection pool size
        .pool_idle_timeout(Duration::from_secs(90)) // Close idle connections after 90s
        .build()
        .map_err(|e| {
            STTError::AuthenticationFailed(format!("Failed to create HTTP client: {e}"))
        })?;

    // URL-encode the API key
    let encoded_api_key: String = form_urlencoded::byte_serialize(api_key.as_bytes()).collect();

    let response = client
        .post(IBM_IAM_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=urn:ibm:params:oauth:grant-type:apikey&apikey={}",
            encoded_api_key
        ))
        .send()
        .await
        .map_err(|e| STTError::AuthenticationFailed(format!("Failed to request IAM token: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(STTError::AuthenticationFailed(format!(
            "IAM token request failed ({status}): {body}"
        )));
    }

    let token_response: IamTokenResponse = response
        .json()
        .await
        .map_err(|e| STTError::AuthenticationFailed(format!("Failed to parse IAM token: {e}")))?;

    // Calculate expiration time with safety margin
    // Use saturating_sub to avoid underflow if expires_in is unexpectedly small
    let safety_margin = 300; // 5 minutes before expiration
    let expires_in_secs = token_response.expires_in.saturating_sub(safety_margin);
    let expires_at = std::time::Instant::now() + Duration::from_secs(expires_in_secs.max(60));

    Ok(IamToken {
        access_token: token_response.access_token,
        expires_at,
    })
}

// =============================================================================
// Connection State
// =============================================================================

/// Connection state for the WebSocket client.
#[derive(Debug, Clone)]
enum ConnectionState {
    /// Not connected to IBM Watson.
    Disconnected,
    /// In the process of establishing connection.
    Connecting,
    /// Connected and ready to receive audio.
    Connected,
    /// An error occurred.
    #[allow(dead_code)]
    Error(String),
}

// =============================================================================
// IbmWatsonSTT Client
// =============================================================================

/// IBM Watson Speech-to-Text WebSocket client.
///
/// This struct implements real-time speech-to-text using the IBM Watson
/// Speech-to-Text WebSocket API. It manages:
///
/// - IAM token authentication and refresh
/// - WebSocket connection lifecycle
/// - Audio data streaming to IBM Watson
/// - Transcription result callbacks (both interim and final)
/// - Error handling and recovery
/// - Keep-alive mechanism
///
/// # Example
///
/// ```rust,no_run
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::ibm_watson::IbmWatsonSTT;
/// use std::sync::Arc;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = STTConfig {
///         api_key: "your-ibm-api-key".to_string(),
///         language: "en-US".to_string(),
///         sample_rate: 16000,
///         ..Default::default()
///     };
///
///     let mut stt = IbmWatsonSTT::new(config)?;
///
///     // Set the IBM Cloud instance ID
///     stt.set_instance_id("your-instance-id".to_string());
///
///     // Register result callback
///     stt.on_result(Arc::new(|result| {
///         Box::pin(async move {
///             println!("Transcript: {}", result.transcript);
///         })
///     })).await?;
///
///     // Connect to IBM Watson
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
pub struct IbmWatsonSTT {
    /// Configuration for the STT client.
    config: Option<IbmWatsonSTTConfig>,

    /// Current connection state.
    state: ConnectionState,

    /// State change notification.
    state_notify: Arc<Notify>,

    /// WebSocket sender for audio data.
    /// Uses bounded channel (32 items) for backpressure.
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

    /// Cached IAM token.
    iam_token: Arc<RwLock<Option<IamToken>>>,

    /// Connection ready flag for atomic checks.
    connected: Arc<AtomicBool>,
}

impl Default for IbmWatsonSTT {
    fn default() -> Self {
        Self {
            config: None,
            state: ConnectionState::Disconnected,
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
            iam_token: Arc::new(RwLock::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl IbmWatsonSTT {
    /// Set the IBM Cloud instance ID.
    ///
    /// The instance ID is found in your IBM Cloud service credentials.
    /// It's required for constructing the WebSocket URL.
    pub fn set_instance_id(&mut self, instance_id: String) {
        if let Some(config) = &mut self.config {
            config.instance_id = instance_id;
        }
    }

    /// Set the IBM Cloud region.
    pub fn set_region(&mut self, region: super::config::IbmRegion) {
        if let Some(config) = &mut self.config {
            config.region = region;
        }
    }

    /// Get the current IAM token, refreshing if necessary.
    async fn get_access_token(&self) -> Result<String, STTError> {
        let api_key = self
            .config
            .as_ref()
            .map(|c| c.base.api_key.clone())
            .ok_or_else(|| {
                STTError::ConfigurationError("No configuration available".to_string())
            })?;

        // Check if we have a valid cached token
        {
            let token_guard = self.iam_token.read().await;
            if let Some(token) = token_guard.as_ref() {
                if !token.is_expired() {
                    return Ok(token.access_token.clone());
                }
            }
        }

        // Need to fetch a new token
        let new_token = fetch_iam_token(&api_key).await?;
        let access_token = new_token.access_token.clone();

        // Cache the new token
        {
            let mut token_guard = self.iam_token.write().await;
            *token_guard = Some(new_token);
        }

        Ok(access_token)
    }

    /// Handle incoming WebSocket messages from IBM Watson.
    fn handle_websocket_message(
        message: Message,
        result_tx: &mpsc::Sender<STTResult>,
        error_tx: &mpsc::Sender<STTError>,
        interim_results_enabled: bool,
    ) -> Result<bool, STTError> {
        match message {
            Message::Text(text) => {
                debug!("Received IBM Watson message: {}", text);

                match IbmWatsonMessage::parse(&text) {
                    Ok(parsed_msg) => match parsed_msg {
                        IbmWatsonMessage::Listening(_) => {
                            info!("IBM Watson is listening and ready for audio");
                        }

                        IbmWatsonMessage::Results(results) => {
                            for result in results.results {
                                // For interim results, check if enabled
                                if !result.is_final && !interim_results_enabled {
                                    continue;
                                }

                                if let Some(stt_result) = result.to_stt_result() {
                                    if !stt_result.transcript.is_empty() {
                                        if result_tx.try_send(stt_result).is_err() {
                                            warn!("Failed to send result - channel closed");
                                        }
                                    }
                                }
                            }
                        }

                        IbmWatsonMessage::State(state) => {
                            debug!("IBM Watson state change: {}", state.state);
                        }

                        IbmWatsonMessage::SpeakerLabels(labels) => {
                            debug!(
                                "Received speaker labels: {} entries",
                                labels.speaker_labels.len()
                            );
                            // Speaker labels could be used to enrich results in a future version
                        }

                        IbmWatsonMessage::Error(error_msg) => {
                            error!("IBM Watson error: {}", error_msg.error);

                            if error_msg.is_critical() {
                                let stt_error = STTError::ProviderError(error_msg.error.clone());
                                let _ = error_tx.try_send(stt_error);
                                return Ok(true); // Signal to close connection
                            } else if error_msg.is_inactivity_timeout() {
                                warn!("IBM Watson inactivity timeout");
                                // Don't close connection on inactivity, just warn
                            } else {
                                let stt_error = STTError::ProviderError(error_msg.error.clone());
                                let _ = error_tx.try_send(stt_error);
                            }
                        }
                    },
                    Err(e) => {
                        warn!("Failed to parse IBM Watson message: {} - raw: {}", e, text);
                    }
                }
            }

            Message::Binary(data) => {
                // IBM Watson may send binary data for certain responses
                debug!(
                    "Received binary message from IBM Watson: {} bytes",
                    data.len()
                );
            }

            Message::Close(close_frame) => {
                info!("IBM Watson WebSocket closed: {:?}", close_frame);
                return Ok(true); // Signal to close
            }

            Message::Ping(_) => {
                debug!("Received ping from IBM Watson");
            }

            Message::Pong(_) => {
                debug!("Received pong from IBM Watson");
            }

            _ => {
                debug!("Received unexpected message type from IBM Watson");
            }
        }

        Ok(false) // Don't close connection
    }

    /// Start the WebSocket connection to IBM Watson Speech Services.
    async fn start_connection(&mut self) -> Result<(), STTError> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| STTError::ConfigurationError("No configuration available".to_string()))?
            .clone();

        // Validate instance ID
        if config.instance_id.is_empty() {
            return Err(STTError::ConfigurationError(
                "IBM Watson instance_id is required. Set it using set_instance_id()".to_string(),
            ));
        }

        // Get access token
        let access_token = self.get_access_token().await?;

        // Build WebSocket URL
        let ws_url = config.build_websocket_url(&access_token);

        // Create channels for communication
        let (ws_tx, mut ws_rx) = mpsc::channel::<Bytes>(32);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
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
        let start_message = config.build_start_message();
        let interim_results_enabled = config.interim_results;
        let connected_flag = self.connected.clone();

        // Start the connection task
        let connection_handle = tokio::spawn(async move {
            // Connect to IBM Watson with timeout
            let connect_result =
                match timeout(Duration::from_secs(30), connect_async(&ws_url)).await {
                    Ok(result) => result,
                    Err(_) => {
                        let stt_error = STTError::ConnectionFailed(
                            "Connection to IBM Watson timed out after 30 seconds".to_string(),
                        );
                        error!("{}", stt_error);
                        let _ = error_tx.try_send(stt_error);
                        return;
                    }
                };

            let (ws_stream, _response) = match connect_result {
                Ok(result) => result,
                Err(e) => {
                    let error_msg = format!("Failed to connect to IBM Watson: {e}");
                    let stt_error = if error_msg.contains("401")
                        || error_msg.contains("Unauthorized")
                    {
                        STTError::AuthenticationFailed(
                            "IBM Watson authentication failed. Check API key and instance ID."
                                .to_string(),
                        )
                    } else if error_msg.contains("403") || error_msg.contains("Forbidden") {
                        STTError::AuthenticationFailed(
                            "IBM Watson access forbidden. Check service permissions.".to_string(),
                        )
                    } else {
                        STTError::ConnectionFailed(error_msg)
                    };
                    error!("{}", stt_error);
                    let _ = error_tx.try_send(stt_error);
                    return;
                }
            };

            info!("Connected to IBM Watson Speech-to-Text WebSocket");

            let (mut ws_sink, mut ws_stream) = ws_stream.split();

            // Send start message to configure recognition
            let start_json = serde_json::to_string(&start_message).unwrap();
            if let Err(e) = ws_sink.send(Message::Text(start_json.into())).await {
                let stt_error =
                    STTError::ConnectionFailed(format!("Failed to send start message: {e}"));
                error!("{}", stt_error);
                let _ = error_tx.try_send(stt_error);
                return;
            }

            debug!("Sent start message to IBM Watson");

            // Wait for "listening" state message
            let listen_timeout = timeout(Duration::from_secs(10), async {
                while let Some(msg) = ws_stream.next().await {
                    if let Ok(Message::Text(text)) = msg {
                        if let Ok(IbmWatsonMessage::Listening(_)) = IbmWatsonMessage::parse(&text) {
                            return true;
                        }
                    }
                }
                false
            })
            .await;

            match listen_timeout {
                Ok(true) => {
                    // Successfully received listening state
                    connected_flag.store(true, Ordering::Release);
                    let _ = connected_tx.send(());
                }
                _ => {
                    let stt_error = STTError::ConnectionFailed(
                        "Did not receive listening state from IBM Watson".to_string(),
                    );
                    error!("{}", stt_error);
                    let _ = error_tx.try_send(stt_error);
                    return;
                }
            }

            // Keep-alive mechanism
            let mut keepalive_timer = interval(Duration::from_secs(5));
            let mut last_audio_time = Instant::now();

            // Main event loop
            loop {
                tokio::select! {
                    // Prioritize audio sending for lowest latency
                    biased;

                    // Handle outgoing audio data
                    Some(audio_data) = ws_rx.recv() => {
                        // IBM Watson expects raw audio as binary WebSocket frames
                        let message = Message::Binary(audio_data);
                        if let Err(e) = ws_sink.send(message).await {
                            let stt_error = STTError::NetworkError(format!(
                                "Failed to send audio to IBM Watson: {e}"
                            ));
                            error!("{}", stt_error);
                            let _ = error_tx.try_send(stt_error);
                            break;
                        }
                        last_audio_time = Instant::now();
                    }

                    // Handle incoming messages with idle timeout
                    message = timeout(WS_MESSAGE_TIMEOUT, ws_stream.next()) => {
                        match message {
                            Ok(Some(Ok(msg))) => {
                                match Self::handle_websocket_message(
                                    msg,
                                    &result_tx,
                                    &error_tx,
                                    interim_results_enabled,
                                ) {
                                    Ok(should_close) => {
                                        if should_close {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        error!("IBM Watson message handling error: {}", e);
                                        let _ = error_tx.try_send(e);
                                        break;
                                    }
                                }
                            }
                            Ok(Some(Err(e))) => {
                                let stt_error = STTError::NetworkError(format!(
                                    "WebSocket error: {e}"
                                ));
                                error!("{}", stt_error);
                                let _ = error_tx.try_send(stt_error);
                                break;
                            }
                            Ok(None) => {
                                info!("IBM Watson WebSocket stream ended");
                                break;
                            }
                            Err(_elapsed) => {
                                // Idle timeout - no message received for 60s
                                let stt_error = STTError::NetworkError(
                                    "WebSocket idle timeout - no message for 60 seconds".into()
                                );
                                error!("IBM Watson STT idle timeout: {}", stt_error);
                                let _ = error_tx.try_send(stt_error);
                                break;
                            }
                        }
                    }

                    // Handle keep-alive timer
                    _ = keepalive_timer.tick() => {
                        // Send silence frames to prevent timeout
                        if last_audio_time.elapsed() >= Duration::from_secs(10) {
                            // Send a small buffer of silence (32 samples at 16kHz = 2ms)
                            let silence_frame = vec![0u8; 64];
                            let message = Message::Binary(silence_frame.into());
                            if let Err(e) = ws_sink.send(message).await {
                                let stt_error = STTError::NetworkError(format!(
                                    "Failed to send keep-alive: {e}"
                                ));
                                error!("{}", stt_error);
                                let _ = error_tx.try_send(stt_error);
                                break;
                            }
                            debug!("Sent keep-alive silence frame to IBM Watson");
                            last_audio_time = Instant::now();
                        }
                    }

                    // Handle shutdown signal
                    _ = &mut shutdown_rx => {
                        info!("Received shutdown signal for IBM Watson STT");
                        // Send stop message to gracefully end recognition
                        let stop_message = StopMessage::new();
                        if let Ok(stop_json) = serde_json::to_string(&stop_message) {
                            let _ = ws_sink.send(Message::Text(stop_json.into())).await;
                        }
                        let _ = ws_sink.send(Message::Close(None)).await;
                        break;
                    }
                }
            }

            connected_flag.store(false, Ordering::Release);
            info!("IBM Watson STT WebSocket connection closed");
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
                        "IBM Watson STT result (no callback): {} (final: {}, confidence: {})",
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
                    error!("IBM Watson STT error (no callback registered): {}", error);
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
                info!("Successfully connected to IBM Watson Speech-to-Text");
                Ok(())
            }
            Ok(Err(_)) => {
                let error_msg = "Connection channel closed before confirmation".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
            Err(_) => {
                let error_msg = "Connection timeout waiting for IBM Watson".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
        }
    }

    /// Get the IBM Watson-specific configuration.
    pub fn get_ibm_config(&self) -> Option<&IbmWatsonSTTConfig> {
        self.config.as_ref()
    }
}

impl Drop for IbmWatsonSTT {
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
impl BaseSTT for IbmWatsonSTT {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        // Validate API key
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "IBM Watson API key is required".to_string(),
            ));
        }

        // Create IBM Watson-specific configuration
        // Instance ID can be set later via set_instance_id()
        let ibm_config = IbmWatsonSTTConfig::from_base(config, String::new());

        Ok(Self {
            config: Some(ibm_config),
            state: ConnectionState::Disconnected,
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
            iam_token: Arc::new(RwLock::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        // Check if already connected
        if self.connected.load(Ordering::Acquire) {
            return Err(STTError::ConnectionFailed(
                "Already connected to IBM Watson".to_string(),
            ));
        }

        self.start_connection().await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
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
        self.connected.store(false, Ordering::Release);
        self.state_notify.notify_waiters();

        info!("Disconnected from IBM Watson Speech-to-Text");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::Acquire) && self.ws_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to IBM Watson Speech-to-Text".to_string(),
            ));
        }

        if let Some(ws_sender) = &self.ws_sender {
            let data_len = audio_data.len();

            // Zero-copy - Bytes passed directly to WebSocket
            ws_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio data: {e}")))?;

            debug!("Queued {} bytes of audio for IBM Watson STT", data_len);
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

        // Preserve IBM-specific settings from existing config
        let existing = self.config.take();

        let ibm_config = IbmWatsonSTTConfig {
            base: config,
            region: existing.as_ref().map(|c| c.region).unwrap_or_default(),
            instance_id: existing
                .as_ref()
                .map(|c| c.instance_id.clone())
                .unwrap_or_default(),
            model: existing
                .as_ref()
                .map(|c| c.model.clone())
                .unwrap_or_default(),
            encoding: existing.as_ref().map(|c| c.encoding).unwrap_or_default(),
            interim_results: existing.as_ref().map(|c| c.interim_results).unwrap_or(true),
            word_timestamps: existing.as_ref().is_some_and(|c| c.word_timestamps),
            word_confidence: existing.as_ref().is_some_and(|c| c.word_confidence),
            speaker_labels: existing.as_ref().is_some_and(|c| c.speaker_labels),
            smart_formatting: existing.as_ref().is_some_and(|c| c.smart_formatting),
            profanity_filter: existing.as_ref().is_some_and(|c| c.profanity_filter),
            redaction: existing.as_ref().is_some_and(|c| c.redaction),
            inactivity_timeout: existing
                .as_ref()
                .map(|c| c.inactivity_timeout)
                .unwrap_or(30),
            language_model_id: existing.as_ref().and_then(|c| c.language_model_id.clone()),
            acoustic_model_id: existing.as_ref().and_then(|c| c.acoustic_model_id.clone()),
            background_audio_suppression: existing
                .as_ref()
                .and_then(|c| c.background_audio_suppression),
            speech_detector_sensitivity: existing
                .as_ref()
                .and_then(|c| c.speech_detector_sensitivity),
            end_of_phrase_silence_time: existing
                .as_ref()
                .and_then(|c| c.end_of_phrase_silence_time),
            split_transcript_at_phrase_end: existing
                .as_ref()
                .is_some_and(|c| c.split_transcript_at_phrase_end),
            low_latency: existing.as_ref().is_some_and(|c| c.low_latency),
            character_insertion_bias: existing.as_ref().and_then(|c| c.character_insertion_bias),
        };

        self.config = Some(ibm_config);

        // Reconnect with new configuration
        self.connect().await
    }

    fn get_provider_info(&self) -> &'static str {
        "IBM Watson Speech-to-Text"
    }
}

// =============================================================================
// IBM Watson-Specific Helper Methods
// =============================================================================

impl IbmWatsonSTT {
    /// Update IBM Watson-specific settings.
    ///
    /// This allows updating IBM-specific parameters without affecting
    /// the base STT configuration. Requires reconnection.
    pub async fn update_ibm_settings(
        &mut self,
        region: Option<super::config::IbmRegion>,
        model: Option<super::config::IbmModel>,
        interim_results: Option<bool>,
        smart_formatting: Option<bool>,
        speaker_labels: Option<bool>,
    ) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }

        if let Some(config) = &mut self.config {
            if let Some(r) = region {
                config.region = r;
            }
            if let Some(m) = model {
                config.model = m;
            }
            if let Some(ir) = interim_results {
                config.interim_results = ir;
            }
            if let Some(sf) = smart_formatting {
                config.smart_formatting = sf;
            }
            if let Some(sl) = speaker_labels {
                config.speaker_labels = sl;
            }
        }

        self.connect().await
    }

    /// Set a custom language model.
    ///
    /// Use this to specify a custom language model trained on your specific
    /// domain or vocabulary.
    pub async fn set_language_model(&mut self, model_id: Option<String>) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }

        if let Some(config) = &mut self.config {
            config.language_model_id = model_id;
        }

        self.connect().await
    }

    /// Configure background audio suppression.
    ///
    /// Level between 0.0 (no suppression) and 1.0 (maximum suppression).
    pub async fn set_background_suppression(&mut self, level: f32) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }

        if let Some(config) = &mut self.config {
            config.background_audio_suppression = Some(level.clamp(0.0, 1.0));
        }

        self.connect().await
    }

    /// Enable low latency mode for faster interim results.
    ///
    /// Note: This may reduce accuracy slightly.
    pub async fn set_low_latency(&mut self, enabled: bool) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }

        if let Some(config) = &mut self.config {
            config.low_latency = enabled;
        }

        self.connect().await
    }
}
