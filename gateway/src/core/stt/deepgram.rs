use bytes::Bytes;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;

use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{Instant, interval, timeout};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, warn};

use super::base::{BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback};
use crate::core::resilience::{CircuitBreaker, ReconnectGovernor};
use crate::core::websocket::{ReconnectionConfig, ReconnectionManager};

/// Why the Deepgram inner event loop returned — drives the outer reconnect loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeepgramInnerOutcome {
    /// Client-initiated close (shutdown/CloseStream). Stop; do not reconnect.
    Intentional,
    /// Transport-level failure (socket reset, idle timeout, server EOF). Reconnect to
    /// preserve the session — finals after the drop are recovered on the new connection.
    Reconnect,
    /// A provider `Error` frame (bad config/auth). Stop; reconnecting would fail identically.
    Fatal,
}

/// Extract the `host[:port]` authority from a `ws://`/`wss://` base URL, for the `Host`
/// header when an `endpoint_override` is in effect.
fn ws_authority(base: &str) -> Option<String> {
    let rest = base
        .strip_prefix("wss://")
        .or_else(|| base.strip_prefix("ws://"))
        .or_else(|| base.strip_prefix("https://"))
        .or_else(|| base.strip_prefix("http://"))
        .unwrap_or(base);
    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    if authority.is_empty() {
        None
    } else {
        Some(authority.to_string())
    }
}

/// Type alias for the complex callback function type
type AsyncSTTCallback = Box<
    dyn Fn(STTResult) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Type alias for the error callback function type
type AsyncErrorCallback = Box<
    dyn Fn(STTError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Per-message idle timeout for WebSocket message reception.
/// Resets after each successful message. Catches stuck/dead connections.
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// Simplified connection state with atomic updates
#[derive(Debug, Clone)]
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    #[allow(dead_code)] // We store error details for debugging but don't actively use them
    Error(String),
}

/// Configuration specific to Deepgram STT
#[derive(Debug, Clone)]
pub struct DeepgramSTTConfig {
    /// Base STT configuration
    pub base: STTConfig,
    /// Enable speaker diarization
    pub diarize: bool,
    /// Enable interim results
    pub interim_results: bool,
    /// Enable filler words detection
    pub filler_words: bool,
    /// Enable profanity filtering
    pub profanity_filter: bool,
    /// Enable smart formatting
    pub smart_format: bool,
    /// Keywords to boost recognition
    pub keywords: Vec<String>,
    /// Redaction settings (pii, numbers, ssn, etc.)
    pub redact: Vec<String>,
    /// Voice activity detection events
    pub vad_events: bool,
    /// Endpointing timeout in seconds
    pub endpointing: Option<u32>,
    /// Custom tags for request identification
    pub tag: Option<String>,
    /// Utterance end timeout in milliseconds
    pub utterance_end_ms: Option<u32>,
    /// Use EU endpoint for data residency compliance.
    ///
    /// When enabled, requests are routed to api.eu.deepgram.com
    /// for European data residency requirements.
    pub use_eu_endpoint: bool,
    /// Enable PHI (Protected Health Information) redaction.
    ///
    /// When enabled, medical and health-related sensitive data
    /// will be redacted from transcripts. Useful for HIPAA compliance.
    /// This adds `redact=phi` to the API request.
    pub redact_phi: bool,
    /// Convert spoken numbers to numeric digits (`numerals=true`).
    ///
    /// Deepgram's `numerals` feature IS supported on the streaming endpoint
    /// (confirmed in the live AsyncAPI query-parameter schema and the streaming
    /// feature overview, June 2026), so we map it onto the request URL.
    pub numerals: bool,
    /// Transcribe each audio channel independently (`multichannel=true`).
    ///
    /// Deepgram's `multichannel` feature IS supported on the streaming endpoint
    /// (listed under "Media Input Settings: All available" in the streaming
    /// feature overview and present in the AsyncAPI streaming schema), so we map
    /// it onto the request URL.
    pub multichannel: bool,
    /// Override the WebSocket base endpoint (scheme://host[:port]) — e.g. `ws://127.0.0.1:PORT`
    /// for a local mock (W-T0 harness) or a proxy. When `None`, the production Deepgram
    /// endpoint (or the EU endpoint if `use_eu_endpoint`) is used. Generalizes the OpenAI
    /// `OPENAI_BASE_URL` pattern; required so reconnection chaos tests can drive a real mock.
    pub endpoint_override: Option<String>,
    /// Named-entity detection (`detect_entities=true`). Deepgram supports `detect_entities` on the
    /// streaming endpoint (live AsyncAPI query-parameter schema, June 2026). Mapped from the typed
    /// `SttFeatures::entity_detection`.
    pub detect_entities: bool,
    /// Find-and-replace pairs (`replace=FIND:REPLACE`). One `replace=` param per pair; each value is
    /// percent-encoded. Streaming-supported.
    pub replace: Vec<String>,
    /// Search terms / keyword spotting (`search=TERM`). One `search=` param per term; each value is
    /// percent-encoded. Streaming-supported.
    pub search: Vec<String>,
    /// Dictation mode (`dictation=true`) — convert spoken punctuation commands ("comma", "period")
    /// into punctuation. Streaming-supported.
    pub dictation: bool,
    /// Callback URL (`callback=URL`) — Deepgram POSTs results to this URL. Streaming-supported.
    pub callback: Option<String>,
    /// HTTP method for the callback (`callback_method=POST|PUT`). Streaming-supported.
    pub callback_method: Option<String>,
    /// Model Improvement Program opt-out (`mip_opt_out=true`). Streaming-supported.
    pub mip_opt_out: bool,
    /// Arbitrary request metadata tags (`extra=KEY:VALUE`). One `extra=` param per entry; each value
    /// is percent-encoded. Streaming-supported.
    pub extra: Vec<String>,
    /// Pin a specific model version (`version=ID`). Streaming-supported.
    pub version: Option<String>,
}

impl Default for DeepgramSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig::default(),
            diarize: false,
            interim_results: true,
            filler_words: false,
            profanity_filter: false,
            smart_format: true,
            keywords: Vec::new(),
            redact: Vec::new(),
            vad_events: true,
            endpointing: Some(200),
            tag: None,
            utterance_end_ms: Some(500),
            use_eu_endpoint: false,
            redact_phi: false,
            numerals: false,
            multichannel: false,
            endpoint_override: None,
            detect_entities: false,
            replace: Vec::new(),
            search: Vec::new(),
            dictation: false,
            callback: None,
            callback_method: None,
            mip_opt_out: false,
            extra: Vec::new(),
            version: None,
        }
    }
}

impl DeepgramSTTConfig {
    /// Build from the standardized config (W1 keystone), honoring advanced features that the
    /// flat factory path hardcodes off. Diarization, keyterms, redaction, VAD events, filler
    /// words and endpointing are now reachable through `StandardSTTConfig`. Unmodeled fields
    /// (EU endpoint, PHI redaction, tag) keep their defaults.
    pub fn from_standard(std: &super::standard::StandardSTTConfig) -> Self {
        let f = &std.features;
        let ex = &std.extras.0;
        // Helper: read a `Vec<String>` from an extras key that may be a JSON string or array.
        let str_vec = |key: &str| -> Vec<String> {
            match ex.get(key) {
                Some(serde_json::Value::String(s)) => vec![s.clone()],
                Some(serde_json::Value::Array(a)) => a
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect(),
                _ => Vec::new(),
            }
        };
        Self {
            base: std.base.clone(),
            diarize: f.diarization.unwrap_or(false),
            interim_results: f.interim_results.unwrap_or(true),
            filler_words: f.filler_words.unwrap_or(false),
            profanity_filter: f.profanity_filter.unwrap_or(false),
            smart_format: f.smart_format.unwrap_or(true),
            keywords: f.keyterms.clone().unwrap_or_default(),
            redact: f.redaction.clone().unwrap_or_default(),
            vad_events: f.vad_events.unwrap_or(false),
            endpointing: f.endpointing_ms.or(Some(200)),
            utterance_end_ms: f.utterance_end_ms.or(Some(500)),
            // `numerals` and `multichannel` ARE supported on Deepgram's streaming endpoint
            // (confirmed June 2026 against the AsyncAPI streaming query-parameter schema and the
            // streaming feature overview), so map them straight through to the wire.
            numerals: f.numerals.unwrap_or(false),
            multichannel: f.multichannel.unwrap_or(false),
            // Endpoint override (W-T0): carried via the standardized extras passthrough so the
            // restored, *featured* session can be pointed at a mock/proxy without touching the
            // ~80 StandardSTTConfig construction sites.
            endpoint_override: std.endpoint_override().map(|s| s.to_string()),
            // --- Newly-wired Deepgram streaming features ----------------------------------------
            // Entity detection is a typed `SttFeatures` field; the rest are provider-specific knobs
            // forwarded through the open `ProviderExtras` passthrough. All confirmed on the
            // streaming endpoint (Deepgram live AsyncAPI query-parameter schema, June 2026).
            detect_entities: f.entity_detection.unwrap_or(false),
            replace: str_vec("replace"),
            search: str_vec("search"),
            dictation: ex.get("dictation").and_then(|v| v.as_bool()).unwrap_or(false),
            callback: ex.get("callback").and_then(|v| v.as_str()).map(|s| s.to_string()),
            callback_method: ex
                .get("callback_method")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            mip_opt_out: ex.get("mip_opt_out").and_then(|v| v.as_bool()).unwrap_or(false),
            extra: str_vec("extra"),
            version: ex.get("version").and_then(|v| v.as_str()).map(|s| s.to_string()),
            // CAPABILITY GAP — intentionally NOT mapped to the wire:
            //   * `alternatives` (N-best): absent from Deepgram's streaming AsyncAPI
            //     query-parameter schema and from the streaming feature overview — it is a
            //     pre-recorded-only feature, so emitting `&alternatives=N` on the websocket would
            //     be a no-op at best. Leave it unmapped until Deepgram documents streaming support.
            //   * `language_detection` -> `detect_language`: Deepgram's docs state language
            //     detection "is not currently supported for streaming"; multilingual nova-2/nova-3
            //     models handle code-switching instead. Emitting `&detect_language=true` on the
            //     live endpoint is unsupported, so it stays a gap. (`f.language_detection`,
            //     `f.alternatives` are deliberately not read here.)
            ..Default::default()
        }
    }
}

/// Deepgram transcription response structure
#[derive(Debug, Deserialize, Serialize)]
pub struct DeepgramResponse {
    #[serde(rename = "type")]
    pub response_type: String,
    pub channel: Option<DeepgramChannel>,
    pub metadata: Option<DeepgramMetadata>,
    pub is_final: Option<bool>,
    pub speech_final: Option<bool>,
    pub duration: Option<f64>,
    pub start: Option<f64>,
    pub channel_index: Option<Vec<u32>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeepgramChannel {
    pub alternatives: Vec<DeepgramAlternative>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeepgramAlternative {
    pub transcript: String,
    pub confidence: f32,
    pub words: Option<Vec<DeepgramWord>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeepgramWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
    pub confidence: f32,
    pub punctuated_word: Option<String>,
    pub speaker: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeepgramMetadata {
    pub transaction_key: Option<String>,
    pub request_id: Option<String>,
    pub sha256: Option<String>,
    pub created: Option<String>,
    pub duration: Option<f64>,
    pub channels: Option<u32>,
    pub models: Option<Vec<String>>,
    pub model_info: Option<DeepgramModelInfo>,
    pub model_uuid: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeepgramModelInfo {
    pub name: String,
    pub version: String,
    pub arch: String,
}

/// Deepgram error response structure
#[derive(Debug, Deserialize, Serialize)]
pub struct DeepgramError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub description: String,
    pub message: String,
    pub variant: Option<String>,
}

/// Deepgram STT WebSocket client
pub struct DeepgramSTT {
    /// Configuration for the STT client
    config: Option<DeepgramSTTConfig>,
    /// Current connection state
    state: ConnectionState,
    /// State change notification
    state_notify: Arc<Notify>,
    /// WebSocket sender for audio data (bounded channel for backpressure)
    ws_sender: Option<mpsc::Sender<Bytes>>,
    /// Shutdown signal sender
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Result channel sender
    result_tx: Option<mpsc::Sender<STTResult>>,
    /// Error channel sender for streaming errors
    error_tx: Option<mpsc::Sender<STTError>>,
    /// Connection handle
    connection_handle: Option<tokio::task::JoinHandle<()>>,
    /// Result forwarding task handle
    result_forward_handle: Option<tokio::task::JoinHandle<()>>,
    /// Error forwarding task handle
    error_forward_handle: Option<tokio::task::JoinHandle<()>>,
    /// Shared callback storage for async access
    result_callback: Arc<Mutex<Option<AsyncSTTCallback>>>,
    /// Error callback storage for streaming errors
    error_callback: Arc<Mutex<Option<AsyncErrorCallback>>>,
    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState. When
    /// `None` (e.g. constructed directly in a unit test), the connect path falls back to its own
    /// per-session governor/breaker so storm control degrades gracefully rather than panicking.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
    /// Intentional-disconnect flag shared with the hand-rolled reconnect loop (W-D1). Cleared on
    /// `start_connection`, set in `disconnect()` before firing `shutdown_tx`. The outer reconnect
    /// loop checks it at the top and converts a racy `Reconnect` outcome into `Intentional`, so a
    /// client close racing a server-side close can never trigger a spurious reconnect (the unbiased
    /// inner select may pick the stream-ended arm over the shutdown arm).
    intentional_disconnect: Arc<AtomicBool>,
}

/// Constants for Deepgram regional endpoints
pub const DEEPGRAM_DEFAULT_ENDPOINT: &str = "wss://api.deepgram.com";
pub const DEEPGRAM_EU_ENDPOINT: &str = "wss://api.eu.deepgram.com";

/// Percent-encode a query-string value so phrases with spaces ("John Smith") or `word:intensifier`
/// syntax don't produce a malformed Deepgram URL (which closes the stream). Spaces become `+`,
/// reserved characters are escaped.
fn encode_query_value(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

impl DeepgramSTT {
    /// Internal: construct the provider from an already-mapped Deepgram config.
    fn from_deepgram_config(deepgram_config: DeepgramSTTConfig) -> Self {
        Self {
            config: Some(deepgram_config),
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
            resilience: None,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The shared circuit breaker this session will use on its connect path, if the
    /// process-global resilience handles have been injected (W-D2). Two `DeepgramSTT` built from
    /// the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`, so a trip
    /// in one is observed by the other. Returns `None` before `set_resilience` (per-session
    /// fallback).
    pub fn resilience_breaker(&self) -> Option<&Arc<CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }

    /// W1 keystone — construct directly from the standardized config so advanced features
    /// (diarization, keyterms, redaction, vad_events, …) are honored END-TO-END. The flat
    /// `BaseSTT::new` path hardcodes those off; this is the reachable standardized path.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }
        Ok(Self::from_deepgram_config(DeepgramSTTConfig::from_standard(
            std,
        )))
    }

    /// Build the WebSocket URL with query parameters (optimized string building)
    fn build_websocket_url(&self, config: &DeepgramSTTConfig) -> Result<String, STTError> {
        let mut url = String::with_capacity(256); // Pre-allocate expected size

        // Select endpoint. An explicit override (mock/proxy) wins; otherwise pick the EU or
        // default production endpoint per the residency setting.
        let endpoint: &str = if let Some(o) = &config.endpoint_override {
            o.trim_end_matches('/')
        } else if config.use_eu_endpoint {
            DEEPGRAM_EU_ENDPOINT
        } else {
            DEEPGRAM_DEFAULT_ENDPOINT
        };

        url.push_str(endpoint);
        // An omitted/empty model maps to Deepgram's recommended default (the
        // WS-config contract: "each provider maps an empty model to its
        // recommended default"). Emitting a literal `model=` empty param makes
        // Deepgram reject the handshake with a misleading 401.
        url.push_str("/v1/listen?model=");
        if config.base.model.is_empty() {
            url.push_str("nova-2");
        } else {
            url.push_str(&config.base.model);
        }
        url.push_str("&language=");
        url.push_str(&config.base.language);
        url.push_str("&sample_rate=");
        url.push_str(&config.base.sample_rate.to_string());
        url.push_str("&channels=");
        url.push_str(&config.base.channels.to_string());
        url.push_str("&punctuate=");
        url.push_str(if config.base.punctuation { "true" } else { "false" });
        url.push_str("&interim_results=");
        url.push_str(if config.interim_results { "true" } else { "false" });
        url.push_str("&smart_format=");
        url.push_str(if config.smart_format { "true" } else { "false" });
        url.push_str("&encoding=");
        url.push_str(&config.base.encoding);

        // Add optional parameters only if they're set
        if let Some(endpointing) = config.endpointing {
            url.push_str("&endpointing=");
            url.push_str(&endpointing.to_string());
        }

        if let Some(tag) = &config.tag {
            url.push_str("&tag=");
            url.push_str(&encode_query_value(tag));
        }

        if !config.keywords.is_empty() {
            // nova-3 replaced `keywords` with model-driven `keyterm` prompting: `keywords` is
            // silently ineffective on nova-3, and `keyterm` is invalid on nova-2/earlier. Send
            // one `keyterm=` per term for nova-3 (phrases allowed); comma-joined `keywords=` else.
            // Each value MUST be percent-encoded — keyterms/keywords routinely contain spaces (e.g.
            // "John Smith") or `word:intensifier` syntax, and an unencoded space produces a
            // malformed URL that Deepgram rejects, closing the stream.
            if config.base.model.starts_with("nova-3") {
                for term in &config.keywords {
                    url.push_str("&keyterm=");
                    url.push_str(&encode_query_value(term));
                }
            } else {
                url.push_str("&keywords=");
                let encoded: Vec<String> =
                    config.keywords.iter().map(|k| encode_query_value(k)).collect();
                url.push_str(&encoded.join(","));
            }
        }

        // Add diarization (speaker identification)
        if config.diarize {
            url.push_str("&diarize=true");
        }

        // Add filler words detection (um, uh, etc.)
        if config.filler_words {
            url.push_str("&filler_words=true");
        }

        // Add profanity filtering
        if config.profanity_filter {
            url.push_str("&profanity_filter=true");
        }

        // Add VAD events (voice activity detection)
        if config.vad_events {
            url.push_str("&vad_events=true");
        }

        // Add numerals (spoken numbers -> digits). Supported on the streaming endpoint.
        if config.numerals {
            url.push_str("&numerals=true");
        }

        // Add multichannel (independent per-channel transcription). Supported on streaming.
        if config.multichannel {
            url.push_str("&multichannel=true");
        }

        // Add entity detection (`detect_entities`). Supported on streaming.
        if config.detect_entities {
            url.push_str("&detect_entities=true");
        }

        // Find-and-replace: one `replace=` per FIND:REPLACE pair (percent-encoded).
        for pair in &config.replace {
            url.push_str("&replace=");
            url.push_str(&encode_query_value(pair));
        }

        // Search / keyword spotting: one `search=` per term (percent-encoded).
        for term in &config.search {
            url.push_str("&search=");
            url.push_str(&encode_query_value(term));
        }

        // Dictation mode (spoken-punctuation commands → punctuation).
        if config.dictation {
            url.push_str("&dictation=true");
        }

        // Callback URL — Deepgram POSTs results to this URL (percent-encoded).
        if let Some(callback) = &config.callback {
            url.push_str("&callback=");
            url.push_str(&encode_query_value(callback));
        }

        // Callback HTTP method (POST/PUT).
        if let Some(method) = &config.callback_method {
            url.push_str("&callback_method=");
            url.push_str(&encode_query_value(method));
        }

        // Model Improvement Program opt-out.
        if config.mip_opt_out {
            url.push_str("&mip_opt_out=true");
        }

        // Extra request metadata: one `extra=` per KEY:VALUE entry (percent-encoded).
        for entry in &config.extra {
            url.push_str("&extra=");
            url.push_str(&encode_query_value(entry));
        }

        // Pin a specific model version.
        if let Some(version) = &config.version {
            url.push_str("&version=");
            url.push_str(&encode_query_value(version));
        }

        // Add redaction (PII, numbers, SSN, etc.)
        if !config.redact.is_empty() {
            for redact_item in &config.redact {
                url.push_str("&redact=");
                url.push_str(&encode_query_value(redact_item));
            }
        }

        // Add PHI redaction (Protected Health Information) for HIPAA compliance
        if config.redact_phi {
            url.push_str("&redact=phi");
        }

        // Add utterance-end timeout. `utterance_end_ms` IS a valid Deepgram streaming parameter —
        // it makes Deepgram emit `UtteranceEnd` events after the given silence gap, which is the
        // robust end-of-turn signal when interim results are flowing. Deepgram requires
        // `interim_results=true` and a value of at least 1000 ms (it rejects smaller values with a
        // 400), so we only emit it when those constraints hold. (The default config value of 500 is
        // below Deepgram's minimum and is therefore intentionally not sent.)
        if let Some(utterance_end_ms) = config.utterance_end_ms
            && config.interim_results
            && utterance_end_ms >= 1000
        {
            url.push_str("&utterance_end_ms=");
            url.push_str(&utterance_end_ms.to_string());
        }

        Ok(url)
    }

    /// Handle incoming WebSocket messages (optimized for hot path)
    fn handle_websocket_message(
        message: Message,
        result_tx: &mpsc::Sender<STTResult>,
    ) -> Result<(), STTError> {
        match message {
            Message::Text(text) => {
                debug!("Received text message: {}", text);

                // Parse the response once and branch on type
                match serde_json::from_str::<DeepgramResponse>(&text) {
                    Ok(response) => {
                        match response.response_type.as_str() {
                            "Results" => {
                                if let Some(channel) = response.channel
                                    && let Some(alternative) = channel.alternatives.first()
                                {
                                    let stt_result = STTResult::new(
                                        alternative.transcript.clone(),
                                        response.is_final.unwrap_or(false),
                                        response.speech_final.unwrap_or(false),
                                        alternative.confidence,
                                    );

                                    // Send result (non-blocking with bounded channel)
                                    if let Err(e) = result_tx.try_send(stt_result) {
                                        match e {
                                            mpsc::error::TrySendError::Full(_) => {
                                                warn!("STT result channel full - dropping result");
                                            }
                                            mpsc::error::TrySendError::Closed(_) => {
                                                warn!("STT result channel closed");
                                            }
                                        }
                                    }
                                }
                            }
                            "Metadata" => {
                                // Log metadata for debugging but don't process
                                debug!("Received metadata response");
                            }
                            "Error" => {
                                // Try to parse as error for better message
                                let error_msg = if let Ok(error) =
                                    serde_json::from_str::<DeepgramError>(&text)
                                {
                                    format!("{}: {}", error.error_type, error.description)
                                } else {
                                    "Unknown error from Deepgram".to_string()
                                };
                                return Err(STTError::ProviderError(error_msg));
                            }
                            _ => {
                                // Ignore unknown message types
                                debug!("Received unknown message type: {}", response.response_type);
                            }
                        }
                    }
                    Err(_) => {
                        // Not a DeepgramResponse, might be a simple error or other message
                        if text.contains("\"type\":\"Error\"") {
                            let error_msg =
                                if let Ok(error) = serde_json::from_str::<DeepgramError>(&text) {
                                    format!("{}: {}", error.error_type, error.description)
                                } else {
                                    "Unknown error from Deepgram".to_string()
                                };
                            return Err(STTError::ProviderError(error_msg));
                        }
                        // Ignore other non-parseable messages
                    }
                }
            }
            Message::Close(close_frame) => {
                info!("WebSocket connection closed: {:?}", close_frame);
            }
            _ => {
                // Ignore other message types for performance
            }
        }

        Ok(())
    }

    /// Start the WebSocket connection task (optimized for minimal latency)
    async fn start_connection(&mut self, config: DeepgramSTTConfig) -> Result<(), STTError> {
        let ws_url = self.build_websocket_url(&config)?;
        info!("Deepgram STT WebSocket URL: {}", ws_url);

        // Create channels for communication (bounded for backpressure)
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
        let api_key = config.base.api_key.clone();
        let use_eu_endpoint = config.use_eu_endpoint;
        // The Host header must match the endpoint actually being dialed. For an override
        // (mock/proxy) derive it from the override authority; otherwise the production host.
        let host: String = if let Some(o) = &config.endpoint_override {
            ws_authority(o).unwrap_or_else(|| "api.deepgram.com".to_string())
        } else if use_eu_endpoint {
            "api.eu.deepgram.com".to_string()
        } else {
            "api.deepgram.com".to_string()
        };
        // Per-connection reconnection policy (backoff/jitter is inherently per-connection). The
        // STORM-CONTROL governor and the PROVIDER circuit breaker, however, are the single
        // process-global handles injected from CoreState (W-D2) so every Deepgram session trips
        // the same breaker and shares the one process-wide reconnect cap. When no handles were
        // injected (a unit test constructing the provider directly), fall back to per-session
        // ones so behaviour degrades gracefully instead of panicking.
        let reconnection = ReconnectionConfig::aggressive();
        let reset_after_ms = reconnection.reset_after_ms;
        let (governor, breaker) = match &self.resilience {
            Some(r) => ((*r.governor).clone(), std::sync::Arc::clone(&r.breaker)),
            None => (
                ReconnectGovernor::default(),
                std::sync::Arc::new(CircuitBreaker::with_defaults()),
            ),
        };

        // Fresh session: clear any intent left over from a prior disconnect so the reconnect loop
        // does not stop immediately. The same flag is shared into the task so disconnect() can stop
        // a reconnect that races a server-side close (W-D1).
        self.intentional_disconnect.store(false, Ordering::SeqCst);
        let intentional_disconnect = Arc::clone(&self.intentional_disconnect);

        // Start the connection task
        let connection_handle = tokio::spawn(async move {
            let manager = ReconnectionManager::new(reconnection);
            // `connected_tx` is a oneshot: signal success exactly once (the first connect).
            let mut connected_tx = Some(connected_tx);
            // Outer reconnect loop. Each iteration establishes a *featured* connection (the
            // URL carries diarize/keyterm/redaction/...) and runs the inner event loop until
            // it yields an outcome. Reconnectable outcomes loop; intentional/exhausted stop.
            'reconnect: loop {
                // W-D1: an intentional disconnect (possibly set during the previous iteration's
                // backoff) must stop the loop before dialing again.
                if intentional_disconnect.load(Ordering::Acquire) {
                    info!("Deepgram: intentional disconnect; not reconnecting");
                    break 'reconnect;
                }
                // Storm control + breaker gate before dialing.
                if !breaker.allow_request() {
                    crate::core::metrics::bridge::record_reconnect("deepgram", "circuit_open");
                    if !manager.should_retry() {
                        error!("Deepgram circuit open and reconnection budget exhausted");
                        break 'reconnect;
                    }
                    let delay = manager.next_delay_duration();
                    tokio::time::sleep(delay).await;
                    continue 'reconnect;
                }
                let _permit = governor.acquire().await;
                manager.start_attempt();

                // --- Dial + restore the featured session (re-send via the featured URL) ---
                let request = match tokio_tungstenite::tungstenite::http::Request::builder()
                    .method("GET")
                    .uri(&ws_url)
                    .header("Host", host.clone())
                    .header("Upgrade", "websocket")
                    .header("Connection", "upgrade")
                    .header("Sec-WebSocket-Key", generate_key())
                    .header("Sec-WebSocket-Version", "13")
                    .header("Authorization", format!("Token {api_key}"))
                    .body(())
                {
                    Ok(request) => request,
                    Err(e) => {
                        // A malformed request is fatal — retrying is pointless.
                        let stt_error = STTError::ConnectionFailed(format!(
                            "Failed to create WebSocket request: {e}"
                        ));
                        error!("{}", stt_error);
                        let _ = error_tx.try_send(stt_error);
                        break 'reconnect;
                    }
                };

                let ws_stream = match connect_async(request).await {
                    Ok((s, _)) => s,
                    Err(e) => {
                        // Eligible for reconnection: the upstream may just be flapping.
                        let stt_error = STTError::ConnectionFailed(format!(
                            "Failed to connect to Deepgram: {e}"
                        ));
                        error!("{}", stt_error);
                        breaker.record_failure();
                        crate::core::metrics::bridge::record_reconnect("deepgram", "failure");
                        manager.record_attempt(false);
                        drop(_permit);
                        if connected_tx.is_some() {
                            // Never connected yet and dialing failed — surface to the
                            // waiting connect() and give up (matches prior behavior of not
                            // hanging the initial connect on a dead endpoint).
                            let _ = error_tx.try_send(stt_error);
                            break 'reconnect;
                        }
                        if !manager.should_retry() {
                            let _ = error_tx.try_send(stt_error);
                            break 'reconnect;
                        }
                        let delay = manager.next_delay_duration();
                        tokio::time::sleep(delay).await;
                        continue 'reconnect;
                    }
                };

                info!("Connected to Deepgram WebSocket");
                // The dial succeeded — tell the breaker. We do NOT zero the attempt counter
                // here: a connect that immediately drops must still count against
                // `max_attempts` (a fast connect→drop flap would otherwise retry forever). The
                // counter is reset only after the connection proves *stable* (below).
                breaker.record_success();
                crate::core::metrics::bridge::record_reconnect("deepgram", "success");
                drop(_permit);
                let connected_since = Instant::now();

                // Signal the waiting connect() exactly once.
                if let Some(tx) = connected_tx.take() {
                    let _ = tx.send(());
                }

                let (mut ws_sink, mut ws_stream) = ws_stream.split();

                // Keep-alive mechanism to prevent connection timeout
                // Deepgram connections timeout after 10 seconds of inactivity
                // Send keep-alive messages every 1 second when no audio is being sent
                let mut keepalive_timer = interval(Duration::from_secs(1));
                let mut last_activity = Instant::now();

                // Outcome of the inner event loop: do we reconnect, or stop intentionally?
                let mut outcome: DeepgramInnerOutcome;

                // Main event loop
                loop {
                    tokio::select! {
                        // Handle outgoing audio data
                        Some(audio_data) = ws_rx.recv() => {
                            let message = Message::Binary(audio_data);
                            if let Err(e) = ws_sink.send(message).await {
                                let stt_error = STTError::NetworkError(format!(
                                    "Failed to send WebSocket message: {e}"
                                ));
                                error!("{}", stt_error);
                                let _ = error_tx.try_send(stt_error);
                                outcome = DeepgramInnerOutcome::Reconnect;
                                break;
                            }
                            // Update last activity time when audio is sent
                            last_activity = Instant::now();
                        }

                        // Handle incoming messages with idle timeout
                        message = timeout(WS_MESSAGE_TIMEOUT, ws_stream.next()) => {
                            match message {
                                Ok(Some(Ok(msg))) => {
                                    if let Err(e) = Self::handle_websocket_message(msg, &result_tx) {
                                        error!("Streaming error from Deepgram: {}", e);
                                        let _ = error_tx.try_send(e);
                                        // A provider `Error` frame is typically fatal (bad
                                        // config / auth); do not hammer it with reconnects.
                                        outcome = DeepgramInnerOutcome::Fatal;
                                        break;
                                    }
                                }
                                Ok(Some(Err(e))) => {
                                    let stt_error = STTError::NetworkError(format!(
                                        "WebSocket error: {e}"
                                    ));
                                    error!("{}", stt_error);
                                    let _ = error_tx.try_send(stt_error);
                                    outcome = DeepgramInnerOutcome::Reconnect;
                                    break;
                                }
                                Ok(None) => {
                                    info!("WebSocket stream ended");
                                    // The server closed the socket mid-stream — reconnect to
                                    // preserve the session (no finals lost across the gap).
                                    outcome = DeepgramInnerOutcome::Reconnect;
                                    break;
                                }
                                Err(_elapsed) => {
                                    // Idle timeout - no message received for 60s
                                    let stt_error = STTError::NetworkError(
                                        "WebSocket idle timeout - no message for 60 seconds".into()
                                    );
                                    error!("Deepgram STT idle timeout: {}", stt_error);
                                    let _ = error_tx.try_send(stt_error);
                                    outcome = DeepgramInnerOutcome::Reconnect;
                                    break;
                                }
                            }
                        }

                        // Handle keep-alive timer
                        _ = keepalive_timer.tick() => {
                            // Check if we need to send a keep-alive message
                            // Only send if no audio was sent in the last second
                            if last_activity.elapsed() >= Duration::from_secs(1) {
                                let keepalive_message = Message::Text(r#"{"type":"KeepAlive"}"#.into());
                                if let Err(e) = ws_sink.send(keepalive_message).await {
                                    let stt_error = STTError::NetworkError(format!(
                                        "Failed to send keep-alive message: {e}"
                                    ));
                                    error!("{}", stt_error);
                                    let _ = error_tx.try_send(stt_error);
                                    outcome = DeepgramInnerOutcome::Reconnect;
                                    break;
                                }
                                debug!("Sent keep-alive message to Deepgram");
                            }
                        }

                        // Handle shutdown signal
                        _ = &mut shutdown_rx => {
                            info!("Received shutdown signal, sending CloseStream to Deepgram");
                            // CRITICAL: Send CloseStream message to signal end of audio
                            // This tells Deepgram to finalize any pending transcripts and send speech_final
                            let close_stream_message = Message::Text(r#"{"type":"CloseStream"}"#.into());
                            if let Err(e) = ws_sink.send(close_stream_message).await {
                                warn!("Failed to send CloseStream message: {}", e);
                                outcome = DeepgramInnerOutcome::Intentional;
                                break;
                            }
                            debug!("Sent CloseStream message to Deepgram, waiting for final results");

                            // CRITICAL: Continue receiving messages after CloseStream to capture speech_final
                            // Don't just sleep - actively receive and process any final results
                            let close_timeout = tokio::time::Instant::now() + Duration::from_millis(1000);
                            let mut received_speech_final = false;

                            while tokio::time::Instant::now() < close_timeout {
                                tokio::select! {
                                    biased; // Prefer receiving messages over timeout

                                    message = tokio::time::timeout(Duration::from_millis(200), ws_stream.next()) => {
                                        match message {
                                            Ok(Some(Ok(msg))) => {
                                                // Check if this is a speech_final result before processing
                                                if let Message::Text(ref text) = msg
                                                    && (text.contains("\"speech_final\":true") || text.contains("\"speech_final\": true")) {
                                                        info!("Received speech_final after CloseStream");
                                                        received_speech_final = true;
                                                    }

                                                if let Err(e) = Self::handle_websocket_message(msg, &result_tx) {
                                                    warn!("Error handling message after CloseStream: {}", e);
                                                }

                                                // If we got speech_final, we can exit sooner
                                                if received_speech_final {
                                                    debug!("Got speech_final, exiting early");
                                                    break;
                                                }
                                            }
                                            Ok(Some(Err(e))) => {
                                                debug!("WebSocket error after CloseStream: {}", e);
                                                break;
                                            }
                                            Ok(None) => {
                                                info!("WebSocket stream ended after CloseStream");
                                                break;
                                            }
                                            Err(_) => {
                                                // 200ms timeout - continue waiting
                                                debug!("Still waiting for final results after CloseStream...");
                                            }
                                        }
                                    }
                                }
                            }

                            if !received_speech_final {
                                debug!("CloseStream timeout reached without receiving speech_final");
                            }

                            outcome = DeepgramInnerOutcome::Intentional;
                            break;
                        }
                    }
                }

                // Send graceful close frame before disconnecting from this socket.
                if let Err(e) = ws_sink.close().await {
                    debug!("Error closing WebSocket sink: {}", e);
                }

                // A connection that survived `reset_after_ms` is stable: clear backoff so a
                // long, occasionally-flaky session never exhausts max_attempts over its life.
                if reset_after_ms > 0
                    && connected_since.elapsed().as_millis() as u64 >= reset_after_ms
                {
                    manager.reset();
                }

                // W-D1: if the client asked to disconnect while the inner loop was running, a
                // server-close that won the unbiased select may have classified this as Reconnect.
                // Honor the intent: convert it to Intentional so we neither reconnect nor record a
                // spurious breaker failure.
                if matches!(outcome, DeepgramInnerOutcome::Reconnect)
                    && intentional_disconnect.load(Ordering::Acquire)
                {
                    outcome = DeepgramInnerOutcome::Intentional;
                }

                match outcome {
                    DeepgramInnerOutcome::Intentional | DeepgramInnerOutcome::Fatal => {
                        info!("Deepgram WebSocket connection closed");
                        break 'reconnect;
                    }
                    DeepgramInnerOutcome::Reconnect => {
                        breaker.record_failure();
                        if !manager.should_retry() {
                            crate::core::metrics::bridge::record_reconnect("deepgram", "exhausted");
                            warn!("Deepgram reconnection budget exhausted; closing session");
                            break 'reconnect;
                        }
                        let delay = manager.next_delay_duration();
                        info!(
                            "Deepgram transport dropped; reconnecting (attempt {}) in {:?}",
                            manager.attempt_count() + 1,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        // loop -> re-dial the same featured URL (restore_session)
                    }
                }
            }
        });

        self.connection_handle = Some(connection_handle);

        // Start result forwarding task with shared callback
        let callback_ref = self.result_callback.clone();
        let result_forwarding_handle = tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                if let Some(callback) = callback_ref.lock().await.as_ref() {
                    callback(result).await;
                } else {
                    // Log the result if no callback is set
                    debug!(
                        "Received STT result: {} (confidence: {})",
                        result.transcript, result.confidence
                    );
                }
            }
        });

        // Store the result forwarding handle for cleanup
        self.result_forward_handle = Some(result_forwarding_handle);

        // Start error forwarding task with shared callback
        let error_callback_ref = self.error_callback.clone();
        let error_forwarding_handle = tokio::spawn(async move {
            while let Some(error) = error_rx.recv().await {
                if let Some(callback) = error_callback_ref.lock().await.as_ref() {
                    callback(error).await;
                } else {
                    // Log the error if no callback is set
                    error!(
                        "STT streaming error but no error callback registered: {}",
                        error
                    );
                }
            }
        });

        // Store the error forwarding handle for cleanup
        self.error_forward_handle = Some(error_forwarding_handle);

        // Update state and wait for connection
        self.state = ConnectionState::Connecting;

        // Wait for connection to be established with timeout
        match timeout(Duration::from_secs(10), connected_rx).await {
            Ok(Ok(())) => {
                self.state = ConnectionState::Connected;
                self.state_notify.notify_waiters();
                info!("Successfully connected to Deepgram STT");
                Ok(())
            }
            Ok(Err(_)) => {
                let error_msg = "Connection channel closed".to_string();
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

impl Default for DeepgramSTT {
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
            resilience: None,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for DeepgramSTT {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        // Validate API key
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }

        // Create Deepgram-specific configuration, preserving config values
        let deepgram_config = DeepgramSTTConfig {
            base: config,
            diarize: false,
            interim_results: true,
            filler_words: false,
            profanity_filter: false,
            smart_format: true,
            keywords: Vec::new(),
            redact: Vec::new(),
            vad_events: true,
            endpointing: Some(200),
            tag: None,
            utterance_end_ms: Some(500),
            ..Default::default()
        };

        Ok(Self::from_deepgram_config(deepgram_config))
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        // Get the stored configuration
        let config = self.config.as_ref().ok_or_else(|| {
            STTError::ConfigurationError("No configuration available".to_string())
        })?;

        // Start the connection
        self.start_connection(config.clone()).await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // W-D1: record the intent BEFORE firing the shutdown signal so the reconnect loop never
        // re-dials on a disconnect that races a server-side close.
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        // Send shutdown signal
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Wait for connection task to finish (this now includes waiting for speech_final)
        if let Some(handle) = self.connection_handle.take() {
            let _ = timeout(Duration::from_secs(5), handle).await;
        }

        // CRITICAL: Give result forwarding task time to process any pending results
        // The connection task may have enqueued final results that need to be forwarded
        // to the callback before we abort the forwarding task
        tokio::time::sleep(Duration::from_millis(100)).await;

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

        // Clean up channels but PRESERVE callbacks
        // Callbacks should persist across reconnections so they work when we reconnect
        self.ws_sender = None;
        self.result_tx = None;
        self.error_tx = None;
        // NOTE: Do NOT clear result_callback and error_callback here
        // They need to be preserved for reconnection to work correctly

        // Update state
        self.state = ConnectionState::Disconnected;
        self.state_notify.notify_waiters();

        info!("Disconnected from Deepgram STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        matches!(self.state, ConnectionState::Connected) && self.ws_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to Deepgram".to_string(),
            ));
        }

        if let Some(ws_sender) = &self.ws_sender {
            let data_len = audio_data.len();

            // Send audio data with backpressure handling (zero-copy - Bytes passed directly)
            ws_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio data: {e}")))?;

            debug!("Sent {} bytes of audio data", data_len);
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
        // For Deepgram, we need to reconnect to update configuration
        if self.is_ready() {
            self.disconnect().await?;
        }

        // Update stored configuration, preserving config values
        let deepgram_config = DeepgramSTTConfig {
            base: config,
            diarize: false,
            interim_results: true,
            filler_words: false,
            profanity_filter: false,
            smart_format: true,
            keywords: Vec::new(),
            redact: Vec::new(),
            vad_events: true,
            endpointing: Some(200),
            tag: None,
            utterance_end_ms: Some(500),
            ..Default::default()
        };
        self.config = Some(deepgram_config);

        self.connect().await?;
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Deepgram STT WebSocket"
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared process-global handles; the next `start_connection` will use them so
        // every Deepgram session trips the same breaker and shares the one reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

impl Drop for DeepgramSTT {
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
    use tokio::time::Duration;

    #[tokio::test]
    async fn test_deepgram_stt_creation() {
        let config = STTConfig {
            model: "nova-3".to_string(),
            provider: "deepgram".to_string(),
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
        };

        let stt = <DeepgramSTT as BaseSTT>::new(config).unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), "Deepgram STT WebSocket");
    }

    // W-D1: disconnect() must record intent on the flag shared with the hand-rolled reconnect
    // loop, so a client close racing a server-side close can never trigger a spurious reconnect.
    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
        let config = STTConfig {
            model: "nova-3".to_string(),
            provider: "deepgram".to_string(),
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
        };
        let mut stt = <DeepgramSTT as BaseSTT>::new(config).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the reconnect-loop intentional-disconnect flag",
        );
    }

    #[tokio::test]
    async fn test_deepgram_stt_config_validation() {
        // Test with empty API key
        let config = STTConfig {
            model: "nova-3".to_string(),
            provider: "deepgram".to_string(),
            api_key: String::new(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
        };

        let result = <DeepgramSTT as BaseSTT>::new(config);
        assert!(result.is_err());
        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key is required"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[tokio::test]
    async fn test_deepgram_config_url_building() {
        let stt = DeepgramSTT::default();
        let config = DeepgramSTTConfig {
            base: STTConfig {
                model: "nova-3".to_string(),
                provider: "deepgram".to_string(),
                api_key: "test_key".to_string(),
                language: "en-US".to_string(),
                sample_rate: 16000,
                channels: 1,
                punctuation: false, // Set to false here for testing
                encoding: "linear16".to_string(),
            },
            interim_results: true,
            smart_format: false,
            endpointing: Some(300),
            utterance_end_ms: Some(1000),
            tag: Some("test-tag".to_string()),
            keywords: vec!["hello".to_string(), "world".to_string()],
            ..Default::default()
        };

        let url = stt.build_websocket_url(&config).unwrap();
        assert!(url.contains("wss://api.deepgram.com/v1/listen"));
        assert!(url.contains("model=nova-3"));
        assert!(url.contains("language=en-US"));
        assert!(url.contains("sample_rate=16000"));
        assert!(url.contains("channels=1"));
        assert!(url.contains("punctuate=false"));
        assert!(url.contains("interim_results=true"));
        assert!(url.contains("smart_format=false"));
        // nova-3 uses keyterm prompting (one param per term), NOT the legacy keywords= param.
        assert!(url.contains("keyterm=hello"));
        assert!(url.contains("keyterm=world"));
        assert!(!url.contains("keywords="));
        assert!(url.contains("endpointing=300"));
        assert!(url.contains("tag=test-tag"));
        // utterance_end_ms is a valid streaming param when interim_results=true and value >= 1000.
        assert!(url.contains("utterance_end_ms=1000"));
    }

    #[tokio::test]
    async fn test_deepgram_url_encodes_keyterms_and_gates_utterance_end() {
        let stt = DeepgramSTT::default();
        let base = |model: &str| STTConfig {
            model: model.to_string(),
            api_key: "k".to_string(),
            ..Default::default()
        };

        // Multi-word keyterms MUST be percent-encoded (space → `+`), else the URL is malformed and
        // Deepgram closes the stream (regression guard for the live "John Smith" failure).
        let cfg = DeepgramSTTConfig {
            base: base("nova-3"),
            keywords: vec!["John Smith".to_string()],
            ..Default::default()
        };
        let url = stt.build_websocket_url(&cfg).unwrap();
        assert!(url.contains("keyterm=John+Smith"), "keyterm not encoded: {url}");
        assert!(!url.contains("keyterm=John Smith"), "raw space leaked into URL: {url}");

        // Legacy keywords on nova-2 are encoded per-term and comma-joined.
        let cfg2 = DeepgramSTTConfig {
            base: base("nova-2"),
            keywords: vec!["New York".to_string(), "L.A.".to_string()],
            ..Default::default()
        };
        let url2 = stt.build_websocket_url(&cfg2).unwrap();
        assert!(url2.contains("keywords=New+York,L.A."), "keywords not encoded: {url2}");

        // utterance_end_ms gating: below 1000 OR interim_results=false ⇒ NOT emitted.
        let below = DeepgramSTTConfig {
            base: base("nova-3"),
            interim_results: true,
            utterance_end_ms: Some(500),
            ..Default::default()
        };
        assert!(!stt.build_websocket_url(&below).unwrap().contains("utterance_end_ms"));

        let no_interim = DeepgramSTTConfig {
            base: base("nova-3"),
            interim_results: false,
            utterance_end_ms: Some(2000),
            ..Default::default()
        };
        assert!(
            !stt.build_websocket_url(&no_interim)
                .unwrap()
                .contains("utterance_end_ms")
        );

        // interim_results=true AND value >= 1000 ⇒ emitted.
        let ok = DeepgramSTTConfig {
            base: base("nova-3"),
            interim_results: true,
            utterance_end_ms: Some(1500),
            ..Default::default()
        };
        assert!(
            stt.build_websocket_url(&ok)
                .unwrap()
                .contains("utterance_end_ms=1500")
        );
    }

    #[tokio::test]
    async fn test_deepgram_keywords_vs_keyterm_by_model() {
        let stt = DeepgramSTT::default();
        let mk = |model: &str| DeepgramSTTConfig {
            base: STTConfig {
                model: model.to_string(),
                api_key: "k".to_string(),
                ..Default::default()
            },
            keywords: vec!["foo".to_string(), "bar".to_string()],
            ..Default::default()
        };
        // nova-2 (and earlier) -> legacy keyword boosting.
        let url_n2 = stt.build_websocket_url(&mk("nova-2")).unwrap();
        assert!(url_n2.contains("keywords=foo,bar"));
        assert!(!url_n2.contains("keyterm="));
        // nova-3 -> keyterm prompting.
        let url_n3 = stt.build_websocket_url(&mk("nova-3")).unwrap();
        assert!(url_n3.contains("keyterm=foo"));
        assert!(url_n3.contains("keyterm=bar"));
        assert!(!url_n3.contains("keywords="));
    }

    // W1 keystone: advanced features that the flat factory path hardcodes off are now reachable
    // for Deepgram via StandardSTTConfig and actually reach the wire.
    #[tokio::test]
    async fn test_deepgram_from_standard_unlocks_advanced_features() {
        use super::super::standard::{SttFeatures, StandardSTTConfig};
        let stt = DeepgramSTT::default();
        let std_cfg = StandardSTTConfig {
            base: STTConfig {
                model: "nova-3".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                keyterms: Some(vec!["WaaV".into()]),
                redaction: Some(vec!["pii".into()]),
                vad_events: Some(true),
                profanity_filter: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let cfg = DeepgramSTTConfig::from_standard(&std_cfg);
        // Config fields mapped from the standardized features.
        assert!(cfg.diarize);
        assert_eq!(cfg.keywords, vec!["WaaV".to_string()]);
        assert_eq!(cfg.redact, vec!["pii".to_string()]);
        assert!(cfg.vad_events);
        assert!(cfg.profanity_filter);
        // And they reach the actual request URL (previously impossible via the factory).
        let url = stt.build_websocket_url(&cfg).unwrap();
        assert!(url.contains("diarize=true"));
        assert!(url.contains("keyterm=WaaV")); // nova-3 keyterm prompting
        assert!(url.contains("vad_events=true"));
        assert!(url.contains("profanity_filter=true"));
    }

    // WIRE-LEVEL guard for numerals + multichannel: assert the params land in the request URL
    // (not merely on the config struct). This is the exact bug class the last review caught —
    // a feature mapped onto the struct but never serialized to the wire.
    #[tokio::test]
    async fn test_deepgram_numerals_multichannel_reach_the_wire() {
        let stt = DeepgramSTT::default();
        let base = STTConfig {
            model: "nova-3".to_string(),
            api_key: "k".to_string(),
            ..Default::default()
        };

        // Off by default => params MUST NOT appear.
        let off = DeepgramSTTConfig {
            base: base.clone(),
            ..Default::default()
        };
        let url_off = stt.build_websocket_url(&off).unwrap();
        assert!(!url_off.contains("numerals="), "numerals leaked when off: {url_off}");
        assert!(!url_off.contains("multichannel="), "multichannel leaked when off: {url_off}");

        // On => exact wire params MUST appear.
        let on = DeepgramSTTConfig {
            base,
            numerals: true,
            multichannel: true,
            ..Default::default()
        };
        let url_on = stt.build_websocket_url(&on).unwrap();
        assert!(url_on.contains("&numerals=true"), "numerals missing from URL: {url_on}");
        assert!(url_on.contains("&multichannel=true"), "multichannel missing from URL: {url_on}");
    }

    // WIRE-LEVEL keystone test: the standardized SttFeatures -> from_standard -> URL path.
    // numerals/multichannel must round-trip all the way to the request URL through the reachable
    // standardized config. alternatives/language_detection are streaming capability gaps and must
    // NOT appear on the wire even when requested in the standardized features.
    #[tokio::test]
    async fn test_deepgram_from_standard_numerals_multichannel_and_gaps() {
        use super::super::standard::{SttFeatures, StandardSTTConfig};
        let stt = DeepgramSTT::default();
        let std_cfg = StandardSTTConfig {
            base: STTConfig {
                model: "nova-3".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: SttFeatures {
                numerals: Some(true),
                multichannel: Some(true),
                // Requested but NOT supported on Deepgram streaming -> must stay capability gaps.
                alternatives: Some(3),
                language_detection: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let cfg = DeepgramSTTConfig::from_standard(&std_cfg);
        // Supported features mapped onto the config...
        assert!(cfg.numerals);
        assert!(cfg.multichannel);
        // ...and they reach the actual request URL.
        let url = stt.build_websocket_url(&cfg).unwrap();
        assert!(url.contains("&numerals=true"), "numerals not on wire: {url}");
        assert!(url.contains("&multichannel=true"), "multichannel not on wire: {url}");
        // Capability gaps: these params must NEVER appear on the streaming URL, even though the
        // standardized features requested them (Deepgram does not support them on streaming).
        assert!(!url.contains("alternatives="), "alternatives must be a streaming gap: {url}");
        assert!(!url.contains("detect_language="), "detect_language must be a streaming gap: {url}");
    }

    // WIRE-LEVEL guard: entity detection + find/replace + search + dictation + callback +
    // callback_method + mip_opt_out + extra + version must all land in the request URL with their
    // exact Deepgram param names — NOT merely on the config struct (the recurring bug class).
    #[tokio::test]
    async fn test_deepgram_extended_features_reach_the_wire() {
        let stt = DeepgramSTT::default();
        let base = STTConfig {
            model: "nova-3".to_string(),
            api_key: "k".to_string(),
            ..Default::default()
        };

        // Off by default => none of the params appear.
        let off = DeepgramSTTConfig {
            base: base.clone(),
            ..Default::default()
        };
        let url_off = stt.build_websocket_url(&off).unwrap();
        for p in [
            "detect_entities=", "replace=", "search=", "dictation=", "callback=",
            "callback_method=", "mip_opt_out=", "extra=", "version=",
        ] {
            assert!(!url_off.contains(p), "{p} leaked when unset: {url_off}");
        }

        // On => exact wire params must appear, multi-valued ones repeated and percent-encoded.
        let on = DeepgramSTTConfig {
            base,
            detect_entities: true,
            replace: vec!["John Smith:Jon Smith".to_string()],
            search: vec!["medication".to_string(), "blood pressure".to_string()],
            dictation: true,
            callback: Some("https://example.com/cb?x=1".to_string()),
            callback_method: Some("PUT".to_string()),
            mip_opt_out: true,
            extra: vec!["session:abc 123".to_string()],
            version: Some("2024-01-09.0".to_string()),
            ..Default::default()
        };
        let url = stt.build_websocket_url(&on).unwrap();
        assert!(url.contains("&detect_entities=true"), "detect_entities missing: {url}");
        // find/replace + search values are percent-encoded (space -> +, ':' escaped).
        assert!(url.contains("&replace=John+Smith%3AJon+Smith"), "replace not encoded: {url}");
        assert!(url.contains("&search=medication"), "search[0] missing: {url}");
        assert!(url.contains("&search=blood+pressure"), "search[1] not encoded: {url}");
        assert!(url.contains("&dictation=true"), "dictation missing: {url}");
        assert!(
            url.contains("&callback=https%3A%2F%2Fexample.com%2Fcb%3Fx%3D1"),
            "callback not encoded: {url}"
        );
        assert!(url.contains("&callback_method=PUT"), "callback_method missing: {url}");
        assert!(url.contains("&mip_opt_out=true"), "mip_opt_out missing: {url}");
        assert!(url.contains("&extra=session%3Aabc+123"), "extra not encoded: {url}");
        assert!(url.contains("&version=2024-01-09.0"), "version missing: {url}");
    }

    // WIRE-LEVEL keystone: SttFeatures (typed entity_detection) + ProviderExtras passthrough ->
    // from_standard -> URL. Proves the standardized config actually drives these params onto the
    // wire, end-to-end through the reachable path.
    #[tokio::test]
    async fn test_deepgram_from_standard_extended_features_reach_the_wire() {
        use super::super::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let stt = DeepgramSTT::default();
        let extras = ProviderExtras(
            serde_json::json!({
                "replace": ["color:colour"],
                "search": "refund",
                "dictation": true,
                "callback": "https://cb.test/hook",
                "callback_method": "POST",
                "mip_opt_out": true,
                "extra": ["tenant:acme", "trace:42"],
                "version": "beta"
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let std_cfg = StandardSTTConfig {
            base: STTConfig {
                model: "nova-3".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: SttFeatures {
                entity_detection: Some(true),
                ..Default::default()
            },
            extras,
        };
        let cfg = DeepgramSTTConfig::from_standard(&std_cfg);
        // Mapped onto the config...
        assert!(cfg.detect_entities);
        assert_eq!(cfg.replace, vec!["color:colour".to_string()]);
        assert_eq!(cfg.search, vec!["refund".to_string()]);
        assert!(cfg.dictation);
        assert_eq!(cfg.callback.as_deref(), Some("https://cb.test/hook"));
        assert_eq!(cfg.callback_method.as_deref(), Some("POST"));
        assert!(cfg.mip_opt_out);
        assert_eq!(cfg.extra, vec!["tenant:acme".to_string(), "trace:42".to_string()]);
        assert_eq!(cfg.version.as_deref(), Some("beta"));
        // ...and reach the request URL.
        let url = stt.build_websocket_url(&cfg).unwrap();
        assert!(url.contains("&detect_entities=true"), "{url}");
        assert!(url.contains("&replace=color%3Acolour"), "{url}");
        assert!(url.contains("&search=refund"), "{url}");
        assert!(url.contains("&dictation=true"), "{url}");
        assert!(url.contains("&callback=https%3A%2F%2Fcb.test%2Fhook"), "{url}");
        assert!(url.contains("&callback_method=POST"), "{url}");
        assert!(url.contains("&mip_opt_out=true"), "{url}");
        assert!(url.contains("&extra=tenant%3Aacme"), "{url}");
        assert!(url.contains("&extra=trace%3A42"), "{url}");
        assert!(url.contains("&version=beta"), "{url}");
    }

    #[tokio::test]
    async fn test_message_handling() {
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(256);

        let json_response = r#"
        {
            "type": "Results",
            "channel": {
                "alternatives": [
                    {
                        "transcript": "Hello world",
                        "confidence": 0.98,
                        "words": []
                    }
                ]
            },
            "is_final": true,
            "speech_final": true,
            "duration": 2.0,
            "start": 0.0
        }
        "#;

        let message = Message::Text(json_response.to_string().into());
        let result = DeepgramSTT::handle_websocket_message(message, &result_tx);

        assert!(result.is_ok());

        // Check that result was sent
        let received_result = tokio::time::timeout(Duration::from_millis(100), result_rx.recv())
            .await
            .expect("Should receive result within timeout")
            .expect("Should receive a result");
        assert_eq!(received_result.transcript, "Hello world");
        assert_eq!(received_result.confidence, 0.98);
        assert!(received_result.is_final);
        assert!(received_result.is_speech_final);
    }

    #[tokio::test]
    async fn test_keepalive_message_format() {
        // Test that the keep-alive message format is correct
        let keepalive_json = r#"{"type":"KeepAlive"}"#;
        let parsed: serde_json::Value = serde_json::from_str(keepalive_json).unwrap();

        assert_eq!(parsed["type"], "KeepAlive");

        // Test that the message can be created successfully
        let message = Message::Text(keepalive_json.into());
        match message {
            Message::Text(_) => {} // Success
            _ => panic!("Expected Text message"),
        }
    }

    #[tokio::test]
    async fn test_deepgram_advanced_config_url_building() {
        let stt = DeepgramSTT::default();
        let config = DeepgramSTTConfig {
            base: STTConfig {
                model: "nova-3".to_string(),
                provider: "deepgram".to_string(),
                api_key: "test_key".to_string(),
                language: "en-US".to_string(),
                sample_rate: 16000,
                channels: 1,
                punctuation: true,
                encoding: "linear16".to_string(),
            },
            diarize: true,
            interim_results: true,
            filler_words: true,
            profanity_filter: true,
            smart_format: true,
            keywords: vec!["hello".to_string()],
            redact: vec!["pci".to_string(), "ssn".to_string()],
            vad_events: true,
            endpointing: Some(300),
            tag: Some("test".to_string()),
            utterance_end_ms: Some(750),
            ..Default::default()
        };

        let url = stt.build_websocket_url(&config).unwrap();

        // Verify all advanced parameters are included
        assert!(url.contains("diarize=true"), "Missing diarize param");
        assert!(
            url.contains("filler_words=true"),
            "Missing filler_words param"
        );
        assert!(
            url.contains("profanity_filter=true"),
            "Missing profanity_filter param"
        );
        assert!(url.contains("vad_events=true"), "Missing vad_events param");
        assert!(url.contains("redact=pci"), "Missing redact=pci param");
        assert!(url.contains("redact=ssn"), "Missing redact=ssn param");
        // Note: utterance_end_ms is not a valid Deepgram API parameter and is now disabled
        // assert!(
        //     url.contains("utterance_end_ms=750"),
        //     "Missing utterance_end_ms param"
        // );
    }

    #[tokio::test]
    async fn test_deepgram_config_defaults() {
        let config = DeepgramSTTConfig::default();

        // Verify defaults
        assert!(!config.diarize);
        assert!(config.interim_results);
        assert!(!config.filler_words);
        assert!(!config.profanity_filter);
        assert!(config.smart_format);
        assert!(config.keywords.is_empty());
        assert!(config.redact.is_empty());
        assert!(config.vad_events);
        assert_eq!(config.endpointing, Some(200));
        assert_eq!(config.utterance_end_ms, Some(500));
    }

    // --- W-D2 cross-session wiring: shared breaker + process-global governor ----------------

    /// Build a `DeepgramSTT` the same way the VoiceManager does for a live session: construct
    /// from a standardized config, then inject the shared resilience handles from the registry.
    fn session_from_registry(
        reg: &crate::core::resilience::ResilienceRegistry,
    ) -> DeepgramSTT {
        use crate::core::stt::base::BaseSTT;
        let std = crate::core::stt::standard::StandardSTTConfig::from_base(STTConfig {
            provider: "deepgram".to_string(),
            api_key: "test-key".to_string(),
            ..Default::default()
        });
        let mut stt = DeepgramSTT::new_standard(&std).expect("build deepgram session");
        stt.set_resilience(reg.handles_for("deepgram"));
        stt
    }

    #[tokio::test]
    async fn two_deepgram_sessions_share_one_breaker_so_a_trip_is_cross_session_visible() {
        // RED before wiring: each session created its OWN breaker, so a trip in session A was
        // invisible to session B. With the shared registry breaker injected, the trip is visible.
        let reg = crate::core::resilience::ResilienceRegistry::new(8);
        let session_a = session_from_registry(&reg);
        let session_b = session_from_registry(&reg);

        let breaker_a = session_a.resilience_breaker().expect("A has shared breaker");
        let breaker_b = session_b.resilience_breaker().expect("B has shared breaker");
        assert!(
            Arc::ptr_eq(breaker_a, breaker_b),
            "both Deepgram sessions must share the one provider breaker"
        );

        // Session A trips its breaker (a burst of upstream failures).
        for _ in 0..10 {
            breaker_a.record_failure();
        }
        assert_eq!(breaker_a.state(), crate::core::resilience::CircuitState::Open);
        // Session B — a different session — sees the open breaker and is denied a reconnect.
        assert!(
            !breaker_b.allow_request(),
            "a trip in session A must be visible to session B (provider-level tripping)"
        );
    }

    #[tokio::test]
    async fn two_sessions_share_the_process_global_governor_cap() {
        // The governor cap is process-global: both sessions draw from the SAME governor, so two
        // sessions' reconnects share one cap (storm control across sessions).
        let reg = crate::core::resilience::ResilienceRegistry::new(3);
        let session_a = session_from_registry(&reg);
        let session_b = session_from_registry(&reg);

        let gov_a = &session_a.resilience.as_ref().unwrap().governor;
        let gov_b = &session_b.resilience.as_ref().unwrap().governor;
        assert!(
            Arc::ptr_eq(gov_a, gov_b),
            "both sessions must share one process-global governor"
        );
        assert_eq!(gov_a.max_concurrent(), 3, "the cap is the registry's, not a per-session default");

        // A permit taken via session A's governor is visible as in-flight via session B's
        // governor handle — proving the cap is shared, not per-session.
        let _permit = gov_a.acquire().await;
        assert_eq!(gov_b.in_flight(), 1, "in-flight is shared across sessions");
        assert_eq!(gov_b.available_permits(), 2);
    }
}
