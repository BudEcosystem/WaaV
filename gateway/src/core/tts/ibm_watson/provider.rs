//! IBM Watson TTS provider implementation.
//!
//! This module provides the IBM Watson TTS provider that implements the `BaseTTS` trait
//! using IBM Watson Text-to-Speech REST API.
//!
//! # API Reference
//!
//! - Service: IBM Watson Text-to-Speech
//! - Operation: Synthesize
//! - Voices: 30+ V3 Neural voices across multiple languages
//! - Output formats: wav, mp3, ogg, flac, webm, l16 (pcm), mulaw, alaw
//!
//! # Authentication
//!
//! IBM Watson TTS uses IAM token-based authentication:
//! 1. API key is exchanged for a bearer token via IAM endpoint
//! 2. Token is cached and automatically refreshed before expiration
//! 3. Token is included in Authorization header for all requests
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig};
//! use waav_gateway::core::tts::ibm_watson::{IbmWatsonTTS, IbmWatsonTTSConfig, IbmVoice};
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = IbmWatsonTTSConfig {
//!         instance_id: std::env::var("IBM_WATSON_INSTANCE_ID").unwrap(),
//!         voice: IbmVoice::EnUsAllisonV3Voice,
//!         ..Default::default()
//!     };
//!     config.base.api_key = std::env::var("IBM_WATSON_API_KEY").unwrap();
//!
//!     let mut tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
//!     tts.connect().await.unwrap();
//!
//!     // Register audio callback
//!     // tts.on_audio(Arc::new(MyCallback)).unwrap();
//!
//!     // Synthesize text
//!     tts.speak("Hello, world!", true).await.unwrap();
//! }
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use reqwest::Client;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use url::form_urlencoded;

use super::config::{IBM_IAM_URL, IbmOutputFormat, IbmVoice, IbmWatsonTTSConfig, MAX_TEXT_LENGTH};
use crate::core::stt::ibm_watson::IbmRegion;
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use crate::utils::req_manager::ReqManager;

/// IBM Watson TTS API base URL (for documentation purposes).
pub const IBM_WATSON_TTS_URL: &str = "https://api.us-south.text-to-speech.watson.cloud.ibm.com";

// =============================================================================
// IAM Token Management
// =============================================================================

/// IAM token response from IBM Cloud.
#[derive(Debug, Clone)]
struct IamToken {
    /// The access token
    access_token: String,
    /// Token expiration time
    expires_at: Instant,
}

impl IamToken {
    /// Check if the token is expired or about to expire (within 5 minutes).
    fn is_expired(&self) -> bool {
        self.expires_at <= Instant::now() + Duration::from_secs(300)
    }
}

/// IAM token response from IBM Cloud API.
#[derive(Debug, serde::Deserialize)]
struct IamTokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}

/// Fetch IAM token from IBM Cloud.
async fn fetch_iam_token(api_key: &str) -> TTSResult<IamToken> {
    let client = Client::new();

    // URL-encode the API key
    let encoded_api_key: String = form_urlencoded::byte_serialize(api_key.as_bytes()).collect();

    let response = client
        .post(IBM_IAM_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=urn:ibm:params:oauth:grant-type:apikey&apikey={}",
            encoded_api_key
        ))
        .send()
        .await
        .map_err(|e| TTSError::ConnectionFailed(format!("Failed to fetch IAM token: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(TTSError::ConnectionFailed(format!(
            "IAM token request failed with status {}: {}",
            status, body
        )));
    }

    let token_response: IamTokenResponse = response
        .json()
        .await
        .map_err(|e| TTSError::ConnectionFailed(format!("Failed to parse IAM token: {}", e)))?;

    // Default to 1 hour if expires_in is not provided
    let expires_in = if token_response.expires_in > 0 {
        token_response.expires_in
    } else {
        3600
    };

    let expires_at = Instant::now() + Duration::from_secs(expires_in);

    debug!(
        "IAM token fetched successfully, expires in {} seconds",
        expires_in
    );

    Ok(IamToken {
        access_token: token_response.access_token,
        expires_at,
    })
}

// =============================================================================
// IBM Watson TTS Provider
// =============================================================================

/// IBM Watson Text-to-Speech provider implementation.
///
/// This provider uses the IBM Watson TTS REST API to synthesize speech from text.
/// It supports:
/// - Multiple V3 neural voices across many languages
/// - Multiple audio output formats (wav, mp3, ogg, flac, etc.)
/// - Rate and pitch adjustment via SSML
/// - Custom pronunciation dictionaries
/// - IAM token-based authentication with automatic refresh
///
/// The provider uses HTTP requests with connection pooling for efficient
/// communication with the IBM Watson TTS API.
pub struct IbmWatsonTTS {
    /// IBM Watson TTS configuration
    config: IbmWatsonTTSConfig,
    /// HTTP client for API requests
    client: Arc<RwLock<Option<Client>>>,
    /// Cached IAM token
    token: Arc<RwLock<Option<IamToken>>>,
    /// Connection state
    connected: Arc<AtomicBool>,
    /// Audio callback
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
    /// Request counter for logging (atomic for lock-free access)
    request_counter: Arc<AtomicU64>,
}

impl IbmWatsonTTS {
    /// Create a new IBM Watson TTS instance from base TTSConfig.
    ///
    /// This creates a default IBM Watson configuration and overrides voice and
    /// sample rate from the base config if provided.
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        // Parse voice from config
        let voice = config
            .voice_id
            .as_ref()
            .map(|v| IbmVoice::from_str_or_default(v))
            .unwrap_or_default();

        // Parse output format
        let output_format = config
            .audio_format
            .as_ref()
            .map(|f| IbmOutputFormat::from_str_or_default(f))
            .unwrap_or_default();

        // Build IBM-specific config from base config
        let ibm_config = IbmWatsonTTSConfig {
            base: config.clone(),
            region: IbmRegion::default(),
            instance_id: String::new(), // Must be set via environment or explicit config
            voice,
            output_format,
            rate_percentage: None,
            pitch_percentage: None,
            customization_ids: Vec::new(),
            spell_out_mode: None,
        };

        Ok(Self {
            config: ibm_config,
            client: Arc::new(RwLock::new(None)),
            token: Arc::new(RwLock::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
            audio_callback: Arc::new(RwLock::new(None)),
            request_counter: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Create a new IBM Watson TTS instance from IbmWatsonTTSConfig.
    ///
    /// Use this when you want full control over IBM-specific settings.
    pub fn new_from_ibm_config(config: IbmWatsonTTSConfig) -> TTSResult<Self> {
        Ok(Self {
            config,
            client: Arc::new(RwLock::new(None)),
            token: Arc::new(RwLock::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
            audio_callback: Arc::new(RwLock::new(None)),
            request_counter: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Set the instance ID (required before connecting).
    pub fn set_instance_id(&mut self, instance_id: String) {
        self.config.instance_id = instance_id;
    }

    /// Set the region.
    pub fn set_region(&mut self, region: IbmRegion) {
        self.config.region = region;
    }

    /// Set the voice.
    pub fn set_voice(&mut self, voice: IbmVoice) {
        self.config.voice = voice.clone();
        self.config.base.voice_id = Some(voice.as_str().to_string());
    }

    /// Set the output format.
    pub fn set_output_format(&mut self, format: IbmOutputFormat) {
        self.config.output_format = format;
        self.config.base.audio_format = Some(format.as_str().to_string());
    }

    /// Set the rate percentage (-100 to +100).
    pub fn set_rate_percentage(&mut self, rate: i32) -> TTSResult<()> {
        if !(-100..=100).contains(&rate) {
            return Err(TTSError::InvalidConfiguration(
                "Rate percentage must be between -100 and 100".to_string(),
            ));
        }
        self.config.rate_percentage = Some(rate);
        Ok(())
    }

    /// Set the pitch percentage (-100 to +100).
    pub fn set_pitch_percentage(&mut self, pitch: i32) -> TTSResult<()> {
        if !(-100..=100).contains(&pitch) {
            return Err(TTSError::InvalidConfiguration(
                "Pitch percentage must be between -100 and 100".to_string(),
            ));
        }
        self.config.pitch_percentage = Some(pitch);
        Ok(())
    }

    /// Get or refresh the IAM access token.
    async fn get_access_token(&self) -> TTSResult<String> {
        // Check cached token
        {
            let token_guard = self.token.read().await;
            if let Some(ref token) = *token_guard
                && !token.is_expired()
            {
                return Ok(token.access_token.clone());
            }
        }

        // Fetch new token
        debug!("Fetching new IAM token...");
        let new_token = fetch_iam_token(&self.config.base.api_key).await?;
        let access_token = new_token.access_token.clone();

        // Cache the token
        *self.token.write().await = Some(new_token);

        Ok(access_token)
    }

    /// Synthesize text to audio using IBM Watson TTS.
    async fn synthesize(&self, text: &str) -> TTSResult<Bytes> {
        let client = {
            let client_guard = self.client.read().await;
            client_guard
                .clone()
                .ok_or_else(|| TTSError::ProviderNotReady("HTTP client not initialized".into()))?
        };

        // Validate text length (IBM Watson counts bytes, not characters)
        // Note: str::len() in Rust returns byte length, not character count
        let text_bytes = text.len();
        if text_bytes > MAX_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text size {} bytes exceeds maximum {} bytes (5KB)",
                text_bytes, MAX_TEXT_LENGTH
            )));
        }

        // Increment request counter (lock-free atomic operation)
        let request_id = self.request_counter.fetch_add(1, Ordering::Relaxed) + 1;

        debug!(
            request_id = request_id,
            text_len = text.len(),
            voice = %self.config.voice,
            format = %self.config.output_format,
            "Synthesizing text with IBM Watson TTS"
        );

        // Get access token
        let access_token = self.get_access_token().await?;

        // Build URL with query parameters
        let base_url = self.config.build_synthesis_url();
        let query_params = self.config.build_query_params();
        let url = reqwest::Url::parse_with_params(&base_url, &query_params)
            .map_err(|e| TTSError::InternalError(format!("Failed to build URL: {}", e)))?;

        // Prepare request body
        let body = self.prepare_request_body(text);

        // Get Accept header for audio format
        let accept_header = self.config.accept_header();

        // Make the request
        let response = client
            .post(url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .header("Accept", accept_header)
            .body(body)
            .send()
            .await
            .map_err(|e| {
                error!(request_id = request_id, error = %e, "IBM Watson TTS API request failed");
                TTSError::NetworkError(format!("Request failed: {}", e))
            })?;

        // Check response status with specific error classification
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(
                request_id = request_id,
                status = %status,
                body = %body,
                "IBM Watson TTS API returned error"
            );

            // Classify error for proper handling
            let error = match status.as_u16() {
                400 => TTSError::InvalidConfiguration(format!("Bad request: {}", body)),
                401 => {
                    // Clear cached token so next request will refresh
                    // Note: This is handled by caller through token refresh
                    TTSError::ConnectionFailed(format!(
                        "Authentication failed - token may have expired: {}",
                        body
                    ))
                }
                403 => TTSError::ConnectionFailed(format!(
                    "Access forbidden - check service permissions: {}",
                    body
                )),
                404 => TTSError::InvalidConfiguration(format!(
                    "Resource not found - check instance ID and region: {}",
                    body
                )),
                429 => TTSError::ProviderError(format!(
                    "Rate limit exceeded - retry after backoff: {}",
                    body
                )),
                500..=599 => TTSError::ProviderError(format!(
                    "Server error ({}) - may be transient, retry recommended: {}",
                    status, body
                )),
                _ => TTSError::ProviderError(format!("TTS API error ({}): {}", status, body)),
            };

            return Err(error);
        }

        // Read audio bytes
        let audio_bytes = response.bytes().await.map_err(|e| {
            error!(request_id = request_id, error = %e, "Failed to read audio response");
            TTSError::AudioGenerationFailed(format!("Failed to read audio: {}", e))
        })?;

        debug!(
            request_id = request_id,
            audio_bytes = audio_bytes.len(),
            "Successfully synthesized audio"
        );

        Ok(audio_bytes)
    }

    /// Prepare the request body with optional SSML prosody for rate/pitch.
    pub(crate) fn prepare_request_body(&self, text: &str) -> String {
        // Check if we need SSML for rate/pitch adjustment
        let needs_ssml =
            self.config.rate_percentage.is_some() || self.config.pitch_percentage.is_some();

        if needs_ssml {
            // Wrap text in SSML with prosody element
            let mut prosody_attrs = String::new();

            if let Some(rate) = self.config.rate_percentage {
                let rate_str = if rate >= 0 {
                    format!("+{}%", rate)
                } else {
                    format!("{}%", rate)
                };
                prosody_attrs.push_str(&format!(" rate=\"{}\"", rate_str));
            }

            if let Some(pitch) = self.config.pitch_percentage {
                let pitch_str = if pitch >= 0 {
                    format!("+{}%", pitch)
                } else {
                    format!("{}%", pitch)
                };
                prosody_attrs.push_str(&format!(" pitch=\"{}\"", pitch_str));
            }

            // Escape XML special characters in text
            let escaped_text = text
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&apos;");

            // Build SSML with required namespace per W3C spec
            let ssml = format!(
                "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\"><prosody{}>{}</prosody></speak>",
                prosody_attrs, escaped_text
            );

            serde_json::json!({ "text": ssml }).to_string()
        } else {
            // Plain text
            serde_json::json!({ "text": text }).to_string()
        }
    }

    /// Process audio and deliver to callback with proper chunking.
    async fn deliver_audio(&self, audio_bytes: Bytes) -> TTSResult<()> {
        let callback = self.audio_callback.read().await.clone();

        let Some(cb) = callback else {
            debug!("No audio callback registered, discarding audio");
            return Ok(());
        };

        let format = self.config.output_format.as_str().to_string();
        let sample_rate = self.config.effective_sample_rate();

        // For PCM/L16, chunk the audio for streaming delivery
        // For compressed formats (MP3, OGG, etc.), deliver as single chunk
        match self.config.output_format {
            IbmOutputFormat::L16 | IbmOutputFormat::Mulaw | IbmOutputFormat::Alaw => {
                // Raw audio: chunk into ~10ms segments for streaming
                let bytes_per_sample = match self.config.output_format {
                    IbmOutputFormat::L16 => 2,   // 16-bit
                    IbmOutputFormat::Mulaw => 1, // 8-bit
                    IbmOutputFormat::Alaw => 1,  // 8-bit
                    _ => 2,
                };
                let samples_per_chunk = sample_rate / 100; // 10ms worth
                let chunk_size = (samples_per_chunk * bytes_per_sample) as usize;

                let audio_vec = audio_bytes.to_vec();
                let mut offset = 0;

                while offset < audio_vec.len() {
                    let end = (offset + chunk_size).min(audio_vec.len());
                    let chunk = audio_vec[offset..end].to_vec();
                    let chunk_len = chunk.len();

                    let duration_ms =
                        Some(((chunk_len / bytes_per_sample as usize) as u32 * 1000) / sample_rate);

                    let audio_data = AudioData {
                        data: chunk,
                        sample_rate,
                        format: format.clone(),
                        duration_ms,
                    };

                    cb.on_audio(audio_data).await;
                    offset = end;
                }
            }
            _ => {
                // Compressed/container formats: deliver as single chunk
                let audio_data = AudioData {
                    data: audio_bytes.to_vec(),
                    sample_rate,
                    format,
                    duration_ms: None,
                };

                cb.on_audio(audio_data).await;
            }
        }

        // Notify completion
        cb.on_complete().await;

        Ok(())
    }

    /// Get the configured voice.
    pub fn voice(&self) -> IbmVoice {
        self.config.voice.clone()
    }

    /// Get the configured output format.
    pub fn output_format(&self) -> IbmOutputFormat {
        self.config.output_format
    }

    /// Get the configured region.
    pub fn region(&self) -> IbmRegion {
        self.config.region
    }

    /// Get the IBM Watson TTS configuration.
    pub fn ibm_config(&self) -> &IbmWatsonTTSConfig {
        &self.config
    }
}

#[async_trait]
impl BaseTTS for IbmWatsonTTS {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        IbmWatsonTTS::new(config)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        if self.connected.load(Ordering::Acquire) {
            debug!("IBM Watson TTS already connected");
            return Ok(());
        }

        // Load instance ID from environment if not set
        if self.config.instance_id.is_empty() {
            self.config.instance_id = std::env::var("IBM_WATSON_INSTANCE_ID").unwrap_or_default();
        }

        // Load API key from environment if not set
        if self.config.base.api_key.is_empty() {
            self.config.base.api_key = std::env::var("IBM_WATSON_API_KEY").unwrap_or_default();
        }

        // Validate configuration
        self.config
            .validate()
            .map_err(TTSError::InvalidConfiguration)?;

        info!(
            region = %self.config.region,
            voice = %self.config.voice,
            format = %self.config.output_format,
            "Connecting to IBM Watson TTS"
        );

        // Build HTTP client with timeouts
        let client = Client::builder()
            .timeout(Duration::from_secs(
                self.config.base.request_timeout.unwrap_or(60),
            ))
            .connect_timeout(Duration::from_secs(
                self.config.base.connection_timeout.unwrap_or(30),
            ))
            .pool_max_idle_per_host(self.config.base.request_pool_size.unwrap_or(4))
            .pool_idle_timeout(Duration::from_secs(90)) // Close idle connections after 90s
            .build()
            .map_err(|e| {
                TTSError::ConnectionFailed(format!("Failed to create HTTP client: {}", e))
            })?;

        *self.client.write().await = Some(client);

        // Pre-fetch IAM token to validate credentials
        self.get_access_token().await?;

        self.connected.store(true, Ordering::Release);

        info!("IBM Watson TTS connected successfully");
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        if !self.connected.load(Ordering::Acquire) {
            debug!("IBM Watson TTS already disconnected");
            return Ok(());
        }

        info!("Disconnecting from IBM Watson TTS");

        // Clear client
        *self.client.write().await = None;

        // Clear token
        *self.token.write().await = None;

        // Clear callback
        *self.audio_callback.write().await = None;

        self.connected.store(false, Ordering::Release);

        info!("IBM Watson TTS disconnected");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    fn get_connection_state(&self) -> ConnectionState {
        if self.connected.load(Ordering::Acquire) {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    async fn speak(&mut self, text: &str, _flush: bool) -> TTSResult<()> {
        // Auto-connect if needed
        if !self.is_ready() {
            warn!("IBM Watson TTS not ready, attempting to connect...");
            self.connect().await?;
        }

        // Skip empty text
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }

        // Synthesize
        let audio_bytes = self.synthesize(text).await?;

        // Deliver to callback
        self.deliver_audio(audio_bytes).await?;

        Ok(())
    }

    async fn clear(&mut self) -> TTSResult<()> {
        // IBM Watson TTS is synchronous (one request at a time)
        // Nothing to clear
        debug!("IBM Watson TTS clear (no-op for synchronous API)");
        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        // IBM Watson TTS is synchronous, no buffering
        debug!("IBM Watson TTS flush (no-op for synchronous API)");
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        // Use try_write which doesn't block - safe in both sync and async contexts
        if let Ok(mut guard) = self.audio_callback.try_write() {
            *guard = Some(callback);
            Ok(())
        } else {
            Err(TTSError::InternalError(
                "Failed to register audio callback - lock contention".into(),
            ))
        }
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        // Use try_write which doesn't block - safe in both sync and async contexts
        if let Ok(mut guard) = self.audio_callback.try_write() {
            *guard = None;
            Ok(())
        } else {
            Err(TTSError::InternalError(
                "Failed to remove audio callback - lock contention".into(),
            ))
        }
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "ibm-watson",
            "version": "1.0.0",
            "api_type": "REST",
            "connection_pooling": true,
            "region": self.config.region.as_str(),
            "supported_formats": [
                "wav", "mp3", "ogg-opus", "ogg-vorbis", "flac",
                "webm", "l16", "mulaw", "alaw", "basic"
            ],
            "max_text_length": MAX_TEXT_LENGTH,
            "supported_voices": [
                // US English
                "en-US_AllisonV3Voice", "en-US_EmilyV3Voice", "en-US_HenryV3Voice",
                "en-US_KevinV3Voice", "en-US_LisaV3Voice", "en-US_MichaelV3Voice",
                "en-US_OliviaV3Voice",
                // UK English
                "en-GB_CharlotteV3Voice", "en-GB_JamesV3Voice", "en-GB_KateV3Voice",
                // Australian English
                "en-AU_CraigV3Voice", "en-AU_MadisonV3Voice",
                // German
                "de-DE_BirgitV3Voice", "de-DE_DieterV3Voice", "de-DE_ErikaV3Voice",
                // Spanish
                "es-ES_EnriqueV3Voice", "es-ES_LauraV3Voice",
                "es-LA_SofiaV3Voice", "es-US_SofiaV3Voice",
                // French
                "fr-FR_NicolasV3Voice", "fr-FR_ReneeV3Voice", "fr-CA_LouiseV3Voice",
                // Italian
                "it-IT_FrancescaV3Voice",
                // Japanese
                "ja-JP_EmiV3Voice",
                // Korean
                "ko-KR_HyunjunV3Voice", "ko-KR_SiwooV3Voice",
                "ko-KR_YoungmiV3Voice", "ko-KR_YunaV3Voice",
                // Dutch
                "nl-NL_EmmaV3Voice", "nl-NL_LiamV3Voice",
                // Portuguese
                "pt-BR_IsabelaV3Voice",
                // Chinese
                "zh-CN_LiNaVoice", "zh-CN_WangWeiVoice", "zh-CN_ZhangJingVoice"
            ],
            "features": {
                "ssml": true,
                "rate_control": true,
                "pitch_control": true,
                "custom_pronunciation": true,
                "multiple_languages": true
            },
            "documentation": "https://cloud.ibm.com/apidocs/text-to-speech"
        })
    }

    async fn set_req_manager(&mut self, _req_manager: Arc<ReqManager>) {
        // IBM Watson TTS uses its own HTTP client with IAM authentication
        // This is a no-op, but could be enhanced to use shared connection pool
        debug!("IBM Watson TTS does not use shared ReqManager (uses IAM auth)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ibm_watson_tts_creation() {
        let config = TTSConfig {
            provider: "ibm-watson".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("en-US_AllisonV3Voice".to_string()),
            audio_format: Some("wav".to_string()),
            sample_rate: Some(22050),
            ..Default::default()
        };

        let tts = IbmWatsonTTS::new(config).unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert_eq!(tts.voice(), IbmVoice::EnUsAllisonV3Voice);
        assert_eq!(tts.output_format(), IbmOutputFormat::Wav);
    }

    #[tokio::test]
    async fn test_ibm_watson_tts_from_ibm_config() {
        let config = IbmWatsonTTSConfig {
            voice: IbmVoice::EnGbCharlotteV3Voice,
            output_format: IbmOutputFormat::OggOpus,
            region: IbmRegion::EuGb,
            instance_id: "test-instance".to_string(),
            base: TTSConfig {
                api_key: "test-key".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
        assert_eq!(tts.voice(), IbmVoice::EnGbCharlotteV3Voice);
        assert_eq!(tts.output_format(), IbmOutputFormat::OggOpus);
        assert_eq!(tts.region(), IbmRegion::EuGb);
    }

    #[tokio::test]
    async fn test_ibm_watson_tts_set_voice() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        tts.set_voice(IbmVoice::DeDeBirgitV3Voice);
        assert_eq!(tts.voice(), IbmVoice::DeDeBirgitV3Voice);
    }

    #[tokio::test]
    async fn test_ibm_watson_tts_set_output_format() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        tts.set_output_format(IbmOutputFormat::Mp3);
        assert_eq!(tts.output_format(), IbmOutputFormat::Mp3);
    }

    #[tokio::test]
    async fn test_ibm_watson_tts_set_rate_percentage() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        assert!(tts.set_rate_percentage(50).is_ok());
        assert!(tts.set_rate_percentage(-50).is_ok());
        assert!(tts.set_rate_percentage(150).is_err());
        assert!(tts.set_rate_percentage(-150).is_err());
    }

    #[tokio::test]
    async fn test_ibm_watson_tts_set_pitch_percentage() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        assert!(tts.set_pitch_percentage(25).is_ok());
        assert!(tts.set_pitch_percentage(-25).is_ok());
        assert!(tts.set_pitch_percentage(101).is_err());
    }

    #[test]
    fn test_prepare_request_body_plain_text() {
        let config = IbmWatsonTTSConfig::default();
        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();

        let body = tts.prepare_request_body("Hello, world!");
        assert!(body.contains("Hello, world!"));
        assert!(!body.contains("<speak"));
    }

    #[test]
    fn test_prepare_request_body_with_ssml() {
        let mut config = IbmWatsonTTSConfig::default();
        config.rate_percentage = Some(50);
        config.pitch_percentage = Some(-25);

        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
        let body = tts.prepare_request_body("Hello, world!");

        // Note: JSON escapes quotes, so we check for escaped versions
        assert!(body.contains("<speak"));
        assert!(body.contains("xmlns=")); // SSML namespace
        assert!(body.contains("<prosody"));
        assert!(body.contains(r#"rate=\"+50%\""#));
        assert!(body.contains(r#"pitch=\"-25%\""#));
        assert!(body.contains("Hello, world!"));
    }

    #[test]
    fn test_prepare_request_body_escapes_xml() {
        let mut config = IbmWatsonTTSConfig::default();
        config.rate_percentage = Some(10);

        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
        let body = tts.prepare_request_body("Tom & Jerry <3 cats > dogs");

        assert!(body.contains("&amp;"));
        assert!(body.contains("&lt;"));
        assert!(body.contains("&gt;"));
    }

    #[test]
    fn test_provider_info() {
        let config = TTSConfig::default();
        let tts = IbmWatsonTTS::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "ibm-watson");
        assert_eq!(info["api_type"], "REST");
        assert!(
            info["supported_formats"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("wav"))
        );
        assert!(info["features"]["ssml"].as_bool().unwrap());
        assert!(info["features"]["rate_control"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_ibm_watson_tts_set_region() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        tts.set_region(IbmRegion::AuSyd);
        assert_eq!(tts.region(), IbmRegion::AuSyd);
    }

    #[tokio::test]
    async fn test_ibm_watson_tts_set_instance_id() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        tts.set_instance_id("new-instance-id".to_string());
        assert_eq!(tts.ibm_config().instance_id, "new-instance-id");
    }
}
