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

    // Get API key: Client-provided key takes priority over server config (BYOK pattern)
    // This allows multi-tenant setups where clients bring their own API keys
    let api_key = if let Some(client_key) = client_api_key(request.tts_config.api_key.as_deref()) {
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

    // Apply pronunciation replacements
    let mut processed_text = request.text.clone();
    for pronunciation in &tts_config.pronunciations {
        processed_text = processed_text.replace(&pronunciation.word, &pronunciation.pronunciation);
    }

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

#[cfg(test)]
mod tests {
    use super::*;

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
