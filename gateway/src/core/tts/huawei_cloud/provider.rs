//! Huawei Cloud Speech Interaction Service (SIS) TTS Provider
//!
//! This module implements the `BaseTTS` trait for Huawei Cloud's
//! Text-to-Speech services.
//!
//! # Architecture
//!
//! The provider supports two operation modes:
//! - **Standard TTS (HTTP)**: For batch text-to-speech synthesis
//! - **Real-time TTS (WebSocket)**: For low-latency streaming synthesis
//!
//! # Authentication
//!
//! Uses IAM token-based authentication:
//! 1. Exchange username/password for IAM token
//! 2. Token valid for 24 hours (auto-refreshed with 1-hour buffer)
//! 3. Include X-Auth-Token header in all requests

use async_trait::async_trait;
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::config::{HuaweiCloudTtsConfig, HuaweiTtsRequest, HuaweiTtsResponse, MAX_TEXT_LENGTH};
use crate::core::stt::huawei_cloud::HuaweiTokenManager;
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};

// =============================================================================
// Constants
// =============================================================================

/// Provider information string.
const PROVIDER_INFO: &str = "Huawei Cloud TTS (华为云语音)";

/// HTTP request timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// User-Agent header value.
const USER_AGENT: &str = "WaaV-Gateway/1.0 (Huawei-Cloud-TTS)";

// =============================================================================
// Huawei Cloud TTS Provider
// =============================================================================

/// Huawei Cloud Text-to-Speech provider.
///
/// Supports both standard HTTP REST API and real-time WebSocket streaming.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::{BaseTTS, TTSConfig};
/// use waav_gateway::core::tts::huawei_cloud::HuaweiCloudTts;
///
/// let config = TTSConfig {
///     // API key format: username|password|domain_name|project_id
///     api_key: "user|pass|domain|project123".to_string(),
///     voice_id: Some("xiaoyan".to_string()),
///     sample_rate: Some(16000),
///     audio_format: Some("wav".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = HuaweiCloudTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("你好世界", true).await?;
/// tts.disconnect().await?;
/// ```
pub struct HuaweiCloudTts {
    /// Base configuration for BaseTTS trait.
    #[allow(dead_code)]
    base_config: TTSConfig,

    /// Huawei Cloud-specific configuration.
    config: HuaweiCloudTtsConfig,

    /// IAM token manager (shared with STT if using same credentials).
    token_manager: Arc<HuaweiTokenManager>,

    /// HTTP client for REST API.
    client: Client,

    /// Connection state.
    connected: Arc<AtomicBool>,

    /// Audio callback.
    audio_callback: Arc<Mutex<Option<Arc<dyn AudioCallback>>>>,

    /// Total bytes synthesized counter.
    bytes_synthesized: Arc<AtomicU64>,

    /// Total requests counter.
    requests_count: Arc<AtomicU64>,
}

impl HuaweiCloudTts {
    /// Create a new Huawei Cloud TTS provider (internal).
    fn create_internal(config: TTSConfig) -> TTSResult<Self> {
        let huawei_config = HuaweiCloudTtsConfig::from_base(config.clone())?;
        Self::create_from_huawei_config(config, huawei_config)
    }

    /// Create a provider from a pre-built Huawei-specific config (shared by `from_standard`).
    fn create_from_huawei_config(
        base_config: TTSConfig,
        huawei_config: HuaweiCloudTtsConfig,
    ) -> TTSResult<Self> {
        huawei_config.validate()?;

        // Create HTTP client with connection pooling
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| TTSError::InternalError(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            base_config,
            config: huawei_config,
            token_manager: Arc::new(HuaweiTokenManager::new()),
            client,
            connected: Arc::new(AtomicBool::new(false)),
            audio_callback: Arc::new(Mutex::new(None)),
            bytes_synthesized: Arc::new(AtomicU64::new(0)),
            requests_count: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Build from the standardized TTS config (W1 keystone), mirroring `DeepgramTTS::from_standard`.
    /// Delegates feature mapping to [`HuaweiCloudTtsConfig::from_standard`] (speed/pitch/volume,
    /// sample_rate, plus the `region` extras passthrough) so advanced features reach the provider
    /// config the request builder reads.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> TTSResult<Self> {
        let huawei_config = HuaweiCloudTtsConfig::from_standard(std)?;
        Self::create_from_huawei_config(std.base.clone(), huawei_config)
    }

    /// Get the IAM token, fetching if necessary.
    async fn get_token(&self) -> Result<String, TTSError> {
        self.token_manager
            .get_token(
                &self.config.username,
                &self.config.password,
                &self.config.domain_name,
                // Convert TTS region to STT region (they use the same enum)
                crate::core::stt::huawei_cloud::HuaweiCloudRegion::from_str(
                    self.config.region.code(),
                )
                .unwrap_or_default(),
            )
            .await
            .map_err(|e| TTSError::AuthenticationFailed(e.to_string()))
    }

    /// Check if text exceeds maximum length.
    fn check_text_length(text: &str) -> bool {
        text.chars().count() <= MAX_TEXT_LENGTH
    }

    /// Split text into chunks that fit within the character limit.
    fn split_text(text: &str) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut current_chunk = String::new();
        let mut current_len = 0;

        // Try to split at sentence boundaries
        for c in text.chars() {
            current_chunk.push(c);
            current_len += 1;

            // Check for natural break points
            let is_sentence_end = matches!(c, '。' | '！' | '？' | '.' | '!' | '?' | '\n');

            if current_len >= MAX_TEXT_LENGTH - 10 || (is_sentence_end && current_len > 50) {
                if !current_chunk.trim().is_empty() {
                    chunks.push(current_chunk.trim().to_string());
                }
                current_chunk = String::new();
                current_len = 0;
            }
        }

        if !current_chunk.trim().is_empty() {
            chunks.push(current_chunk.trim().to_string());
        }

        chunks
    }

    /// Synthesize a single chunk of text using standard HTTP API.
    async fn synthesize_chunk(&self, text: &str) -> TTSResult<Vec<u8>> {
        // Get access token
        let token = self.get_token().await?;

        // Build request
        let request = HuaweiTtsRequest::new(text, &self.config);
        let request_json = request
            .to_json()
            .map_err(|e| TTSError::InternalError(format!("Failed to serialize request: {}", e)))?;

        let url = self.config.get_tts_url();

        debug!(
            "Sending TTS request to Huawei Cloud: {} chars to {}",
            text.chars().count(),
            url
        );

        // Send request
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Auth-Token", &token)
            .body(request_json)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("TTS request failed: {}", e)))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read response: {}", e)))?;

        if !status.is_success() {
            // Try to parse error response
            if let Ok(tts_response) = HuaweiTtsResponse::from_json(&body)
                && let Some(error) = tts_response.get_error() {
                    // Handle specific error codes
                    if error.contains("SIS.0001") || error.contains("SIS.0002") {
                        return Err(TTSError::AuthenticationFailed(error));
                    } else if error.contains("SIS.0006") || error.contains("SIS.0007") {
                        return Err(TTSError::RateLimited {
                            retry_after_secs: None,
                            message: error,
                        });
                    }
                    return Err(TTSError::ProviderError(error));
                }
            return Err(TTSError::ProviderError(format!(
                "TTS request failed with status {}: {}",
                status, body
            )));
        }

        // Parse response
        let tts_response = HuaweiTtsResponse::from_json(&body)
            .map_err(|e| TTSError::InternalError(format!("Failed to parse response: {}", e)))?;

        if !tts_response.is_success() {
            if let Some(error) = tts_response.get_error() {
                return Err(TTSError::ProviderError(error));
            }
            return Err(TTSError::ProviderError("Unknown error".to_string()));
        }

        // Get audio data
        let audio_data = tts_response
            .get_audio_data()
            .ok_or_else(|| TTSError::ProviderError("No audio data in response".to_string()))?;

        debug!("Received {} bytes of audio data", audio_data.len());

        // Update counters
        self.bytes_synthesized
            .fetch_add(audio_data.len() as u64, Ordering::Relaxed);
        self.requests_count.fetch_add(1, Ordering::Relaxed);

        Ok(audio_data)
    }
}

#[async_trait]
impl BaseTTS for HuaweiCloudTts {
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        Self::create_internal(config)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Connecting to Huawei Cloud TTS...");

        // Pre-fetch access token to validate credentials
        self.get_token().await?;

        self.connected.store(true, Ordering::SeqCst);
        info!(
            "Connected to Huawei Cloud TTS ({} mode)",
            if self.config.mode.is_websocket() {
                "Real-time"
            } else {
                "Standard"
            }
        );

        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Disconnecting from Huawei Cloud TTS...");

        // Clear callbacks
        *self.audio_callback.lock().await = None;

        self.connected.store(false, Ordering::SeqCst);

        info!(
            "Disconnected from Huawei Cloud TTS (synthesized {} bytes, {} requests)",
            self.bytes_synthesized.load(Ordering::Relaxed),
            self.requests_count.load(Ordering::Relaxed)
        );

        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn get_connection_state(&self) -> ConnectionState {
        if self.connected.load(Ordering::SeqCst) {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    async fn speak(&mut self, text: &str, _flush: bool) -> TTSResult<()> {
        if !self.connected.load(Ordering::SeqCst) {
            // Auto-connect if not connected
            self.connect().await?;
        }

        // Handle empty text
        if text.trim().is_empty() {
            return Ok(());
        }

        // Use standard HTTP mode for now
        // (Real-time WebSocket can be added later for streaming)

        // Split text if necessary
        let chunks = if Self::check_text_length(text) {
            vec![text.to_string()]
        } else {
            warn!(
                "Text exceeds maximum length ({} chars), splitting into chunks",
                text.chars().count()
            );
            Self::split_text(text)
        };

        // Process each chunk
        for (i, chunk) in chunks.iter().enumerate() {
            if chunks.len() > 1 {
                debug!("Processing chunk {}/{}", i + 1, chunks.len());
            }

            // Synthesize audio
            let audio_data = self.synthesize_chunk(chunk).await?;

            // Send to callback
            let callback = self.audio_callback.lock().await;
            if let Some(cb) = callback.as_ref() {
                let audio = AudioData {
                    data: audio_data,
                    sample_rate: self.config.sample_rate,
                    format: self.config.audio_format.as_str().to_string(),
                    duration_ms: None,
                };
                cb.on_audio(audio).await;
            }
        }

        // Signal completion
        let callback = self.audio_callback.lock().await;
        if let Some(cb) = callback.as_ref() {
            cb.on_complete().await;
        }

        Ok(())
    }

    async fn clear(&mut self) -> TTSResult<()> {
        // No-op for REST API (no queue to clear)
        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        // No-op for REST API (each speak() call is immediately processed)
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        let audio_callback = self.audio_callback.clone();
        tokio::spawn(async move {
            *audio_callback.lock().await = Some(callback);
        });
        Ok(())
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        let audio_callback = self.audio_callback.clone();
        tokio::spawn(async move {
            *audio_callback.lock().await = None;
        });
        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "huawei-cloud",
            "name": PROVIDER_INFO,
            "version": "1.0.0",
            "description": "Huawei Cloud Speech Interaction Service (SIS) TTS with Chinese voice synthesis",
            "voice": self.config.voice.name(),
            "voice_property": self.config.voice.as_property(),
            "voice_premium": self.config.voice.is_premium(),
            "audio_format": self.config.audio_format.as_str(),
            "sample_rate": self.config.sample_rate,
            "speed": self.config.speed,
            "pitch": self.config.pitch,
            "volume": self.config.volume,
            "region": self.config.region.code(),
            "mode": if self.config.mode.is_websocket() { "realtime" } else { "standard" },
            "max_text_length": MAX_TEXT_LENGTH,
            "features": [
                "rest-api",
                "multiple-voices",
                "speed-control",
                "pitch-control",
                "volume-control",
                "premium-voices",
                "standard-voices",
                "child-voices",
                "english-voice"
            ],
            "languages": ["zh", "en"],
            "statistics": {
                "bytes_synthesized": self.bytes_synthesized.load(Ordering::Relaxed),
                "requests_count": self.requests_count.load(Ordering::Relaxed)
            }
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: "test_user|test_pass|test_domain|test_project".to_string(),
            voice_id: Some("xiaoyan".to_string()),
            sample_rate: Some(16000),
            audio_format: Some("wav".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_new_provider() {
        let config = create_test_config();
        let result = HuaweiCloudTts::new(config);
        assert!(result.is_ok());
    }

    // W1 keystone: the provider struct's `from_standard` maps prosody features through onto the
    // Huawei-specific config the request builder reads. Asserts speed/pitch/volume reach the config.
    #[test]
    fn from_standard_maps_prosody_to_provider_config() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                speed: Some(1.0), // 1.0 multiplier -> 0 (normal) in Huawei's -500..=500 range
                pitch: Some(120.0),
                volume: Some(80.0),
                sample_rate: Some(8000),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = HuaweiCloudTts::from_standard(&std).unwrap();
        assert_eq!(tts.config.speed, 0);
        assert_eq!(tts.config.pitch, 120);
        assert_eq!(tts.config.volume, 80);
        assert_eq!(tts.config.sample_rate, 8000);
    }

    #[test]
    fn test_new_provider_empty_api_key() {
        let config = TTSConfig {
            api_key: "".to_string(),
            ..Default::default()
        };
        let result = HuaweiCloudTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_provider_invalid_api_key_format() {
        let config = TTSConfig {
            api_key: "missing_pipe_separator".to_string(),
            ..Default::default()
        };
        let result = HuaweiCloudTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_not_connected_initially() {
        let config = create_test_config();
        let tts = HuaweiCloudTts::new(config).unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let tts = HuaweiCloudTts::new(config).unwrap();
        let info = tts.get_provider_info();
        let info_str = info.to_string();
        assert!(info_str.contains("huawei") || info_str.contains("Huawei"));
        assert_eq!(info["provider"], "huawei-cloud");
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut tts = HuaweiCloudTts::new(config).unwrap();
        let result = tts.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_flush_ok() {
        let config = create_test_config();
        let tts = HuaweiCloudTts::new(config).unwrap();
        let result = tts.flush().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_clear_ok() {
        let config = create_test_config();
        let mut tts = HuaweiCloudTts::new(config).unwrap();
        let result = tts.clear().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_text_length_short() {
        let text = "你好世界";
        assert!(HuaweiCloudTts::check_text_length(text));
    }

    #[test]
    fn test_check_text_length_long() {
        // Create text that exceeds 500 characters
        let text: String = "中".repeat(600);
        assert!(!HuaweiCloudTts::check_text_length(&text));
    }

    #[test]
    fn test_split_text_short() {
        let text = "你好世界";
        let chunks = HuaweiCloudTts::split_text(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }

    #[test]
    fn test_split_text_long() {
        // Create text that needs splitting
        let text: String = "中".repeat(600);
        let chunks = HuaweiCloudTts::split_text(&text);
        assert!(chunks.len() > 1);

        // Verify total length matches
        let total_len: usize = chunks.iter().map(|c| c.chars().count()).sum();
        assert_eq!(total_len, 600);
    }

    #[test]
    fn test_split_text_empty() {
        let text = "";
        let chunks = HuaweiCloudTts::split_text(text);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_split_text_sentence_boundaries() {
        let text = "这是第一句话。这是第二句话！这是第三句话？";
        let chunks = HuaweiCloudTts::split_text(text);
        // Text is short, should be in one chunk
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn test_provider_info_statistics() {
        let config = create_test_config();
        let tts = HuaweiCloudTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert!(info["statistics"]["bytes_synthesized"].is_number());
        assert!(info["statistics"]["requests_count"].is_number());
    }

    #[test]
    fn test_provider_info_features() {
        let config = create_test_config();
        let tts = HuaweiCloudTts::new(config).unwrap();
        let info = tts.get_provider_info();

        let features = info["features"].as_array().unwrap();
        assert!(features.iter().any(|f| f == "rest-api"));
        assert!(features.iter().any(|f| f == "multiple-voices"));
        assert!(features.iter().any(|f| f == "speed-control"));
    }

    #[test]
    fn test_provider_info_voice() {
        let config = create_test_config();
        let tts = HuaweiCloudTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["voice_property"], "chinese_xiaoyan_common");
        assert_eq!(info["voice_premium"], false);
    }

    #[test]
    fn test_premium_voice_config() {
        let mut config = create_test_config();
        config.voice_id = Some("huaxiaoxia".to_string());

        let tts = HuaweiCloudTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["voice_property"], "chinese_huaxiaoxia_common");
        assert_eq!(info["voice_premium"], true);
    }

    #[test]
    fn test_audio_format_config() {
        let mut config = create_test_config();
        config.audio_format = Some("mp3".to_string());

        let tts = HuaweiCloudTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["audio_format"], "mp3");
    }

    #[test]
    fn test_sample_rate_config() {
        let mut config = create_test_config();
        config.sample_rate = Some(8000);

        let tts = HuaweiCloudTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["sample_rate"], 8000);
    }

    #[test]
    fn test_mode_config() {
        let config = create_test_config();
        let tts = HuaweiCloudTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["mode"], "standard");
    }
}
