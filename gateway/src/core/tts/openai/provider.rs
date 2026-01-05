//! OpenAI TTS provider implementation.
//!
//! This module provides the OpenAI TTS provider that implements the `BaseTTS` trait
//! using OpenAI's text-to-speech API.
//!
//! # API Reference
//!
//! - Endpoint: `POST https://api.openai.com/v1/audio/speech`
//! - Models: tts-1, tts-1-hd, gpt-4o-mini-tts
//! - Voices: alloy, ash, ballad, coral, echo, fable, onyx, nova, sage, shimmer, verse
//! - Output: mp3, opus, aac, flac, wav, pcm (24kHz)
//! - Speed: 0.25 to 4.0

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use xxhash_rust::xxh3::xxh3_128;

use super::config::{AudioOutputFormat, OpenAITTSModel, OpenAIVoice};
use crate::core::tts::base::{AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSResult};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};
use crate::utils::req_manager::ReqManager;

/// OpenAI TTS API endpoint
pub const OPENAI_TTS_URL: &str = "https://api.openai.com/v1/audio/speech";

// =============================================================================
// Request Builder
// =============================================================================

/// OpenAI-specific TTS request builder
#[derive(Clone)]
struct OpenAIRequestBuilder {
    /// Base TTS configuration
    config: TTSConfig,
    /// Parsed OpenAI model
    model: OpenAITTSModel,
    /// Parsed OpenAI voice
    voice: OpenAIVoice,
    /// Parsed audio output format
    response_format: AudioOutputFormat,
    /// Speaking speed (0.25 to 4.0)
    speed: f32,
    /// Pronunciation replacer
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl TTSRequestBuilder for OpenAIRequestBuilder {
    /// Build the OpenAI-specific HTTP request
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build the request body
        let mut body = json!({
            "model": self.model.as_str(),
            "input": text,
            "voice": self.voice.as_str(),
            "response_format": self.response_format.as_str(),
        });

        // Add speed if not default (1.0)
        if (self.speed - 1.0).abs() > 0.001 {
            body["speed"] = json!(self.speed);
        }

        client
            .post(OPENAI_TTS_URL)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
    }

    /// Get the configuration
    fn get_config(&self) -> &TTSConfig {
        &self.config
    }

    /// Get precompiled pronunciation replacer
    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        self.pronunciation_replacer.as_ref()
    }
}

// =============================================================================
// Config Hash for Caching
// =============================================================================

/// Compute a hash of the TTS configuration for caching purposes
fn compute_tts_config_hash(
    config: &TTSConfig,
    model: &OpenAITTSModel,
    voice: &OpenAIVoice,
) -> String {
    let mut s = String::new();
    s.push_str("openai");
    s.push('|');
    s.push_str(model.as_str());
    s.push('|');
    s.push_str(voice.as_str());
    s.push('|');
    s.push_str(config.audio_format.as_deref().unwrap_or("mp3"));
    s.push('|');
    if let Some(sr) = config.sample_rate {
        s.push_str(&sr.to_string());
    }
    s.push('|');
    if let Some(rate) = config.speaking_rate {
        s.push_str(&format!("{rate:.3}"));
    }
    let hash = xxh3_128(s.as_bytes());
    format!("{hash:032x}")
}

// =============================================================================
// OpenAI TTS Provider
// =============================================================================

/// OpenAI TTS provider implementation using the OpenAI Audio Speech API
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::{BaseTTS, TTSConfig, OpenAITTS, AudioCallback, AudioData};
/// use std::sync::Arc;
///
/// struct MyCallback;
/// impl AudioCallback for MyCallback {
///     fn on_audio(&self, audio: AudioData) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
///         Box::pin(async { println!("Received {} bytes", audio.data.len()); })
///     }
///     fn on_error(&self, _: TTSError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
///         Box::pin(async {})
///     }
///     fn on_complete(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
///         Box::pin(async {})
///     }
/// }
///
/// #[tokio::main]
/// async fn main() {
///     let config = TTSConfig {
///         api_key: "sk-...".to_string(),
///         voice_id: Some("nova".to_string()),
///         model: "tts-1".to_string(),
///         ..Default::default()
///     };
///
///     let mut tts = OpenAITTS::new(config).unwrap();
///     tts.connect().await.unwrap();
///     tts.on_audio(Arc::new(MyCallback)).unwrap();
///     tts.speak("Hello, world!", true).await.unwrap();
/// }
/// ```
pub struct OpenAITTS {
    /// Generic HTTP-based TTS provider
    provider: TTSProvider,
    /// Request builder with OpenAI-specific configuration
    request_builder: OpenAIRequestBuilder,
    /// Precomputed config hash for caching
    config_hash: String,
}

impl OpenAITTS {
    /// Create a new OpenAI TTS instance
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        // Parse model from config
        let model = if config.model.is_empty() {
            OpenAITTSModel::default()
        } else {
            OpenAITTSModel::from_str_or_default(&config.model)
        };

        // Parse voice from config
        let voice = if let Some(ref voice_id) = config.voice_id {
            OpenAIVoice::from_str_or_default(voice_id)
        } else {
            OpenAIVoice::default()
        };

        // Parse audio format
        let response_format = if let Some(ref format) = config.audio_format {
            AudioOutputFormat::from_str_or_default(format)
        } else {
            // Default to PCM for consistency with other providers in WaaV
            AudioOutputFormat::Pcm
        };

        // Parse speed (default 1.0, clamp to valid range)
        let speed = config.speaking_rate.unwrap_or(1.0).clamp(0.25, 4.0);

        // Create pronunciation replacer if needed
        let pronunciation_replacer = if !config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&config.pronunciations))
        } else {
            None
        };

        let request_builder = OpenAIRequestBuilder {
            config: config.clone(),
            model,
            voice,
            response_format,
            speed,
            pronunciation_replacer,
        };

        let config_hash = compute_tts_config_hash(&config, &model, &voice);

        Ok(Self {
            provider: TTSProvider::new()?,
            request_builder,
            config_hash,
        })
    }

    /// Get the configured model
    pub fn model(&self) -> OpenAITTSModel {
        self.request_builder.model
    }

    /// Get the configured voice
    pub fn voice(&self) -> OpenAIVoice {
        self.request_builder.voice
    }

    /// Get the configured output format
    pub fn output_format(&self) -> AudioOutputFormat {
        self.request_builder.response_format
    }
}

impl Default for OpenAITTS {
    fn default() -> Self {
        Self::new(TTSConfig::default()).unwrap()
    }
}

#[async_trait]
impl BaseTTS for OpenAITTS {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        OpenAITTS::new(config)
    }

    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        self.provider
            .generic_connect_with_config(OPENAI_TTS_URL, &self.request_builder.config)
            .await
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.provider.generic_disconnect().await
    }

    fn is_ready(&self) -> bool {
        self.provider.is_ready()
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.provider.get_connection_state()
    }

    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        // Handle reconnection if needed
        if !self.is_ready() {
            tracing::info!("OpenAI TTS not ready, attempting to connect...");
            self.connect().await?;
        }

        // Set config hash once on first speak (idempotent)
        self.provider
            .set_tts_config_hash(self.config_hash.clone())
            .await;

        self.provider
            .generic_speak(self.request_builder.clone(), text, flush)
            .await
    }

    async fn clear(&mut self) -> TTSResult<()> {
        self.provider.generic_clear().await
    }

    async fn flush(&self) -> TTSResult<()> {
        self.provider.generic_flush().await
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.provider.generic_on_audio(callback)
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        self.provider.generic_remove_audio_callback()
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "openai",
            "version": "1.0.0",
            "api_type": "HTTP REST",
            "connection_pooling": true,
            "supported_formats": ["mp3", "opus", "aac", "flac", "wav", "pcm"],
            "default_sample_rate": 24000,
            "supported_models": [
                "tts-1",
                "tts-1-hd",
                "gpt-4o-mini-tts"
            ],
            "supported_voices": [
                "alloy", "ash", "ballad", "coral", "echo",
                "fable", "onyx", "nova", "sage", "shimmer", "verse"
            ],
            "speed_range": {
                "min": 0.25,
                "max": 4.0,
                "default": 1.0
            },
            "endpoint": OPENAI_TTS_URL,
            "documentation": "https://platform.openai.com/docs/api-reference/audio/createSpeech",
        })
    }

    async fn set_req_manager(&mut self, req_manager: Arc<ReqManager>) {
        self.provider.set_req_manager(req_manager).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_openai_tts_creation() {
        let config = TTSConfig {
            provider: "openai".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("nova".to_string()),
            model: "tts-1-hd".to_string(),
            audio_format: Some("pcm".to_string()),
            speaking_rate: Some(1.2),
            ..Default::default()
        };

        let tts = OpenAITTS::new(config).unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert_eq!(tts.model(), OpenAITTSModel::Tts1Hd);
        assert_eq!(tts.voice(), OpenAIVoice::Nova);
        assert_eq!(tts.output_format(), AudioOutputFormat::Pcm);
    }

    #[tokio::test]
    async fn test_openai_tts_default_values() {
        let config = TTSConfig {
            provider: "openai".to_string(),
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        let tts = OpenAITTS::new(config).unwrap();
        assert_eq!(tts.model(), OpenAITTSModel::Tts1);
        assert_eq!(tts.voice(), OpenAIVoice::Alloy);
        assert_eq!(tts.output_format(), AudioOutputFormat::Pcm);
    }

    #[tokio::test]
    async fn test_http_request_building() {
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            voice_id: Some("nova".to_string()),
            model: "tts-1".to_string(),
            audio_format: Some("mp3".to_string()),
            speaking_rate: Some(1.5),
            ..Default::default()
        };

        let builder = OpenAIRequestBuilder {
            config,
            model: OpenAITTSModel::Tts1,
            voice: OpenAIVoice::Nova,
            response_format: AudioOutputFormat::Mp3,
            speed: 1.5,
            pronunciation_replacer: None,
        };

        let client = reqwest::Client::new();
        let request = builder.build_http_request(&client, "Hello world");
        let built = request.build().unwrap();

        // Verify URL
        assert_eq!(built.url().as_str(), OPENAI_TTS_URL);

        // Verify headers
        let auth_header = built.headers().get("Authorization").unwrap();
        assert_eq!(auth_header, "Bearer test_key");

        let content_type = built.headers().get("Content-Type").unwrap();
        assert_eq!(content_type, "application/json");
    }

    #[tokio::test]
    async fn test_speed_clamping() {
        // Test speed below minimum
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            speaking_rate: Some(0.1), // Below 0.25 minimum
            ..Default::default()
        };
        let tts = OpenAITTS::new(config).unwrap();
        assert!((tts.request_builder.speed - 0.25).abs() < 0.001);

        // Test speed above maximum
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            speaking_rate: Some(5.0), // Above 4.0 maximum
            ..Default::default()
        };
        let tts = OpenAITTS::new(config).unwrap();
        assert!((tts.request_builder.speed - 4.0).abs() < 0.001);

        // Test speed within range
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            speaking_rate: Some(2.0),
            ..Default::default()
        };
        let tts = OpenAITTS::new(config).unwrap();
        assert!((tts.request_builder.speed - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_provider_info() {
        let tts = OpenAITTS::default();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "openai");
        assert!(
            info["supported_models"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("tts-1"))
        );
        assert!(
            info["supported_voices"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("nova"))
        );
    }

    #[test]
    fn test_config_hash_uniqueness() {
        let config1 = TTSConfig {
            api_key: "key".to_string(),
            model: "tts-1".to_string(),
            ..Default::default()
        };

        let config2 = TTSConfig {
            api_key: "key".to_string(),
            model: "tts-1-hd".to_string(),
            ..Default::default()
        };

        let hash1 = compute_tts_config_hash(&config1, &OpenAITTSModel::Tts1, &OpenAIVoice::Alloy);
        let hash2 = compute_tts_config_hash(&config2, &OpenAITTSModel::Tts1Hd, &OpenAIVoice::Alloy);

        assert_ne!(hash1, hash2);
    }
}
