//! Baidu AI Cloud TTS REST Provider
//!
//! This module implements the `BaseTTS` trait for Baidu AI Cloud's
//! Text-to-Speech REST API.
//!
//! # Architecture
//!
//! The provider uses HTTP REST API for text-to-speech synthesis.
//! Authentication is done via OAuth 2.0 client credentials flow
//! with automatic token caching and refresh.
//!
//! # Request Flow
//!
//! ```text
//! Client                                    Baidu API
//!   |                                           |
//!   |---- OAuth Token Request ----------------->|
//!   |<--- Access Token (30 day validity) -------|
//!   |                                           |
//!   |---- TTS Request (POST form-encoded) ----->|
//!   |<--- Audio Data (MP3/PCM/WAV) -------------|
//! ```
//!
//! # Text Limitations
//!
//! - Maximum text length: 1024 GBK bytes (~512 Chinese characters)
//! - URL encoding required for special characters
//! - Polyphonic characters: Use format like "重(chong2)报集团"

use async_trait::async_trait;
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use super::config::{
    BAIDU_OAUTH_URL, BaiduOAuthError, BaiduOAuthResponse, BaiduTtsConfig, BaiduTtsErrorResponse,
    MAX_TEXT_LENGTH_GBK,
};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};

// =============================================================================
// Constants
// =============================================================================

/// Provider information string.
const PROVIDER_INFO: &str = "Baidu AI Cloud TTS (百度语音)";

/// HTTP request timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Token refresh margin (refresh 1 day before expiration).
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(86400);

/// User-Agent header value.
const USER_AGENT: &str = "WaaV-Gateway/1.0 (Baidu-TTS)";

// =============================================================================
// Token Manager
// =============================================================================

/// Manages OAuth 2.0 access tokens with automatic caching and refresh.
struct TokenManager {
    /// Current access token.
    access_token: RwLock<Option<String>>,
    /// Token expiration time.
    expires_at: RwLock<Option<Instant>>,
    /// API key.
    api_key: String,
    /// Secret key.
    secret_key: String,
    /// HTTP client.
    client: Client,
}

impl TokenManager {
    /// Create a new token manager.
    fn new(api_key: String, secret_key: String, client: Client) -> Self {
        Self {
            access_token: RwLock::new(None),
            expires_at: RwLock::new(None),
            api_key,
            secret_key,
            client,
        }
    }

    /// Get a valid access token, refreshing if necessary.
    async fn get_token(&self) -> Result<String, TTSError> {
        // Check if we have a valid cached token
        {
            let token = self.access_token.read().await;
            let expires_at = self.expires_at.read().await;

            if let (Some(t), Some(exp)) = (token.as_ref(), expires_at.as_ref()) {
                if *exp > Instant::now() + TOKEN_REFRESH_MARGIN {
                    return Ok(t.clone());
                }
            }
        }

        // Refresh token
        self.refresh_token().await
    }

    /// Refresh the access token from Baidu OAuth endpoint.
    async fn refresh_token(&self) -> Result<String, TTSError> {
        debug!("Refreshing Baidu OAuth access token...");

        let url = format!(
            "{}?grant_type=client_credentials&client_id={}&client_secret={}",
            BAIDU_OAUTH_URL, self.api_key, self.secret_key
        );

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("OAuth request failed: {}", e)))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read OAuth response: {}", e)))?;

        if !status.is_success() {
            // Try to parse error response
            if let Ok(error) = serde_json::from_str::<BaiduOAuthError>(&body) {
                return Err(TTSError::AuthenticationFailed(format!(
                    "OAuth error: {} - {}",
                    error.error,
                    error.error_description.unwrap_or_default()
                )));
            }
            return Err(TTSError::AuthenticationFailed(format!(
                "OAuth failed with status {}: {}",
                status, body
            )));
        }

        let oauth_response: BaiduOAuthResponse = serde_json::from_str(&body).map_err(|e| {
            TTSError::InternalError(format!("Failed to parse OAuth response: {}", e))
        })?;

        // Calculate expiration time
        let expires_at = Instant::now() + Duration::from_secs(oauth_response.expires_in);

        // Update cached token
        {
            let mut token = self.access_token.write().await;
            let mut exp = self.expires_at.write().await;
            *token = Some(oauth_response.access_token.clone());
            *exp = Some(expires_at);
        }

        info!(
            "Baidu OAuth token refreshed, expires in {} seconds",
            oauth_response.expires_in
        );

        Ok(oauth_response.access_token)
    }

    /// Invalidate the cached token.
    async fn invalidate(&self) {
        let mut token = self.access_token.write().await;
        let mut exp = self.expires_at.write().await;
        *token = None;
        *exp = None;
    }
}

// =============================================================================
// Baidu TTS Provider
// =============================================================================

/// Baidu AI Cloud Text-to-Speech REST provider.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::{BaseTTS, TTSConfig};
/// use waav_gateway::core::tts::baidu::BaiduTts;
///
/// let config = TTSConfig {
///     api_key: "your_api_key|your_secret_key".to_string(),
///     voice_id: Some("0".to_string()),  // 度小美
///     sample_rate: Some(16000),
///     ..Default::default()
/// };
///
/// let mut tts = BaiduTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("你好世界", true).await?;
/// tts.disconnect().await?;
/// ```
pub struct BaiduTts {
    /// Base configuration for BaseTTS trait.
    #[allow(dead_code)]
    base_config: TTSConfig,

    /// Baidu-specific configuration.
    config: BaiduTtsConfig,

    /// Token manager for OAuth.
    token_manager: Arc<TokenManager>,

    /// HTTP client for TTS requests.
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

impl BaiduTts {
    /// Create a new Baidu TTS provider (internal).
    fn create_internal(config: TTSConfig) -> TTSResult<Self> {
        let baidu_config = BaiduTtsConfig::from_base(config.clone())?;
        baidu_config.validate()?;

        // Create HTTP client with connection pooling
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| TTSError::InternalError(format!("Failed to create HTTP client: {}", e)))?;

        // Create token manager
        let token_manager = Arc::new(TokenManager::new(
            baidu_config.api_key.clone(),
            baidu_config.secret_key.clone(),
            client.clone(),
        ));

        Ok(Self {
            base_config: config,
            config: baidu_config,
            token_manager,
            client,
            connected: Arc::new(AtomicBool::new(false)),
            audio_callback: Arc::new(Mutex::new(None)),
            bytes_synthesized: Arc::new(AtomicU64::new(0)),
            requests_count: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Prepare text for TTS request.
    /// Note: reqwest's `.form()` method handles URL encoding automatically.
    fn prepare_text(text: &str) -> String {
        text.to_string()
    }

    /// Check if text exceeds maximum length in GBK bytes.
    fn check_text_length(text: &str) -> bool {
        // Approximate GBK length: Chinese chars ~2 bytes, ASCII ~1 byte
        let approx_gbk_len: usize = text.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
        approx_gbk_len <= MAX_TEXT_LENGTH_GBK
    }

    /// Split text into chunks that fit within the GBK limit.
    fn split_text(text: &str) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut current_chunk = String::new();
        let mut current_len = 0;

        for c in text.chars() {
            let char_len = if c.is_ascii() { 1 } else { 2 };

            if current_len + char_len > MAX_TEXT_LENGTH_GBK - 10 {
                // Leave margin
                if !current_chunk.is_empty() {
                    chunks.push(current_chunk.clone());
                    current_chunk.clear();
                    current_len = 0;
                }
            }

            current_chunk.push(c);
            current_len += char_len;
        }

        if !current_chunk.is_empty() {
            chunks.push(current_chunk);
        }

        chunks
    }

    /// Synthesize a single chunk of text.
    async fn synthesize_chunk(&self, text: &str) -> TTSResult<Vec<u8>> {
        // Get access token
        let token = self.token_manager.get_token().await?;

        // Build request form data
        let prepared_text = Self::prepare_text(text);
        let endpoint = self.config.get_endpoint_url();

        let form_data = [
            ("tex", prepared_text.as_str()),
            ("tok", token.as_str()),
            ("cuid", self.config.cuid.as_str()),
            ("ctp", "1"),
            ("lan", "zh"),
            ("spd", &self.config.speed.to_string()),
            ("pit", &self.config.pitch.to_string()),
            ("vol", &self.config.volume.to_string()),
            ("per", &self.config.voice.as_id().to_string()),
            ("aue", &self.config.audio_format.as_aue().to_string()),
        ];

        debug!("Sending TTS request to Baidu: {} chars", text.len());

        // Send request
        let response = self
            .client
            .post(endpoint)
            .form(&form_data)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("TTS request failed: {}", e)))?;

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Check if response is JSON (error) or audio
        if content_type.contains("application/json") {
            let body = response.text().await.map_err(|e| {
                TTSError::NetworkError(format!("Failed to read error response: {}", e))
            })?;

            let error: BaiduTtsErrorResponse = serde_json::from_str(&body).map_err(|e| {
                TTSError::InternalError(format!("Failed to parse error response: {}", e))
            })?;

            // Handle specific error codes
            return match error.err_no {
                502 => {
                    // Token expired, invalidate and retry
                    self.token_manager.invalidate().await;
                    Err(TTSError::AuthenticationFailed(format!(
                        "Token validation failed: {}",
                        error.err_msg
                    )))
                }
                503 => Err(TTSError::RateLimited {
                    retry_after_secs: None,
                    message: format!("Quota exceeded: {}", error.err_msg),
                }),
                _ => Err(TTSError::ProviderError(format!(
                    "TTS error {}: {}",
                    error.err_no, error.err_msg
                ))),
            };
        }

        if !status.is_success() {
            return Err(TTSError::ProviderError(format!(
                "TTS request failed with status {}",
                status
            )));
        }

        // Read audio data
        let audio_data = response
            .bytes()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read audio data: {}", e)))?
            .to_vec();

        debug!("Received {} bytes of audio data", audio_data.len());

        // Update counters
        self.bytes_synthesized
            .fetch_add(audio_data.len() as u64, Ordering::Relaxed);
        self.requests_count.fetch_add(1, Ordering::Relaxed);

        Ok(audio_data)
    }
}

#[async_trait]
impl BaseTTS for BaiduTts {
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

        info!("Connecting to Baidu AI Cloud TTS...");

        // Pre-fetch access token to validate credentials
        self.token_manager.get_token().await?;

        self.connected.store(true, Ordering::SeqCst);
        info!("Connected to Baidu AI Cloud TTS (REST API)");

        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Disconnecting from Baidu AI Cloud TTS...");

        // Clear callbacks
        *self.audio_callback.lock().await = None;

        self.connected.store(false, Ordering::SeqCst);

        info!(
            "Disconnected from Baidu AI Cloud TTS (synthesized {} bytes, {} requests)",
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

        // Split text if necessary
        let chunks = if Self::check_text_length(text) {
            vec![text.to_string()]
        } else {
            warn!("Text exceeds maximum length, splitting into chunks");
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
                    sample_rate: self.config.get_sample_rate(),
                    format: self.config.audio_format.as_format_str().to_string(),
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
            "provider": "baidu",
            "name": PROVIDER_INFO,
            "version": "1.0.0",
            "description": "Baidu AI Cloud Text-to-Speech REST API with Chinese dialect support",
            "voice": self.config.voice.name(),
            "voice_id": self.config.voice.as_id(),
            "voice_category": self.config.voice.category().name(),
            "audio_format": self.config.audio_format.as_format_str(),
            "sample_rate": self.config.get_sample_rate(),
            "speed": self.config.speed,
            "pitch": self.config.pitch,
            "volume": self.config.volume,
            "endpoint": self.config.get_endpoint_url(),
            "max_text_length_gbk": MAX_TEXT_LENGTH_GBK,
            "features": [
                "rest-api",
                "multiple-voices",
                "speed-control",
                "pitch-control",
                "volume-control",
                "chinese-dialects",
                "premium-voices",
                "large-model-voices"
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
            api_key: "test_api_key|test_secret_key".to_string(),
            voice_id: Some("0".to_string()),
            sample_rate: Some(16000),
            audio_format: Some("mp3".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_new_provider() {
        let config = create_test_config();
        let result = BaiduTts::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_new_provider_empty_api_key() {
        let config = TTSConfig {
            api_key: "".to_string(),
            ..Default::default()
        };
        let result = BaiduTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_provider_invalid_api_key_format() {
        let config = TTSConfig {
            api_key: "missing_pipe_separator".to_string(),
            ..Default::default()
        };
        let result = BaiduTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_not_connected_initially() {
        let config = create_test_config();
        let tts = BaiduTts::new(config).unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let tts = BaiduTts::new(config).unwrap();
        let info = tts.get_provider_info();
        let info_str = info.to_string();
        assert!(info_str.contains("baidu") || info_str.contains("Baidu"));
        assert_eq!(info["provider"], "baidu");
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut tts = BaiduTts::new(config).unwrap();
        let result = tts.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_flush_ok() {
        let config = create_test_config();
        let tts = BaiduTts::new(config).unwrap();
        let result = tts.flush().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_clear_ok() {
        let config = create_test_config();
        let mut tts = BaiduTts::new(config).unwrap();
        let result = tts.clear().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_prepare_text() {
        let text = "你好世界";
        let prepared = BaiduTts::prepare_text(text);
        assert_eq!(prepared, text);
    }

    #[test]
    fn test_prepare_text_special_chars() {
        let text = "Hello & World <test>";
        let prepared = BaiduTts::prepare_text(text);
        // Note: reqwest's .form() handles URL encoding, so text is returned as-is
        assert_eq!(prepared, text);
    }

    #[test]
    fn test_check_text_length_short() {
        let text = "你好"; // 4 GBK bytes
        assert!(BaiduTts::check_text_length(text));
    }

    #[test]
    fn test_check_text_length_long() {
        // Create text that exceeds 1024 GBK bytes
        let text: String = "中".repeat(600); // 1200 GBK bytes
        assert!(!BaiduTts::check_text_length(&text));
    }

    #[test]
    fn test_split_text_short() {
        let text = "你好世界";
        let chunks = BaiduTts::split_text(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }

    #[test]
    fn test_split_text_long() {
        // Create text that needs splitting
        let text: String = "中".repeat(600); // 1200 GBK bytes
        let chunks = BaiduTts::split_text(&text);
        assert!(chunks.len() > 1);

        // Verify all chunks together equal original text
        let reconstructed: String = chunks.join("");
        assert_eq!(reconstructed, text);
    }

    #[test]
    fn test_split_text_empty() {
        let text = "";
        let chunks = BaiduTts::split_text(text);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_provider_info_statistics() {
        let config = create_test_config();
        let tts = BaiduTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert!(info["statistics"]["bytes_synthesized"].is_number());
        assert!(info["statistics"]["requests_count"].is_number());
    }

    #[test]
    fn test_provider_info_features() {
        let config = create_test_config();
        let tts = BaiduTts::new(config).unwrap();
        let info = tts.get_provider_info();

        let features = info["features"].as_array().unwrap();
        assert!(features.iter().any(|f| f == "rest-api"));
        assert!(features.iter().any(|f| f == "multiple-voices"));
        assert!(features.iter().any(|f| f == "chinese-dialects"));
    }

    #[test]
    fn test_provider_info_voice() {
        let config = create_test_config();
        let tts = BaiduTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["voice_id"], 0);
        assert_eq!(info["voice_category"], "Basic");
    }

    #[test]
    fn test_premium_voice_config() {
        let mut config = create_test_config();
        config.voice_id = Some("5003".to_string()); // Premium voice

        let tts = BaiduTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["voice_id"], 5003);
        assert_eq!(info["voice_category"], "Premium");
    }

    #[test]
    fn test_large_model_voice_config() {
        let mut config = create_test_config();
        config.voice_id = Some("4189".to_string()); // Large model voice

        let tts = BaiduTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["voice_id"], 4189);
        assert_eq!(info["voice_category"], "Large Model");
    }

    #[test]
    fn test_audio_format_config() {
        let mut config = create_test_config();
        config.audio_format = Some("wav".to_string());

        let tts = BaiduTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["audio_format"], "wav");
        assert_eq!(info["sample_rate"], 16000);
    }

    #[test]
    fn test_pcm8k_format_config() {
        let mut config = create_test_config();
        config.audio_format = Some("pcm8k".to_string());

        let tts = BaiduTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["audio_format"], "pcm");
        assert_eq!(info["sample_rate"], 8000);
    }
}
