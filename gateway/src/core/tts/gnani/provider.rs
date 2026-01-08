//! Gnani.ai TTS Provider Implementation
//!
//! Implements the BaseTTS trait for Gnani's Text-to-Speech REST API.

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};

use super::config::GnaniTTSConfig;

/// Gnani Text-to-Speech provider
///
/// Implements TTS using Gnani's REST API with support for
/// multi-speaker voices and SSML gender selection.
pub struct GnaniTTS {
    /// Provider-specific configuration
    config: Option<GnaniTTSConfig>,

    /// HTTP client for REST API calls
    http_client: Option<reqwest::Client>,

    /// Connection state
    is_connected: Arc<AtomicBool>,

    /// Audio callback for streaming output
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,

    /// Connection state enum
    connection_state: ConnectionState,
}

impl Default for GnaniTTS {
    fn default() -> Self {
        Self {
            config: None,
            http_client: None,
            is_connected: Arc::new(AtomicBool::new(false)),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: ConnectionState::Disconnected,
        }
    }
}

impl GnaniTTS {
    /// Create a new Gnani TTS instance
    pub fn create(config: TTSConfig) -> TTSResult<Self> {
        let gnani_config =
            GnaniTTSConfig::from_base(config).map_err(|e| TTSError::InvalidConfiguration(e))?;

        Ok(Self {
            config: Some(gnani_config),
            ..Default::default()
        })
    }

    /// Build HTTP client
    fn build_client(config: &GnaniTTSConfig) -> TTSResult<reqwest::Client> {
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.request_timeout_secs));

        // Add certificate if provided (optional for TTS)
        if let Some(ref path) = config.certificate_path {
            if path.exists() {
                if let Ok(cert_pem) = std::fs::read(path) {
                    if let Ok(cert) = reqwest::Certificate::from_pem(&cert_pem) {
                        builder = builder.add_root_certificate(cert);
                    }
                }
            }
        }

        builder.build().map_err(|e| {
            TTSError::InvalidConfiguration(format!("Failed to build HTTP client: {}", e))
        })
    }

    /// Synthesize text to audio using Gnani API
    async fn synthesize(&self, text: &str) -> TTSResult<Vec<u8>> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| TTSError::InvalidConfiguration("No configuration set".to_string()))?;

        let client = self
            .http_client
            .as_ref()
            .ok_or_else(|| TTSError::ProviderNotReady("Not connected".to_string()))?;

        // Build request body
        let request_body = GnaniTTSRequest {
            input: GnaniInput {
                text: text.to_string(),
            },
            voice: GnaniVoice {
                language_code: config.language_code.as_str().to_string(),
                name: config
                    .voice_name
                    .clone()
                    .unwrap_or_else(|| "gnani".to_string()),
                ssml_gender: config.ssml_gender.as_str().to_string(),
            },
            audio_config: GnaniAudioConfig {
                audio_encoding: "pcm16".to_string(),
                sample_rate: config.output_sample_rate,
            },
        };

        debug!(
            text_len = text.len(),
            language = %config.language_code.as_str(),
            gender = %config.ssml_gender.as_str(),
            "Gnani TTS synthesis request"
        );

        let response = client
            .post(config.endpoint())
            .header("token", &config.token)
            .header("accesskey", &config.access_key)
            .header("lang", config.language_code.as_str())
            .header("product", "tts")
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(TTSError::ProviderError(format!(
                "Gnani TTS API error {}: {}",
                status, error_text
            )));
        }

        let result: GnaniTTSResponse = response
            .json()
            .await
            .map_err(|e| TTSError::ProviderError(format!("Failed to parse response: {}", e)))?;

        // Decode base64 audio
        let audio_bytes = BASE64
            .decode(&result.audio_content)
            .map_err(|e| TTSError::AudioGenerationFailed(format!("Base64 decode error: {}", e)))?;

        debug!(
            audio_bytes = audio_bytes.len(),
            "Gnani TTS synthesis complete"
        );

        Ok(audio_bytes)
    }
}

#[async_trait]
impl BaseTTS for GnaniTTS {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        GnaniTTS::create(config)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| TTSError::InvalidConfiguration("No configuration set".to_string()))?;

        // Validate configuration
        config.validate().map_err(TTSError::InvalidConfiguration)?;

        info!(
            language = %config.language_code.as_str(),
            gender = %config.ssml_gender.as_str(),
            "Connecting to Gnani TTS"
        );

        // Build HTTP client
        self.http_client = Some(Self::build_client(config)?);

        self.is_connected.store(true, Ordering::Release);
        self.connection_state = ConnectionState::Connected;

        info!("Connected to Gnani TTS");
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        info!("Disconnecting from Gnani TTS");

        self.http_client = None;
        self.is_connected.store(false, Ordering::Release);
        self.connection_state = ConnectionState::Disconnected;

        info!("Disconnected from Gnani TTS");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_connected.load(Ordering::Acquire)
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.connection_state.clone()
    }

    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        if !self.is_ready() {
            self.connect().await?;
        }

        if text.is_empty() {
            return Ok(());
        }

        // Synthesize audio
        let audio_bytes = self.synthesize(text).await?;

        // Get sample rate from config
        let sample_rate = self
            .config
            .as_ref()
            .map(|c| c.output_sample_rate)
            .unwrap_or(8000);

        // Invoke callback with audio data
        if let Some(callback) = self.audio_callback.read().await.as_ref() {
            let audio_data = AudioData {
                data: audio_bytes,
                sample_rate,
                format: "pcm16".to_string(),
                duration_ms: None,
            };
            callback.on_audio(audio_data).await;

            if flush {
                callback.on_complete().await;
            }
        }

        Ok(())
    }

    async fn clear(&mut self) -> TTSResult<()> {
        // No queuing in HTTP-based TTS, nothing to clear
        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        // HTTP-based TTS sends immediately, nothing to flush
        if let Some(callback) = self.audio_callback.read().await.as_ref() {
            callback.on_complete().await;
        }
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        let audio_callback = self.audio_callback.clone();
        tokio::spawn(async move {
            *audio_callback.write().await = Some(callback);
        });
        Ok(())
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        let audio_callback = self.audio_callback.clone();
        tokio::spawn(async move {
            *audio_callback.write().await = None;
        });
        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "gnani",
            "name": "Gnani.ai TTS",
            "description": "Multi-speaker Indic TTS with 12 language support",
            "api_type": "HTTP REST",
            "endpoint": "https://asr.gnani.ai/synthesize",
            "features": ["multi-speaker", "ssml-gender", "indic-languages"],
            "supported_languages": [
                "En-IN", "Hi-IN", "Kn-IN", "Ta-IN", "Te-IN", "Mr-IN",
                "Ml-IN", "Gu-IN", "Bn-IN", "Pa-IN", "Ne-NP"
            ]
        })
    }
}

/// Gnani TTS request structure
#[derive(Debug, Clone, serde::Serialize)]
struct GnaniTTSRequest {
    input: GnaniInput,
    voice: GnaniVoice,
    #[serde(rename = "audioConfig")]
    audio_config: GnaniAudioConfig,
}

/// Input text for TTS
#[derive(Debug, Clone, serde::Serialize)]
struct GnaniInput {
    text: String,
}

/// Voice configuration
#[derive(Debug, Clone, serde::Serialize)]
struct GnaniVoice {
    #[serde(rename = "languageCode")]
    language_code: String,
    name: String,
    #[serde(rename = "ssmlGender")]
    ssml_gender: String,
}

/// Audio output configuration
#[derive(Debug, Clone, serde::Serialize)]
struct GnaniAudioConfig {
    #[serde(rename = "audioEncoding")]
    audio_encoding: String,
    #[serde(rename = "sampleRate")]
    sample_rate: u32,
}

/// Gnani TTS response structure
#[derive(Debug, Clone, serde::Deserialize)]
struct GnaniTTSResponse {
    #[serde(rename = "audioContent")]
    audio_content: String, // Base64 encoded PCM16 audio
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            provider: "gnani".to_string(),
            api_key: String::new(),
            voice_id: Some("Hi-IN".to_string()),
            model: "default".to_string(),
            speaking_rate: Some(1.0),
            audio_format: Some("pcm16".to_string()),
            sample_rate: Some(8000),
            connection_timeout: Some(10),
            request_timeout: Some(30),
            pronunciations: Vec::new(),
            request_pool_size: None,
            emotion_config: None,
        }
    }

    #[test]
    fn test_gnani_tts_creation() {
        let config = create_test_config();
        let result = GnaniTTS::create(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_gnani_tts_not_connected_initially() {
        let config = create_test_config();
        let tts = GnaniTTS::create(config).unwrap();
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_gnani_tts_provider_info() {
        let config = create_test_config();
        let tts = GnaniTTS::create(config).unwrap();
        let info = tts.get_provider_info();
        assert_eq!(info["provider"], "gnani");
        assert!(info["features"].as_array().unwrap().len() > 0);
    }

    #[test]
    fn test_gnani_tts_request_serialization() {
        let request = GnaniTTSRequest {
            input: GnaniInput {
                text: "नमस्ते".to_string(),
            },
            voice: GnaniVoice {
                language_code: "Hi-IN".to_string(),
                name: "gnani".to_string(),
                ssml_gender: "FEMALE".to_string(),
            },
            audio_config: GnaniAudioConfig {
                audio_encoding: "pcm16".to_string(),
                sample_rate: 8000,
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("नमस्ते"));
        assert!(json.contains("Hi-IN"));
        assert!(json.contains("FEMALE"));
        assert!(json.contains("8000"));
    }

    #[tokio::test]
    async fn test_gnani_tts_speak_requires_connection() {
        let config = create_test_config();
        let mut tts = GnaniTTS::create(config).unwrap();

        // speak() auto-connects, but will fail without credentials
        let result = tts.speak("test", false).await;
        // Should fail due to missing credentials
        assert!(result.is_err());
    }
}
