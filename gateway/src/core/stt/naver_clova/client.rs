//! NAVER CLOVA Speech Recognition (CSR) STT client implementation.
//!
//! This module provides a REST API-based STT client for the NAVER Cloud Platform
//! CLOVA Speech Recognition service.
//!
//! # Protocol
//!
//! Unlike WebSocket-based STT providers, NAVER CLOVA CSR uses a REST API.
//! Audio is collected in a buffer and sent in a single batch request when
//! `disconnect()` is called or when the buffer reaches the maximum duration.

use super::config::{
    MAX_AUDIO_DURATION_SECONDS, NAVER_CSR_ENDPOINT, NaverClovaAudioFormat, NaverClovaSttConfig,
    NaverClovaSttResponse,
};
use crate::core::stt::http_resilience::HttpBreaker;

use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use crate::core::stt::wav as stt_wav;
use bytes::Bytes;
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// NAVER CLOVA Speech Recognition (CSR) STT client.
///
/// This client implements a batch-based approach where audio is buffered
/// and transcribed when the connection is closed or buffer is full.
pub struct NaverClovaStt {
    /// Provider configuration
    config: NaverClovaSttConfig,
    /// HTTP client for REST API calls
    http_client: Client,
    /// Audio buffer for batch processing
    audio_buffer: Arc<Mutex<Vec<u8>>>,
    /// Whether the client is ready to receive audio
    is_ready: AtomicBool,
    /// Callback for transcription results
    result_callback: Arc<Mutex<Option<STTResultCallback>>>,
    /// Callback for errors
    error_callback: Arc<Mutex<Option<STTErrorCallback>>>,
    /// Original STT config for get_config()
    original_config: STTConfig,
    /// Shared per-provider circuit breaker for the REST transport (uniform with the WS fleet):
    /// consulted before each upstream call, fed by the unified HTTP status classification.
    /// Inert until `set_resilience` injects the process-global handles (W-D2).
    resilience: HttpBreaker,
}

fn naver_clova_stt_http_client() -> Result<Client, reqwest::Error> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
        .timeout(std::time::Duration::from_secs(30))
        .build()
}

impl NaverClovaStt {
    /// W1 keystone — construct from the standardized config. NAVER CLOVA CSR is a batch (REST)
    /// recognizer whose config exposes none of the standardized advanced features, so this is a
    /// uniform standardized entry point that delegates to `from_standard` (a pure `from_base`
    /// passthrough): the base config is carried through unchanged and every advanced feature is a
    /// capability gap left at default.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let naver_config = NaverClovaSttConfig::from_standard(std)?;

        let http_client = naver_clova_stt_http_client().map_err(|e| {
            STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
        })?;

        Ok(Self {
            config: naver_config,
            http_client,
            audio_buffer: Arc::new(Mutex::new(Vec::new())),
            is_ready: AtomicBool::new(false),
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            original_config: std.base.clone(),
            resilience: HttpBreaker::new("naver_clova"),
        })
    }

    /// The shared circuit breaker this client feeds, if the process-global resilience handles
    /// have been injected (W-D2). Two `NaverClovaStt` built from the same
    /// [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(&self) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.breaker()
    }

    /// Calculate the maximum audio buffer size in bytes based on configuration.
    fn max_buffer_bytes(&self) -> usize {
        // Calculate max bytes for 60 seconds of audio
        // Formula: sample_rate * bytes_per_sample * channels * duration_seconds
        let bytes_per_sample = 2; // 16-bit PCM = 2 bytes
        let channels = 1; // Mono
        self.config.sample_rate as usize * bytes_per_sample * channels * MAX_AUDIO_DURATION_SECONDS
    }

    /// Build the API endpoint URL with language parameter. When `endpoint_override` is set (the
    /// standardized override, used by the credential-free mock harness), its scheme://host replaces
    /// the production host while the `/recog/v1/stt` path and `?lang=` query are preserved.
    fn build_endpoint_url(&self) -> String {
        let base = if let Some(ref ov) = self.config.endpoint_override {
            format!("{}/recog/v1/stt", ov.trim_end_matches('/'))
        } else {
            NAVER_CSR_ENDPOINT.to_string()
        };
        format!("{base}?lang={}", self.config.language.code())
    }

    /// Transcribe the buffered audio using the REST API.
    async fn transcribe_buffer(&self) -> Result<String, STTError> {
        let buffer = {
            let guard = self.audio_buffer.lock().await;
            guard.clone()
        };

        if buffer.is_empty() {
            debug!("NAVER CLOVA: No audio to transcribe");
            return Ok(String::new());
        }

        let buffer_len = buffer.len();
        let duration_secs = buffer_len as f32 / (self.config.sample_rate as f32 * 2.0); // 2 bytes per sample for 16-bit PCM

        info!(
            "NAVER CLOVA: Transcribing {} bytes ({:.2} seconds) of audio",
            buffer_len, duration_secs
        );

        // Add WAV header if format is PCM (CSR expects raw audio with proper content-type)
        let audio_data = match self.config.audio_format {
            NaverClovaAudioFormat::Pcm => buffer,
            NaverClovaAudioFormat::Wav => self.add_wav_header(&buffer)?,
            NaverClovaAudioFormat::Mp3 => buffer,
        };

        let content_type = self.config.audio_format.content_type();
        let url = self.build_endpoint_url();

        debug!(
            "NAVER CLOVA: Sending request to {} with content-type {}",
            url, content_type
        );

        // Consult the shared per-provider breaker before paying the upstream round-trip: an
        // open breaker fails fast with a typed classified refusal (uniform with the WS fleet).
        self.resilience.check()?;

        let response = self
            .http_client
            .post(&url)
            .header("X-NCP-APIGW-API-KEY-ID", &self.config.client_id)
            .header("X-NCP-APIGW-API-KEY", &self.config.client_secret)
            .header("Content-Type", content_type)
            .body(audio_data)
            .send()
            .await
            .map_err(|e| {
                self.resilience.record_send_error();
                STTError::NetworkError(format!("Failed to send audio: {e}"))
            })?;

        let status = response.status();
        self.resilience.record_status(status);
        let body = response
            .text()
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to read response: {e}")))?;

        if !status.is_success() {
            error!(
                "NAVER CLOVA: API error (status {}): {}",
                status.as_u16(),
                body
            );

            // Try to parse error response
            if let Ok(error_resp) = serde_json::from_str::<serde_json::Value>(&body) {
                let error_code = error_resp
                    .get("errorCode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let error_message = error_resp
                    .get("errorMessage")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&body);

                return match status.as_u16() {
                    401 => Err(STTError::AuthenticationFailed(format!(
                        "Invalid credentials: {error_message}"
                    ))),
                    413 => Err(STTError::AudioProcessingError(format!(
                        "Audio too long (max 60 seconds): {error_message}"
                    ))),
                    429 => Err(STTError::ProviderError(format!(
                        "Rate limit exceeded: {error_message}"
                    ))),
                    _ => Err(STTError::ProviderError(format!(
                        "API error {}: {} - {}",
                        error_code, status, error_message
                    ))),
                };
            }

            return Err(STTError::ProviderError(format!(
                "API error (status {}): {}",
                status.as_u16(),
                body
            )));
        }

        // Parse successful response
        let result: NaverClovaSttResponse = serde_json::from_str(&body)
            .map_err(|e| STTError::ProviderError(format!("Failed to parse response: {e}")))?;

        info!("NAVER CLOVA: Transcription result: '{}'", result.text);

        Ok(result.text)
    }

    /// Add a WAV header to raw PCM data.
    fn add_wav_header(&self, pcm_data: &[u8]) -> Result<Vec<u8>, STTError> {
        stt_wav::encode_pcm16_wav(pcm_data, self.config.sample_rate, 1)
            .map_err(|e| STTError::AudioProcessingError(format!("Invalid WAV parameters: {e}")))
    }

    /// Clear the audio buffer.
    async fn clear_buffer(&self) {
        let mut guard = self.audio_buffer.lock().await;
        guard.clear();
    }
}

#[async_trait::async_trait]
impl BaseSTT for NaverClovaStt {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        let naver_config = NaverClovaSttConfig::from_base(config.clone())?;

        let http_client = naver_clova_stt_http_client().map_err(|e| {
            STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
        })?;

        info!(
            "NAVER CLOVA STT: Initialized with language={}, sample_rate={}",
            naver_config.language.code(),
            naver_config.sample_rate
        );

        Ok(Self {
            config: naver_config,
            http_client,
            audio_buffer: Arc::new(Mutex::new(Vec::new())),
            is_ready: AtomicBool::new(false),
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            original_config: config,
            resilience: HttpBreaker::new("naver_clova"),
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        // NAVER CLOVA uses REST API, so no persistent connection needed
        // Just verify credentials are present and mark as ready
        if self.config.client_id.is_empty() || self.config.client_secret.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Client ID and Client Secret are required".to_string(),
            ));
        }

        self.clear_buffer().await;
        self.is_ready.store(true, Ordering::SeqCst);

        info!("NAVER CLOVA STT: Ready to receive audio");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        if !self.is_ready.load(Ordering::SeqCst) {
            return Ok(());
        }

        // Transcribe any remaining audio in the buffer
        let buffer_len = {
            let guard = self.audio_buffer.lock().await;
            guard.len()
        };

        if buffer_len > 0 {
            info!("NAVER CLOVA STT: Transcribing remaining audio on disconnect");
            match self.transcribe_buffer().await {
                Ok(text) => {
                    if !text.is_empty() {
                        // Send result via callback
                        let callback_guard = self.result_callback.lock().await;
                        if let Some(callback) = callback_guard.as_ref() {
                            let result = STTResult::new(text, true, true, 1.0);
                            callback(result).await;
                        }
                    }
                }
                Err(e) => {
                    warn!("NAVER CLOVA STT: Error transcribing on disconnect: {e}");
                    // Send error via callback
                    let error_guard = self.error_callback.lock().await;
                    if let Some(callback) = error_guard.as_ref() {
                        callback(e).await;
                    }
                }
            }
        }

        self.clear_buffer().await;
        self.is_ready.store(false, Ordering::SeqCst);

        info!("NAVER CLOVA STT: Disconnected");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_ready.load(Ordering::SeqCst)
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        let max_bytes = self.max_buffer_bytes();

        {
            let mut guard = self.audio_buffer.lock().await;

            // Check if adding this audio would exceed the limit
            if guard.len() + audio_data.len() > max_bytes {
                // Buffer is full, transcribe current buffer first
                drop(guard); // Release lock before transcribing

                info!("NAVER CLOVA STT: Buffer full, transcribing...");
                match self.transcribe_buffer().await {
                    Ok(text) => {
                        if !text.is_empty() {
                            let callback_guard = self.result_callback.lock().await;
                            if let Some(callback) = callback_guard.as_ref() {
                                let result = STTResult::new(text, true, true, 1.0);
                                callback(result).await;
                            }
                        }
                    }
                    Err(e) => {
                        let error_guard = self.error_callback.lock().await;
                        if let Some(callback) = error_guard.as_ref() {
                            callback(e.clone()).await;
                        }
                        return Err(e);
                    }
                }

                // Clear and add new audio
                self.clear_buffer().await;
                let mut guard = self.audio_buffer.lock().await;
                guard.extend_from_slice(&audio_data);
            } else {
                guard.extend_from_slice(&audio_data);
            }
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        let mut guard = self.result_callback.lock().await;
        *guard = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        let mut guard = self.error_callback.lock().await;
        *guard = Some(callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        Some(&self.original_config)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        let naver_config = NaverClovaSttConfig::from_base(config.clone())?;
        self.config = naver_config;
        self.original_config = config;
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "NAVER CLOVA CSR (네이버 클로바) v1.0"
    }

    /// W-D2: attach the shared per-provider circuit breaker so every NAVER CLOVA session trips
    /// (and observes) the SAME breaker, uniform with the WS fleet. The REST transport consults
    /// it before each upstream call and feeds it the unified HTTP status classification.
    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        self.resilience.set_handles(resilience);
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::NaverClovaLanguage;
    use super::*;

    fn make_test_config() -> STTConfig {
        STTConfig {
            provider: "naver-clova".to_string(),
            api_key: "test_client_id|test_client_secret".to_string(),
            language: "ko-KR".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: String::new(),
        }
    }

    #[test]
    fn test_new_valid_config() {
        let config = make_test_config();
        let result = NaverClovaStt::new(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert!(!stt.is_ready());
        assert!(stt.get_config().is_some());
    }

    // W1 keystone: NAVER CLOVA CSR exposes no mappable advanced features, so the meaningful
    // assertion is that the base config survives through `new_standard` onto the provider config
    // (credentials parsed, language mapped) even when advanced features are requested (they are
    // capability gaps, intentionally dropped) — proving the standardized path is wired.
    #[test]
    fn test_naver_clova_new_standard_carries_base() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: make_test_config(),
            // Advanced features the provider cannot express; must not break the standardized path.
            features: SttFeatures {
                diarization: Some(true),
                interim_results: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let stt = NaverClovaStt::new_standard(&std).unwrap();
        assert_eq!(stt.config.client_id, "test_client_id"); // base api_key survived
        assert_eq!(stt.config.client_secret, "test_client_secret");
        assert_eq!(stt.config.language, NaverClovaLanguage::Korean); // base language survived
    }

    #[test]
    fn test_new_invalid_api_key() {
        let config = STTConfig {
            api_key: "invalid_format".to_string(),
            ..make_test_config()
        };

        let result = NaverClovaStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..make_test_config()
        };

        let result = NaverClovaStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_rejects_pathological_sample_rate_before_runtime_math() {
        let config = STTConfig {
            sample_rate: u32::MAX,
            ..make_test_config()
        };

        let err = match NaverClovaStt::new(config) {
            Ok(_) => panic!("constructor must reject pathological sample rates"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("at most"), "{err}");
    }

    #[tokio::test]
    async fn test_connect_disconnect() {
        let config = make_test_config();
        let mut stt = NaverClovaStt::new(config).unwrap();

        assert!(!stt.is_ready());

        stt.connect().await.unwrap();
        assert!(stt.is_ready());

        stt.disconnect().await.unwrap();
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = make_test_config();
        let mut stt = NaverClovaStt::new(config).unwrap();

        let audio = Bytes::from(vec![0u8; 1000]);
        let result = stt.send_audio(audio).await;

        assert!(result.is_err());
        assert!(matches!(result, Err(STTError::ConnectionFailed(_))));
    }

    #[tokio::test]
    async fn test_send_audio_connected() {
        let config = make_test_config();
        let mut stt = NaverClovaStt::new(config).unwrap();

        stt.connect().await.unwrap();

        let audio = Bytes::from(vec![0u8; 1000]);
        let result = stt.send_audio(audio).await;

        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn naver_clova_stt_redirect_policy_rejects_private_hop() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local redirect test server");
        let addr = listener.local_addr().expect("local addr");

        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: http://127.0.0.1:9/metadata\r\n",
                "Content-Length: 0\r\n",
                "\r\n"
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
        });

        let stt = NaverClovaStt::new(make_test_config()).expect("construct NAVER CLOVA STT");
        let err = stt
            .http_client
            .get(format!("http://{addr}/start"))
            .send()
            .await
            .expect_err("private redirect target must be rejected");
        let mut error_chain = err.to_string();
        let mut source = std::error::Error::source(&err);
        while let Some(err) = source {
            error_chain.push_str(": ");
            error_chain.push_str(&err.to_string());
            source = err.source();
        }
        assert!(error_chain.contains("SSRF protection"), "{error_chain}");
    }

    #[tokio::test]
    async fn test_buffer_accumulation() {
        let config = make_test_config();
        let mut stt = NaverClovaStt::new(config).unwrap();

        stt.connect().await.unwrap();

        // Send multiple audio chunks
        for _ in 0..5 {
            let audio = Bytes::from(vec![0u8; 1000]);
            stt.send_audio(audio).await.unwrap();
        }

        // Check buffer accumulated
        let guard = stt.audio_buffer.lock().await;
        assert_eq!(guard.len(), 5000);
    }

    #[test]
    fn test_max_buffer_bytes() {
        let config = make_test_config();
        let stt = NaverClovaStt::new(config).unwrap();

        // 16kHz * 2 bytes * 1 channel * 60 seconds = 1,920,000 bytes
        let expected = (16000 * 2) * 60;
        assert_eq!(stt.max_buffer_bytes(), expected);
    }

    #[test]
    fn test_build_endpoint_url() {
        let config = make_test_config();
        let stt = NaverClovaStt::new(config).unwrap();

        let url = stt.build_endpoint_url();
        assert!(url.contains("lang=Kor"));
        assert!(url.starts_with(NAVER_CSR_ENDPOINT));
    }

    #[test]
    fn test_wav_header_generation() {
        let config = make_test_config();
        let stt = NaverClovaStt::new(config).unwrap();

        let pcm_data = vec![0u8; 1000];
        let wav_data = stt.add_wav_header(&pcm_data).expect("valid WAV header");

        // WAV header is 44 bytes
        assert_eq!(wav_data.len(), 44 + 1000);

        // Check RIFF header
        assert_eq!(&wav_data[0..4], b"RIFF");
        assert_eq!(&wav_data[8..12], b"WAVE");
        assert_eq!(&wav_data[12..16], b"fmt ");
        assert_eq!(&wav_data[36..40], b"data");
    }

    #[tokio::test]
    async fn test_callback_registration() {
        let config = make_test_config();
        let mut stt = NaverClovaStt::new(config).unwrap();

        let result_callback = Arc::new(|_result: STTResult| {
            Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        let error_callback = Arc::new(|_error: STTError| {
            Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        assert!(stt.on_result(result_callback).await.is_ok());
        assert!(stt.on_error(error_callback).await.is_ok());
    }

    #[test]
    fn test_provider_info() {
        let config = make_test_config();
        let stt = NaverClovaStt::new(config).unwrap();

        assert!(stt.get_provider_info().contains("NAVER CLOVA"));
    }

    #[tokio::test]
    async fn test_update_config() {
        let config = make_test_config();
        let mut stt = NaverClovaStt::new(config).unwrap();

        let new_config = STTConfig {
            language: "en-US".to_string(),
            ..make_test_config()
        };

        stt.update_config(new_config).await.unwrap();

        assert_eq!(stt.config.language, NaverClovaLanguage::English);
    }

    #[test]
    fn test_language_mapping() {
        // Test Korean
        let mut config = make_test_config();
        config.language = "ko-KR".to_string();
        let stt = NaverClovaStt::new(config).unwrap();
        assert_eq!(stt.config.language, NaverClovaLanguage::Korean);

        // Test English
        let mut config = make_test_config();
        config.language = "en-US".to_string();
        let stt = NaverClovaStt::new(config).unwrap();
        assert_eq!(stt.config.language, NaverClovaLanguage::English);

        // Test Japanese
        let mut config = make_test_config();
        config.language = "ja-JP".to_string();
        let stt = NaverClovaStt::new(config).unwrap();
        assert_eq!(stt.config.language, NaverClovaLanguage::Japanese);

        // Test Chinese
        let mut config = make_test_config();
        config.language = "zh-CN".to_string();
        let stt = NaverClovaStt::new(config).unwrap();
        assert_eq!(stt.config.language, NaverClovaLanguage::Chinese);
    }
}
