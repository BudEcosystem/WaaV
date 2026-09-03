//! Viettel AI TTS client implementation.
//!
//! This module provides a REST API-based TTS client for Viettel Group's
//! AI Text-to-Speech service.
//!
//! # Protocol
//!
//! Viettel AI TTS uses REST API:
//! 1. POST JSON request with text, voice, speed parameters
//! 2. Receive WAV audio binary response directly
//!
//! # Audio Format
//!
//! Output is WAV format at 16kHz sample rate.

use super::config::{
    AUDIO_SAMPLE_RATE, MAX_SPEED, MIN_SPEED, VIETTEL_TTS_ENDPOINT, ViettelTtsConfig,
    ViettelTtsRequest, ViettelTtsResponse, ViettelVoice,
};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Viettel AI TTS client.
///
/// This client implements a REST API-based approach where each `speak()` call:
/// 1. Sends an HTTP POST with JSON body containing text and parameters
/// 2. Receives WAV audio binary response directly
pub struct ViettelTts {
    /// Provider configuration.
    config: ViettelTtsConfig,
    /// HTTP client for REST API calls.
    http_client: Option<Client>,
    /// Whether the client is ready to receive requests.
    is_ready: AtomicBool,
    /// Audio callback for streaming audio to caller.
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
    /// Current connection state.
    connection_state: Arc<RwLock<ConnectionState>>,
    /// Request counter for generating unique IDs.
    request_counter: AtomicU64,
}

fn viettel_tts_http_client(timeout_secs: u64) -> Result<Client, reqwest::Error> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
}

fn default_viettel_tts_http_client() -> Option<Client> {
    match viettel_tts_http_client(super::config::DEFAULT_REQUEST_TIMEOUT) {
        Ok(client) => Some(client),
        Err(err) => {
            warn!(
                error = %err,
                "failed to create default Viettel TTS HTTP client; default instance is inert until rebuilt with a valid config"
            );
            None
        }
    }
}

impl ViettelTts {
    /// Synthesize text and return audio data.
    async fn synthesize(&self, text: &str) -> TTSResult<Vec<u8>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        // Validate text length
        ViettelTtsConfig::validate_text(text)?;

        debug!(
            "Viettel TTS: Synthesizing {} characters with voice '{}' (speed: {})",
            text.chars().count(),
            self.config.voice.display_name(),
            self.config.speed
        );

        // Generate unique request ID
        let request_id = self.request_counter.fetch_add(1, Ordering::SeqCst);

        // Build request body
        let request_body = ViettelTtsRequest {
            text: text.to_string(),
            voice: self.config.voice.voice_id().to_string(),
            id: request_id.to_string(),
            without_filter: self.config.without_filter,
            speed: self.config.speed,
            tts_return_option: self.config.tts_return_option,
        };

        // Send request
        let http_client = self.http_client.as_ref().ok_or_else(|| {
            TTSError::InvalidConfiguration(
                "Viettel TTS default HTTP client is unavailable; construct with ViettelTts::new or from_standard".to_string(),
            )
        })?;

        let response = http_client
            .post(crate::core::tts::standard::override_rest_endpoint(
                VIETTEL_TTS_ENDPOINT,
                self.config.endpoint_override.as_deref(),
            ))
            .header("Content-Type", "application/json")
            .header("token", &self.config.api_key)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to send TTS request: {e}")))?;

        let status = response.status();

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            error!(
                "Viettel TTS: API error (status {}): {}",
                status.as_u16(),
                body
            );

            return match status.as_u16() {
                401 => Err(TTSError::AuthenticationFailed(format!(
                    "Invalid or expired token: {body}"
                ))),
                429 => Err(TTSError::RateLimited {
                    retry_after_secs: None,
                    message: body,
                }),
                400 => Err(TTSError::InvalidConfiguration(format!(
                    "Bad request: {body}"
                ))),
                _ => Err(TTSError::ProviderError(format!(
                    "API error (status {}): {}",
                    status.as_u16(),
                    body
                ))),
            };
        }

        // Response is either binary audio data (default) or a JSON envelope
        // containing a provider-hosted audio URL when tts_return_option=1.
        let audio_data = response
            .bytes()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read audio data: {e}")))?
            .to_vec();

        // Verify we got actual audio data
        if audio_data.is_empty() {
            return Err(TTSError::ProviderError(
                "Received empty audio data".to_string(),
            ));
        }

        if audio_data.starts_with(b"{") {
            let response: ViettelTtsResponse =
                serde_json::from_slice(&audio_data).map_err(|e| {
                    TTSError::ProviderError(format!(
                        "Failed to parse Viettel TTS JSON response: {e}"
                    ))
                })?;

            if !response.is_success() {
                return Err(TTSError::ProviderError(
                    response.status_message().to_string(),
                ));
            }

            if response.audio_url.trim().is_empty() {
                return Err(TTSError::ProviderError(
                    "Viettel TTS returned JSON response without audio_url".to_string(),
                ));
            }

            return self.download_audio(&response.audio_url).await;
        }

        info!("Viettel TTS: Received {} bytes of audio", audio_data.len());

        Ok(audio_data)
    }

    /// Download audio from a Viettel-hosted URL response.
    async fn download_audio(&self, url: &str) -> TTSResult<Vec<u8>> {
        let url = crate::core::tts::standard::validate_provider_audio_url("Viettel TTS", url)?;
        let audio_client = crate::core::net::ssrf_protected_client_builder(
            crate::core::tts::standard::PROVIDER_AUDIO_URL_SCHEMES,
        )
        .timeout(std::time::Duration::from_secs(
            self.config.request_timeout_secs,
        ))
        .build()
        .map_err(|e| {
            TTSError::NetworkError(format!("Failed to create SSRF-protected audio client: {e}"))
        })?;

        let response = audio_client
            .get(url)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to download audio: {e}")))?;

        if !response.status().is_success() {
            return Err(TTSError::ProviderError(format!(
                "Failed to download audio: status {}",
                response.status()
            )));
        }

        let audio_data = response
            .bytes()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read downloaded audio: {e}")))?
            .to_vec();

        if audio_data.is_empty() {
            return Err(TTSError::ProviderError(
                "Received empty downloaded audio data".to_string(),
            ));
        }

        Ok(audio_data)
    }

    /// Build from the standardized TTS config (W1 keystone). Mirrors [`BaseTTS::new`] but maps the
    /// standardized features via [`ViettelTtsConfig::from_standard`] (which honors `speed` and the
    /// `without_filter` / `tts_return_option` extras) before constructing the timeout-bounded HTTP
    /// client. Features Viettel cannot express stay at provider defaults (capability gaps).
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let viettel_config = ViettelTtsConfig::from_standard(std)?;

        let timeout_secs = viettel_config.request_timeout_secs;
        let http_client = viettel_tts_http_client(timeout_secs).map_err(|e| {
            TTSError::InvalidConfiguration(format!("Failed to create HTTP client: {e}"))
        })?;

        info!(
            "Viettel TTS: Initialized (standardized) with voice='{}', speed={}",
            viettel_config.voice.display_name(),
            viettel_config.speed
        );

        Ok(Self {
            config: viettel_config,
            http_client: Some(http_client),
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            request_counter: AtomicU64::new(1),
        })
    }

    /// Get the Viettel-specific configuration.
    pub fn get_viettel_config(&self) -> &ViettelTtsConfig {
        &self.config
    }

    /// Set the voice.
    pub fn set_voice(&mut self, voice: ViettelVoice) {
        self.config.voice = voice;
    }

    /// Set the speech speed.
    pub fn set_speed(&mut self, speed: f32) {
        self.config.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
    }

    /// Set the without_filter option.
    pub fn set_without_filter(&mut self, without_filter: bool) {
        self.config.without_filter = without_filter;
    }
}

impl Default for ViettelTts {
    fn default() -> Self {
        Self {
            config: ViettelTtsConfig::default(),
            http_client: default_viettel_tts_http_client(),
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            request_counter: AtomicU64::new(1),
        }
    }
}

#[async_trait::async_trait]
impl BaseTTS for ViettelTts {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        let viettel_config = ViettelTtsConfig::from_base(config)?;

        let timeout_secs = viettel_config.request_timeout_secs;
        let http_client = viettel_tts_http_client(timeout_secs).map_err(|e| {
            TTSError::InvalidConfiguration(format!("Failed to create HTTP client: {e}"))
        })?;

        info!(
            "Viettel TTS: Initialized with voice='{}', speed={}",
            viettel_config.voice.display_name(),
            viettel_config.speed
        );

        Ok(Self {
            config: viettel_config,
            http_client: Some(http_client),
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            request_counter: AtomicU64::new(1),
        })
    }

    async fn connect(&mut self) -> TTSResult<()> {
        // Validate credentials
        if self.config.api_key.is_empty() {
            return Err(TTSError::AuthenticationFailed(
                "Viettel AI API token is required".to_string(),
            ));
        }

        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Connecting;
        }

        // REST API doesn't need a persistent connection
        // Just mark as ready
        self.is_ready.store(true, Ordering::SeqCst);

        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Connected;
        }

        info!("Viettel TTS: Ready to synthesize");
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.is_ready.store(false, Ordering::SeqCst);

        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Disconnected;
        }

        info!("Viettel TTS: Disconnected");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_ready.load(Ordering::SeqCst)
    }

    fn get_connection_state(&self) -> ConnectionState {
        if self.is_ready() {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        if !self.is_ready() {
            // Auto-connect if not ready
            self.connect().await?;
        }

        if text.is_empty() {
            return Ok(());
        }

        // Synthesize the audio
        let audio_data = self.synthesize(text).await?;

        if audio_data.is_empty() {
            return Ok(());
        }

        // Send audio via callback
        let callback_guard = self.audio_callback.read().await;
        if let Some(callback) = callback_guard.as_ref() {
            let data = AudioData {
                data: audio_data,
                sample_rate: AUDIO_SAMPLE_RATE,
                format: "wav".to_string(),
                duration_ms: None,
            };

            callback.on_audio(data).await;

            if flush {
                callback.on_complete().await;
            }
        }

        Ok(())
    }

    async fn clear(&mut self) -> TTSResult<()> {
        // No queue to clear for REST API
        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        // REST API sends immediately, nothing to flush
        // Just trigger complete callback if registered
        let callback_guard = self.audio_callback.read().await;
        if let Some(callback) = callback_guard.as_ref() {
            callback.on_complete().await;
        }
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        // Use tokio::spawn to avoid blocking in single-threaded runtime
        let audio_callback = Arc::clone(&self.audio_callback);
        tokio::spawn(async move {
            let mut guard = audio_callback.write().await;
            *guard = Some(callback);
        });
        Ok(())
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        let audio_callback = Arc::clone(&self.audio_callback);
        tokio::spawn(async move {
            let mut guard = audio_callback.write().await;
            *guard = None;
        });
        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "viettel-ai",
            "name": "Viettel AI TTS (Viettel Group)",
            "version": "1.0.0",
            "voice": self.config.voice.voice_id(),
            "voice_name": self.config.voice.display_name(),
            "speed": self.config.speed,
            "format": "wav",
            "supported_formats": ["wav"],
            "supported_languages": ["vi"],
            "sample_rate": AUDIO_SAMPLE_RATE,
            "features": {
                "speed_control": true,
                "regional_accents": true,
                "northern_accent": true,
                "southern_accent": true,
                "central_accent": true,
                "human_like_quality": "95%"
            },
            "voices": [
                {"id": "doanngocle", "name": "Doan Ngoc Le", "region": "northern", "gender": "female"},
                {"id": "hn_female_ngochuyen_news_48k-fhg", "name": "Ngoc Huyen", "region": "northern", "gender": "female"},
                {"id": "hn_female_thuthao_news_48k-fhg", "name": "Thu Thao", "region": "northern", "gender": "female"},
                {"id": "hn_female_phuongtrang_news_48k-fhg", "name": "Phuong Trang", "region": "northern", "gender": "female"},
                {"id": "hn_male_xuankien_news_48k-fhg", "name": "Xuan Kien", "region": "northern", "gender": "male"},
                {"id": "hn_male_quang_news_48k-fhg", "name": "Quang", "region": "northern", "gender": "male"},
                {"id": "sg_female_thuphuong_news_48k-fhg", "name": "Thu Phuong", "region": "southern", "gender": "female"},
                {"id": "sg_female_minhly_news_48k-fhg", "name": "Minh Ly", "region": "southern", "gender": "female"},
                {"id": "sg_female_huonggiang_news_48k-fhg", "name": "Huong Giang", "region": "southern", "gender": "female"},
                {"id": "sg_male_trongphuc_news_48k-fhg", "name": "Trong Phuc", "region": "southern", "gender": "male"},
                {"id": "hn_female_maiphuong_news_48k-fhg", "name": "Mai Phuong", "region": "central", "gender": "female"},
                {"id": "hn_male_thanhtung_news_48k-fhg", "name": "Thanh Tung", "region": "central", "gender": "male"}
            ]
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::AtomicUsize;

    fn make_test_config() -> TTSConfig {
        TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: "test_token".to_string(),
            voice_id: Some("doanngocle".to_string()),
            ..Default::default()
        }
    }

    struct MockAudioCallback {
        audio_count: AtomicUsize,
        complete_count: AtomicUsize,
    }

    impl MockAudioCallback {
        fn new() -> Self {
            Self {
                audio_count: AtomicUsize::new(0),
                complete_count: AtomicUsize::new(0),
            }
        }

        #[allow(dead_code)]
        fn get_audio_count(&self) -> usize {
            self.audio_count.load(Ordering::SeqCst)
        }

        #[allow(dead_code)]
        fn get_complete_count(&self) -> usize {
            self.complete_count.load(Ordering::SeqCst)
        }
    }

    impl AudioCallback for MockAudioCallback {
        fn on_audio(
            &self,
            _audio_data: AudioData,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.audio_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }

        fn on_error(&self, _error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(async {})
        }

        fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.complete_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }
    }

    #[test]
    fn test_new_valid_config() {
        let config = make_test_config();
        let result = ViettelTts::new(config);
        assert!(result.is_ok());

        let tts = result.unwrap();
        assert!(!tts.is_ready());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn viettel_tts_redirect_policy_rejects_private_hop() {
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

        let tts = ViettelTts::new(make_test_config()).expect("construct Viettel TTS");
        let err = tts
            .http_client
            .as_ref()
            .expect("strict constructor builds an HTTP client")
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

    #[tokio::test(flavor = "current_thread")]
    async fn provider_audio_url_rejects_unsafe_targets() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        struct RestoreLoopbackFlag(Option<String>);
        impl Drop for RestoreLoopbackFlag {
            fn drop(&mut self) {
                // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
                unsafe {
                    if let Some(previous) = self.0.take() {
                        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous);
                    } else {
                        std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
                    }
                }
            }
        }
        let _restore_loopback =
            RestoreLoopbackFlag(std::env::var("WAAV_ALLOW_LOOPBACK_ENDPOINTS").ok());
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let tts = ViettelTts::new(make_test_config()).expect("construct Viettel TTS");
        let err = tts
            .download_audio("http://127.0.0.1:9000/audio.wav")
            .await
            .expect_err("provider-returned loopback audio URL must be rejected");
        assert!(
            err.to_string().contains("SSRF protection"),
            "unexpected error: {err}"
        );

        let err = tts
            .download_audio("file:///tmp/audio.wav")
            .await
            .expect_err("provider-returned file audio URL must be rejected");
        assert!(
            err.to_string().contains("URL scheme"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn url_return_json_without_audio_url_is_not_treated_as_audio() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        struct RestoreLoopbackFlag(Option<String>);
        impl Drop for RestoreLoopbackFlag {
            fn drop(&mut self) {
                // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
                unsafe {
                    if let Some(previous) = self.0.take() {
                        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous);
                    } else {
                        std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
                    }
                }
            }
        }
        let _restore_loopback =
            RestoreLoopbackFlag(std::env::var("WAAV_ALLOW_LOOPBACK_ENDPOINTS").ok());
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1") };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local Viettel JSON mock");
        let addr = listener.local_addr().expect("local addr");

        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 2048];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
            let body = r#"{"status":0,"audio_url":"","message":""}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
        });

        let mut tts = ViettelTts::new(make_test_config()).expect("construct Viettel TTS");
        tts.config.endpoint_override = Some(format!("http://{addr}"));
        tts.config.tts_return_option = 1;

        let err = tts
            .synthesize("Xin chao")
            .await
            .expect_err("JSON envelope without audio_url must not be emitted as audio bytes");
        assert!(
            err.to_string().contains("without audio_url"),
            "unexpected error: {err}"
        );
    }

    // W1 keystone (TTS): the struct-level `from_standard` builds a real `ViettelTts` through the
    // standardized path, carrying the `speed` feature (Viettel's only prosody knob) onto the
    // provider config the request builder reads. Mirrors `DeepgramTTS::from_standard`.
    #[test]
    fn from_standard_builds_provider_with_speed() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "viettel_ai".into(),
                api_key: "test_token".into(),
                voice_id: Some("doanngocle".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = ViettelTts::from_standard(&std).unwrap();
        assert!((tts.config.speed - 1.5).abs() < f32::EPSILON);
        assert_eq!(tts.config.voice, ViettelVoice::DoanNgocLe);
        assert_eq!(tts.config.api_key, "test_token");
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: String::new(),
            ..Default::default()
        };

        let result = ViettelTts::new(config);
        assert!(result.is_err());

        match result {
            Err(TTSError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("token"));
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[test]
    fn test_provider_info() {
        let tts = ViettelTts::new(make_test_config()).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "viettel-ai");
        assert_eq!(info["name"], "Viettel AI TTS (Viettel Group)");
        assert_eq!(info["voices"].as_array().unwrap().len(), 12);
    }

    #[test]
    fn test_default_state() {
        let tts = ViettelTts::default();
        assert!(!tts.is_ready());
    }

    #[tokio::test]
    async fn default_without_http_client_returns_typed_error_without_panic() {
        let mut tts = ViettelTts::default();
        tts.http_client = None;

        let err = tts
            .synthesize("Xin chao")
            .await
            .expect_err("inert default client must fail with a typed error");

        match err {
            TTSError::InvalidConfiguration(msg) => {
                assert!(msg.contains("default HTTP client"), "{msg}");
            }
            other => panic!("expected InvalidConfiguration, got {other:?}"),
        }
    }

    #[test]
    fn test_set_voice() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        tts.set_voice(ViettelVoice::SouthernMale);

        assert_eq!(tts.config.voice, ViettelVoice::SouthernMale);
    }

    #[test]
    fn test_set_speed() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();

        // Normal speed
        tts.set_speed(1.5);
        assert!((tts.config.speed - 1.5).abs() < f32::EPSILON);

        // Below minimum - should clamp
        tts.set_speed(0.1);
        assert!((tts.config.speed - MIN_SPEED).abs() < f32::EPSILON);

        // Above maximum - should clamp
        tts.set_speed(5.0);
        assert!((tts.config.speed - MAX_SPEED).abs() < f32::EPSILON);
    }

    #[test]
    fn test_set_without_filter() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        assert!(!tts.config.without_filter);

        tts.set_without_filter(true);
        assert!(tts.config.without_filter);
    }

    #[tokio::test]
    async fn test_connect_success() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        let result = tts.connect().await;

        assert!(result.is_ok());
        assert!(tts.is_ready());
    }

    #[tokio::test]
    async fn test_connect_empty_api_key() {
        let mut tts = ViettelTts::default();
        let result = tts.connect().await;

        assert!(result.is_err());
        match result {
            Err(TTSError::AuthenticationFailed(_)) => {}
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_disconnect() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.disconnect().await;
        assert!(result.is_ok());
        assert!(!tts.is_ready());
    }

    #[tokio::test]
    async fn test_speak_empty_text() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.speak("", false).await;
        assert!(result.is_ok()); // Empty text should succeed (no-op)
    }

    #[tokio::test]
    async fn test_flush() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let callback = Arc::new(MockAudioCallback::new());
        tts.on_audio(callback.clone()).unwrap();

        // Give time for the async callback registration
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let result = tts.flush().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_clear() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.clear().await;
        assert!(result.is_ok()); // Clear is a no-op for REST API
    }

    #[test]
    fn test_voice_parsing() {
        let test_cases = vec![
            ("doanngocle", ViettelVoice::DoanNgocLe),
            ("doan_ngoc_le", ViettelVoice::DoanNgocLe),
            ("default", ViettelVoice::DoanNgocLe),
            ("female", ViettelVoice::DoanNgocLe),
            ("ngochuyen", ViettelVoice::NorthernFemale1),
            ("thuthao", ViettelVoice::NorthernFemale2),
            ("xuankien", ViettelVoice::NorthernMale1),
            ("quang", ViettelVoice::NorthernMale2),
            ("thuphuong", ViettelVoice::SouthernFemale1),
            ("minhly", ViettelVoice::SouthernFemale2),
            ("trongphuc", ViettelVoice::SouthernMale),
            ("maiphuong", ViettelVoice::CentralFemale),
            ("thanhtung", ViettelVoice::CentralMale),
        ];

        for (voice_id, expected_voice) in test_cases {
            let config = TTSConfig {
                provider: "viettel-ai".to_string(),
                api_key: "test".to_string(),
                voice_id: Some(voice_id.to_string()),
                ..Default::default()
            };

            let tts = ViettelTts::new(config).unwrap();
            assert_eq!(
                tts.config.voice, expected_voice,
                "Voice ID '{}' should map to {:?}",
                voice_id, expected_voice
            );
        }
    }

    #[test]
    fn test_custom_voice() {
        let config = TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: "test".to_string(),
            voice_id: Some("some_unknown_voice".to_string()),
            ..Default::default()
        };

        let tts = ViettelTts::new(config).unwrap();
        match &tts.config.voice {
            ViettelVoice::Custom(id) => assert_eq!(id, "some_unknown_voice"),
            _ => panic!("Expected Custom variant"),
        }
    }

    #[test]
    fn test_connection_state() {
        let tts = ViettelTts::new(make_test_config()).unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_connection_state_after_connect() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Connected);
    }

    #[test]
    fn test_speed_from_speaking_rate() {
        // Normal speed
        let config = TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: "test".to_string(),
            speaking_rate: Some(1.0),
            ..Default::default()
        };
        let tts = ViettelTts::new(config).unwrap();
        assert!((tts.config.speed - 1.0).abs() < f32::EPSILON);

        // Fast speed
        let config = TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: "test".to_string(),
            speaking_rate: Some(1.8),
            ..Default::default()
        };
        let tts = ViettelTts::new(config).unwrap();
        assert!((tts.config.speed - 1.8).abs() < f32::EPSILON);

        // Slow speed
        let config = TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: "test".to_string(),
            speaking_rate: Some(0.7),
            ..Default::default()
        };
        let tts = ViettelTts::new(config).unwrap();
        assert!((tts.config.speed - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn test_get_viettel_config() {
        let tts = ViettelTts::new(make_test_config()).unwrap();
        let config = tts.get_viettel_config();

        assert_eq!(config.api_key, "test_token");
        assert_eq!(config.voice, ViettelVoice::DoanNgocLe);
    }

    #[test]
    fn test_request_counter() {
        let tts = ViettelTts::new(make_test_config()).unwrap();

        let id1 = tts.request_counter.fetch_add(1, Ordering::SeqCst);
        let id2 = tts.request_counter.fetch_add(1, Ordering::SeqCst);

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }

    #[test]
    fn test_voice_regions() {
        assert_eq!(ViettelVoice::DoanNgocLe.region(), "Northern");
        assert_eq!(ViettelVoice::NorthernFemale1.region(), "Northern");
        assert_eq!(ViettelVoice::SouthernFemale1.region(), "Southern");
        assert_eq!(ViettelVoice::SouthernMale.region(), "Southern");
        assert_eq!(ViettelVoice::CentralFemale.region(), "Central");
        assert_eq!(ViettelVoice::CentralMale.region(), "Central");
    }

    #[test]
    fn test_voice_genders() {
        assert_eq!(ViettelVoice::DoanNgocLe.gender(), "female");
        assert_eq!(ViettelVoice::NorthernFemale1.gender(), "female");
        assert_eq!(ViettelVoice::NorthernMale1.gender(), "male");
        assert_eq!(ViettelVoice::SouthernFemale1.gender(), "female");
        assert_eq!(ViettelVoice::SouthernMale.gender(), "male");
        assert_eq!(ViettelVoice::CentralFemale.gender(), "female");
        assert_eq!(ViettelVoice::CentralMale.gender(), "male");
    }
}
