//! Resemble AI TTS Provider Implementation
//!
//! This module provides the core TTS implementation for Resemble AI,
//! using HTTP streaming for real-time audio synthesis.

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use tracing::{debug, info};

use super::RESEMBLE_TTS_STREAM_URL;
use super::config::{ResembleStreamRequest, ResembleTtsConfig, ResembleVoicesResponse};
use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};

fn resemble_tts_http_client() -> Result<reqwest::Client, reqwest::Error> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES).build()
}

// =============================================================================
// Request Builder
// =============================================================================

/// Request builder for Resemble AI TTS API
///
/// Implements the `TTSRequestBuilder` trait to create HTTP requests
/// with the correct URL, headers, and body for Resemble's streaming API.
#[derive(Clone)]
pub struct ResembleRequestBuilder {
    /// Resemble-specific configuration
    config: ResembleTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Precompiled pronunciation replacer
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl ResembleRequestBuilder {
    /// Create a new request builder from configurations
    pub fn new(resemble_config: ResembleTtsConfig, base_config: TTSConfig) -> Self {
        // Build pronunciation replacer from base config
        let pronunciation_replacer = if !base_config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&base_config.pronunciations))
        } else {
            None
        };

        Self {
            config: resemble_config,
            base_config,
            pronunciation_replacer,
        }
    }

    /// Get the Resemble configuration
    pub fn resemble_config(&self) -> &ResembleTtsConfig {
        &self.config
    }
}

impl TTSRequestBuilder for ResembleRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build request body
        let request_body = ResembleStreamRequest::from_config(&self.config, text);

        // Resemble uses Bearer token authentication.
        let auth_value = format!("Bearer {}", self.config.api_key);

        debug!(
            "Resemble AI TTS request: voice_uuid={}, model={}, text_len={}",
            request_body.voice_uuid,
            self.config.model,
            text.len()
        );

        // Build and return the request
        client
            .post(crate::core::tts::standard::override_rest_endpoint(
                RESEMBLE_TTS_STREAM_URL,
                self.config.endpoint_override.as_deref(),
            ))
            .header(AUTHORIZATION, auth_value)
            .header(CONTENT_TYPE, "application/json")
            .json(&request_body)
    }

    fn get_config(&self) -> &TTSConfig {
        &self.base_config
    }

    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        self.pronunciation_replacer.as_ref()
    }
}

// =============================================================================
// Resemble TTS Provider
// =============================================================================

/// Resemble AI TTS Provider
///
/// Provides real-time text-to-speech synthesis using Resemble AI's streaming API.
///
/// # Features
///
/// - **Multiple Models**: Chatterbox, Chatterbox Turbo (low-latency), Chatterbox Multilingual
/// - **149+ Languages**: Extensive language support with multilingual model
/// - **Voice Cloning**: Clone voices with just 10 seconds of audio
/// - **HD Mode**: Higher quality synthesis option
/// - **Paralinguistic Tags**: [cough], [laugh], [chuckle] with Turbo model
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::resemble::ResembleTts;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("voice-uuid".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = ResembleTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("Hello, world!", true).await?;
/// ```
pub struct ResembleTts {
    /// Generic HTTP TTS provider (handles connection pooling, queuing, callbacks)
    provider: TTSProvider,
    /// Resemble-specific configuration
    resemble_config: ResembleTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
}

impl ResembleTts {
    /// Create request builder for this provider
    fn create_request_builder(&self) -> ResembleRequestBuilder {
        ResembleRequestBuilder::new(self.resemble_config.clone(), self.base_config.clone())
    }

    /// List available voices from Resemble AI API
    ///
    /// # Arguments
    /// * `api_key` - The API key for authentication
    /// * `page` - Page number (1-based)
    /// * `page_size` - Results per page (10-1000)
    ///
    /// # Returns
    /// * `TTSResult<ResembleVoicesResponse>` - Paginated list of available voices
    pub async fn list_voices(
        api_key: &str,
        page: u32,
        page_size: u32,
    ) -> TTSResult<ResembleVoicesResponse> {
        let client = resemble_tts_http_client()
            .map_err(|e| TTSError::NetworkError(format!("Failed to build HTTP client: {e}")))?;

        let url = format!(
            "{}?page={}&page_size={}",
            super::RESEMBLE_VOICES_URL,
            page,
            page_size
        );

        let response = client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", api_key))
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to fetch voices: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(TTSError::ProviderError(format!(
                "Failed to list voices ({}): {}",
                status, error_body
            )));
        }

        let voices_response: ResembleVoicesResponse = response.json().await.map_err(|e| {
            TTSError::InternalError(format!("Failed to parse voices response: {}", e))
        })?;

        Ok(voices_response)
    }

    /// Get the Resemble-specific configuration
    pub fn resemble_config(&self) -> &ResembleTtsConfig {
        &self.resemble_config
    }

    /// Builds the provider from the standardized config (W1 keystone for TTS — uniform entry
    /// point, mirroring `DeepgramTTS::from_standard`).
    ///
    /// Delegates the feature mapping to [`ResembleTtsConfig::from_standard`] (sample_rate → the
    /// real field; the non-standard `project_uuid`/`use_hd` knobs flow through the `extras`
    /// passthrough). Voice-tone features (stability, style, emotion, instructions, SSML,
    /// speed/pitch/volume, word_timestamps, streaming, seed) have no Resemble field and are skipped.
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let resemble_config = ResembleTtsConfig::from_standard(std)?;

        info!(
            "Creating Resemble AI TTS provider: voice_uuid={}, model={}",
            resemble_config.voice_uuid, resemble_config.model
        );

        Ok(Self {
            provider: TTSProvider::new(),
            resemble_config,
            base_config: std.base.clone(),
        })
    }
}

#[async_trait]
impl BaseTTS for ResembleTts {
    /// Create a new Resemble AI TTS provider
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        // Parse Resemble-specific configuration from base config
        let resemble_config = ResembleTtsConfig::from_base(&config)?;

        info!(
            "Creating Resemble AI TTS provider: voice_uuid={}, model={}",
            resemble_config.voice_uuid, resemble_config.model
        );

        Ok(Self {
            provider: TTSProvider::new(),
            resemble_config,
            base_config: config,
        })
    }

    /// Get the underlying TTSProvider for HTTP-based providers
    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    /// Connect to the TTS provider
    ///
    /// Initializes the HTTP connection pool and prepares for synthesis.
    async fn connect(&mut self) -> TTSResult<()> {
        debug!("Connecting Resemble AI TTS provider");
        self.provider
            .generic_connect_with_config(
                &crate::core::tts::standard::override_rest_endpoint(
                    RESEMBLE_TTS_STREAM_URL,
                    self.resemble_config.endpoint_override.as_deref(),
                ),
                &self.base_config,
            )
            .await
    }

    /// Synthesize text to speech
    ///
    /// # Arguments
    /// * `text` - The text to synthesize (max 2000 characters for streaming)
    /// * `flush` - Whether to interrupt current playback
    ///
    /// # Returns
    /// * `TTSResult<()>` - Success or error
    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        if text.is_empty() || text.trim().is_empty() {
            return Ok(());
        }

        // Validate text length
        ResembleTtsConfig::validate_text(text)?;

        debug!(
            "Resemble AI TTS speak: text='{}', flush={}, voice_uuid={}, model={}",
            text, flush, self.resemble_config.voice_uuid, self.resemble_config.model
        );

        let request_builder = self.create_request_builder();
        self.provider
            .generic_speak(request_builder, text, flush)
            .await
    }

    /// Flush any pending synthesis
    async fn flush(&self) -> TTSResult<()> {
        self.provider.generic_flush().await
    }

    /// Check if the provider is ready for synthesis
    fn is_ready(&self) -> bool {
        self.provider.is_ready()
    }

    /// Get the current connection state
    fn get_connection_state(&self) -> ConnectionState {
        self.provider.get_connection_state()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::Pronunciation;
    use crate::core::tts::resemble::config::ResembleModel;
    use std::io::ErrorKind;

    // W1 keystone (TTS): the standardized `sample_rate` override reaches the built provider's
    // Resemble config through the struct-level `from_standard`, and the non-standard
    // `project_uuid`/`use_hd` knobs flow through the extras passthrough.
    #[test]
    fn from_standard_maps_features_to_provider() {
        use crate::core::stt::standard::ProviderExtras;
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("project_uuid".into(), serde_json::json!("proj-123"));
        extras.insert("use_hd".into(), serde_json::json!(true));
        let std = StandardTTSConfig {
            base: TTSConfig {
                api_key: "test-key".to_string(),
                voice_id: Some("voice-uuid".to_string()),
                ..Default::default()
            },
            features: TtsFeatures {
                sample_rate: Some(44100),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let tts = ResembleTts::from_standard(&std).unwrap();
        let cfg = tts.resemble_config();
        assert_eq!(cfg.api_key, "test-key");
        assert_eq!(cfg.voice_uuid, "voice-uuid");
        assert_eq!(cfg.sample_rate, 44100);
        assert_eq!(cfg.project_uuid, Some("proj-123".to_string()));
        assert!(cfg.use_hd);
    }

    #[test]
    fn invalid_resemble_api_key_header_value_is_request_build_error() {
        let config = TTSConfig {
            api_key: "bad\nkey".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };
        let resemble_config = ResembleTtsConfig::from_base(&config).unwrap();
        let builder = ResembleRequestBuilder::new(resemble_config, config);
        let err = builder
            .build_http_request(&reqwest::Client::new(), "hello")
            .build()
            .expect_err("malformed Resemble API key must not become an empty Authorization header");

        assert!(err.is_builder(), "unexpected reqwest error: {err}");
    }

    // WIRE-LEVEL (S1/S5 recurring bug class): assert the `apply_custom_pronunciations` flag set via
    // the standardized extras passthrough actually reaches the SERIALIZED `/stream` request BODY —
    // not merely the config struct. We build the real reqwest request through the same path the
    // live provider uses and inspect its body bytes.
    #[tokio::test]
    async fn apply_custom_pronunciations_reaches_stream_request_body() {
        use crate::core::stt::standard::ProviderExtras;
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};

        let mut extras = serde_json::Map::new();
        extras.insert(
            "apply_custom_pronunciations".into(),
            serde_json::json!(true),
        );
        let std = StandardTTSConfig {
            base: TTSConfig {
                api_key: "test-key".to_string(),
                voice_id: Some("voice-uuid".to_string()),
                ..Default::default()
            },
            features: TtsFeatures::default(),
            extras: ProviderExtras(extras),
        };
        let tts = ResembleTts::from_standard(&std).unwrap();
        // (1) config carries the flag …
        assert!(tts.resemble_config().apply_custom_pronunciations);

        // (2) … AND the built HTTP request body contains the api_param.
        let builder = tts.create_request_builder();
        let client = reqwest::Client::new();
        let request = builder
            .build_http_request(&client, "Hello world")
            .build()
            .unwrap();
        assert_eq!(request.url().as_str(), RESEMBLE_TTS_STREAM_URL);
        let body_bytes = request.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        assert_eq!(
            body["apply_custom_pronunciations"], true,
            "apply_custom_pronunciations must reach the /stream request body"
        );
        assert_eq!(body["voice_uuid"], "voice-uuid");
        assert_eq!(body["data"], "Hello world");
    }

    // Default-off guard: when the flag is not requested, the body keeps its pre-feature shape
    // (the key is omitted, not emitted as `false`).
    #[test]
    fn apply_custom_pronunciations_omitted_from_body_by_default() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };
        let resemble_config = ResembleTtsConfig::from_base(&config).unwrap();
        let request = ResembleStreamRequest::from_config(&resemble_config, "Hi");
        let json = serde_json::to_string(&request).unwrap();
        assert!(!json.contains("apply_custom_pronunciations"));
    }

    #[tokio::test]
    async fn resemble_tts_redirect_policy_rejects_private_hop() {
        let _env = crate::core::net::ssrf_env_lock();
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(err) => {
                if err.kind() == ErrorKind::PermissionDenied {
                    eprintln!("Skipping resemble_tts_redirect_policy_rejects_private_hop: {err}");
                    return;
                }
                panic!("Failed to bind redirect test server listener: {err}");
            }
        };
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: http://127.0.0.1:9/metadata\r\n",
                "Content-Length: 0\r\n",
                "\r\n"
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });

        let err = resemble_tts_http_client()
            .unwrap()
            .get(format!("http://{addr}/start"))
            .send()
            .await
            .expect_err("private redirect target must be rejected");
        let mut error_chain = err.to_string();
        let mut source = std::error::Error::source(&err);
        while let Some(error) = source {
            error_chain.push_str(": ");
            error_chain.push_str(&error.to_string());
            source = error.source();
        }
        assert!(
            error_chain.contains("redirect URL rejected"),
            "unexpected redirect error: {error_chain}"
        );
    }

    #[test]
    fn test_resemble_tts_creation() {
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("test-voice-uuid".to_string()),
            ..Default::default()
        };

        let tts = ResembleTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert_eq!(tts.resemble_config().voice_uuid, "test-voice-uuid");
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_resemble_tts_requires_api_key() {
        let config = TTSConfig {
            voice_id: Some("test-voice-uuid".to_string()),
            ..Default::default()
        };
        let result = ResembleTts::new(config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(
                msg.contains("API") || msg.contains("api_key") || msg.contains("RESEMBLE_API_KEY")
            );
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_resemble_tts_requires_voice_uuid() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: None,
            ..Default::default()
        };
        let result = ResembleTts::new(config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("voice_uuid") || msg.contains("voice_id"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_request_builder_headers() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };

        let resemble_config = ResembleTtsConfig::from_base(&config).unwrap();
        let builder = ResembleRequestBuilder::new(resemble_config, config);

        // Verify config
        assert_eq!(builder.resemble_config().voice_uuid, "voice-uuid");
        assert_eq!(builder.resemble_config().model, ResembleModel::Chatterbox);
    }

    #[test]
    fn test_request_builder_with_turbo() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            model: "chatterbox-turbo".to_string(),
            ..Default::default()
        };

        let resemble_config = ResembleTtsConfig::from_base(&config).unwrap();
        let builder = ResembleRequestBuilder::new(resemble_config, config);

        assert_eq!(
            builder.resemble_config().model,
            ResembleModel::ChatterboxTurbo
        );
    }

    #[test]
    fn test_stream_request_serialization() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };

        let resemble_config = ResembleTtsConfig::from_base(&config).unwrap();
        let request = ResembleStreamRequest::from_config(&resemble_config, "Hello world");

        let json = serde_json::to_string(&request).unwrap();

        // Verify JSON structure
        assert!(json.contains("\"data\":\"Hello world\""));
        assert!(json.contains("\"voice_uuid\":\"voice-uuid\""));
        // Default model should not include model field
        assert!(!json.contains("\"model\""));
    }

    #[test]
    fn test_stream_request_turbo_serialization() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            model: "chatterbox-turbo".to_string(),
            ..Default::default()
        };

        let resemble_config = ResembleTtsConfig::from_base(&config).unwrap();
        let request = ResembleStreamRequest::from_config(&resemble_config, "Hello world");

        let json = serde_json::to_string(&request).unwrap();

        // Turbo model should be included
        assert!(json.contains("\"model\":\"chatterbox-turbo\""));
    }

    #[test]
    fn test_pronunciation_replacer() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            pronunciations: vec![
                Pronunciation {
                    word: "API".to_string(),
                    pronunciation: "A P I".to_string(),
                },
                Pronunciation {
                    word: "Resemble".to_string(),
                    pronunciation: "Re-zemble".to_string(),
                },
            ],
            ..Default::default()
        };

        let resemble_config = ResembleTtsConfig::from_base(&config).unwrap();
        let builder = ResembleRequestBuilder::new(resemble_config, config);

        assert!(builder.get_pronunciation_replacer().is_some());
    }

    #[tokio::test]
    async fn test_resemble_tts_connect_disconnect() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };

        let mut tts = ResembleTts::new(config).unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);

        // Connect
        let result = tts.connect().await;
        assert!(result.is_ok());
        assert!(tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Connected);

        // Disconnect
        let result = tts.disconnect().await;
        assert!(result.is_ok());
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_resemble_tts_speak_not_connected() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };

        let mut tts = ResembleTts::new(config).unwrap();

        // Should fail when not connected
        let result = tts.speak("Hello", true).await;
        assert!(result.is_err());

        if let Err(TTSError::ProviderNotReady(_)) = result {
            // Expected
        } else {
            panic!("Expected ProviderNotReady error");
        }
    }

    #[tokio::test]
    async fn test_resemble_tts_empty_text() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };

        let mut tts = ResembleTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Empty text should succeed without making a request
        let result = tts.speak("", true).await;
        assert!(result.is_ok());

        let result = tts.speak("   ", true).await;
        assert!(result.is_ok());

        tts.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn test_resemble_tts_text_too_long() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };

        let mut tts = ResembleTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Text over 2000 characters should fail
        let long_text = "a".repeat(2001);
        let result = tts.speak(&long_text, true).await;
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("2000"));
        } else {
            panic!("Expected InvalidConfiguration error about text length");
        }

        tts.disconnect().await.unwrap();
    }
}
