use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt;
use tokio::sync::{Mutex, Notify, RwLock, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::base::{AudioCallback, AudioData, ConnectionState, TTSConfig, TTSError, TTSResult};
use crate::core::cache::store::CacheStore;
use crate::utils::req_manager::{ReqManager, ReqManagerConfig};
use regex::Regex;
use std::time::Duration;
use xxhash_rust::xxh3::xxh3_128;

/// A delivered audio payload: raw bytes plus an optional FORMAT OVERRIDE set
/// when boundary sniffing (P0.1) proved the bytes are a container that differs
/// from the declared format — the dispatcher then labels `AudioData` with the
/// truth instead of the declaration.
type AudioPayload = (Vec<u8>, Option<&'static str>);

fn pcm_bytes_per_sample(format: &str) -> Option<usize> {
    match format {
        "linear16" | "pcm" | "pcm16" => Some(2),
        "mulaw" | "ulaw" | "alaw" => Some(1),
        _ => None,
    }
}

fn pcm_stream_chunk_bytes(sample_rate: u32, bytes_per_sample: usize) -> usize {
    let bytes_per_sample = bytes_per_sample.max(1);
    let samples = (sample_rate.max(100) / 100).max(1) as usize;
    samples
        .saturating_mul(bytes_per_sample)
        .max(bytes_per_sample)
}

fn pcm_duration_ms_for_bytes(bytes_len: usize, format: &str, sample_rate: u32) -> Option<u32> {
    let bytes_per_sample = pcm_bytes_per_sample(format)?;
    if sample_rate == 0 {
        return None;
    }

    let samples = bytes_len / bytes_per_sample;
    let duration_ms = (samples as u128).saturating_mul(1000) / u128::from(sample_rate);
    Some(duration_ms.min(u128::from(u32::MAX)) as u32)
}

/// Request entry for ordered processing
struct RequestEntry {
    receiver: mpsc::Receiver<Result<AudioPayload, TTSError>>,
    /// Audio format for this specific request
    format: String,
    /// Sample rate for this specific request
    sample_rate: u32,
}

/// A queued speak job containing all data needed to execute a TTS request
struct SpeakJob {
    /// The trimmed text to synthesize
    text: String,
    /// The HTTP request builder (boxed trait object)
    request_builder: Box<dyn TTSRequestBuilderDyn>,
    /// The channel sender for streaming audio chunks
    sender: mpsc::Sender<Result<AudioPayload, TTSError>>,
    /// The cancellation token for this request
    cancel_token: CancellationToken,
    /// The request manager for HTTP client pooling
    req_manager: Arc<ReqManager>,
    /// Optional cache store and full cache key (config_hash:text_hash)
    cache_and_key: Option<(Arc<CacheStore>, String)>,
}

/// Trait object-safe version of TTSRequestBuilder for dynamic dispatch
trait TTSRequestBuilderDyn: Send + Sync {
    fn build_http_request_with_context(
        &self,
        client: &reqwest::Client,
        text: &str,
        previous_text: Option<&str>,
    ) -> reqwest::RequestBuilder;
    fn get_config(&self) -> &TTSConfig;
    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer>;
    fn expects_provider_audio_url_response(&self) -> bool;
    fn provider_audio_url_label(&self) -> &'static str;
    fn parse_provider_audio_url_response(&self, body: &[u8]) -> TTSResult<Option<String>>;
}

/// Blanket implementation to convert any TTSRequestBuilder to the trait object version
impl<T: TTSRequestBuilder> TTSRequestBuilderDyn for T {
    fn build_http_request_with_context(
        &self,
        client: &reqwest::Client,
        text: &str,
        previous_text: Option<&str>,
    ) -> reqwest::RequestBuilder {
        TTSRequestBuilder::build_http_request_with_context(self, client, text, previous_text)
    }

    fn get_config(&self) -> &TTSConfig {
        TTSRequestBuilder::get_config(self)
    }

    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        TTSRequestBuilder::get_pronunciation_replacer(self)
    }

    fn expects_provider_audio_url_response(&self) -> bool {
        TTSRequestBuilder::expects_provider_audio_url_response(self)
    }

    fn provider_audio_url_label(&self) -> &'static str {
        TTSRequestBuilder::provider_audio_url_label(self)
    }

    fn parse_provider_audio_url_response(&self, body: &[u8]) -> TTSResult<Option<String>> {
        TTSRequestBuilder::parse_provider_audio_url_response(self, body)
    }
}

/// Compiled pronunciation replacement patterns
#[derive(Clone)]
pub struct PronunciationReplacer {
    patterns: Vec<(Regex, String)>,
}

impl PronunciationReplacer {
    /// Create a new pronunciation replacer from config
    pub fn new(pronunciations: &[super::base::Pronunciation]) -> Self {
        let patterns = pronunciations
            .iter()
            .filter_map(|p| {
                // Create word-boundary aware regex for each pronunciation
                let pattern = format!(r"\b{}\b", regex::escape(&p.word));
                match Regex::new(&pattern) {
                    Ok(regex) => Some((regex, p.pronunciation.clone())),
                    Err(e) => {
                        error!(
                            "Failed to compile pronunciation pattern for '{}': {}",
                            p.word, e
                        );
                        None
                    }
                }
            })
            .collect();
        Self { patterns }
    }

    /// Apply all pronunciation replacements to text
    pub fn apply(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (pattern, replacement) in &self.patterns {
            result = pattern
                .replace_all(&result, replacement.as_str())
                .into_owned();
        }
        result
    }
}

/// Trait for creating HTTP requests for TTS providers
pub trait TTSRequestBuilder: Send + Sync {
    /// Build the HTTP request with provider-specific URL, headers and body
    /// This is the only provider-specific part that needs to be implemented
    ///
    /// # Arguments
    /// * `client` - The HTTP client to use for building the request
    /// * `text` - The text to synthesize
    ///
    /// # Returns
    /// A request builder ready to be sent
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder;

    /// Build the HTTP request with additional context (e.g., previous text)
    /// Default implementation falls back to build_http_request without context
    ///
    /// # Arguments
    /// * `client` - The HTTP client to use for building the request
    /// * `text` - The text to synthesize
    /// * `previous_text` - Optional previous text for context continuity
    ///
    /// # Returns
    /// A request builder ready to be sent
    fn build_http_request_with_context(
        &self,
        client: &reqwest::Client,
        text: &str,
        previous_text: Option<&str>,
    ) -> reqwest::RequestBuilder {
        // Default implementation ignores context
        let _ = previous_text;
        self.build_http_request(client, text)
    }

    /// Get the configuration for this request builder
    fn get_config(&self) -> &TTSConfig;

    /// Get precompiled pronunciation replacer
    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        None
    }

    /// Whether a successful HTTP response is a JSON envelope containing a provider-hosted audio URL
    /// instead of the audio bytes themselves.
    fn expects_provider_audio_url_response(&self) -> bool {
        false
    }

    /// Human-readable provider label used in SSRF rejection errors for provider-hosted audio URLs.
    fn provider_audio_url_label(&self) -> &'static str {
        "TTS provider"
    }

    /// Parse a successful response body and return the provider-hosted audio URL, when applicable.
    fn parse_provider_audio_url_response(&self, _body: &[u8]) -> TTSResult<Option<String>> {
        Ok(None)
    }
}

/// Generic HTTP-based TTS provider implementation using ReqManager
pub struct TTSProvider {
    /// Connection state
    connected: Arc<AtomicBool>,
    /// Audio callback for handling audio data
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
    /// Ordered queue of pending requests (preserves speak() arrival order)
    pending_queue: Arc<Mutex<VecDeque<RequestEntry>>>,
    /// Request manager for HTTP connections
    req_manager: Arc<RwLock<Option<Arc<ReqManager>>>>,
    /// Sequential queue of speak jobs to be processed one at a time
    speak_queue: Arc<Mutex<VecDeque<SpeakJob>>>,
    /// Notification to wake the queue worker when new jobs are added
    speak_queue_notify: Arc<Notify>,
    /// Queue worker task that processes speak jobs sequentially
    queue_worker_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// Flag to quickly check if queue worker is running (avoids lock)
    queue_worker_running: Arc<AtomicBool>,
    /// Notification for dispatcher when new requests are available
    pending_notify: Arc<Notify>,
    /// Dispatcher task that delivers audio in-order to the callback
    dispatcher_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// Flag to quickly check if dispatcher is running (avoids lock)
    dispatcher_running: Arc<AtomicBool>,
    /// Cancellation token that controls current generation lifecycle
    cancel_token: CancellationToken,
    /// Optional cache store for TTS audio
    cache: Arc<RwLock<Option<Arc<CacheStore>>>>,
    /// Precomputed hash for the TTS configuration (stable for provider lifetime)
    tts_config_hash: Arc<RwLock<Option<String>>>,
    /// Previous text for context continuity (session-scoped)
    previous_text: Arc<RwLock<Option<String>>>,
}

impl TTSProvider {
    /// Create a new HTTP-based TTS provider instance
    pub fn new() -> Self {
        Self {
            connected: Arc::new(AtomicBool::new(false)),
            audio_callback: Arc::new(RwLock::new(None)),
            pending_queue: Arc::new(Mutex::new(VecDeque::new())),
            req_manager: Arc::new(RwLock::new(None)),
            speak_queue: Arc::new(Mutex::new(VecDeque::new())),
            speak_queue_notify: Arc::new(Notify::new()),
            queue_worker_task: Arc::new(Mutex::new(None)),
            queue_worker_running: Arc::new(AtomicBool::new(false)),
            pending_notify: Arc::new(Notify::new()),
            dispatcher_task: Arc::new(Mutex::new(None)),
            dispatcher_running: Arc::new(AtomicBool::new(false)),
            cancel_token: CancellationToken::new(),
            cache: Arc::new(RwLock::new(None)),
            tts_config_hash: Arc::new(RwLock::new(None)),
            previous_text: Arc::new(RwLock::new(None)),
        }
    }

    async fn download_provider_audio_url(
        provider: &'static str,
        audio_url: &str,
    ) -> TTSResult<Vec<u8>> {
        let audio_url =
            crate::core::tts::standard::validate_provider_audio_url(provider, audio_url)?;
        let client = crate::core::net::ssrf_protected_client_builder(
            crate::core::tts::standard::PROVIDER_AUDIO_URL_SCHEMES,
        )
        .build()
        .map_err(|e| TTSError::NetworkError(format!("Failed to build HTTP client: {e}")))?;

        let response = client
            .get(audio_url)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to download audio: {e}")))?;

        if !response.status().is_success() {
            return Err(TTSError::ProviderError(format!(
                "Failed to download audio: status {}",
                response.status()
            )));
        }

        let audio = response
            .bytes()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read downloaded audio: {e}")))?
            .to_vec();
        if audio.is_empty() {
            return Err(TTSError::AudioGenerationFailed(
                "Received empty downloaded audio data".to_string(),
            ));
        }

        Ok(audio)
    }

    async fn send_buffered_audio_payload(
        audio: Vec<u8>,
        config: &TTSConfig,
        sender: &mpsc::Sender<Result<AudioPayload, TTSError>>,
        token: &CancellationToken,
        cache_and_key: Option<(Arc<CacheStore>, String)>,
    ) -> TTSResult<()> {
        if audio.is_empty() {
            return Err(TTSError::AudioGenerationFailed(
                "Received empty audio data".to_string(),
            ));
        }

        let encoding = config.audio_format.as_deref().unwrap_or("linear16");
        let sample_rate = config.sample_rate.unwrap_or(24000);
        let mut chunk_target_bytes = pcm_bytes_per_sample(encoding)
            .map(|bytes_per_sample| pcm_stream_chunk_bytes(sample_rate, bytes_per_sample))
            .unwrap_or(0);

        let mut format_override: Option<&'static str> = None;
        if crate::core::tts::sniff::is_pcm_family(encoding)
            && let Some(container) = crate::core::tts::sniff::sniff_container(&audio)
        {
            error!(
                declared = encoding,
                actual = container.as_format_str(),
                "TTS provider returned a container mislabeled as PCM — passing through unsliced \
                 with the sniffed format"
            );
            crate::core::metrics::bridge::record_tts_format_mismatch(&config.provider, "http");
            chunk_target_bytes = 0;
            format_override = Some(container.as_format_str());
        }

        if chunk_target_bytes == 0 {
            if !token.is_cancelled() {
                let _ = sender.send(Ok((audio.clone(), format_override))).await;
            }
        } else {
            for chunk in audio.chunks(chunk_target_bytes) {
                if token.is_cancelled() {
                    break;
                }
                let _ = sender.send(Ok((chunk.to_vec(), None))).await;
            }
        }

        if let Some((cache, key)) = cache_and_key {
            if format_override.is_some() {
                tracing::warn!(
                    "skipping TTS cache write: payload format differs from the declared key format"
                );
            } else if let Err(e) = cache.put(key, audio).await {
                error!("Failed to cache TTS audio: {:?}", e);
            }
        }

        Ok(())
    }

    /// Generic send_request implementation that handles all the common logic
    /// Now accepts trait object for dynamic dispatch in the queue worker
    async fn send_request_dyn(
        request_builder: &dyn TTSRequestBuilderDyn,
        req_manager: Arc<ReqManager>,
        text: String,
        sender: mpsc::Sender<Result<AudioPayload, TTSError>>,
        token: CancellationToken,
        cache_and_key: Option<(Arc<CacheStore>, String)>,
        previous_text_store: Arc<RwLock<Option<String>>>,
    ) {
        if token.is_cancelled() {
            let _ = sender
                .send(Err(TTSError::InternalError("Cancelled".to_string())))
                .await;
            return;
        }

        // Get previous text for context (if available)
        let previous_text = previous_text_store.read().await.clone();

        // Apply pronunciation replacements using precompiled regex patterns
        let processed_text = if let Some(replacer) = request_builder.get_pronunciation_replacer() {
            replacer.apply(&text)
        } else {
            text.clone()
        };

        // Try cache first
        if let Some((cache, key)) = cache_and_key.as_ref() {
            match cache.get(key).await {
                Ok(Some(bytes)) => {
                    // P0.1: never trust a cached blob's declared format. Entries
                    // written before sniffing existed can hold container bytes
                    // under a PCM-declared key — evict and refetch (self-healing)
                    // instead of replaying corruption forever.
                    let declared = request_builder
                        .get_config()
                        .audio_format
                        .as_deref()
                        .unwrap_or("linear16")
                        .to_string();
                    let sniffed = crate::core::tts::sniff::sniff_container(&bytes);
                    if crate::core::tts::sniff::is_pcm_family(&declared)
                        && let Some(container) = sniffed
                    {
                        error!(
                            declared,
                            actual = container.as_format_str(),
                            "cached TTS audio is a container mislabeled as PCM — evicting and refetching"
                        );
                        crate::core::metrics::bridge::record_tts_format_mismatch(
                            &request_builder.get_config().provider,
                            "cache",
                        );
                        if let Err(e) = cache.delete(key).await {
                            error!("Failed to evict mismatched cache entry: {e:?}");
                        }
                        // fall through to the HTTP path below
                    } else {
                        debug!(
                            "Cache HIT - Sending cached audio for text: '{}', {} bytes",
                            processed_text,
                            bytes.len()
                        );
                        if let Err(e) = sender.send(Ok((bytes.clone().to_vec(), None))).await {
                            error!("Failed to send cached audio through channel: {:?}", e);
                        } else {
                            // Update previous_text for context continuity even on cache hit
                            // This ensures next TTS request has proper context (e.g., for ElevenLabs)
                            *previous_text_store.write().await = Some(processed_text.clone());
                            debug!("Updated previous_text on cache hit: '{}'", processed_text);
                        }
                        return;
                    }
                }
                Ok(None) => {
                    debug!("Cache miss for text: '{}'", processed_text);
                }
                Err(e) => {
                    error!("Cache get error: {:?}", e);
                }
            }
        }

        // Acquire a client from the pool
        let client_guard = match req_manager.acquire().await {
            Ok(guard) => guard,
            Err(e) => {
                error!("Failed to acquire HTTP client: {}", e);
                let _ = sender
                    .send(Err(TTSError::NetworkError(format!(
                        "Failed to acquire client: {e}"
                    ))))
                    .await;
                return;
            }
        };

        // Build request with provider-specific URL, headers and body using processed text
        // Pass previous_text for context continuity (ElevenLabs uses this)
        let request = request_builder.build_http_request_with_context(
            client_guard.client(),
            &processed_text,
            previous_text.as_deref(),
        );

        // Send request
        let response_result = request.send().await;
        let config = request_builder.get_config();

        match response_result {
            Ok(response) => {
                if !response.status().is_success() {
                    info!("ERROR Response for text: {}", processed_text);
                    let status = response.status();

                    // Parse Retry-After header for rate limiting (429)
                    let retry_after_secs = response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok());

                    let error_body = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "Unknown error".to_string());

                    // Handle specific HTTP status codes with appropriate error types
                    let tts_error = match status.as_u16() {
                        429 => {
                            warn!(
                                "TTS rate limited. Retry-After: {:?} seconds. Body: {}",
                                retry_after_secs, error_body
                            );
                            TTSError::RateLimited {
                                retry_after_secs,
                                message: error_body,
                            }
                        }
                        401 | 403 => {
                            error!("TTS authentication failed ({}): {}", status, error_body);
                            TTSError::AuthenticationFailed(format!(
                                "API error ({status}): {error_body}"
                            ))
                        }
                        _ => {
                            error!("TTS API error ({}): {}", status, error_body);
                            TTSError::ProviderError(format!("API error ({status}): {error_body}"))
                        }
                    };

                    let _ = sender.send(Err(tts_error)).await;
                    return;
                }

                if request_builder.expects_provider_audio_url_response() {
                    let body = match response.bytes().await {
                        Ok(body) => body,
                        Err(e) => {
                            let _ = sender
                                .send(Err(TTSError::NetworkError(format!(
                                    "Failed to read response body: {e}"
                                ))))
                                .await;
                            return;
                        }
                    };
                    let audio_url = match request_builder.parse_provider_audio_url_response(&body) {
                        Ok(Some(audio_url)) => audio_url,
                        Ok(None) => {
                            let _ = sender
                                .send(Err(TTSError::ProviderError(format!(
                                    "{} response did not include an audio URL",
                                    request_builder.provider_audio_url_label()
                                ))))
                                .await;
                            return;
                        }
                        Err(e) => {
                            let _ = sender.send(Err(e)).await;
                            return;
                        }
                    };

                    match Self::download_provider_audio_url(
                        request_builder.provider_audio_url_label(),
                        &audio_url,
                    )
                    .await
                    {
                        Ok(audio) => {
                            match Self::send_buffered_audio_payload(
                                audio,
                                config,
                                &sender,
                                &token,
                                cache_and_key,
                            )
                            .await
                            {
                                Ok(()) => {
                                    if !token.is_cancelled() {
                                        *previous_text_store.write().await =
                                            Some(processed_text.clone());
                                        debug!(
                                            "Updated previous_text for next request: '{}'",
                                            processed_text
                                        );
                                    }
                                }
                                Err(e) => {
                                    let _ = sender.send(Err(e)).await;
                                }
                            }
                        }
                        Err(e) => {
                            let _ = sender.send(Err(e)).await;
                        }
                    }
                    return;
                }

                // Stream body in chunks
                let encoding = config.audio_format.as_deref().unwrap_or("linear16");
                let sample_rate = config.sample_rate.unwrap_or(24000);
                let mut chunk_target_bytes = pcm_bytes_per_sample(encoding)
                    .map(|bytes_per_sample| pcm_stream_chunk_bytes(sample_rate, bytes_per_sample))
                    .unwrap_or(0);

                let mut buffer: Vec<u8> = Vec::with_capacity(chunk_target_bytes.max(512));
                // Only allocate full_audio buffer if we're caching
                let mut full_audio: Option<Vec<u8>> = if cache_and_key.is_some() {
                    Some(Vec::new())
                } else {
                    None
                };

                let mut stream = response.bytes_stream();
                // P0.1: sniff the stream PRELUDE. If the provider returned a
                // container (WAV/MP3/OGG/FLAC) while a PCM family is declared,
                // slicing it into "PCM" frames corrupts the audio — switch the
                // whole stream to passthrough and label chunks with the truth.
                // Review-found: a container header can be SPLIT across HTTP
                // chunks (RIFF needs 12 bytes), so buffer a small prelude until
                // the verdict is provable instead of deciding on chunk #1 alone.
                const SNIFF_PRELUDE_BYTES: usize = 16;
                let mut format_override: Option<&'static str> = None;
                let mut sniff_pending = chunk_target_bytes > 0;
                let mut prelude: Vec<u8> = Vec::new();
                while let Some(item) = stream.next().await {
                    if token.is_cancelled() {
                        break;
                    }

                    match item {
                        Ok(bytes) => {
                            let mut incoming = bytes.as_ref();

                            if sniff_pending && !incoming.is_empty() {
                                prelude.extend_from_slice(incoming);
                                if prelude.len() < SNIFF_PRELUDE_BYTES {
                                    // Hold judgement until the magic is provable
                                    // (or the stream ends — flushed below).
                                    continue;
                                }
                                sniff_pending = false;
                                if let Some(container) =
                                    crate::core::tts::sniff::sniff_container(&prelude)
                                {
                                    error!(
                                        declared = encoding,
                                        actual = container.as_format_str(),
                                        "TTS provider returned a container mislabeled as PCM — \
                                         passing through unsliced with the sniffed format"
                                    );
                                    crate::core::metrics::bridge::record_tts_format_mismatch(
                                        &config.provider,
                                        "http",
                                    );
                                    chunk_target_bytes = 0; // passthrough for the whole stream
                                    format_override = Some(container.as_format_str());
                                }
                                // Process the buffered prelude through whichever
                                // path the verdict selected.
                                let buffered = std::mem::take(&mut prelude);
                                if chunk_target_bytes == 0 {
                                    if let Some(ref mut full) = full_audio {
                                        full.extend_from_slice(&buffered);
                                    }
                                    let _ = sender.send(Ok((buffered, format_override))).await;
                                    continue;
                                }
                                // PCM verdict: run the prelude through the
                                // aggregation loop below by treating it as the
                                // incoming slice for this iteration.
                                prelude = buffered;
                                incoming = prelude.as_slice();
                            }

                            if chunk_target_bytes == 0 {
                                // Non-PCM/containerized formats: forward chunks as-is
                                let chunk_vec = incoming.to_vec();
                                // Only accumulate if caching
                                if let Some(ref mut full) = full_audio {
                                    full.extend_from_slice(&chunk_vec);
                                }
                                debug!(
                                    "Sending audio chunk ({} bytes) - will wait for receiver...",
                                    chunk_vec.len()
                                );
                                let _ = sender.send(Ok((chunk_vec, format_override))).await;
                                debug!("Audio chunk sent and received");
                                continue;
                            }

                            // PCM-like formats: aggregate into ~10ms chunks
                            while !incoming.is_empty() {
                                let needed = chunk_target_bytes.saturating_sub(buffer.len());
                                let take = needed.min(incoming.len());
                                buffer.extend_from_slice(&incoming[..take]);
                                incoming = &incoming[take..];

                                if buffer.len() >= chunk_target_bytes {
                                    // Take exactly chunk_target_bytes from buffer, leave any excess
                                    let chunk: Vec<u8> =
                                        buffer.drain(..chunk_target_bytes).collect();
                                    // Only accumulate if caching
                                    if let Some(ref mut full) = full_audio {
                                        full.extend_from_slice(&chunk);
                                    }
                                    debug!(
                                        "Sending PCM chunk ({} bytes) - will wait for receiver...",
                                        chunk.len()
                                    );
                                    let _ = sender.send(Ok((chunk, None))).await;
                                    debug!("PCM chunk sent and received");
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to read audio chunk: {}", e);
                            let _ = sender
                                .send(Err(TTSError::AudioGenerationFailed(format!(
                                    "Failed to read audio: {e}"
                                ))))
                                .await;
                            return;
                        }
                    }
                }

                // Flush an undecided prelude (stream ended before 16 bytes —
                // sniff on what we have, then deliver it).
                if sniff_pending && !prelude.is_empty() && !token.is_cancelled() {
                    if let Some(container) = crate::core::tts::sniff::sniff_container(&prelude) {
                        crate::core::metrics::bridge::record_tts_format_mismatch(
                            &config.provider,
                            "http",
                        );
                        format_override = Some(container.as_format_str());
                    }
                    if let Some(ref mut full) = full_audio {
                        full.extend_from_slice(&prelude);
                    }
                    let _ = sender.send(Ok((prelude, format_override))).await;
                }

                // Flush remainder
                if !buffer.is_empty() && !token.is_cancelled() {
                    // Only accumulate if caching
                    if let Some(ref mut full) = full_audio {
                        full.extend_from_slice(&buffer);
                    }
                    debug!(
                        "Sending final buffer ({} bytes) - will wait for receiver...",
                        buffer.len()
                    );
                    let _ = sender.send(Ok((buffer, format_override))).await;
                    debug!("Final buffer sent and received");
                }

                // Store the full audio in cache if provided. NEVER cache a
                // format-mismatched payload — the cache key encodes the
                // DECLARED format, so storing the container would poison the
                // entry for every future hit (P0.1).
                if let Some(((cache, key), full_audio)) = cache_and_key.zip(full_audio) {
                    if format_override.is_some() {
                        tracing::warn!(
                            "skipping TTS cache write: payload format differs from the declared key format"
                        );
                    } else {
                        match cache.put(key, full_audio).await {
                            Ok(_) => {}
                            Err(e) => error!("Failed to cache TTS audio: {:?}", e),
                        }
                    }
                }

                // Update previous_text for context continuity on next request
                // Only update if not cancelled and generation succeeded
                if !token.is_cancelled() {
                    *previous_text_store.write().await = Some(processed_text.clone());
                    debug!(
                        "Updated previous_text for next request: '{}'",
                        processed_text
                    );
                }
            }
            Err(e) => {
                error!("HTTP request failed: {}", e);
                let _ = sender
                    .send(Err(TTSError::NetworkError(format!("Request failed: {e}"))))
                    .await;
            }
        }
    }

    /// Set the request manager for this instance
    pub async fn set_req_manager(&mut self, req_manager: Arc<ReqManager>) {
        *self.req_manager.write().await = Some(req_manager);
    }

    /// Get the request manager
    pub async fn get_req_manager(&self) -> Option<Arc<ReqManager>> {
        self.req_manager.read().await.clone()
    }

    /// Process audio chunks with duration calculation
    pub fn process_audio_chunk(bytes: Vec<u8>, format: &str, sample_rate: u32) -> AudioData {
        let duration_ms = pcm_duration_ms_for_bytes(bytes.len(), format, sample_rate);

        AudioData {
            data: bytes,
            sample_rate,
            format: format.to_string(),
            duration_ms,
        }
    }

    /// Spawn the dispatcher if not already running.
    ///
    /// The dispatcher is responsible for delivering audio to the callback in the
    /// exact order texts were enqueued via `speak`.
    async fn ensure_dispatcher(&self) {
        // OPTIMIZATION: Fast-path check using atomic flag (no lock needed)
        if self.dispatcher_running.load(Ordering::Acquire) {
            return;
        }

        let mut task_guard = self.dispatcher_task.lock().await;
        // Double-check after acquiring lock (may have started while we waited)
        if task_guard.is_some() {
            return;
        }

        let pending_queue = self.pending_queue.clone();
        let audio_callback = self.audio_callback.clone();
        let pending_notify = self.pending_notify.clone();
        let token = self.cancel_token.clone();
        let running_flag = self.dispatcher_running.clone();

        let handle = tokio::spawn(async move {
            debug!("TTS dispatcher task started");
            running_flag.store(true, Ordering::Release);

            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        debug!("TTS dispatcher cancelled");
                        break;
                    }
                    _ = async {
                        // Get next request from queue
                        let entry = {
                            let mut queue = pending_queue.lock().await;
                            let size = queue.len();
                            if size > 0 {
                                debug!("TTS dispatcher: {} requests in queue", size);
                            }
                            queue.pop_front()
                        };

                        let Some(mut entry) = entry else {
                            // No pending items, wait for notification
                            pending_notify.notified().await;
                            return;
                        };

                        debug!("TTS dispatcher processing request from queue");

                        // Use the format and sample rate from this specific request
                        let format = entry.format.clone();
                        let sample_rate = entry.sample_rate;

                        // Get callback once for this text
                        let cb_opt = audio_callback.read().await.clone();

                        // Process all chunks from this request's receiver
                        debug!("TTS dispatcher waiting for audio chunks from receiver");
                        let mut chunk_count = 0;
                        while let Some(result) = entry.receiver.recv().await {
                            match result {
                                Ok((bytes, format_override)) => {
                                    chunk_count += 1;
                                    debug!("TTS dispatcher received chunk #{}: {} bytes", chunk_count, bytes.len());
                                    if let Some(cb) = cb_opt.as_ref() {
                                        // P0.1: a sniff override means the bytes are a
                                        // container — label with the truth and skip the
                                        // PCM duration math (meaningless for containers).
                                        let effective_format: &str =
                                            format_override.unwrap_or(format.as_str());
                                        let duration_ms = pcm_duration_ms_for_bytes(
                                            bytes.len(),
                                            effective_format,
                                            sample_rate,
                                        );

                                        let audio_data = AudioData {
                                            data: bytes,
                                            sample_rate,
                                            format: effective_format.to_string(),
                                            duration_ms,
                                        };
                                        debug!("TTS dispatcher calling audio callback with {} bytes", audio_data.data.len());
                                        cb.on_audio(audio_data).await;
                                        debug!("TTS dispatcher audio callback completed");
                                    }
                                }
                                Err(err) => {
                                    error!("TTS dispatcher received error: {:?}", err);
                                    if let Some(cb) = cb_opt.as_ref() {
                                        cb.on_error(err).await;
                                    }
                                    break;
                                }
                            }
                        }
                        debug!("TTS dispatcher finished processing request with {} chunks", chunk_count);

                        // Only notify completion if this is the last request in the queue
                        // This ensures that multiple consecutive speak() calls only trigger
                        // one completion event after all audio has been generated
                        let is_last_request = {
                            let queue = pending_queue.lock().await;
                            queue.is_empty()
                        };

                        if is_last_request {
                            if let Some(cb) = cb_opt.as_ref() {
                                debug!("TTS dispatcher notifying completion (last request in queue)");
                                cb.on_complete().await;
                            }
                        } else {
                            debug!("TTS dispatcher skipping completion (more requests in queue)");
                        }
                    } => {}
                }
            }

            running_flag.store(false, Ordering::Release);
            debug!("TTS dispatcher task exited");
        });

        *task_guard = Some(handle);
    }

    /// Spawn the queue worker if not already running.
    ///
    /// The queue worker processes speak jobs sequentially - it pulls jobs from the
    /// speak_queue one at a time and only starts the next job after the previous
    /// one has finished streaming all its audio.
    async fn ensure_queue_worker(&self) {
        // OPTIMIZATION: Fast-path check using atomic flag (no lock needed)
        if self.queue_worker_running.load(Ordering::Acquire) {
            return;
        }

        let mut task_guard = self.queue_worker_task.lock().await;
        // Double-check after acquiring lock (may have started while we waited)
        if task_guard.is_some() {
            return;
        }

        let speak_queue = self.speak_queue.clone();
        let speak_queue_notify = self.speak_queue_notify.clone();
        let token = self.cancel_token.clone();
        let running_flag = self.queue_worker_running.clone();
        let previous_text_store = self.previous_text.clone();

        let handle = tokio::spawn(async move {
            debug!("TTS queue worker task started");
            running_flag.store(true, Ordering::Release);

            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        debug!("TTS queue worker cancelled");
                        break;
                    }
                    _ = async {
                        // Wait for notification OR immediately process if queue has items
                        let job = {
                            let mut queue = speak_queue.lock().await;
                            if queue.is_empty() {
                                drop(queue);
                                speak_queue_notify.notified().await;
                                speak_queue.lock().await.pop_front()
                            } else {
                                let size = queue.len();
                                debug!("TTS queue worker: {} jobs in queue", size);
                                queue.pop_front()
                            }
                        };

                        let Some(job) = job else {
                            // No job available after notification, continue waiting
                            return;
                        };

                        debug!("TTS queue worker processing job for text: '{}'", job.text);

                        // Check if cancelled before starting
                        if job.cancel_token.is_cancelled() {
                            debug!("TTS queue worker: job cancelled before start, sending error");
                            let _ = job.sender
                                .send(Err(TTSError::InternalError("Cancelled".to_string())))
                                .await;
                            return;
                        }

                        // Process the job by calling send_request_dyn
                        // This will block until all audio chunks are streamed through the sender
                        Self::send_request_dyn(
                            job.request_builder.as_ref(),
                            job.req_manager,
                            job.text.clone(),
                            job.sender,
                            job.cancel_token,
                            job.cache_and_key,
                            previous_text_store.clone(),
                        )
                        .await;

                        debug!("TTS queue worker finished processing job for text: '{}'", job.text);
                    } => {}
                }
            }

            running_flag.store(false, Ordering::Release);
            debug!("TTS queue worker task exited");
        });

        *task_guard = Some(handle);
    }

    /// Generic connect implementation with TTSConfig-based timeouts
    pub async fn generic_connect(&mut self, _default_url: &str) -> TTSResult<()> {
        // This method now expects the request manager to be pre-configured
        // with the correct timeouts and pool size from TTSConfig
        if self.req_manager.read().await.is_none() {
            return Err(TTSError::ConnectionFailed(
                "Request manager not configured. Call generic_connect_with_config instead."
                    .to_string(),
            ));
        }

        self.connected.store(true, Ordering::Relaxed);
        info!("TTS provider connected and ready");

        Ok(())
    }

    /// Connect with configuration-based request manager
    pub async fn generic_connect_with_config(
        &mut self,
        default_url: &str,
        config: &TTSConfig,
    ) -> TTSResult<()> {
        // Check if we have a request manager
        if self.req_manager.read().await.is_none() {
            // Create request manager with config-based settings
            let pool_size = config.request_pool_size.unwrap_or(4);
            let mut req_config = ReqManagerConfig {
                max_concurrent_requests: pool_size,
                ..Default::default()
            };

            // Apply timeout configurations
            if let Some(connect_timeout) = config.connection_timeout {
                req_config.connect_timeout = Duration::from_secs(connect_timeout);
            }
            if let Some(request_timeout) = config.request_timeout {
                req_config.request_timeout = Duration::from_secs(request_timeout);
            }

            match ReqManager::with_config(req_config).await {
                Ok(manager) => {
                    // Warm up connections using HEAD (more universally supported than OPTIONS)
                    // Note: Some TTS APIs (like DeepGram) don't support HEAD/OPTIONS on /speak
                    // The warmup may fail but that's okay - it just means first request will be slower
                    let _ = manager.warmup(default_url, "HEAD").await;
                    *self.req_manager.write().await = Some(Arc::new(manager));
                    info!(
                        "Created ReqManager with pool_size={}, connect_timeout={:?}s, request_timeout={:?}s",
                        pool_size, config.connection_timeout, config.request_timeout
                    );
                }
                Err(e) => {
                    return Err(TTSError::ConnectionFailed(format!(
                        "Failed to create request manager: {e}"
                    )));
                }
            }
        }

        self.connected.store(true, Ordering::Relaxed);
        info!("TTS provider connected and ready");

        Ok(())
    }

    /// Generic disconnect implementation
    pub async fn generic_disconnect(&mut self) -> TTSResult<()> {
        // Cancel token to stop both dispatcher and queue worker
        self.cancel_token.cancel();

        // Stop queue worker
        {
            let mut worker = self.queue_worker_task.lock().await;
            if let Some(handle) = worker.take() {
                // Notify to wake the worker so it can exit
                self.speak_queue_notify.notify_one();
                crate::core::observability::abort_and_await_task("tts-queue-worker", handle).await;
            }
            self.queue_worker_running.store(false, Ordering::Release);
        }

        // Stop dispatcher
        {
            let mut disp = self.dispatcher_task.lock().await;
            if let Some(handle) = disp.take() {
                crate::core::observability::abort_and_await_task("tts-dispatcher", handle).await;
            }
            self.dispatcher_running.store(false, Ordering::Release);
        }

        // Clear speak queue and send cancellation errors to any pending jobs
        {
            let mut queue = self.speak_queue.lock().await;
            while let Some(job) = queue.pop_front() {
                let _ = job
                    .sender
                    .send(Err(TTSError::InternalError("Disconnected".to_string())))
                    .await;
            }
        }

        // Clear pending queue
        self.pending_queue.lock().await.clear();

        // Clear state
        self.connected.store(false, Ordering::Relaxed);
        *self.audio_callback.write().await = None;
        *self.previous_text.write().await = None;

        // Reset token for next connection
        self.cancel_token = CancellationToken::new();

        info!("TTS provider disconnected");
        Ok(())
    }

    /// Generic speak implementation with request builder
    pub async fn generic_speak<R: TTSRequestBuilder + Clone + 'static>(
        &mut self,
        request_builder: R,
        text: &str,
        _flush: bool,
    ) -> TTSResult<()> {
        if !self.is_ready() {
            return Err(TTSError::ProviderNotReady(
                "Provider not connected".to_string(),
            ));
        }

        // Skip empty text
        if text.is_empty() || text.trim().is_empty() {
            return Ok(());
        }

        // Prepare text and generate hash
        let text_trimmed = text.trim().to_string();
        let text_hash = format!("{:032x}", xxh3_128(text_trimmed.as_bytes()));

        // Create channel for this request with buffer size of 1
        let (sender, receiver) = mpsc::channel::<Result<AudioPayload, TTSError>>(1);

        // Get format and sample rate from current config
        let format = request_builder
            .get_config()
            .audio_format
            .clone()
            .unwrap_or_else(|| "linear16".to_string());
        let sample_rate = request_builder.get_config().sample_rate.unwrap_or(24000);

        // IMPORTANT: Ensure dispatcher is running BEFORE enqueuing job
        // This is critical for cached audio which returns immediately
        self.ensure_dispatcher().await;

        // Add to pending queue FIRST before starting the job
        {
            let mut queue = self.pending_queue.lock().await;
            queue.push_back(RequestEntry {
                receiver,
                format: format.clone(),
                sample_rate,
            });
            debug!(
                "Added request to pending queue for text: '{}', queue size: {}",
                text_trimmed,
                queue.len()
            );
        }

        // Notify dispatcher that a new request is available
        self.pending_notify.notify_one();

        // OPTIMIZATION: Acquire cache-related locks together to reduce lock contention
        let (cache_opt, config_hash_opt) = {
            let cache_guard = self.cache.read().await;
            let hash_guard = self.tts_config_hash.read().await;
            (cache_guard.clone(), hash_guard.clone())
        };

        let cache_and_key = match (cache_opt, config_hash_opt) {
            (Some(cache), Some(cfg_hash)) => Some((cache, format!("{cfg_hash}:{text_hash}"))),
            _ => None,
        };

        // Get request manager
        let req_mgr_opt = self.req_manager.read().await.clone();
        let Some(req_mgr) = req_mgr_opt else {
            return Err(TTSError::InternalError(
                "Request manager not configured".to_string(),
            ));
        };

        let token = self.cancel_token.clone();

        // Create SpeakJob and enqueue it
        let job = SpeakJob {
            text: text_trimmed.clone(),
            request_builder: Box::new(request_builder),
            sender,
            cancel_token: token,
            req_manager: req_mgr,
            cache_and_key,
        };

        // Add job to speak queue
        let queue_len = {
            let mut queue = self.speak_queue.lock().await;
            queue.push_back(job);
            let len = queue.len();
            debug!(
                "Enqueued speak job for text: '{}', speak queue size: {}",
                text_trimmed, len
            );
            len
        };

        // Ensure queue worker is running
        self.ensure_queue_worker().await;

        // Notify the queue worker that new work is available
        self.speak_queue_notify.notify_one();

        debug!(
            "Speak job enqueued successfully for text: '{}' (queue size: {})",
            text_trimmed, queue_len
        );

        Ok(())
    }

    /// Generic clear implementation
    pub async fn generic_clear(&mut self) -> TTSResult<()> {
        // Cancel current generation
        self.cancel_token.cancel();

        // Stop queue worker
        {
            let mut worker = self.queue_worker_task.lock().await;
            if let Some(handle) = worker.take() {
                // Notify to wake the worker so it can exit
                self.speak_queue_notify.notify_one();
                crate::core::observability::abort_and_await_task("tts-queue-worker", handle).await;
            }
            self.queue_worker_running.store(false, Ordering::Release);
        }

        // Stop dispatcher
        {
            let mut disp = self.dispatcher_task.lock().await;
            if let Some(handle) = disp.take() {
                crate::core::observability::abort_and_await_task("tts-dispatcher", handle).await;
            }
            self.dispatcher_running.store(false, Ordering::Release);
        }

        // Clear speak queue and send cancellation errors to any pending jobs
        {
            let mut queue = self.speak_queue.lock().await;
            while let Some(job) = queue.pop_front() {
                debug!("Clearing queued job for text: '{}'", job.text);
                let _ = job
                    .sender
                    .send(Err(TTSError::InternalError("Cancelled".to_string())))
                    .await;
            }
        }

        // Clear pending queue
        self.pending_queue.lock().await.clear();

        // Clear previous_text context
        *self.previous_text.write().await = None;

        // Reset token for next cycle
        self.cancel_token = CancellationToken::new();

        debug!("Cleared speak queue, pending queue, and stopped workers");
        Ok(())
    }

    /// Check if ready
    pub fn is_ready(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Get connection state
    pub fn get_connection_state(&self) -> ConnectionState {
        if self.connected.load(Ordering::Relaxed) {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    /// Generic flush implementation for HTTP-based TTS providers
    ///
    /// # Behavior
    ///
    /// For HTTP-based providers (Deepgram, OpenAI, Play.ht, etc.), flush is a no-op
    /// because each speak() request is immediately sent as an HTTP request. There is
    /// no internal queue to flush.
    ///
    /// The `flush` parameter in `speak(text, flush)` controls:
    /// - `flush=true`: Start synthesis immediately, interrupting any current playback
    /// - `flush=false`: Queue the text to be synthesized after current audio completes
    ///
    /// For WebSocket-based providers (Cartesia, ElevenLabs streaming), the provider's
    /// own flush implementation handles sending queued text immediately.
    ///
    /// # Implementation Notes
    ///
    /// - HTTP providers use concurrent request dispatch - audio is sent to callback
    ///   as it arrives from the API
    /// - No internal buffering or queuing happens at the provider level
    /// - Use `clear()` to cancel pending synthesis (if supported by the provider)
    ///
    /// # Returns
    /// * `TTSResult<()>` - Always succeeds for HTTP providers
    pub async fn generic_flush(&self) -> TTSResult<()> {
        // No-op in concurrent mode; dispatcher processes items as they arrive
        Ok(())
    }

    /// Generic on_audio implementation
    pub fn generic_on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        // Use try_write to avoid blocking
        if let Ok(mut audio_callback) = self.audio_callback.try_write() {
            *audio_callback = Some(callback);
            Ok(())
        } else {
            Err(TTSError::InternalError(
                "Failed to register audio callback".to_string(),
            ))
        }
    }

    /// Generic remove_audio_callback implementation
    pub fn generic_remove_audio_callback(&mut self) -> TTSResult<()> {
        // Use try_write to avoid blocking
        if let Ok(mut audio_callback) = self.audio_callback.try_write() {
            *audio_callback = None;
            Ok(())
        } else {
            Err(TTSError::InternalError(
                "Failed to remove audio callback".to_string(),
            ))
        }
    }

    /// Set cache store for provider
    pub async fn set_cache(&mut self, cache: Arc<CacheStore>) {
        *self.cache.write().await = Some(cache);
    }

    /// Set precomputed TTS config hash for cache keying
    pub async fn set_tts_config_hash(&mut self, hash: String) {
        *self.tts_config_hash.write().await = Some(hash);
    }
}

impl Drop for TTSProvider {
    fn drop(&mut self) {
        // Best-effort cancel workers without awaiting
        // Cancel token to signal workers to stop
        self.cancel_token.cancel();

        // Stop queue worker
        if let Ok(mut worker) = self.queue_worker_task.try_lock()
            && let Some(handle) = worker.take()
        {
            // Wake up the worker so it can exit
            self.speak_queue_notify.notify_one();
            handle.abort();
        }

        // Stop dispatcher
        if let Ok(mut disp) = self.dispatcher_task.try_lock()
            && let Some(handle) = disp.take()
        {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_stream_chunk_bytes_never_returns_zero_for_pcm_family() {
        assert_eq!(pcm_stream_chunk_bytes(0, 2), 2);
        assert_eq!(pcm_stream_chunk_bytes(50, 1), 1);
        assert_eq!(pcm_stream_chunk_bytes(24_000, 2), 480);
    }

    #[test]
    fn process_audio_chunk_rejects_zero_sample_rate_duration() {
        let audio = TTSProvider::process_audio_chunk(vec![0; 320], "linear16", 0);
        assert_eq!(audio.duration_ms, None);
        assert_eq!(audio.sample_rate, 0);
    }

    #[test]
    fn process_audio_chunk_calculates_pcm16_duration() {
        let audio = TTSProvider::process_audio_chunk(vec![0; 480], "pcm16", 24_000);
        assert_eq!(audio.duration_ms, Some(10));
    }

    #[test]
    fn pcm_duration_ms_saturates_instead_of_overflowing() {
        let bytes_len = ((u32::MAX as usize) + 1000) * 2;
        assert_eq!(
            pcm_duration_ms_for_bytes(bytes_len, "linear16", 1),
            Some(u32::MAX)
        );
    }

    #[tokio::test]
    async fn provider_audio_url_download_rejects_unsafe_targets() {
        let _env = crate::core::net::ssrf_env_lock();
        let err = TTSProvider::download_provider_audio_url(
            "test provider",
            "http://127.0.0.1:9000/audio.wav",
        )
        .await
        .expect_err("loopback provider audio URL must be rejected");
        assert!(err.to_string().contains("SSRF protection"), "{err}");

        let err =
            TTSProvider::download_provider_audio_url("test provider", "file:///tmp/audio.wav")
                .await
                .expect_err("non-HTTP provider audio URL must be rejected");
        assert!(err.to_string().contains("not allowed"), "{err}");
    }
}
