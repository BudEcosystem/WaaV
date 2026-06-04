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

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aws_config::BehaviorVersion;
use aws_sdk_transcribestreaming::Client as TranscribeClient;
use aws_sdk_transcribestreaming::operation::start_stream_transcription::builders::StartStreamTranscriptionInputBuilder;
use aws_sdk_transcribestreaming::types::{
    AudioEvent, AudioStream, ContentIdentificationType, ContentRedactionType, LanguageCode,
    MediaEncoding as AwsMediaEncoding, PartialResultsStability as AwsPartialResultsStability,
    TranscriptResultStream,
};
use aws_smithy_types::Blob;
use tokio::sync::{Mutex, Notify, RwLock, mpsc, oneshot};
use tokio::time::timeout;

use super::config::{
    AwsRegion, AwsTranscribeSTTConfig, DEFAULT_CHUNK_DURATION_MS, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE,
    MediaEncoding, PartialResultsStability,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
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

/// Per-message idle timeout for transcript stream reception.
/// Resets after each successful message. Catches stuck/dead connections.
const STREAM_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

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
// Supervised transport (W-D1 production adoption)
// =============================================================================

/// A [`WsTransport`] that adapts Amazon Transcribe's streaming result loop to the generic
/// [`ReconnectableStream`] supervisor (W-D1 fleet adoption). One is built per (re)connect by the
/// supervisor's `connect` closure.
///
/// Unlike WebSocket providers, Amazon Transcribe streaming is a single bidirectional HTTP/2
/// request: the featured session (language, diarization, redaction, partials, …) is baked into the
/// connect `StartStreamTranscriptionInput`, so [`restore_session`](WsTransport::restore_session) is
/// a no-op — every (re)connect dials a fresh, fully-featured request. [`run`](WsTransport::run) IS
/// the original transcript-result receiver loop, now returning a [`ReconnectOutcome`] so a transport
/// drop reconnects instead of ending the session.
struct AwsTranscribeTransport {
    /// This connection's OWN result receiver (owned outright, dropped with the transport).
    /// (`event_receiver` is a private module; the public re-export is via `primitives::event_stream`.)
    result_stream: aws_sdk_transcribestreaming::primitives::event_stream::EventReceiver<
        aws_sdk_transcribestreaming::types::TranscriptResultStream,
        aws_sdk_transcribestreaming::types::error::TranscriptResultStreamError,
    >,
    result_tx: mpsc::Sender<STTResult>,
    error_tx: mpsc::Sender<STTError>,
}

#[async_trait]
impl WsTransport for AwsTranscribeTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // The full featured session is baked into the connect `StartStreamTranscriptionInput`, so a
        // (re)connect already dials a fully-featured request — nothing to re-send here.
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        loop {
            match timeout(STREAM_MESSAGE_TIMEOUT, self.result_stream.recv()).await {
                Ok(Ok(Some(event))) => {
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
                                        let confidence = if let Some(items) = &alt.items {
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

                                        if self.result_tx.try_send(stt_result).is_err() {
                                            warn!("Failed to send STT result - channel closed");
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
                Ok(Ok(None)) => {
                    // The input audio stream ended (the client dropped the sender). On an INTENTIONAL
                    // disconnect the supervisor's loop-top/post-run guard observes the shared flag and
                    // completes WITHOUT reconnecting; otherwise this is a mid-stream drop to recover.
                    return ReconnectOutcome::Reconnectable(StreamError::new(
                        "Amazon Transcribe stream ended",
                    ));
                }
                Ok(Err(e)) => {
                    let _ = self.error_tx.try_send(STTError::ProviderError(format!(
                        "Amazon Transcribe stream error: {e}"
                    )));
                    return ReconnectOutcome::Reconnectable(StreamError::new(format!(
                        "stream error: {e}"
                    )));
                }
                Err(_elapsed) => {
                    let _ = self.error_tx.try_send(STTError::NetworkError(
                        "Transcribe idle timeout - no message for 60 seconds".into(),
                    ));
                    return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                }
            }
        }
    }
}

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

    /// Shared audio sender SLOT. Each (re)connect installs a FRESH `mpsc::Sender<Bytes>` here,
    /// dropping the previous one (which closes the previous connection's receiver → its audio input
    /// stream ends → the old HTTP/2 request finalizes via channel-close, NOT via receiver-drop —
    /// sidestepping the AWS SDK's input-driving-task-vs-result-receiver-drop uncertainty). The
    /// receiver lives privately inside each transport's `async_stream`, so there is no shared
    /// `Mutex<Receiver>` to deadlock on across reconnect.
    audio_tx_slot: Arc<RwLock<Option<mpsc::Sender<Bytes>>>>,

    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before dropping the active sender, so a client close
    /// racing a server-side close can never trigger a spurious reconnect (the supervisor's loop-top
    /// guard observes this same `Arc<AtomicBool>`).
    intentional_disconnect: Arc<AtomicBool>,

    /// Shutdown signal sender (legacy; the channel-swap drop of `audio_tx_slot` is now the shutdown
    /// mechanism — this is fired in `disconnect()`/`Drop` but is no longer load-bearing).
    shutdown_tx: Option<oneshot::Sender<()>>,

    /// Result channel sender for internal forwarding.
    result_tx: Option<mpsc::Sender<STTResult>>,

    /// Error channel sender for internal forwarding.
    error_tx: Option<mpsc::Sender<STTError>>,

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

    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven by
    /// the generic [`ReconnectableStream`](crate::core::websocket::ReconnectableStream) supervisor.
    /// `None` before `set_resilience` (a direct unit-test construction) → the supervisor uses its
    /// own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,

    /// Optional injected AWS `HttpClient` (proxy / custom TLS / in-process test connector). When
    /// `Some`, it overrides the default hyper client used for the transcribe-streaming connection
    /// (applied to the `aws_config` loader in `start_connection`). `None` → SDK default hyper client.
    http_client: Option<aws_smithy_runtime_api::client::http::SharedHttpClient>,
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
            audio_tx_slot: Arc::new(RwLock::new(None)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
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
            resilience: None,
            http_client: None,
        })
    }

    /// W1 keystone — construct directly from the standardized config so the advanced features
    /// Amazon Transcribe can express (speaker diarization, content/PII redaction, partial-results
    /// stabilization) are honored END-TO-END. The flat `BaseSTT::new` path hardcodes diarization
    /// and redaction off; this is the reachable standardized path.
    ///
    /// Amazon Transcribe authenticates with AWS credentials (env / credentials file / IAM role),
    /// NOT an `api_key`, so this does not require `base.api_key` — credentials and region are
    /// resolved from the environment exactly as the flat `BaseSTT::new` path does.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let mut aws_config = AwsTranscribeSTTConfig::from_standard(std);
        // Resolve credentials/region from the environment, but ONLY when the env var is actually set
        // — otherwise keep what `from_standard` already pulled from the standardized config/extras
        // (env was clobbering explicit extras credentials with `None`).
        if let Ok(region) = std::env::var("AWS_REGION") {
            aws_config.region = AwsRegion::from_str_or_default(&region);
        }
        if let Ok(k) = std::env::var("AWS_ACCESS_KEY_ID") {
            aws_config.aws_access_key_id = Some(k);
        }
        if let Ok(k) = std::env::var("AWS_SECRET_ACCESS_KEY") {
            aws_config.aws_secret_access_key = Some(k);
        }
        if let Ok(k) = std::env::var("AWS_SESSION_TOKEN") {
            aws_config.aws_session_token = Some(k);
        }
        aws_config.media_encoding = MediaEncoding::from_str_or_default(&std.base.encoding);
        Self::new_with_config(aws_config)
    }

    /// Get the current session ID.
    pub async fn get_session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// Inject a custom AWS `HttpClient` (proxy / custom TLS / in-process test connector).
    /// When set, it overrides the default hyper client used for the transcribe-streaming connection.
    pub fn with_http_client(
        mut self,
        http_client: aws_smithy_runtime_api::client::http::SharedHttpClient,
    ) -> Self {
        self.http_client = Some(http_client);
        self
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

    /// Apply the provider config to a `StartStreamTranscriptionInput` builder — the ACTUAL AWS SDK
    /// request object whose fields are serialized 1:1 to the `x-amzn-transcribe-*` request headers.
    ///
    /// This is the single source of truth for request-parameter wiring: both the live
    /// `start_connection` path (which attaches the audio stream and `send_with`s it) and the
    /// wire-level tests (which assert on the builder's `get_*` accessors) go through here, so a
    /// param can never be set on our config yet silently dropped from the request — the recurring
    /// bug class. (The audio stream is attached separately by the caller.)
    fn apply_request_params(
        config: &AwsTranscribeSTTConfig,
        mut input: StartStreamTranscriptionInputBuilder,
    ) -> StartStreamTranscriptionInputBuilder {
        input = input
            .media_sample_rate_hertz(config.base.sample_rate as i32)
            .media_encoding(Self::convert_media_encoding(&config.media_encoding));

        // Language vs. (single/multiple) language identification.
        if config.identify_multiple_languages {
            // Multi-language (code-switching) identification — header
            // x-amzn-transcribe-identify-multiple-languages. Mutually exclusive with a fixed
            // language_code, so we do NOT set language_code in this branch.
            input = input.identify_multiple_languages(true);
        } else if config.identify_language {
            input = input.identify_language(true);
        } else if let Some(lang) = Self::convert_language_code(&config.base.language) {
            input = input.language_code(lang);
        }

        // Candidate language list for language-ID mode — header x-amzn-transcribe-language-options.
        if !config.language_options.is_empty() {
            input = input.language_options(config.language_options.join(","));
        }

        // Custom vocabularies/filters for language-ID mode (comma-separated, one per language) —
        // headers x-amzn-transcribe-vocabulary-names / -vocabulary-filter-names. Distinct from the
        // single-language vocabulary_name / vocabulary_filter_name below.
        if let Some(v) = &config.vocabulary_names {
            input = input.vocabulary_names(v.clone());
        }
        if let Some(v) = &config.vocabulary_filter_names {
            input = input.vocabulary_filter_names(v.clone());
        }

        // Session resume window (minutes) — header x-amzn-transcribe-session-resume-window.
        if let Some(w) = config.session_resume_window {
            input = input.session_resume_window(w);
        }

        // PII content IDENTIFICATION (flag, not redact) — header
        // x-amzn-transcribe-content-identification-type=PII. Distinct from content REDACTION below;
        // AWS rejects both being set at once, so identification wins when requested.
        if config.enable_content_identification {
            input = input.content_identification_type(ContentIdentificationType::Pii);
        } else if config.enable_content_redaction {
            input = input.content_redaction_type(ContentRedactionType::Pii);
            if !config.pii_entity_types.is_empty() {
                input = input.pii_entity_types(config.pii_entity_types.join(","));
            }
        }

        // Partial-results stabilization.
        if config.enable_partial_results_stabilization {
            input = input
                .enable_partial_results_stabilization(true)
                .partial_results_stability(Self::convert_partial_results_stability(
                    &config.partial_results_stability,
                ));
        }

        // Speaker diarization.
        if config.show_speaker_label {
            input = input.show_speaker_label(true);
        }

        // Single-language custom vocabulary / filter.
        if let Some(vocab) = &config.vocabulary_name {
            input = input.vocabulary_name(vocab.clone());
        }
        if let Some(filter) = &config.vocabulary_filter_name {
            input = input.vocabulary_filter_name(filter.clone());
        }

        // Session id (resume token).
        if let Some(sid) = &config.session_id {
            input = input.session_id(sid.clone());
        }

        input
    }

    /// Start the transcription stream connection.
    async fn start_connection(&mut self, config: AwsTranscribeSTTConfig) -> Result<(), STTError> {
        // Bounded channels for backpressure - 256 should handle bursts while preventing memory exhaustion
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(256);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(64);
        // Resolves on the FIRST successful (re)connect only (Arc<Mutex<Option<..>>> so reconnect
        // attempts don't re-fire it); resolves with Err only if the supervisor exits before ever
        // connecting (exhausted / circuit-open on the very first dial).
        let (connected_tx, connected_rx) = oneshot::channel::<Result<(), STTError>>();
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));

        // Store channels (the slot is filled per (re)connect by the connect closure).
        self.result_tx = Some(result_tx.clone());
        self.error_tx = Some(error_tx.clone());

        // Fresh session: clear any intent left over from a prior disconnect so the supervisor does
        // not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        // Clone data needed for the connection task
        let region_str = config.region.as_str().to_string();

        let is_connected = self.is_connected.clone();
        let session_id_storage = self.session_id.clone();
        let audio_tx_slot = Arc::clone(&self.audio_tx_slot);

        let aws_access_key_id = config.aws_access_key_id.clone();
        let aws_secret_access_key = config.aws_secret_access_key.clone();
        let aws_session_token = config.aws_session_token.clone();
        // Raw endpoint base (e.g. https://127.0.0.1:PORT for a mock e2e harness). The SDK appends
        // the operation path; honored on BOTH the explicit-creds and default-chain loader branches.
        let endpoint_override = config.endpoint_override.clone();
        // Optional injected HttpClient (proxy / custom TLS / in-process test connector). When set,
        // applied to the loader below so it overrides the SDK default hyper client.
        let http_client = self.http_client.clone();
        // The full provider config drives request-parameter wiring via `apply_request_params`
        // (the single source of truth shared with the wire-level tests).
        let request_config = config.clone();

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor with
        // the shared process-global handles from CoreState (W-D1/W-D2 fleet adoption). When no
        // handles were injected, the supervisor uses its own per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        let resilience = self.resilience.clone();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);

        // Spawn ONE supervisor task: it builds the AWS client ONCE, then the supervisor's connect
        // closure dials a fresh HTTP/2 streaming request on every (re)connect, each owning its own
        // audio receiver (channel-swap — see `audio_tx_slot` doc).
        let connection_handle = tokio::spawn(async move {
            // Build AWS config + client ONCE (async cred load); reused across reconnect attempts.
            // Compute the explicit credentials (if any) first, then a SINGLE loader so the endpoint
            // override is honored uniformly whether credentials are explicit or come from the
            // default chain (env vars, IAM roles, etc.).
            let explicit_credentials =
                if aws_access_key_id.is_some() && aws_secret_access_key.is_some() {
                    Some(aws_credential_types::Credentials::new(
                        aws_access_key_id.as_deref().unwrap_or_default(),
                        aws_secret_access_key.as_deref().unwrap_or_default(),
                        aws_session_token,
                        None, // Expiration
                        "waav-gateway",
                    ))
                } else {
                    None
                };

            let mut loader = aws_config::defaults(BehaviorVersion::latest())
                .region(aws_config::Region::new(region_str));
            if let Some(creds) = explicit_credentials {
                loader = loader.credentials_provider(creds);
            }
            if let Some(ep) = endpoint_override.as_deref() {
                loader = loader.endpoint_url(ep);
            }
            if let Some(c) = http_client.clone() {
                loader = loader.http_client(c);
            }
            let aws_config = loader.load().await;

            let client = TranscribeClient::new(&aws_config);

            let supervisor = match resilience {
                Some(r) => ReconnectableStream::with_breaker_and_governor(
                    ReconnectableStreamConfig::new("aws_transcribe", reconnection),
                    r.breaker,
                    (*r.governor).clone(),
                ),
                None => ReconnectableStream::new(ReconnectableStreamConfig::new(
                    "aws_transcribe",
                    reconnection,
                )),
            }
            .with_disconnect_flag(disconnect_flag);

            let exit = supervisor
                .run(|| {
                    let client = client.clone();
                    let request_config = request_config.clone();
                    let audio_tx_slot = Arc::clone(&audio_tx_slot);
                    let session_id_storage = Arc::clone(&session_id_storage);
                    let is_connected = is_connected.clone();
                    let connected_tx = Arc::clone(&connected_tx);
                    let result_tx = result_tx.clone();
                    let error_tx = error_tx.clone();
                    async move {
                        // Channel-swap: each (re)connect owns a FRESH receiver. Installing the new
                        // sender here DROPS the previous one, closing the previous connection's
                        // receiver → its audio stream ends → the old HTTP/2 request finalizes via
                        // channel-close (not via receiver-drop) — sidestepping the SDK's
                        // input-task-vs-result-drop uncertainty. No Mutex, no held guard.
                        let (audio_tx, mut audio_rx) =
                            mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER_SIZE);
                        *audio_tx_slot.write().await = Some(audio_tx);

                        // This connection's audio INPUT stream owns its receiver outright and is
                        // dropped with this transport.
                        let audio_stream = async_stream::stream! {
                            while let Some(audio_data) = audio_rx.recv().await {
                                // AWS SDK Blob requires Vec<u8>, so a copy is unavoidable here.
                                // The upstream send_audio already uses Bytes for zero-copy until this point.
                                let audio_event = AudioEvent::builder()
                                    .audio_chunk(Blob::new(audio_data.to_vec()))
                                    .build();
                                yield Ok(AudioStream::AudioEvent(audio_event));
                            }
                        };

                        // Build the streaming request via the single shared param-wiring helper, so
                        // every configured feature reaches the actual SDK
                        // `StartStreamTranscriptionInput` (and thus the `x-amzn-transcribe-*`
                        // request headers) — no per-field drift between the live path and the
                        // wire-level tests.
                        let input_builder = AwsTranscribeSTT::apply_request_params(
                            &request_config,
                            aws_sdk_transcribestreaming::operation::start_stream_transcription::StartStreamTranscriptionInput::builder(),
                        );

                        let output = input_builder
                            .audio_stream(audio_stream.into())
                            .send_with(&client)
                            .await
                            .map_err(|e| {
                                StreamError::new(format!(
                                    "Failed to start Amazon Transcribe stream: {e}"
                                ))
                            })?;

                        // Store session ID if provided
                        if let Some(sid) = output.session_id() {
                            *session_id_storage.write().await = Some(sid.to_string());
                            info!("Amazon Transcribe session started: {}", sid);
                        }

                        is_connected.store(true, Ordering::Release);
                        // Resolve the waiting connect() exactly once (first connect only).
                        if let Some(tx) = connected_tx.lock().await.take() {
                            let _ = tx.send(Ok(()));
                        }

                        Ok(AwsTranscribeTransport {
                            result_stream: output.transcript_result_stream,
                            result_tx,
                            error_tx,
                        })
                    }
                })
                .await;

            is_connected.store(false, Ordering::Release);
            // If we never connected (exhausted/circuit-open on the very first dial), the connect
            // signal is still pending — resolve it as a failure so connect() doesn't hang.
            if let Some(tx) = connected_tx.lock().await.take() {
                let _ = tx.send(Err(STTError::ConnectionFailed(format!(
                    "Amazon Transcribe supervisor exited: {exit:?}"
                ))));
            }
            info!("Amazon Transcribe connection closed (supervisor exit: {exit:?})");
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
            audio_tx_slot: Arc::new(RwLock::new(None)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
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
            resilience: None,
            http_client: None,
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
            language_options: Vec::new(),
            identify_multiple_languages: false,
            vocabulary_names: None,
            vocabulary_filter_names: None,
            session_resume_window: None,
            enable_content_identification: false,
            enable_content_redaction: false,
            content_redaction_types: Vec::new(),
            pii_entity_types: Vec::new(),
            session_id: None,
            chunk_duration_ms: DEFAULT_CHUNK_DURATION_MS,
            endpoint_override: None,
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
        // Record the intent BEFORE any guard so the supervisor sees it even if the transport's
        // run() just reported a reconnectable drop (the disconnect-vs-stream-end race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);

        // Drop the active sender: ends this connection's audio input stream → the HTTP/2 request
        // EOS-finalizes → run() returns Reconnectable → the supervisor sees the intentional flag
        // and Completes WITHOUT reconnecting. This is now the shutdown mechanism.
        *self.audio_tx_slot.write().await = None;

        // Legacy shutdown signal (no longer load-bearing; fired for harmlessness).
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Wait for the supervisor task to finish
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

        // Read the current sender from the slot. During the brief reconnect window the slot may be
        // None or its sender closed → treat a send failure as a dropped chunk (losing in-flight
        // audio across a reconnect is inherent and expected, matching the other providers).
        let slot = self.audio_tx_slot.read().await;
        match slot.as_ref() {
            Some(tx) => {
                let data_len = audio_data.len();
                if tx.try_send(audio_data).is_err() {
                    debug!("aws_transcribe: audio chunk dropped (reconnecting or backpressure)");
                } else {
                    debug!("Sent {} bytes of audio data to Amazon Transcribe", data_len);
                }
                Ok(())
            }
            None => {
                debug!("aws_transcribe: no active audio sender (reconnect window), chunk dropped");
                Ok(())
            }
        }
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

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `start_connection` drives the generic
        // ReconnectableStream supervisor with them — every Amazon Transcribe session trips the same
        // breaker and shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

impl Drop for AwsTranscribeSTT {
    fn drop(&mut self) {
        // Record intent so a still-running supervisor never reconnects after we're gone.
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        // Legacy shutdown signal (harmless).
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        // Tear down the supervisor task (a sync Drop cannot async-clear the audio slot).
        if let Some(handle) = self.connection_handle.take() {
            handle.abort();
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

    // W1 keystone: standardized advanced features Amazon Transcribe supports (speaker
    // diarization + content/PII redaction) survive through `new_standard` into the provider
    // config — both were hardcoded off on the flat path.
    #[test]
    fn test_new_standard_unlocks_diarization_and_redaction() {
        use crate::core::stt::standard::{SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "aws-transcribe".to_string(),
                api_key: String::new(), // AWS uses credentials, not api_key
                language: "en-US".to_string(),
                sample_rate: 16000,
                channels: 1,
                punctuation: true,
                encoding: "pcm".to_string(),
                model: String::new(),
            },
            features: SttFeatures {
                diarization: Some(true),
                redaction: Some(vec!["NAME".into(), "PHONE".into()]),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let stt = AwsTranscribeSTT::new_standard(&std).unwrap();
        let cfg = stt.config.as_ref().unwrap();
        assert!(cfg.show_speaker_label);
        assert_eq!(cfg.max_speaker_labels, Some(10));
        assert!(cfg.enable_content_redaction);
        assert_eq!(cfg.pii_entity_types, vec!["NAME", "PHONE"]);
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

    // =========================================================================
    // WIRE-LEVEL tests: assert the api_param reaches the actual AWS SDK request
    // object (`StartStreamTranscriptionInput` builder) — whose fields the SDK
    // serializes 1:1 into the documented `x-amzn-transcribe-*` request headers.
    // This is materially stronger than asserting on our own config struct (the
    // recurring "set on config, dropped from the request" bug class): we exercise
    // the exact same `apply_request_params` the live `start_connection` path uses.
    // =========================================================================

    use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
    use aws_sdk_transcribestreaming::operation::start_stream_transcription::StartStreamTranscriptionInput;

    /// Build the SDK request-input builder the way the live path does, from a standardized config.
    fn wire_input(
        features: SttFeatures,
        extras: serde_json::Map<String, serde_json::Value>,
    ) -> aws_sdk_transcribestreaming::operation::start_stream_transcription::builders::StartStreamTranscriptionInputBuilder
    {
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "aws-transcribe".into(),
                api_key: String::new(),
                language: "en-US".into(),
                sample_rate: 16000,
                channels: 1,
                punctuation: true,
                encoding: "pcm".into(),
                model: String::new(),
            },
            features,
            extras: ProviderExtras(extras),
        };
        let cfg = AwsTranscribeSTTConfig::from_standard(&std);
        AwsTranscribeSTT::apply_request_params(&cfg, StartStreamTranscriptionInput::builder())
    }

    /// LanguageOptions (extras) → x-amzn-transcribe-language-options. Accepts list OR csv string.
    #[test]
    fn language_options_reaches_the_request() {
        // Not requested → absent.
        let b = wire_input(SttFeatures::default(), serde_json::Map::new());
        assert!(b.get_language_options().is_none());

        // Array form.
        let mut ex = serde_json::Map::new();
        ex.insert("language_options".into(), serde_json::json!(["en-US", "es-US"]));
        ex.insert("identify_multiple_languages".into(), serde_json::json!(true));
        let b = wire_input(SttFeatures::default(), ex);
        assert_eq!(
            b.get_language_options().as_deref(),
            Some("en-US,es-US"),
            "LanguageOptions must reach the request as a comma-separated header value"
        );

        // CSV-string form is accepted too.
        let mut ex = serde_json::Map::new();
        ex.insert("language_options".into(), serde_json::json!("en-US, fr-FR"));
        let b = wire_input(SttFeatures::default(), ex);
        assert_eq!(b.get_language_options().as_deref(), Some("en-US,fr-FR"));
    }

    /// IdentifyMultipleLanguages (extras) → x-amzn-transcribe-identify-multiple-languages, and it
    /// suppresses language_code (mutually exclusive on the wire).
    #[test]
    fn identify_multiple_languages_reaches_the_request() {
        let b = wire_input(SttFeatures::default(), serde_json::Map::new());
        assert_eq!(b.get_identify_multiple_languages(), &None);
        // language_code is set in the default (single-language) path.
        assert!(b.get_language_code().is_some());

        let mut ex = serde_json::Map::new();
        ex.insert("identify_multiple_languages".into(), serde_json::json!(true));
        let b = wire_input(SttFeatures::default(), ex);
        assert_eq!(
            b.get_identify_multiple_languages(),
            &Some(true),
            "IdentifyMultipleLanguages must reach the request"
        );
        // Mutually exclusive with a fixed language_code — it must NOT be set.
        assert!(
            b.get_language_code().is_none(),
            "language_code must be suppressed when identifying multiple languages"
        );
    }

    /// VocabularyNames (extras) → x-amzn-transcribe-vocabulary-names (language-ID mode).
    #[test]
    fn vocabulary_names_reaches_the_request() {
        let b = wire_input(SttFeatures::default(), serde_json::Map::new());
        assert!(b.get_vocabulary_names().is_none());

        let mut ex = serde_json::Map::new();
        ex.insert("vocabulary_names".into(), serde_json::json!("medical-en,medical-es"));
        let b = wire_input(SttFeatures::default(), ex);
        assert_eq!(
            b.get_vocabulary_names().as_deref(),
            Some("medical-en,medical-es"),
            "VocabularyNames must reach the request"
        );
    }

    /// VocabularyFilterNames (extras) → x-amzn-transcribe-vocabulary-filter-names (language-ID).
    #[test]
    fn vocabulary_filter_names_reaches_the_request() {
        let b = wire_input(SttFeatures::default(), serde_json::Map::new());
        assert!(b.get_vocabulary_filter_names().is_none());

        let mut ex = serde_json::Map::new();
        ex.insert("vocabulary_filter_names".into(), serde_json::json!("filt-en,filt-es"));
        let b = wire_input(SttFeatures::default(), ex);
        assert_eq!(
            b.get_vocabulary_filter_names().as_deref(),
            Some("filt-en,filt-es"),
            "VocabularyFilterNames must reach the request"
        );
    }

    /// SessionResumeWindow (extras) → x-amzn-transcribe-session-resume-window (minutes).
    #[test]
    fn session_resume_window_reaches_the_request() {
        let b = wire_input(SttFeatures::default(), serde_json::Map::new());
        assert_eq!(b.get_session_resume_window(), &None);

        let mut ex = serde_json::Map::new();
        ex.insert("session_resume_window".into(), serde_json::json!(60));
        let b = wire_input(SttFeatures::default(), ex);
        assert_eq!(
            b.get_session_resume_window(),
            &Some(60),
            "SessionResumeWindow must reach the request"
        );
    }

    /// ContentIdentificationType (extras) → x-amzn-transcribe-content-identification-type=PII
    /// (FLAG mode, distinct from redaction). When identification is on, redaction must be off
    /// (AWS rejects both at once).
    #[test]
    fn content_identification_reaches_the_request_and_excludes_redaction() {
        let b = wire_input(SttFeatures::default(), serde_json::Map::new());
        assert!(b.get_content_identification_type().is_none());

        let mut ex = serde_json::Map::new();
        ex.insert("content_identification_type".into(), serde_json::json!("PII"));
        // Even if redaction is ALSO requested, identification wins and redaction stays off.
        let b = wire_input(
            SttFeatures {
                redaction: Some(vec!["NAME".into()]),
                ..Default::default()
            },
            ex,
        );
        assert_eq!(
            b.get_content_identification_type(),
            &Some(ContentIdentificationType::Pii),
            "ContentIdentificationType=PII must reach the request"
        );
        assert!(
            b.get_content_redaction_type().is_none(),
            "redaction must NOT be set alongside content identification (AWS rejects both)"
        );
    }

    /// KEYSTONE: all six extras-driven features land on a single request input together.
    #[test]
    fn from_standard_all_six_streaming_features_reach_the_request() {
        let mut ex = serde_json::Map::new();
        ex.insert("language_options".into(), serde_json::json!(["en-US", "es-US"]));
        ex.insert("identify_multiple_languages".into(), serde_json::json!(true));
        ex.insert("vocabulary_names".into(), serde_json::json!("v-en,v-es"));
        ex.insert("vocabulary_filter_names".into(), serde_json::json!("f-en,f-es"));
        ex.insert("session_resume_window".into(), serde_json::json!(45));
        ex.insert("content_identification_type".into(), serde_json::json!("PII"));

        let b = wire_input(SttFeatures::default(), ex);
        assert_eq!(b.get_language_options().as_deref(), Some("en-US,es-US"));
        assert_eq!(b.get_identify_multiple_languages(), &Some(true));
        assert_eq!(b.get_vocabulary_names().as_deref(), Some("v-en,v-es"));
        assert_eq!(b.get_vocabulary_filter_names().as_deref(), Some("f-en,f-es"));
        assert_eq!(b.get_session_resume_window(), &Some(45));
        assert_eq!(
            b.get_content_identification_type(),
            &Some(ContentIdentificationType::Pii)
        );
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

    // W-D1: disconnect() must record intent on the supervisor-shared flag so a client close racing
    // a server-side close (here: the input-stream-ended → Reconnectable race) can never trigger a
    // spurious reconnect (the supervisor's loop-top guard observes this same `Arc<AtomicBool>`).
    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
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
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the supervisor-shared intentional-disconnect flag",
        );
    }

    // The supervised provider still constructs cleanly and reports not-ready before connect (the
    // transport-level `run` mapping of idle-timeout/stream-end to Reconnectable is exercised only
    // against a live AWS HTTP/2 stream, so we keep coverage here at the construction/readiness
    // boundary rather than inventing a fake AWS stream).
    #[tokio::test]
    async fn provider_constructs_and_is_not_ready_before_connect() {
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
        assert!(!stt.is_connected.load(Ordering::Acquire));
    }
}
