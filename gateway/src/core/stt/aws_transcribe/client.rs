//! Amazon Transcribe Streaming STT client implementation.
//!
//! This module provides the `AwsTranscribeSTT` struct that implements the `BaseSTT`
//! trait for real-time speech-to-text using Amazon Transcribe Streaming API.
//!
//! # Features
//!
//! - Real-time streaming transcription using AWS SDK
//! - Support for 100+ languages
//! - Partial results with stabilization for low-latency applications
//! - Speaker diarization
//! - Content redaction (PII masking)
//! - Custom vocabulary and language models
//!
//! # Audio Format Requirements
//!
//! - PCM: 16-bit signed little-endian, mono
//! - Sample rate: 8,000 Hz to 48,000 Hz (16,000 Hz recommended)
//! - Chunk duration: 50-200 ms for optimal latency
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::aws_transcribe::{AwsTranscribeSTT, AwsTranscribeSTTConfig};
//! use waav_gateway::core::stt::{BaseSTT, STTConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = STTConfig {
//!         provider: "aws-transcribe".to_string(),
//!         api_key: String::new(), // Use AWS credentials from environment
//!         language: "en-US".to_string(),
//!         sample_rate: 16000,
//!         channels: 1,
//!         punctuation: true,
//!         encoding: "pcm".to_string(),
//!         model: String::new(),
//!     };
//!
//!     let mut stt = AwsTranscribeSTT::new(config)?;
//!     stt.connect().await?;
//!
//!     // Send audio chunks...
//!     // stt.send_audio(audio_bytes).await?;
//!
//!     stt.disconnect().await?;
//!     Ok(())
//! }
//! ```

use bytes::Bytes;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aws_config::BehaviorVersion;
use aws_sdk_transcribestreaming::Client as TranscribeClient;
use aws_sdk_transcribestreaming::types::{
    AudioEvent, AudioStream, LanguageCode, MediaEncoding as AwsMediaEncoding,
    PartialResultsStability as AwsPartialResultsStability, TranscriptResultStream,
};
use aws_smithy_types::Blob;
use tokio::sync::{Mutex, Notify, RwLock, mpsc, oneshot};

use super::config::{
    AwsRegion, AwsTranscribeSTTConfig, DEFAULT_CHUNK_DURATION_MS, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE,
    MediaEncoding, PartialResultsStability,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

use tracing::{debug, error, info, warn};

// =============================================================================
// Type Aliases
// =============================================================================

/// Type alias for async STT result callback.
type AsyncSTTCallback = Box<
    dyn Fn(STTResult) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Type alias for async error callback.
type AsyncErrorCallback = Box<
    dyn Fn(STTError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

// =============================================================================
// Connection State
// =============================================================================

/// Connection state for the Amazon Transcribe client.
#[derive(Debug, Clone, PartialEq)]
enum ConnectionState {
    /// Not connected to the service.
    Disconnected,
    /// Currently connecting to the service.
    Connecting,
    /// Connected and ready to transcribe.
    Connected,
    /// Error state with description.
    Error(String),
}

// =============================================================================
// Constants
// =============================================================================

/// Maximum audio chunk size in bytes for sanity check.
///
/// Amazon Transcribe recommends 50-200ms chunks. At 48kHz stereo 16-bit,
/// 200ms is about 38KB. We set a limit of 256KB to prevent memory issues.
const MAX_AUDIO_CHUNK_SIZE: usize = 256 * 1024;

/// Connection timeout in seconds.
const CONNECTION_TIMEOUT_SECS: u64 = 30;

/// Channel buffer size for audio data.
const AUDIO_CHANNEL_BUFFER_SIZE: usize = 32;

// =============================================================================
// Amazon Transcribe STT Client
// =============================================================================

/// Amazon Transcribe Streaming STT client.
///
/// This struct implements the `BaseSTT` trait for real-time speech-to-text
/// transcription using AWS Transcribe Streaming API.
pub struct AwsTranscribeSTT {
    /// Provider-specific configuration.
    config: Option<AwsTranscribeSTTConfig>,

    /// Current connection state.
    state: ConnectionState,

    /// State change notification.
    state_notify: Arc<Notify>,

    /// Audio sender channel (bounded for backpressure).
    audio_tx: Option<mpsc::Sender<Bytes>>,

    /// Shutdown signal sender.
    shutdown_tx: Option<oneshot::Sender<()>>,

    /// Result channel sender for internal forwarding.
    result_tx: Option<mpsc::UnboundedSender<STTResult>>,

    /// Error channel sender for internal forwarding.
    error_tx: Option<mpsc::UnboundedSender<STTError>>,

    /// Connection task handle.
    connection_handle: Option<tokio::task::JoinHandle<()>>,

    /// Result forwarding task handle.
    result_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Error forwarding task handle.
    error_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Shared callback storage for async access.
    result_callback: Arc<Mutex<Option<AsyncSTTCallback>>>,

    /// Error callback storage for streaming errors.
    error_callback: Arc<Mutex<Option<AsyncErrorCallback>>>,

    /// Flag indicating if we're connected and ready.
    is_connected: Arc<AtomicBool>,

    /// Current session ID (if available).
    session_id: Arc<RwLock<Option<String>>>,
}

impl AwsTranscribeSTT {
    /// Create a new Amazon Transcribe STT client with the given configuration.
    pub fn new_with_config(config: AwsTranscribeSTTConfig) -> Result<Self, STTError> {
        // Validate configuration
        config
            .validate()
            .map_err(|e| STTError::ConfigurationError(format!("Invalid configuration: {}", e)))?;

        Ok(Self {
            config: Some(config),
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            audio_tx: None,
            shutdown_tx: None,
            result_tx: None,
            error_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            is_connected: Arc::new(AtomicBool::new(false)),
            session_id: Arc::new(RwLock::new(None)),
        })
    }

    /// Get the current session ID.
    pub async fn get_session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// Convert WaaV MediaEncoding to AWS SDK MediaEncoding.
    fn convert_media_encoding(encoding: &MediaEncoding) -> AwsMediaEncoding {
        match encoding {
            MediaEncoding::Pcm => AwsMediaEncoding::Pcm,
            MediaEncoding::Flac => AwsMediaEncoding::Flac,
            MediaEncoding::OggOpus => AwsMediaEncoding::OggOpus,
        }
    }

    /// Convert WaaV PartialResultsStability to AWS SDK type.
    fn convert_partial_results_stability(
        stability: &PartialResultsStability,
    ) -> AwsPartialResultsStability {
        match stability {
            PartialResultsStability::High => AwsPartialResultsStability::High,
            PartialResultsStability::Medium => AwsPartialResultsStability::Medium,
            PartialResultsStability::Low => AwsPartialResultsStability::Low,
        }
    }

    /// Convert language code string to AWS SDK LanguageCode.
    fn convert_language_code(language: &str) -> Option<LanguageCode> {
        // Map common language codes to AWS SDK enum variants
        match language.to_lowercase().as_str() {
            "en-us" | "en_us" => Some(LanguageCode::EnUs),
            "en-gb" | "en_gb" => Some(LanguageCode::EnGb),
            "en-au" | "en_au" => Some(LanguageCode::EnAu),
            "es-us" | "es_us" => Some(LanguageCode::EsUs),
            "es-es" | "es_es" => Some(LanguageCode::EsEs),
            "fr-fr" | "fr_fr" => Some(LanguageCode::FrFr),
            "fr-ca" | "fr_ca" => Some(LanguageCode::FrCa),
            "de-de" | "de_de" => Some(LanguageCode::DeDe),
            "it-it" | "it_it" => Some(LanguageCode::ItIt),
            "pt-br" | "pt_br" => Some(LanguageCode::PtBr),
            "pt-pt" | "pt_pt" => Some(LanguageCode::PtPt),
            "ja-jp" | "ja_jp" => Some(LanguageCode::JaJp),
            "ko-kr" | "ko_kr" => Some(LanguageCode::KoKr),
            "zh-cn" | "zh_cn" => Some(LanguageCode::ZhCn),
            "hi-in" | "hi_in" => Some(LanguageCode::HiIn),
            "ar-sa" | "ar_sa" => Some(LanguageCode::ArSa),
            "ru-ru" | "ru_ru" => Some(LanguageCode::RuRu),
            "nl-nl" | "nl_nl" => Some(LanguageCode::NlNl),
            "sv-se" | "sv_se" => Some(LanguageCode::SvSe),
            "th-th" | "th_th" => Some(LanguageCode::ThTh),
            "tr-tr" | "tr_tr" => Some(LanguageCode::TrTr),
            "vi-vn" | "vi_vn" => Some(LanguageCode::ViVn),
            _ => {
                // For unsupported codes, default to en-US with a warning
                warn!(
                    "Unsupported language code '{}', defaulting to en-US",
                    language
                );
                Some(LanguageCode::EnUs)
            }
        }
    }

    /// Start the transcription stream connection.
    async fn start_connection(&mut self, config: AwsTranscribeSTTConfig) -> Result<(), STTError> {
        // Create channels for communication
        let (audio_tx, mut audio_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER_SIZE);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let (result_tx, mut result_rx) = mpsc::unbounded_channel::<STTResult>();
        let (error_tx, mut error_rx) = mpsc::unbounded_channel::<STTError>();
        let (connected_tx, connected_rx) = oneshot::channel::<Result<(), STTError>>();

        // Store channels
        self.audio_tx = Some(audio_tx);
        self.shutdown_tx = Some(shutdown_tx);
        self.result_tx = Some(result_tx.clone());
        self.error_tx = Some(error_tx.clone());

        // Clone data needed for the connection task
        let region_str = config.region.as_str().to_string();
        let language_code = config.base.language.clone();
        let sample_rate = config.base.sample_rate as i32;
        let media_encoding = Self::convert_media_encoding(&config.media_encoding);
        let enable_partial_stabilization = config.enable_partial_results_stabilization;
        let partial_stability =
            Self::convert_partial_results_stability(&config.partial_results_stability);
        let show_speaker_label = config.show_speaker_label;
        let vocabulary_name = config.vocabulary_name.clone();
        let vocabulary_filter_name = config.vocabulary_filter_name.clone();
        let session_id = config.session_id.clone();
        let identify_language = config.identify_language;

        let is_connected = self.is_connected.clone();
        let session_id_storage = self.session_id.clone();

        let aws_access_key_id = config.aws_access_key_id.clone();
        let aws_secret_access_key = config.aws_secret_access_key.clone();
        let aws_session_token = config.aws_session_token.clone();

        // Spawn the connection task
        let connection_handle = tokio::spawn(async move {
            // Build AWS config
            let aws_config = if aws_access_key_id.is_some() && aws_secret_access_key.is_some() {
                // Use explicit credentials
                let credentials = aws_credential_types::Credentials::new(
                    aws_access_key_id.as_deref().unwrap_or_default(),
                    aws_secret_access_key.as_deref().unwrap_or_default(),
                    aws_session_token,
                    None, // Expiration
                    "waav-gateway",
                );

                aws_config::defaults(BehaviorVersion::latest())
                    .region(aws_config::Region::new(region_str))
                    .credentials_provider(credentials)
                    .load()
                    .await
            } else {
                // Use default credential chain (env vars, IAM roles, etc.)
                aws_config::defaults(BehaviorVersion::latest())
                    .region(aws_config::Region::new(region_str))
                    .load()
                    .await
            };

            let client = TranscribeClient::new(&aws_config);

            // Convert language code
            let language = Self::convert_language_code(&language_code);

            // Build the streaming request
            let mut request = client
                .start_stream_transcription()
                .media_sample_rate_hertz(sample_rate)
                .media_encoding(media_encoding);

            // Add language code if not using auto-detect
            if !identify_language {
                if let Some(lang) = language {
                    request = request.language_code(lang);
                }
            } else {
                request = request.identify_language(true);
            }

            // Add optional parameters
            if enable_partial_stabilization {
                request = request
                    .enable_partial_results_stabilization(true)
                    .partial_results_stability(partial_stability);
            }

            if show_speaker_label {
                request = request.show_speaker_label(true);
            }

            if let Some(vocab) = vocabulary_name {
                request = request.vocabulary_name(vocab);
            }

            if let Some(filter) = vocabulary_filter_name {
                request = request.vocabulary_filter_name(filter);
            }

            if let Some(sid) = session_id {
                request = request.session_id(sid);
            }

            // Create the audio stream from incoming chunks
            let audio_stream = async_stream::stream! {
                loop {
                    tokio::select! {
                        Some(audio_data) = audio_rx.recv() => {
                            // AWS SDK Blob requires Vec<u8>, so a copy is unavoidable here.
                            // The upstream send_audio already uses Bytes for zero-copy until this point.
                            // TODO: Monitor AWS SDK updates for Blob to accept &[u8] or Bytes directly.
                            let audio_event = AudioEvent::builder()
                                .audio_chunk(Blob::new(audio_data.to_vec()))
                                .build();
                            yield Ok(AudioStream::AudioEvent(audio_event));
                        }
                        _ = &mut shutdown_rx => {
                            debug!("Shutdown signal received, closing audio stream");
                            break;
                        }
                    }
                }
            };

            // Start the transcription stream
            match request.audio_stream(audio_stream.into()).send().await {
                Ok(output) => {
                    // Store session ID if provided
                    if let Some(sid) = output.session_id() {
                        *session_id_storage.write().await = Some(sid.to_string());
                        info!("Amazon Transcribe session started: {}", sid);
                    }

                    is_connected.store(true, Ordering::Release);
                    let _ = connected_tx.send(Ok(()));

                    // Process the transcript result stream
                    let mut result_stream = output.transcript_result_stream;
                    loop {
                        match result_stream.recv().await {
                            Ok(Some(event)) => {
                                match event {
                                    TranscriptResultStream::TranscriptEvent(transcript_event) => {
                                        if let Some(transcript) = transcript_event.transcript {
                                            for result in transcript.results.unwrap_or_default() {
                                                // Get the best transcription
                                                if let Some(alternatives) = result.alternatives
                                                    && let Some(alt) = alternatives.first()
                                                    && let Some(transcript_text) = &alt.transcript
                                                {
                                                    // Skip empty transcripts
                                                    if transcript_text.trim().is_empty() {
                                                        continue;
                                                    }

                                                    let is_partial = result.is_partial;

                                                    // Calculate confidence from items
                                                    let confidence = if let Some(items) = &alt.items
                                                    {
                                                        let confidences: Vec<f64> = items
                                                            .iter()
                                                            .filter_map(|item| item.confidence)
                                                            .collect();
                                                        if !confidences.is_empty() {
                                                            let sum: f64 = confidences.iter().sum();
                                                            (sum / confidences.len() as f64) as f32
                                                        } else {
                                                            0.0
                                                        }
                                                    } else {
                                                        0.0
                                                    };

                                                    let stt_result = STTResult::new(
                                                        transcript_text.clone(),
                                                        !is_partial,
                                                        !is_partial, // is_speech_final same as is_final for Transcribe
                                                        confidence,
                                                    );

                                                    if result_tx.send(stt_result).is_err() {
                                                        warn!(
                                                            "Failed to send STT result - channel closed"
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    _ => {
                                        debug!("Received unknown event type from Transcribe");
                                    }
                                }
                            }
                            Ok(None) => {
                                info!("Transcribe stream ended");
                                break;
                            }
                            Err(e) => {
                                let stt_error = STTError::ProviderError(format!(
                                    "Amazon Transcribe stream error: {}",
                                    e
                                ));
                                error!("{}", stt_error);
                                let _ = error_tx.send(stt_error);
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    let stt_error = STTError::ConnectionFailed(format!(
                        "Failed to start Amazon Transcribe stream: {}",
                        e
                    ));
                    error!("{}", stt_error);
                    let _ = connected_tx.send(Err(stt_error.clone()));
                    let _ = error_tx.send(stt_error);
                }
            }

            is_connected.store(false, Ordering::Release);
            info!("Amazon Transcribe connection closed");
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
                        "Received STT result but no callback registered: {}",
                        result.transcript
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
                    error!(
                        "STT streaming error but no error callback registered: {}",
                        error
                    );
                }
            }
        });
        self.error_forward_handle = Some(error_forward_handle);

        // Update state and wait for connection
        self.state = ConnectionState::Connecting;

        // Wait for connection result with timeout
        match tokio::time::timeout(Duration::from_secs(CONNECTION_TIMEOUT_SECS), connected_rx).await
        {
            Ok(Ok(Ok(()))) => {
                self.state = ConnectionState::Connected;
                self.state_notify.notify_waiters();
                info!("Successfully connected to Amazon Transcribe");
                Ok(())
            }
            Ok(Ok(Err(e))) => {
                self.state = ConnectionState::Error(e.to_string());
                Err(e)
            }
            Ok(Err(_)) => {
                let error_msg = "Connection channel closed unexpectedly".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
            Err(_) => {
                let error_msg = "Connection timeout".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
        }
    }
}

impl Default for AwsTranscribeSTT {
    fn default() -> Self {
        Self {
            config: None,
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            audio_tx: None,
            shutdown_tx: None,
            result_tx: None,
            error_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            is_connected: Arc::new(AtomicBool::new(false)),
            session_id: Arc::new(RwLock::new(None)),
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for AwsTranscribeSTT {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        // Validate sample rate
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&config.sample_rate) {
            return Err(STTError::ConfigurationError(format!(
                "Sample rate must be between {} and {} Hz, got {}",
                MIN_SAMPLE_RATE, MAX_SAMPLE_RATE, config.sample_rate
            )));
        }

        // Create AWS-specific configuration from base config
        let aws_config = AwsTranscribeSTTConfig {
            base: config.clone(),
            region: AwsRegion::from_str_or_default(
                std::env::var("AWS_REGION")
                    .unwrap_or_else(|_| "us-east-1".to_string())
                    .as_str(),
            ),
            aws_access_key_id: std::env::var("AWS_ACCESS_KEY_ID").ok(),
            aws_secret_access_key: std::env::var("AWS_SECRET_ACCESS_KEY").ok(),
            aws_session_token: std::env::var("AWS_SESSION_TOKEN").ok(),
            media_encoding: MediaEncoding::from_str_or_default(&config.encoding),
            enable_partial_results_stabilization: true,
            partial_results_stability: PartialResultsStability::High,
            show_speaker_label: false,
            max_speaker_labels: None,
            enable_channel_identification: false,
            number_of_channels: None,
            vocabulary_name: None,
            vocabulary_filter_name: None,
            vocabulary_filter_method: None,
            language_model_name: None,
            identify_language: false,
            preferred_language: Vec::new(),
            enable_content_redaction: false,
            content_redaction_types: Vec::new(),
            pii_entity_types: Vec::new(),
            session_id: None,
            chunk_duration_ms: DEFAULT_CHUNK_DURATION_MS,
        };

        Self::new_with_config(aws_config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        let config = self.config.as_ref().ok_or_else(|| {
            STTError::ConfigurationError("No configuration available".to_string())
        })?;

        self.start_connection(config.clone()).await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Send shutdown signal
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Wait for connection task to finish
        if let Some(handle) = self.connection_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }

        // Clean up forwarding tasks
        if let Some(handle) = self.result_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        if let Some(handle) = self.error_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        // Clean up channels and state
        self.audio_tx = None;
        self.result_tx = None;
        self.error_tx = None;
        *self.result_callback.lock().await = None;
        *self.error_callback.lock().await = None;
        *self.session_id.write().await = None;
        self.is_connected.store(false, Ordering::Release);

        // Update state
        self.state = ConnectionState::Disconnected;
        self.state_notify.notify_waiters();

        info!("Disconnected from Amazon Transcribe");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        matches!(self.state, ConnectionState::Connected)
            && self.audio_tx.is_some()
            && self.is_connected.load(Ordering::Acquire)
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to Amazon Transcribe".to_string(),
            ));
        }

        // Validate chunk size
        if audio_data.len() > MAX_AUDIO_CHUNK_SIZE {
            return Err(STTError::InvalidAudioFormat(format!(
                "Audio chunk size {} exceeds maximum allowed size of {} bytes",
                audio_data.len(),
                MAX_AUDIO_CHUNK_SIZE
            )));
        }

        if let Some(audio_tx) = &self.audio_tx {
            let data_len = audio_data.len();

            // Send audio data with backpressure handling (zero-copy via Bytes)
            audio_tx
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio data: {}", e)))?;

            debug!("Sent {} bytes of audio data to Amazon Transcribe", data_len);
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
        // For Amazon Transcribe, we need to reconnect to update configuration
        if self.is_ready() {
            self.disconnect().await?;
        }

        // Create new AWS config from base config
        let aws_config = AwsTranscribeSTTConfig {
            base: config.clone(),
            region: self.config.as_ref().map(|c| c.region).unwrap_or_default(),
            aws_access_key_id: self
                .config
                .as_ref()
                .and_then(|c| c.aws_access_key_id.clone()),
            aws_secret_access_key: self
                .config
                .as_ref()
                .and_then(|c| c.aws_secret_access_key.clone()),
            aws_session_token: self
                .config
                .as_ref()
                .and_then(|c| c.aws_session_token.clone()),
            media_encoding: MediaEncoding::from_str_or_default(&config.encoding),
            ..self.config.clone().unwrap_or_default()
        };

        self.config = Some(aws_config);
        self.connect().await?;
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Amazon Transcribe Streaming"
    }
}

impl Drop for AwsTranscribeSTT {
    fn drop(&mut self) {
        // Send shutdown signal if still connected
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_aws_transcribe_creation() {
        let config = STTConfig {
            provider: "aws-transcribe".to_string(),
            api_key: String::new(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm".to_string(),
            model: String::new(),
        };

        let stt = AwsTranscribeSTT::new(config).unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), "Amazon Transcribe Streaming");
    }

    #[tokio::test]
    async fn test_aws_transcribe_invalid_sample_rate() {
        let config = STTConfig {
            provider: "aws-transcribe".to_string(),
            api_key: String::new(),
            language: "en-US".to_string(),
            sample_rate: 4000, // Too low
            channels: 1,
            punctuation: true,
            encoding: "pcm".to_string(),
            model: String::new(),
        };

        let result = AwsTranscribeSTT::new(config);
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("Sample rate"));
        }
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = STTConfig {
            provider: "aws-transcribe".to_string(),
            api_key: String::new(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm".to_string(),
            model: String::new(),
        };

        let mut stt = AwsTranscribeSTT::new(config).unwrap();
        let audio_data: Bytes = vec![0u8; 1024].into();

        let result = stt.send_audio(audio_data).await;
        assert!(result.is_err());
        if let Err(STTError::ConnectionFailed(msg)) = result {
            assert!(msg.contains("Not connected"));
        }
    }

    #[tokio::test]
    async fn test_language_code_conversion() {
        assert_eq!(
            AwsTranscribeSTT::convert_language_code("en-US"),
            Some(LanguageCode::EnUs)
        );
        assert_eq!(
            AwsTranscribeSTT::convert_language_code("EN-US"),
            Some(LanguageCode::EnUs)
        );
        assert_eq!(
            AwsTranscribeSTT::convert_language_code("ja-JP"),
            Some(LanguageCode::JaJp)
        );
        // Unknown code should default to en-US
        assert_eq!(
            AwsTranscribeSTT::convert_language_code("unknown"),
            Some(LanguageCode::EnUs)
        );
    }

    #[tokio::test]
    async fn test_media_encoding_conversion() {
        assert!(matches!(
            AwsTranscribeSTT::convert_media_encoding(&MediaEncoding::Pcm),
            AwsMediaEncoding::Pcm
        ));
        assert!(matches!(
            AwsTranscribeSTT::convert_media_encoding(&MediaEncoding::Flac),
            AwsMediaEncoding::Flac
        ));
        assert!(matches!(
            AwsTranscribeSTT::convert_media_encoding(&MediaEncoding::OggOpus),
            AwsMediaEncoding::OggOpus
        ));
    }

    #[tokio::test]
    async fn test_get_session_id_initially_none() {
        let config = STTConfig {
            provider: "aws-transcribe".to_string(),
            api_key: String::new(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm".to_string(),
            model: String::new(),
        };

        let stt = AwsTranscribeSTT::new(config).unwrap();
        assert!(stt.get_session_id().await.is_none());
    }
}
