use axum::{
    extract::State,
    http::{HeaderName, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use serde::Deserialize;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify};
use tracing::{error, info, warn};

/// Default timeout for TTS synthesis in seconds
const DEFAULT_SPEAK_TIMEOUT_SECS: u64 = 30;

/// Maximum allowed text length in bytes (10KB)
/// This prevents DoS attacks via very long text inputs
const MAX_TEXT_LENGTH: usize = 10 * 1024;

use crate::core::tts::{AudioCallback, AudioData, TTSError, create_tts_provider};
use crate::handlers::ws::config::{TTSWebSocketConfig, client_api_key};
use crate::state::AppState;

/// Request body for the speak endpoint
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SpeakRequest {
    /// The text to synthesize
    #[cfg_attr(feature = "openapi", schema(example = "Hello, world!"))]
    pub text: String,
    /// TTS configuration (without API key)
    pub tts_config: TTSWebSocketConfig,
}

/// Decide what a caller-supplied vendor key in a `/speak` body may do.
///
/// `Ok(Some(key))` uses the caller's key, `Ok(None)` falls back to server config, and `Err`
/// carries the message to return to the caller.
///
/// `allow_client_keys` is what separates the two deployments. Standalone WaaV is BYOK by
/// design, and honouring the caller's key is the feature. Under the Bud control plane the same
/// field is a bypass — the vendor call would carry the caller's own credential, so the request
/// is attributed to no project, counted against no quota and billed to nobody (FRD-018 §5.3.7).
///
/// The bypass is refused rather than ignored: a key that is silently dropped looks like it
/// worked until the vendor answers 401, naming neither WaaV nor the field that caused it.
fn vet_speak_api_key(
    client_key: Option<&str>,
    allow_client_keys: bool,
) -> Result<Option<String>, String> {
    // Normalisation is delegated so this gate and the WebSocket path agree on what counts as a
    // supplied key. `client_api_key` treats None, empty AND whitespace-only as "unset" — the
    // whitespace case matters here, because refusing "   " as a BYOK attempt would fail a
    // request that was not trying to bypass anything.
    match client_api_key(client_key) {
        Some(key) if allow_client_keys => Ok(Some(key)),
        // `/speak` takes no endpoint name -- `resolve_voice_endpoint` is reached only from
        // /v1/audio/speech -- so the message must not tell the caller to use one here.
        Some(_) => Err(
            "Client-supplied api_key in tts_config is not accepted by this gateway. \
             Vendor credentials are owned by the control plane and resolved here; remove the \
             field. To address a specific deployment's credential, call POST /v1/audio/speech \
             with `model` set to your Bud endpoint name."
                .to_string(),
        ),
        None => Ok(None),
    }
}

/// Collector for accumulating audio from TTS provider
struct AudioCollector {
    audio_data: Arc<Mutex<Vec<u8>>>,
    format: Arc<Mutex<Option<String>>>,
    sample_rate: Arc<Mutex<Option<u32>>>,
    completed: Arc<Mutex<bool>>,
    error: Arc<Mutex<Option<TTSError>>>,
    /// Notification for completion - more efficient than polling
    notify: Arc<Notify>,
    /// Request start instant, used to compute time-to-first-byte for metrics.
    start: std::time::Instant,
    /// TTFB in nanoseconds since `start`, set when the first audio chunk arrives
    /// (`u64::MAX` = not yet observed). Lock-free so the audio callback stays cheap.
    first_byte_ns: Arc<std::sync::atomic::AtomicU64>,
}

impl AudioCollector {
    fn new() -> Self {
        Self {
            audio_data: Arc::new(Mutex::new(Vec::new())),
            format: Arc::new(Mutex::new(None)),
            sample_rate: Arc::new(Mutex::new(None)),
            completed: Arc::new(Mutex::new(false)),
            error: Arc::new(Mutex::new(None)),
            notify: Arc::new(Notify::new()),
            start: std::time::Instant::now(),
            first_byte_ns: Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX)),
        }
    }

    /// The measured time-to-first-byte, if any audio was received.
    fn ttfb(&self) -> Option<std::time::Duration> {
        let ns = self
            .first_byte_ns
            .load(std::sync::atomic::Ordering::Relaxed);
        if ns == u64::MAX {
            None
        } else {
            Some(std::time::Duration::from_nanos(ns))
        }
    }

    /// Wait for TTS synthesis to complete with a timeout
    ///
    /// Uses Notify for efficient waiting instead of polling.
    ///
    /// # Arguments
    /// * `timeout_secs` - Maximum time to wait in seconds
    ///
    /// # Returns
    /// * `Ok(())` - Synthesis completed within timeout
    /// * `Err(&'static str)` - Timeout elapsed before completion
    async fn wait_for_completion(&self, timeout_secs: u64) -> Result<(), &'static str> {
        // Check if already completed (avoids unnecessary wait)
        if *self.completed.lock().await {
            return Ok(());
        }

        // Wait for notification with timeout (efficient, no polling)
        let timeout = Duration::from_secs(timeout_secs);
        match tokio::time::timeout(timeout, self.notify.notified()).await {
            Ok(()) => Ok(()),
            Err(_elapsed) => {
                warn!("TTS synthesis timeout after {}s", timeout_secs);
                Err("TTS synthesis timeout")
            }
        }
    }

    async fn get_result(&self) -> Result<(Vec<u8>, String, u32), TTSError> {
        if let Some(err) = self.error.lock().await.clone() {
            return Err(err);
        }

        let audio = self.audio_data.lock().await.clone();
        if audio.is_empty() {
            return Err(TTSError::AudioGenerationFailed(
                "TTS synthesis completed without audio".to_string(),
            ));
        }
        let format = self.format.lock().await.clone().ok_or_else(|| {
            TTSError::InternalError("TTS audio completed without format metadata".to_string())
        })?;
        let sample_rate = self.sample_rate.lock().await.ok_or_else(|| {
            TTSError::InternalError("TTS audio completed without sample_rate metadata".to_string())
        })?;

        Ok((audio, format, sample_rate))
    }

    async fn fail(&self, error: TTSError) {
        *self.error.lock().await = Some(error);
        *self.completed.lock().await = true;
        self.notify.notify_waiters();
    }
}

impl AudioCallback for AudioCollector {
    fn on_audio(
        &self,
        audio_data: AudioData,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            if *self.completed.lock().await || self.error.lock().await.is_some() {
                return;
            }

            if audio_data.sample_rate == 0 {
                self.fail(TTSError::ProviderError(
                    "TTS provider emitted audio with zero sample_rate".to_string(),
                ))
                .await;
                return;
            }
            if audio_data.format.trim().is_empty() {
                self.fail(TTSError::ProviderError(
                    "TTS provider emitted audio with empty format".to_string(),
                ))
                .await;
                return;
            }

            // Record time-to-first-byte exactly once (first chunk with data wins).
            if !audio_data.data.is_empty() {
                use std::sync::atomic::Ordering;
                let elapsed = self.start.elapsed().as_nanos() as u64;
                // Only set if still unset (u64::MAX sentinel); ignore the race loser.
                let _ = self.first_byte_ns.compare_exchange(
                    u64::MAX,
                    elapsed,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }

            // Store format and sample rate from first chunk; every later chunk must agree.
            let mut format = self.format.lock().await;
            let mut sample_rate = self.sample_rate.lock().await;
            match (&*format, *sample_rate) {
                (None, None) => {
                    *format = Some(audio_data.format.clone());
                    *sample_rate = Some(audio_data.sample_rate);
                }
                (Some(existing_format), Some(existing_rate))
                    if existing_format == &audio_data.format
                        && existing_rate == audio_data.sample_rate => {}
                (Some(existing_format), Some(existing_rate)) => {
                    let message = format!(
                        "TTS provider emitted inconsistent audio metadata: first format={existing_format:?}, sample_rate={existing_rate}; later format={:?}, sample_rate={}",
                        audio_data.format, audio_data.sample_rate
                    );
                    drop(sample_rate);
                    drop(format);
                    self.fail(TTSError::ProviderError(message)).await;
                    return;
                }
                _ => {
                    drop(sample_rate);
                    drop(format);
                    self.fail(TTSError::InternalError(
                        "TTS collector metadata state became inconsistent".to_string(),
                    ))
                    .await;
                    return;
                }
            }
            drop(sample_rate);
            drop(format);

            // Accumulate audio data
            self.audio_data
                .lock()
                .await
                .extend_from_slice(&audio_data.data);
        })
    }

    fn on_error(
        &self,
        error: TTSError,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            *self.error.lock().await = Some(error);
            *self.completed.lock().await = true;
            // Wake up any waiting task
            self.notify.notify_waiters();
        })
    }

    fn on_complete(&self) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            *self.completed.lock().await = true;
            // Wake up any waiting task
            self.notify.notify_waiters();
        })
    }
}

/// Handler for the /speak endpoint
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/speak",
        request_body = SpeakRequest,
        responses(
            (status = 200, description = "Audio generated successfully",
                content_type = "audio/pcm",
                headers(
                    ("x-audio-format" = String, description = "Audio format (linear16, mp3, etc.)"),
                    ("x-sample-rate" = u32, description = "Sample rate in Hz")
                )
            ),
            (status = 400, description = "Invalid request (empty text)"),
            (status = 500, description = "TTS synthesis failed")
        ),
        security(
            ("bearer_auth" = [])
        ),
        tag = "tts"
    )
)]
pub async fn speak_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SpeakRequest>,
) -> Response {
    info!(
        "Speak request received - provider: {}, text length: {}",
        request.tts_config.provider,
        request.text.len()
    );

    // Validate text is not empty
    if request.text.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Text cannot be empty"
            })),
        )
            .into_response();
    }

    // Validate text length to prevent DoS
    if request.text.len() > MAX_TEXT_LENGTH {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "Text too long: {} bytes exceeds maximum {} bytes",
                    request.text.len(),
                    MAX_TEXT_LENGTH
                )
            })),
        )
            .into_response();
    }

    // Get API key: a client-provided key takes priority over server config (BYOK pattern),
    // except under the Bud control plane, which owns the tenant's credentials
    let client_key = match vet_speak_api_key(
        request.tts_config.api_key.as_deref(),
        state.allows_client_supplied_keys(),
    ) {
        Ok(key) => key,
        Err(message) => {
            warn!(
                provider = %request.tts_config.provider,
                "Refused client-supplied API key: vendor credentials are owned by the control plane"
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": message })),
            )
                .into_response();
        }
    };

    let api_key = if let Some(client_key) = client_key {
        info!(
            "Using client-provided API key for provider: {}",
            request.tts_config.provider
        );
        client_key
    } else {
        // Fall back to server config
        match state.config.get_api_key(&request.tts_config.provider) {
            Ok(key) => key,
            Err(e) => {
                error!(
                    "Failed to get API key for {}: {}",
                    request.tts_config.provider, e
                );
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("API key not configured for provider: {}", request.tts_config.provider)
                    })),
                )
                    .into_response();
            }
        }
    };

    // Convert WebSocket config to full TTSConfig with API key
    let tts_config = request.tts_config.to_tts_config(api_key);

    // Apply pronunciation replacements — whole words only, via the same matcher the streaming
    // providers use. A plain substring replace turned "Bud -> Buddy" into "Buddyget".
    let processed_text = apply_pronunciations(&request.text, &tts_config.pronunciations);

    // Create TTS provider
    let mut tts_provider = match create_tts_provider(&tts_config.provider, tts_config.clone()) {
        Ok(provider) => provider,
        Err(e) => {
            error!("Failed to create TTS provider: {:?}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to create TTS provider: {}", e)
                })),
            )
                .into_response();
        }
    };

    // Set the request manager from state if available
    // This enables HTTP connection pooling and metrics tracking
    if let Some(req_manager) = state.get_tts_req_manager(&tts_config.provider).await {
        // First try TTSProvider (for streaming TTS providers)
        if let Some(provider) = tts_provider.get_provider() {
            provider.set_req_manager(req_manager.clone()).await;
        }
        // Also call on BaseTTS trait (for providers like Google TTS)
        tts_provider.set_req_manager(req_manager).await;
    }

    // Connect to provider
    if let Err(e) = tts_provider.connect().await {
        error!("Failed to connect to TTS provider: {:?}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("Failed to connect to TTS provider: {}", e)
            })),
        )
            .into_response();
    }

    // Create audio collector
    let collector = Arc::new(AudioCollector::new());

    // Register callback
    if let Err(e) = tts_provider.on_audio(collector.clone()) {
        error!("Failed to register audio callback: {:?}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("Failed to register audio callback: {}", e)
            })),
        )
            .into_response();
    }

    // Synthesize speech
    if let Err(e) = tts_provider.speak(&processed_text, true).await {
        error!("Failed to synthesize speech: {:?}", e);
        state
            .core_state
            .metrics
            .provider(&tts_config.provider, crate::core::metrics::channel::TTS)
            .record_outcome(false, collector.ttfb(), collector.start.elapsed());
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("Failed to synthesize speech: {}", e)
            })),
        )
            .into_response();
    }

    // Wait for completion with timeout
    if let Err(e) = collector
        .wait_for_completion(DEFAULT_SPEAK_TIMEOUT_SECS)
        .await
    {
        // Disconnect on timeout
        let _ = tts_provider.disconnect().await;
        return (
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({
                "error": e
            })),
        )
            .into_response();
    }

    // Disconnect
    let _ = tts_provider.disconnect().await;

    // Get result
    let (audio_data, format, sample_rate) = match collector.get_result().await {
        Ok(result) => result,
        Err(e) => {
            error!("TTS synthesis error: {:?}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("TTS synthesis error: {}", e)
                })),
            )
                .into_response();
        }
    };

    info!(
        "TTS synthesis successful - {} bytes, format: {}, sample_rate: {}",
        audio_data.len(),
        format,
        sample_rate
    );

    // Record provider metrics (W-C1): total request time + TTFB feed both the in-memory
    // snapshot and the Prometheus exposition served at /metrics
    // (waav_provider_requests_total / waav_provider_ttfb_ms).
    state
        .core_state
        .metrics
        .provider(&tts_config.provider, crate::core::metrics::channel::TTS)
        .record_outcome(true, collector.ttfb(), collector.start.elapsed());

    // Determine content type
    let content_type = match format.as_str() {
        "wav" => "audio/wav",
        "mp3" | "mpeg" => "audio/mpeg",
        "ogg" | "opus" => "audio/ogg",
        "linear16" | "pcm" => "audio/pcm",
        "mulaw" => "audio/basic",
        _ => "application/octet-stream",
    };

    // Return binary audio with headers
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::CONTENT_LENGTH,
                audio_data.len().to_string().as_str(),
            ),
            (HeaderName::from_static("x-audio-format"), format.as_str()),
            (
                HeaderName::from_static("x-sample-rate"),
                sample_rate.to_string().as_str(),
            ),
        ],
        audio_data,
    )
        .into_response()
}

/// Synthesise once and return the whole buffer, with no HTTP shape attached.
///
/// Extracted from [`speak_handler`] so the OpenAI-compatible `/v1/audio/speech` route (FRD-018)
/// drives exactly the same provider path rather than a parallel copy of it — two synthesis
/// implementations would drift on timeouts, pronunciation handling and connection pooling, and
/// the drift would only show on one of the two routes.
///
/// Returns `(audio, format, sample_rate)`.
pub async fn synthesize_once(
    state: &AppState,
    tts_config: crate::core::tts::TTSConfig,
    text: &str,
) -> Result<(Vec<u8>, String, u32), String> {
    // The flat config with no canonical features — the shape every caller had before FRD-018
    // Part III C3, preserved so `/speak` and the WS path are untouched by that change.
    synthesize_once_standard(
        state,
        crate::core::tts::standard::StandardTTSConfig::from_base(tts_config),
        text,
    )
    .await
    .map_err(|e| e.to_string())
}

/// Apply a deployment's pronunciation replacements to text, whole words only.
///
/// The one function both one-shot synthesis paths call, so the rule cannot differ between them.
/// It used to be a plain substring `replace` at each site, which rewrote the inside of longer
/// words — "Bud -> Buddy" spoke "Budget" as "Buddyget". `PronunciationReplacer` is the
/// word-boundary matcher the streaming providers already used.
pub(crate) fn apply_pronunciations(
    text: &str,
    pronunciations: &[crate::core::tts::Pronunciation],
) -> String {
    if pronunciations.is_empty() {
        return text.to_string();
    }
    crate::core::tts::provider::PronunciationReplacer::new(pronunciations).apply(text)
}

/// Why a one-shot synthesis failed, split by who can fix it.
///
/// The OpenAI route answers these two differently — 400 and 502 — and a single string could not
/// tell them apart, so a vendor's "that voice does not exist" reached the caller as a gateway
/// fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthesisError {
    /// The vendor refused the request as built ([`TTSError::RequestRejected`]). Fixed by changing
    /// the request or the deployment.
    Rejected(String),
    /// Anything else: provider construction, the connection, a timeout, or the vendor failing a
    /// well-formed request.
    Failed(String),
}

impl std::fmt::Display for SynthesisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

impl From<TTSError> for SynthesisError {
    fn from(e: TTSError) -> Self {
        match e {
            TTSError::RequestRejected(m) => Self::Rejected(m),
            other => Self::Failed(format!("synthesis error: {other}")),
        }
    }
}

/// Synthesise once from the STANDARD config, so a caller's canonical features reach the provider.
///
/// FRD-018 Part III C3. `create_tts_standard` is the dispatch that calls each provider's
/// `from_standard`; `create_tts_provider` drops `features` on the floor by construction, because
/// the flat config has nowhere to put them. Providers without a `from_standard` arm fall back to
/// the flat path inside that dispatch, so this is safe for every vendor including self-hosted.
pub async fn synthesize_once_standard(
    state: &AppState,
    std_config: crate::core::tts::standard::StandardTTSConfig,
    text: &str,
) -> Result<(Vec<u8>, String, u32), SynthesisError> {
    let tts_config = std_config.base.clone();

    // Pronunciation replacements apply to every synthesis path, not just the native one.
    // Whole words only (see `PronunciationReplacer`): a substring replace rewrote the inside of
    // longer words — "Bud -> Buddy" spoke "Budget" as "Buddyget".
    let processed = apply_pronunciations(text, &tts_config.pronunciations);
    // The provider skips blank text without queueing a request, so nothing would ever complete
    // and this would wait out the full timeout before failing as if the vendor were down.
    if processed.trim().is_empty() {
        return Err(SynthesisError::Rejected(
            "the text to synthesise is empty or whitespace; nothing was sent to the vendor"
                .to_string(),
        ));
    }

    let mut provider =
        crate::core::tts::standard::create_tts_standard(&tts_config.provider, std_config)
            .map_err(|e| SynthesisError::Failed(format!("failed to create TTS provider: {e}")))?;

    // Connection pooling and per-provider metrics come from the shared manager; without this
    // the OpenAI route would open a fresh connection per request while `/speak` reuses them.
    if let Some(req_manager) = state.get_tts_req_manager(&tts_config.provider).await {
        if let Some(p) = provider.get_provider() {
            p.set_req_manager(req_manager.clone()).await;
        }
        provider.set_req_manager(req_manager).await;
    }

    provider
        .connect()
        .await
        .map_err(|e| SynthesisError::Failed(format!("failed to connect to TTS provider: {e}")))?;

    let collector = Arc::new(AudioCollector::new());
    provider
        .on_audio(collector.clone())
        .map_err(|e| SynthesisError::Failed(format!("failed to register audio callback: {e}")))?;

    if let Err(e) = provider.speak(&processed, true).await {
        let _ = provider.disconnect().await;
        return Err(SynthesisError::Failed(format!("synthesis failed: {e}")));
    }

    if let Err(e) = collector
        .wait_for_completion(DEFAULT_SPEAK_TIMEOUT_SECS)
        .await
    {
        // Always disconnect on the timeout path: leaking the connection is how a slow vendor
        // turns into exhausted file descriptors.
        let _ = provider.disconnect().await;
        return Err(SynthesisError::Failed(e.to_string()));
    }

    let _ = provider.disconnect().await;

    collector.get_result().await.map_err(SynthesisError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pronunciations_replace_whole_words_only() {
        use crate::core::tts::Pronunciation;
        let list = [Pronunciation {
            word: "Bud".into(),
            pronunciation: "Buddy".into(),
        }];
        // Live: "Check the Budget." was spoken as "Check the Buddyget".
        assert_eq!(
            apply_pronunciations("Hello Bud, check the Budget.", &list),
            "Hello Buddy, check the Budget."
        );
        assert_eq!(apply_pronunciations("Bud.", &list), "Buddy.");
        assert_eq!(apply_pronunciations("no change", &[]), "no change");
    }

    #[test]
    fn only_a_vendor_refusal_is_a_rejection() {
        assert_eq!(
            SynthesisError::from(TTSError::RequestRejected("elevenlabs rejected".into())),
            SynthesisError::Rejected("elevenlabs rejected".into())
        );
        for other in [
            TTSError::ProviderError("down".into()),
            TTSError::AuthenticationFailed("key".into()),
            TTSError::InvalidConfiguration("cfg".into()),
        ] {
            assert!(
                matches!(
                    SynthesisError::from(other.clone()),
                    SynthesisError::Failed(_)
                ),
                "{other:?} must stay a failure"
            );
        }
    }

    #[test]
    fn test_client_api_key_refused_when_bud_owns_credentials() {
        let message = vet_speak_api_key(Some("sk-caller-owned"), false)
            .expect_err("a client key must not resolve under Bud mode");

        assert!(
            message.contains("tts_config"),
            "error must name the field to remove: {message}"
        );
        assert!(
            !message.contains("sk-caller-owned"),
            "the key must not be echoed back to the caller: {message}"
        );
    }

    #[test]
    fn test_client_api_key_honoured_in_standalone_mode() {
        let resolved = vet_speak_api_key(Some("sk-caller-owned"), true).unwrap();

        assert_eq!(resolved.as_deref(), Some("sk-caller-owned"));
    }

    #[test]
    fn test_client_api_key_is_trimmed_in_standalone_mode() {
        // Normalisation is delegated to `client_api_key`; this pins that the delegation is
        // actually in effect, so `/speak` and the WebSocket path cannot disagree about what a
        // supplied key is.
        let resolved = vet_speak_api_key(Some("  sk-caller-owned  "), true).unwrap();

        assert_eq!(resolved.as_deref(), Some("sk-caller-owned"));
    }

    #[test]
    fn test_empty_client_api_key_falls_back_under_bud_mode() {
        let resolved = vet_speak_api_key(Some(""), false)
            .expect("an empty key bypasses nothing and must not fail the request");

        assert_eq!(resolved, None, "an empty key falls back to server config");
    }

    #[test]
    fn test_whitespace_client_api_key_falls_back_rather_than_being_refused() {
        // A whitespace-only value is "unset" spelled badly, not an attempt to bypass the
        // control plane. Refusing it would fail a request that was not doing anything wrong —
        // the case the pre-merge `!key.is_empty()` check got wrong.
        let resolved = vet_speak_api_key(Some("   "), false)
            .expect("a whitespace-only key bypasses nothing and must not fail the request");

        assert_eq!(resolved, None);
    }

    #[test]
    fn test_absent_client_api_key_falls_back_under_bud_mode() {
        assert_eq!(vet_speak_api_key(None, false).unwrap(), None);
    }

    fn chunk(data: &[u8], sample_rate: u32, format: &str) -> AudioData {
        AudioData {
            data: data.to_vec(),
            sample_rate,
            format: format.to_string(),
            duration_ms: None,
        }
    }

    #[tokio::test]
    async fn audio_collector_rejects_completion_without_audio_instead_of_default_headers() {
        let collector = AudioCollector::new();

        collector.on_complete().await;
        let err = collector
            .get_result()
            .await
            .expect_err("empty completion must not default to linear16/24000");

        assert!(
            matches!(err, TTSError::AudioGenerationFailed(ref message) if message.contains("without audio")),
            "unexpected empty-audio error: {err:?}"
        );
    }

    #[tokio::test]
    async fn audio_collector_rejects_zero_sample_rate_before_accumulating_audio() {
        let collector = AudioCollector::new();

        collector.on_audio(chunk(&[1, 2, 3], 0, "linear16")).await;
        let err = collector
            .get_result()
            .await
            .expect_err("zero sample-rate metadata must fail");

        assert!(
            matches!(err, TTSError::ProviderError(ref message) if message.contains("zero sample_rate")),
            "unexpected zero-rate error: {err:?}"
        );
        assert!(
            collector.audio_data.lock().await.is_empty(),
            "invalid zero-rate audio must not be appended"
        );
    }

    #[tokio::test]
    async fn audio_collector_rejects_mixed_chunk_metadata_before_flattening() {
        let collector = AudioCollector::new();

        collector.on_audio(chunk(&[1, 2], 24_000, "linear16")).await;
        collector.on_audio(chunk(&[3, 4], 44_100, "linear16")).await;
        let err = collector
            .get_result()
            .await
            .expect_err("mixed sample-rate chunks must fail");

        assert!(
            matches!(err, TTSError::ProviderError(ref message) if message.contains("inconsistent audio metadata")),
            "unexpected mixed-metadata error: {err:?}"
        );
        assert_eq!(
            collector.audio_data.lock().await.as_slice(),
            &[1, 2],
            "the invalid second chunk must not be appended"
        );
    }
}
