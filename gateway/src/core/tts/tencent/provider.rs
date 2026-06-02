//! Tencent Cloud TTS Provider Implementation
//!
//! Provides HTTP REST-based text-to-speech synthesis using Tencent Cloud API.

use super::config::{
    MAX_TEXT_LENGTH, TTS_ACTION, TTS_VERSION, TencentTtsConfig, TencentTtsResponse,
};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

type HmacSha256 = Hmac<Sha256>;

// =============================================================================
// Provider Implementation
// =============================================================================

/// Tencent Cloud TTS provider.
pub struct TencentTts {
    /// Configuration.
    config: TencentTtsConfig,

    /// HTTP client.
    client: Client,

    /// Connection state.
    state: Arc<RwLock<ConnectionState>>,

    /// Audio callback.
    callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
}

impl TencentTts {
    /// Create a new Tencent TTS provider (internal).
    fn create_internal(config: TTSConfig) -> TTSResult<Self> {
        let tencent_config = TencentTtsConfig::from_base(config)?;
        tencent_config.validate()?;

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| TTSError::NetworkError(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            config: tencent_config,
            client,
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            callback: Arc::new(RwLock::new(None)),
        })
    }

    /// Build from the standardized TTS config (W1 keystone), mirroring `DeepgramTTS::from_standard`.
    /// Delegates the feature mapping to the config-level [`TencentTtsConfig::from_standard`] (which
    /// maps `speed`→`speed`, `volume`→`volume`, `word_timestamps`→`enable_subtitles`,
    /// `emotion`→`emotion_category`, plus the `project_id`/`use_intl_endpoint`/`emotion_intensity`/
    /// `primary_language`/`region` extras), then constructs the provider via [`Self::with_config`]
    /// so the mapped settings are honored end-to-end through the dispatch path.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> TTSResult<Self> {
        let tencent_config = TencentTtsConfig::from_standard(std)?;
        Self::with_config(tencent_config)
    }

    /// Create from Tencent-specific config.
    pub fn with_config(config: TencentTtsConfig) -> TTSResult<Self> {
        config.validate()?;

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| TTSError::NetworkError(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            config,
            client,
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            callback: Arc::new(RwLock::new(None)),
        })
    }

    /// Get current date in YYYY-MM-DD format.
    fn get_current_date() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Calculate date from Unix timestamp (UTC)
        let days_since_epoch = now / 86400;
        let mut remaining_days = days_since_epoch as i32;
        let mut y = 1970i32;

        loop {
            let days_in_year = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                366
            } else {
                365
            };

            if remaining_days < days_in_year {
                break;
            }
            remaining_days -= days_in_year;
            y += 1;
        }

        let is_leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let days_in_months: [i32; 12] = if is_leap {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };

        let mut m = 0usize;
        while m < 12 && remaining_days >= days_in_months[m] {
            remaining_days -= days_in_months[m];
            m += 1;
        }

        format!("{:04}-{:02}-{:02}", y, m + 1, remaining_days + 1)
    }

    /// Get current Unix timestamp.
    fn get_current_timestamp() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    /// Generate TC3-HMAC-SHA256 signature.
    fn sign_request(&self, payload: &str, timestamp: i64) -> TTSResult<(String, String, String)> {
        let date = Self::get_current_date();
        let service = "tts";
        let host = if self.config.use_intl_endpoint {
            "tts.intl.tencentcloudapi.com"
        } else {
            "tts.tencentcloudapi.com"
        };

        // Step 1: Create canonical request
        let http_method = "POST";
        let canonical_uri = "/";
        let canonical_query_string = "";
        let content_hash = hex::encode(Sha256::digest(payload.as_bytes()));
        let canonical_headers = format!(
            "content-type:application/json\nhost:{}\nx-tc-action:{}\n",
            host,
            TTS_ACTION.to_lowercase()
        );
        let signed_headers = "content-type;host;x-tc-action";

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            http_method,
            canonical_uri,
            canonical_query_string,
            canonical_headers,
            signed_headers,
            content_hash
        );

        // Step 2: Create string to sign
        let algorithm = "TC3-HMAC-SHA256";
        let credential_scope = format!("{}/{}/tc3_request", date, service);
        let hashed_canonical_request = hex::encode(Sha256::digest(canonical_request.as_bytes()));

        let string_to_sign = format!(
            "{}\n{}\n{}\n{}",
            algorithm, timestamp, credential_scope, hashed_canonical_request
        );

        // Step 3: Calculate signature
        let secret_date = Self::hmac_sha256(
            format!("TC3{}", self.config.secret_key).as_bytes(),
            date.as_bytes(),
        )?;
        let secret_service = Self::hmac_sha256(&secret_date, service.as_bytes())?;
        let secret_signing = Self::hmac_sha256(&secret_service, b"tc3_request")?;
        let signature = hex::encode(Self::hmac_sha256(
            &secret_signing,
            string_to_sign.as_bytes(),
        )?);

        // Step 4: Create authorization header
        let authorization = format!(
            "{} Credential={}/{}, SignedHeaders={}, Signature={}",
            algorithm, self.config.secret_id, credential_scope, signed_headers, signature
        );

        Ok((authorization, timestamp.to_string(), host.to_string()))
    }

    /// Calculate HMAC-SHA256.
    fn hmac_sha256(key: &[u8], data: &[u8]) -> TTSResult<Vec<u8>> {
        let mut mac = HmacSha256::new_from_slice(key)
            .map_err(|e| TTSError::InternalError(format!("HMAC key error: {}", e)))?;
        mac.update(data);
        Ok(mac.finalize().into_bytes().to_vec())
    }

    /// Build the request payload.
    fn build_payload(&self, text: &str) -> Value {
        let mut payload = json!({
            "Text": text,
            "SessionId": uuid::Uuid::new_v4().to_string(),
            "ProjectId": self.config.project_id,
            "VoiceType": self.config.voice_type.as_id(),
            "Codec": self.config.audio_format.as_codec(),
            "SampleRate": self.config.sample_rate.value(),
            "Speed": self.config.speed,
            "Volume": self.config.volume,
        });

        // Add optional parameters
        if self.config.enable_subtitles {
            payload["EnableSubtitle"] = json!(true);
        }

        if let Some(ref lang) = self.config.primary_language {
            payload["PrimaryLanguage"] = json!(lang);
        }

        if let Some(ref emotion) = self.config.emotion_category {
            payload["EmotionCategory"] = json!(emotion);
        }

        if let Some(intensity) = self.config.emotion_intensity {
            payload["EmotionIntensity"] = json!(intensity);
        }

        payload
    }

    /// Send TTS request and return audio data.
    async fn synthesize(&self, text: &str) -> TTSResult<Vec<u8>> {
        let payload = self.build_payload(text);
        let payload_str = serde_json::to_string(&payload)
            .map_err(|e| TTSError::InternalError(format!("Failed to serialize payload: {}", e)))?;

        let timestamp = Self::get_current_timestamp();
        let (authorization, timestamp_str, host) = self.sign_request(&payload_str, timestamp)?;

        let endpoint = self.config.get_endpoint_url();

        debug!("Sending TTS request to {}", endpoint);

        let response = self
            .client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .header("Host", host)
            .header("Authorization", authorization)
            .header("X-TC-Action", TTS_ACTION)
            .header("X-TC-Version", TTS_VERSION)
            .header("X-TC-Timestamp", timestamp_str)
            .header(
                "X-TC-Region",
                self.config.region.as_deref().unwrap_or("ap-guangzhou"),
            )
            .body(payload_str)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Request failed: {}", e)))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read response: {}", e)))?;

        if !status.is_success() {
            error!("TTS request failed with status {}: {}", status, body);
            return Err(TTSError::ProviderError(format!(
                "HTTP {}: {}",
                status, body
            )));
        }

        // Parse response
        let tts_response: TencentTtsResponse = serde_json::from_str(&body)
            .map_err(|e| TTSError::InternalError(format!("Failed to parse response: {}", e)))?;

        // Check for errors
        if let Some(error) = tts_response.response.error {
            return Err(TTSError::ProviderError(format!(
                "{}: {}",
                error.code, error.message
            )));
        }

        // Decode audio
        let audio_base64 = tts_response
            .response
            .audio
            .ok_or_else(|| TTSError::ProviderError("No audio data in response".to_string()))?;

        let audio_data = BASE64_STANDARD
            .decode(&audio_base64)
            .map_err(|e| TTSError::InternalError(format!("Failed to decode audio: {}", e)))?;

        debug!("Received {} bytes of audio data", audio_data.len());

        Ok(audio_data)
    }

    /// Split text into chunks if necessary.
    fn split_text(&self, text: &str) -> Vec<String> {
        if text.len() <= MAX_TEXT_LENGTH {
            return vec![text.to_string()];
        }

        // Split by sentence boundaries
        let mut chunks = Vec::new();
        let mut current_chunk = String::new();

        for sentence in text.split_inclusive(&['.', '!', '?', '。', '！', '？'][..]) {
            if current_chunk.len() + sentence.len() > MAX_TEXT_LENGTH {
                if !current_chunk.is_empty() {
                    chunks.push(current_chunk.trim().to_string());
                    current_chunk = String::new();
                }
            }
            current_chunk.push_str(sentence);
        }

        if !current_chunk.is_empty() {
            chunks.push(current_chunk.trim().to_string());
        }

        chunks
    }
}

#[async_trait]
impl BaseTTS for TencentTts {
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        Self::create_internal(config)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        let mut state = self.state.write().await;
        *state = ConnectionState::Connected;
        info!("Tencent TTS connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        let mut state = self.state.write().await;
        *state = ConnectionState::Disconnected;
        info!("Tencent TTS disconnected");
        Ok(())
    }

    async fn speak(&mut self, text: &str, is_final: bool) -> TTSResult<()> {
        // Handle empty text
        if text.trim().is_empty() {
            return Ok(());
        }

        // Split text if necessary
        let chunks = self.split_text(text);

        for (i, chunk) in chunks.iter().enumerate() {
            let is_last = is_final && i == chunks.len() - 1;

            match self.synthesize(chunk).await {
                Ok(audio_data) => {
                    let audio = AudioData {
                        data: audio_data,
                        sample_rate: self.config.get_sample_rate(),
                        format: self.config.audio_format.as_format_str().to_string(),
                        duration_ms: None,
                    };

                    let callback = self.callback.read().await;
                    if let Some(cb) = callback.as_ref() {
                        cb.on_audio(audio).await;
                    }
                }
                Err(e) => {
                    warn!("TTS synthesis error: {}", e);
                    let callback = self.callback.read().await;
                    if let Some(cb) = callback.as_ref() {
                        cb.on_error(e).await;
                    }
                }
            }

            if is_last {
                let callback = self.callback.read().await;
                if let Some(cb) = callback.as_ref() {
                    cb.on_complete().await;
                }
            }
        }

        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        // No-op for REST API (each speak() call is immediately processed)
        Ok(())
    }

    async fn clear(&mut self) -> TTSResult<()> {
        // No-op for REST API (no queue to clear)
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        let callback_arc = Arc::clone(&self.callback);
        tokio::spawn(async move {
            let mut cb = callback_arc.write().await;
            *cb = Some(callback);
        });
        Ok(())
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        let callback_arc = Arc::clone(&self.callback);
        tokio::spawn(async move {
            let mut cb = callback_arc.write().await;
            *cb = None;
        });
        Ok(())
    }

    fn get_connection_state(&self) -> ConnectionState {
        // Use try_read to avoid blocking
        if let Ok(state) = self.state.try_read() {
            state.clone()
        } else {
            ConnectionState::Disconnected
        }
    }

    fn is_ready(&self) -> bool {
        matches!(self.get_connection_state(), ConnectionState::Connected)
    }

    fn get_provider_info(&self) -> Value {
        json!({
            "provider": "tencent",
            "name": "Tencent Cloud TTS",
            "voice_id": self.config.voice_type.as_id(),
            "voice_name": self.config.voice_type.name(),
            "sample_rate": self.config.get_sample_rate(),
            "audio_format": self.config.audio_format.as_format_str(),
            "speed": self.config.speed,
            "volume": self.config.volume,
            "features": [
                "chinese",
                "english",
                "multilingual",
                "speed_control",
                "volume_control",
                "word_timestamps",
                "emotion_control"
            ],
            "languages": ["zh", "en", "zh-yue", "zh-sc"],
            "endpoint": self.config.get_endpoint_url()
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::super::config::{TencentTtsAudioFormat, TencentTtsVoice};
    use super::*;

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: "test_secret_id|test_secret_key".to_string(),
            voice_id: Some("0".to_string()),
            audio_format: Some("wav".to_string()),
            sample_rate: Some(16000),
            ..Default::default()
        }
    }

    // =========================================================================
    // Creation Tests
    // =========================================================================

    #[test]
    fn test_provider_creation() {
        let config = create_test_config();
        let result = TencentTts::new(config);
        assert!(result.is_ok());
    }

    // W1 keystone: the standardized dispatch path reaches the provider STRUCT's `from_standard`,
    // building the real provider with mapped prosody/emotion. Tencent supports speed + volume +
    // word_timestamps + emotion, so this asserts those reach the provider's config.
    #[test]
    fn from_standard_builds_provider_with_mapped_features() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "tencent".into(),
                api_key: "secret_id|secret_key".into(),
                voice_id: Some("0".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                volume: Some(8.0),
                word_timestamps: Some(true),
                emotion: Some("happy".into()),
                ssml: Some(true), // capability gap: ignored
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = TencentTts::from_standard(&std).unwrap();
        assert_eq!(tts.config.speed, 1.5);
        assert_eq!(tts.config.volume, 8.0);
        assert!(tts.config.enable_subtitles);
        assert_eq!(tts.config.emotion_category, Some("happy".to_string()));
    }

    #[test]
    fn test_provider_creation_invalid_key() {
        let config = TTSConfig {
            api_key: "no_pipe".to_string(),
            ..Default::default()
        };
        let result = TencentTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_provider_with_config() {
        let config = TencentTtsConfig::new("id", "key")
            .with_voice_type(TencentTtsVoice::FriendlyMale)
            .with_audio_format(TencentTtsAudioFormat::Mp3)
            .with_speed(1.5);

        let result = TencentTts::with_config(config);
        assert!(result.is_ok());
    }

    // =========================================================================
    // State Tests
    // =========================================================================

    #[test]
    fn test_initial_state() {
        let config = create_test_config();
        let tts = TencentTts::new(config).unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert!(!tts.is_ready());
    }

    #[tokio::test]
    async fn test_connect_disconnect() {
        let config = create_test_config();
        let mut tts = TencentTts::new(config).unwrap();

        tts.connect().await.unwrap();
        assert!(tts.is_ready());

        tts.disconnect().await.unwrap();
        assert!(!tts.is_ready());
    }

    // =========================================================================
    // Provider Info Tests
    // =========================================================================

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let tts = TencentTts::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "tencent");
        assert_eq!(info["name"], "Tencent Cloud TTS");
        assert!(info["voice_id"].is_number());
        assert!(info["sample_rate"].is_number());
        assert!(info["features"].is_array());
        assert!(info["languages"].is_array());
    }

    // =========================================================================
    // Text Splitting Tests
    // =========================================================================

    #[test]
    fn test_split_short_text() {
        let config = create_test_config();
        let tts = TencentTts::new(config).unwrap();

        let chunks = tts.split_text("Hello world");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world");
    }

    #[test]
    fn test_split_long_text() {
        let config = create_test_config();
        let tts = TencentTts::new(config).unwrap();

        // Create text longer than MAX_TEXT_LENGTH
        let long_text = "这是第一句话。".repeat(100);
        let chunks = tts.split_text(&long_text);
        assert!(chunks.len() > 1);
    }

    // =========================================================================
    // Signature Tests
    // =========================================================================

    #[test]
    fn test_hmac_sha256() {
        let result = TencentTts::hmac_sha256(b"key", b"data");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 32);
    }

    #[test]
    fn test_sign_request() {
        let config = create_test_config();
        let tts = TencentTts::new(config).unwrap();

        let result = tts.sign_request(r#"{"Text":"hello"}"#, 1234567890);
        assert!(result.is_ok());

        let (auth, timestamp, host) = result.unwrap();
        assert!(auth.starts_with("TC3-HMAC-SHA256"));
        assert_eq!(timestamp, "1234567890");
        assert!(host.contains("tencentcloudapi.com"));
    }

    // =========================================================================
    // Payload Tests
    // =========================================================================

    #[test]
    fn test_build_payload() {
        let config = create_test_config();
        let tts = TencentTts::new(config).unwrap();

        let payload = tts.build_payload("Hello world");
        assert_eq!(payload["Text"], "Hello world");
        assert!(payload["SessionId"].is_string());
        assert!(payload["VoiceType"].is_number());
        assert!(payload["Codec"].is_string());
    }

    #[test]
    fn test_build_payload_with_subtitles() {
        let base_config = create_test_config();
        let mut tencent_config = TencentTtsConfig::from_base(base_config).unwrap();
        tencent_config.enable_subtitles = true;

        let tts = TencentTts::with_config(tencent_config).unwrap();

        let payload = tts.build_payload("Hello world");
        assert_eq!(payload["EnableSubtitle"], true);
    }
}
