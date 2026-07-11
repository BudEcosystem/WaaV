//! CereProc CereVoice Cloud TTS Provider Implementation
//!
//! This module provides the core TTS implementation for CereVoice Cloud API v2,
//! using HTTP requests for audio synthesis with file URL download.

use async_trait::async_trait;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::config::CereprocTtsConfig;
use super::messages::{AuthResponse, CereprocApiError, SpeakResponse};
use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};

fn cereproc_tts_http_client() -> Result<reqwest::Client, reqwest::Error> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES).build()
}

// =============================================================================
// Token Cache
// =============================================================================

/// Cached authentication token with expiration tracking
#[derive(Debug)]
struct TokenCache {
    /// The Bearer token
    token: String,
    /// When the token was obtained
    obtained_at: Instant,
}

impl TokenCache {
    /// Create a new token cache entry
    fn new(token: String) -> Self {
        Self {
            token,
            obtained_at: Instant::now(),
        }
    }

    /// Check if the token is stale (older than TTL)
    fn is_stale(&self) -> bool {
        self.obtained_at.elapsed() > Duration::from_secs(super::TOKEN_CACHE_TTL_SECS)
    }
}

// =============================================================================
// Request Builder
// =============================================================================

/// Request builder for CereProc TTS API
#[derive(Clone)]
pub struct CereprocRequestBuilder {
    /// CereProc-specific configuration
    cereproc_config: CereprocTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Bearer token for authentication
    token: String,
    /// Pronunciation replacer (precompiled regex patterns)
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl CereprocRequestBuilder {
    /// Create a new request builder
    pub fn new(cereproc_config: CereprocTtsConfig, base_config: TTSConfig, token: String) -> Self {
        // Build pronunciation replacer from base config if pronunciations are defined
        let pronunciation_replacer = if base_config.pronunciations.is_empty() {
            None
        } else {
            Some(PronunciationReplacer::new(&base_config.pronunciations))
        };

        Self {
            cereproc_config,
            base_config,
            token,
            pronunciation_replacer,
        }
    }

    /// Get the CereProc configuration
    pub fn cereproc_config(&self) -> &CereprocTtsConfig {
        &self.cereproc_config
    }
}

impl TTSRequestBuilder for CereprocRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build query parameters
        let query_params: Vec<(String, String)> = self
            .cereproc_config
            .build_query_params()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();

        // Build request body (XML with optional emotion wrapper)
        let body = self.cereproc_config.wrap_with_emotion(text);

        debug!(
            "CereProc TTS request: voice={}, format={}, text_len={}",
            self.cereproc_config.voice_id,
            self.cereproc_config.audio_format,
            text.len()
        );

        // Build and return the request
        client
            .post(crate::core::tts::standard::override_rest_endpoint(
                super::CEREVOICE_SPEAK_URL,
                self.cereproc_config.endpoint_override.as_deref(),
            ))
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(CONTENT_TYPE, "text/xml")
            .header(ACCEPT, "application/json")
            .query(&query_params)
            .body(body)
    }

    fn get_config(&self) -> &TTSConfig {
        &self.base_config
    }

    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        self.pronunciation_replacer.as_ref()
    }

    fn expects_provider_audio_url_response(&self) -> bool {
        true
    }

    fn provider_audio_url_label(&self) -> &'static str {
        "CereProc TTS"
    }

    fn parse_provider_audio_url_response(&self, body: &[u8]) -> TTSResult<Option<String>> {
        let response: SpeakResponse = serde_json::from_slice(body).map_err(|e| {
            TTSError::ProviderError(format!("Failed to parse CereProc TTS response: {e}"))
        })?;

        if !response.is_success() {
            return Err(TTSError::ProviderError(format!(
                "CereProc synthesis failed: {}",
                response.status_message()
            )));
        }

        if response.file_url.trim().is_empty() {
            return Err(TTSError::ProviderError(
                "CereProc synthesis succeeded without fileUrl".to_string(),
            ));
        }

        Ok(Some(response.file_url))
    }
}

// =============================================================================
// CereProc TTS Provider
// =============================================================================

/// CereProc CereVoice Cloud TTS provider
///
/// Implements text-to-speech synthesis using the CereVoice Cloud API v2.
/// Uses HTTP requests with Bearer token authentication and XML/SSML body.
pub struct CereprocTts {
    /// Generic TTS provider for HTTP connection management
    provider: TTSProvider,
    /// CereProc-specific configuration
    cereproc_config: CereprocTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Cached authentication token
    token_cache: Arc<RwLock<Option<TokenCache>>>,
    /// HTTP client for authentication requests
    auth_client: reqwest::Client,
}

impl CereprocTts {
    /// Create a new CereProc TTS provider from TTSConfig
    pub fn create(config: TTSConfig) -> TTSResult<Self> {
        let cereproc_config = CereprocTtsConfig::from_base(&config)?;
        let auth_client = cereproc_tts_http_client()
            .map_err(|e| TTSError::NetworkError(format!("Failed to build HTTP client: {e}")))?;

        info!(
            "Creating CereProc TTS provider: voice={}, format={}, sample_rate={}",
            cereproc_config.voice_id, cereproc_config.audio_format, cereproc_config.sample_rate
        );

        Ok(Self {
            provider: TTSProvider::new(),
            cereproc_config,
            base_config: config,
            token_cache: Arc::new(RwLock::new(None)),
            auth_client,
        })
    }

    /// Build from the standardized config (W1 keystone). Mirrors [`DeepgramTTS::from_standard`]:
    /// the provider struct (not just the config) is the dispatch entry point. CereProc maps
    /// `emotion` → its custom `<emotion>` SSML tags and `sample_rate` → the output sample rate via
    /// [`CereprocTtsConfig::from_standard`]; the feature-aware CereProc config is what the request
    /// builder (`wrap_with_emotion` / `build_query_params`) reads. Features CereProc can't express
    /// are skipped (capability gaps).
    ///
    /// [`DeepgramTTS::from_standard`]: crate::core::tts::deepgram::DeepgramTTS::from_standard
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let cereproc_config = CereprocTtsConfig::from_standard(std)?;
        let auth_client = cereproc_tts_http_client()
            .map_err(|e| TTSError::NetworkError(format!("Failed to build HTTP client: {e}")))?;

        info!(
            "Creating CereProc TTS provider (standardized): voice={}, format={}, sample_rate={}",
            cereproc_config.voice_id, cereproc_config.audio_format, cereproc_config.sample_rate
        );

        Ok(Self {
            provider: TTSProvider::new(),
            cereproc_config,
            base_config: std.base.clone(),
            token_cache: Arc::new(RwLock::new(None)),
            auth_client,
        })
    }

    // -------------------------------------------------------------------------
    // Authentication Methods
    // -------------------------------------------------------------------------

    /// Perform login to obtain Bearer token
    async fn login(&self) -> TTSResult<String> {
        debug!("Logging in to CereVoice Cloud API");

        let response = self
            .auth_client
            .post(crate::core::tts::standard::override_rest_endpoint(
                super::CEREVOICE_AUTH_URL,
                self.cereproc_config.endpoint_override.as_deref(),
            ))
            .header(
                AUTHORIZATION,
                self.cereproc_config.credentials.basic_auth_value(),
            )
            .send()
            .await
            .map_err(|e| {
                TTSError::ConnectionFailed(format!("Failed to connect to CereVoice auth: {}", e))
            })?;

        if response.status().is_success() {
            let auth_response: AuthResponse = response.json().await.map_err(|e| {
                TTSError::ConnectionFailed(format!("Failed to parse auth response: {}", e))
            })?;

            debug!("CereVoice login successful, obtained token");
            Ok(auth_response.token)
        } else {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();

            // Try to parse as API error
            if let Ok(api_error) = serde_json::from_str::<CereprocApiError>(&error_text) {
                Err(TTSError::AuthenticationFailed(api_error.display_message()))
            } else {
                Err(TTSError::AuthenticationFailed(format!(
                    "Login failed with status {}: {}",
                    status, error_text
                )))
            }
        }
    }

    /// Get a valid token (from cache or by logging in)
    async fn get_token(&self) -> TTSResult<String> {
        // Check cache first
        {
            let cache = self.token_cache.read().await;
            if let Some(ref cached) = *cache {
                if !cached.is_stale() {
                    return Ok(cached.token.clone());
                }
                debug!("Token is stale, refreshing");
            }
        }

        // Need to login for fresh token
        let token = self.login().await?;

        // Update cache
        {
            let mut cache = self.token_cache.write().await;
            *cache = Some(TokenCache::new(token.clone()));
        }

        Ok(token)
    }

    /// Invalidate the cached token (force re-login on next request)
    async fn invalidate_token(&self) {
        let mut cache = self.token_cache.write().await;
        *cache = None;
        debug!("Token cache invalidated");
    }

    /// Create a request builder with current token
    async fn create_request_builder(&self) -> TTSResult<CereprocRequestBuilder> {
        let token = self.get_token().await?;
        Ok(CereprocRequestBuilder::new(
            self.cereproc_config.clone(),
            self.base_config.clone(),
            token,
        ))
    }

    /// Check available credits
    pub async fn check_credits(&self) -> TTSResult<u64> {
        let token = self.get_token().await?;

        let response = self
            .auth_client
            .get(super::CEREVOICE_CREDIT_URL)
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| TTSError::ConnectionFailed(format!("Failed to check credits: {}", e)))?;

        if response.status().is_success() {
            let credit: super::messages::CreditResponse = response.json().await.map_err(|e| {
                TTSError::ConnectionFailed(format!("Failed to parse credit response: {}", e))
            })?;
            Ok(credit.total_available())
        } else {
            Err(TTSError::ConnectionFailed(
                "Failed to retrieve credit balance".to_string(),
            ))
        }
    }
}

// =============================================================================
// BaseTTS Implementation
// =============================================================================

#[async_trait]
impl BaseTTS for CereprocTts {
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        Self::create(config)
    }

    /// Expose the inner generic provider so callback registration (e.g. `on_audio`) reaches the
    /// `TTSProvider` that actually emits synthesized audio via `generic_speak`.
    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    /// Connect to the CereVoice Cloud API
    async fn connect(&mut self) -> TTSResult<()> {
        debug!("Connecting to CereVoice Cloud API");

        // Perform login to verify credentials and get token
        let token = self.login().await?;
        {
            let mut cache = self.token_cache.write().await;
            *cache = Some(TokenCache::new(token));
        }

        // Initialize HTTP connection pool
        self.provider
            .generic_connect_with_config(
                &crate::core::tts::standard::override_rest_endpoint(
                    super::CEREVOICE_SPEAK_URL,
                    self.cereproc_config.endpoint_override.as_deref(),
                ),
                &self.base_config,
            )
            .await
    }

    /// Disconnect from the CereVoice Cloud API
    async fn disconnect(&mut self) -> TTSResult<()> {
        debug!("Disconnecting from CereVoice Cloud API");

        // Invalidate token cache
        self.invalidate_token().await;

        // Disconnect HTTP provider
        self.provider.generic_disconnect().await
    }

    /// Synthesize text to speech
    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        if text.is_empty() || text.trim().is_empty() {
            return Ok(());
        }

        // Check if provider is ready (connected)
        if !self.is_ready() {
            return Err(TTSError::ProviderNotReady(
                "Provider not connected. Call connect() first.".to_string(),
            ));
        }

        // Validate text length
        self.cereproc_config.validate_text(text)?;

        debug!(
            "CereProc TTS speak: text='{}', flush={}, voice={}, format={}",
            text, flush, self.cereproc_config.voice_id, self.cereproc_config.audio_format
        );

        // Create request builder (ensures we have a valid token)
        let request_builder = match self.create_request_builder().await {
            Ok(builder) => builder,
            Err(e) => {
                // If token is invalid, try to refresh
                if matches!(e, TTSError::AuthenticationFailed(_)) {
                    self.invalidate_token().await;
                    self.create_request_builder().await?
                } else {
                    return Err(e);
                }
            }
        };

        // Execute request through generic provider
        match self
            .provider
            .generic_speak(request_builder, text, flush)
            .await
        {
            Ok(()) => Ok(()),
            Err(TTSError::AuthenticationFailed(_)) => {
                // Token might have expired, retry with fresh token
                warn!("Authentication failed, retrying with fresh token");
                self.invalidate_token().await;
                let request_builder = self.create_request_builder().await?;
                self.provider
                    .generic_speak(request_builder, text, flush)
                    .await
            }
            Err(e) => Err(e),
        }
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
    use crate::core::tts::cereproc::CereprocAudioFormat;
    use std::io::ErrorKind;

    // The provider STRUCT's `from_standard` (the dispatch entry point) carries CereProc's
    // expressible advanced features — emotion (via its custom <emotion> SSML tags) and the output
    // sample_rate — onto the stored CereProc config the request builder reads.
    #[test]
    fn from_standard_carries_emotion_and_sample_rate_to_provider() {
        use crate::core::tts::cereproc::config::CereprocEmotion;
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "cereproc".into(),
                api_key: "user@example.com:password123".into(),
                voice_id: Some("Stuart".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                emotion: Some("happy".into()),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = CereprocTts::from_standard(&std).unwrap();
        assert_eq!(tts.cereproc_config.emotion, Some(CereprocEmotion::Happy));
        assert_eq!(tts.cereproc_config.sample_rate, 16000);
    }

    // WIRE-LEVEL capability-gap tripwire: `features.language`, `features.streaming`, and
    // `extras["accent"]` have no parameter on the v2 `/speak` endpoint (language/accent are
    // determined by the voice; `/speak` returns a fileUrl, not a stream). This test asserts that
    // none of them leak onto the wire — neither into the query string nor the XML body — so a
    // future change that fabricates them as `/speak` params fails here. Confirmed against the
    // published CereVoice Cloud v2 client surface: `/speak` accepts only
    // voice/audioFormat/sampleRate/audio3D/metadata.
    #[test]
    fn from_standard_language_streaming_accent_are_capability_gaps_off_the_wire() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("accent".into(), serde_json::json!("us"));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "cereproc".into(),
                api_key: "user@example.com:password123".into(),
                voice_id: Some("Stuart".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                language: Some("fr-FR".into()),
                streaming: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let tts = CereprocTts::from_standard(&std).unwrap();

        // Build a real request with a known token and inspect URL query + XML body.
        let builder = CereprocRequestBuilder::new(
            tts.cereproc_config.clone(),
            tts.base_config.clone(),
            "tok".into(),
        );
        let client = reqwest::Client::new();
        let request = builder
            .build_http_request(&client, "Bonjour")
            .build()
            .unwrap();

        let query = request.url().query().unwrap_or("");
        // The voice is the only language/accent carrier; no lang/accent/streaming query params.
        assert!(query.contains("voice=Stuart"), "voice missing: {query}");
        assert!(
            !query.contains("lang"),
            "language leaked to /speak query: {query}"
        );
        assert!(
            !query.contains("accent"),
            "accent leaked to /speak query: {query}"
        );
        assert!(
            !query.contains("stream"),
            "streaming leaked to /speak query: {query}"
        );

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_str = std::str::from_utf8(body).unwrap();
        assert!(
            !body_str.contains("lang"),
            "language leaked to body: {body_str}"
        );
        assert!(
            !body_str.contains("accent"),
            "accent leaked to body: {body_str}"
        );
    }

    #[tokio::test]
    async fn cereproc_tts_redirect_policy_rejects_private_hop() {
        let _env = crate::core::net::ssrf_env_lock();
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(err) => {
                if err.kind() == ErrorKind::PermissionDenied {
                    eprintln!("Skipping cereproc_tts_redirect_policy_rejects_private_hop: {err}");
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

        let err = cereproc_tts_http_client()
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
    fn test_cereproc_tts_creation() {
        // With valid credentials
        let config = TTSConfig {
            api_key: "user@example.com:password123".to_string(),
            voice_id: Some("Stuart".to_string()),
            ..Default::default()
        };

        let tts = CereprocTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert_eq!(tts.cereproc_config.voice_id, "Stuart");
        assert_eq!(tts.cereproc_config.audio_format, CereprocAudioFormat::Wav);
    }

    #[test]
    fn test_cereproc_tts_requires_credentials() {
        let config = TTSConfig {
            api_key: String::new(),
            voice_id: Some("Stuart".to_string()),
            ..Default::default()
        };

        let result = CereprocTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_cereproc_tts_invalid_credentials_format() {
        let config = TTSConfig {
            api_key: "invalid-no-colon".to_string(),
            voice_id: Some("Stuart".to_string()),
            ..Default::default()
        };

        let result = CereprocTts::new(config);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cereproc_tts_not_connected() {
        let config = TTSConfig {
            api_key: "user@example.com:password".to_string(),
            voice_id: Some("Stuart".to_string()),
            ..Default::default()
        };

        let mut tts = CereprocTts::new(config).unwrap();

        // Should not be ready initially
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);

        // Should fail when not connected
        let result = tts.speak("Hello", true).await;
        assert!(result.is_err());

        if let Err(TTSError::ProviderNotReady(_)) = result {
            // Expected
        } else {
            panic!("Expected ProviderNotReady error, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_cereproc_tts_empty_text() {
        let config = TTSConfig {
            api_key: "user@example.com:password".to_string(),
            voice_id: Some("Stuart".to_string()),
            ..Default::default()
        };

        let mut tts = CereprocTts::new(config).unwrap();

        // Empty text should succeed without making a request
        let result = tts.speak("", true).await;
        assert!(result.is_ok());

        let result = tts.speak("   ", true).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_request_builder() {
        let config = TTSConfig {
            api_key: "user@example.com:password".to_string(),
            voice_id: Some("Stuart".to_string()),
            ..Default::default()
        };

        let cereproc_config = CereprocTtsConfig::from_base(&config).unwrap();
        let builder =
            CereprocRequestBuilder::new(cereproc_config, config.clone(), "test-token".to_string());

        // Verify get_config works
        assert_eq!(builder.get_config().api_key, config.api_key);
    }

    #[test]
    fn invalid_cereproc_token_header_value_is_request_build_error() {
        let config = TTSConfig {
            api_key: "user@example.com:password".to_string(),
            voice_id: Some("Stuart".to_string()),
            ..Default::default()
        };

        let cereproc_config = CereprocTtsConfig::from_base(&config).unwrap();
        let builder = CereprocRequestBuilder::new(cereproc_config, config, "bad\ntoken".into());
        let err = builder
            .build_http_request(&reqwest::Client::new(), "hello")
            .build()
            .expect_err("malformed CereProc bearer token must not omit Authorization");

        assert!(err.is_builder(), "unexpected reqwest error: {err}");
    }

    #[test]
    fn request_builder_extracts_file_url_response() {
        let config = TTSConfig {
            api_key: "user@example.com:password".to_string(),
            voice_id: Some("Stuart".to_string()),
            ..Default::default()
        };

        let cereproc_config = CereprocTtsConfig::from_base(&config).unwrap();
        let builder = CereprocRequestBuilder::new(cereproc_config, config, "test-token".into());
        let body = br#"{
            "fileUrl": "https://audio.example.com/cereproc.wav",
            "charCount": "11",
            "resultCode": "1",
            "resultDescription": "Success"
        }"#;

        assert!(builder.expects_provider_audio_url_response());
        assert_eq!(
            builder.parse_provider_audio_url_response(body).unwrap(),
            Some("https://audio.example.com/cereproc.wav".to_string())
        );
    }

    #[test]
    fn request_builder_rejects_success_response_without_file_url() {
        let config = TTSConfig {
            api_key: "user@example.com:password".to_string(),
            voice_id: Some("Stuart".to_string()),
            ..Default::default()
        };

        let cereproc_config = CereprocTtsConfig::from_base(&config).unwrap();
        let builder = CereprocRequestBuilder::new(cereproc_config, config, "test-token".into());
        let body = br#"{
            "fileUrl": "   ",
            "charCount": "11",
            "resultCode": "1",
            "resultDescription": "Success"
        }"#;

        let err = builder
            .parse_provider_audio_url_response(body)
            .expect_err("success response without fileUrl must not be treated as audio");
        assert!(err.to_string().contains("without fileUrl"), "{err}");
    }

    #[test]
    fn test_token_cache_staleness() {
        let cache = TokenCache::new("test-token".to_string());

        // Should not be stale immediately
        assert!(!cache.is_stale());
    }

    #[test]
    fn test_config_with_all_audio_formats() {
        for (format_str, expected_format) in [
            ("wav", CereprocAudioFormat::Wav),
            ("mp3", CereprocAudioFormat::Mp3),
            ("ogg", CereprocAudioFormat::Ogg),
            ("raw", CereprocAudioFormat::Raw),
        ] {
            let config = TTSConfig {
                api_key: "user@example.com:password".to_string(),
                audio_format: Some(format_str.to_string()),
                ..Default::default()
            };

            let cereproc_config = CereprocTtsConfig::from_base(&config).unwrap();
            assert_eq!(cereproc_config.audio_format, expected_format);
        }
    }
}
