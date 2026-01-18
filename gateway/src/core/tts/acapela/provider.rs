//! Acapela Cloud TTS Provider Implementation
//!
//! This module provides the core TTS implementation for Acapela Cloud API,
//! using HTTP streaming for real-time audio synthesis.

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::config::{AcapelaAccountInfo, AcapelaTtsConfig, AcapelaVoice};
use super::messages::{AcapelaApiError, LoginRequest, LoginResponse};
use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};

// =============================================================================
// Token Cache
// =============================================================================

/// Cached authentication token
#[derive(Debug, Clone)]
struct TokenCache {
    /// The authentication token
    token: String,
    /// When the token was obtained (for potential refresh logic)
    obtained_at: std::time::Instant,
}

impl TokenCache {
    fn new(token: String) -> Self {
        Self {
            token,
            obtained_at: std::time::Instant::now(),
        }
    }

    /// Check if token might be stale (older than 30 minutes)
    fn might_be_stale(&self) -> bool {
        self.obtained_at.elapsed() > std::time::Duration::from_secs(30 * 60)
    }
}

// =============================================================================
// Request Builder
// =============================================================================

/// Request builder for Acapela Cloud TTS API
///
/// Implements the `TTSRequestBuilder` trait to create HTTP requests
/// with the correct URL, headers, and query parameters for Acapela's streaming API.
#[derive(Clone)]
pub struct AcapelaRequestBuilder {
    /// Acapela-specific configuration
    config: AcapelaTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Precompiled pronunciation replacer
    pronunciation_replacer: Option<PronunciationReplacer>,
    /// Cached authentication token
    token: String,
}

impl AcapelaRequestBuilder {
    /// Create a new request builder from configurations
    pub fn new(acapela_config: AcapelaTtsConfig, base_config: TTSConfig, token: String) -> Self {
        // Build pronunciation replacer from base config
        let pronunciation_replacer = if !base_config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&base_config.pronunciations))
        } else {
            None
        };

        Self {
            config: acapela_config,
            base_config,
            pronunciation_replacer,
            token,
        }
    }

    /// Get the command endpoint URL
    pub fn command_url(&self) -> &str {
        super::ACAPELA_COMMAND_URL
    }

    /// Get the Acapela configuration
    pub fn acapela_config(&self) -> &AcapelaTtsConfig {
        &self.config
    }
}

impl TTSRequestBuilder for AcapelaRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build query parameters
        let params = self.config.build_query_params(text);

        // Build headers
        let mut headers = HeaderMap::new();

        // Acapela uses "Token <token>" format for authorization
        let auth_value = format!("Token {}", self.token);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value).unwrap_or_else(|_| HeaderValue::from_static("")),
        );

        debug!(
            "Acapela TTS request: voice={}, format={}, sample_rate={}, url={}",
            self.config.voice_id,
            self.config.audio_format,
            self.config.sample_rate,
            self.command_url()
        );

        // Build and return the request with query parameters
        client.get(self.command_url()).headers(headers).query(&params)
    }

    fn get_config(&self) -> &TTSConfig {
        &self.base_config
    }

    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        self.pronunciation_replacer.as_ref()
    }
}

// =============================================================================
// Acapela TTS Provider
// =============================================================================

/// Acapela Cloud TTS Provider
///
/// Provides real-time text-to-speech synthesis using Acapela Cloud's streaming API.
///
/// # Features
///
/// - **250+ AI Neural Voices**: Across 30+ languages
/// - **Word Position Events**: Real-time timing for text highlighting
/// - **Viseme Data**: Lip-sync animation support
/// - **Custom Dictionaries**: Upload pronunciation dictionaries
/// - **Multiple Audio Formats**: MP3, WAV, OGG, FLAC, and more
///
/// # Authentication
///
/// Acapela Cloud uses email/password authentication. Pass credentials in the
/// `api_key` field as "email:password" format.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::acapela::AcapelaTts;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "user@example.com:password".to_string(),
///     voice_id: Some("alice".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = AcapelaTts::new(config)?;
/// tts.connect().await?;  // Performs login
/// tts.speak("Hello, world!", true).await?;
/// ```
pub struct AcapelaTts {
    /// Generic HTTP TTS provider (handles connection pooling, queuing, callbacks)
    provider: TTSProvider,
    /// Acapela-specific configuration
    acapela_config: AcapelaTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Cached authentication token
    token_cache: Arc<RwLock<Option<TokenCache>>>,
    /// HTTP client for authentication
    auth_client: reqwest::Client,
}

impl AcapelaTts {
    /// Perform login and get authentication token
    async fn login(&self) -> TTSResult<String> {
        let login_request = LoginRequest {
            email: self.acapela_config.credentials.email.clone(),
            password: self.acapela_config.credentials.password.clone(),
        };

        debug!(
            "Logging in to Acapela Cloud as {}",
            self.acapela_config.credentials.email
        );

        let response = self
            .auth_client
            .post(super::ACAPELA_LOGIN_URL)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&[
                ("email", &login_request.email),
                ("password", &login_request.password),
            ])
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Login request failed: {}", e)))?;

        let status = response.status();

        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();

            // Try to parse as API error
            if let Ok(api_error) = serde_json::from_str::<AcapelaApiError>(&error_body) {
                return Err(match status.as_u16() {
                    401 => TTSError::AuthenticationFailed(api_error.message()),
                    _ => TTSError::ProviderError(format!(
                        "Login failed ({}): {}",
                        status,
                        api_error.message()
                    )),
                });
            }

            return Err(TTSError::ProviderError(format!(
                "Login failed ({}): {}",
                status, error_body
            )));
        }

        let login_response: LoginResponse = response.json().await.map_err(|e| {
            TTSError::InternalError(format!("Failed to parse login response: {}", e))
        })?;

        info!("Successfully logged in to Acapela Cloud");
        Ok(login_response.token)
    }

    /// Get current token, logging in if necessary
    async fn get_token(&self) -> TTSResult<String> {
        // Check cache
        {
            let cache = self.token_cache.read().await;
            if let Some(ref cached) = *cache {
                if !cached.might_be_stale() {
                    return Ok(cached.token.clone());
                }
                debug!("Cached token might be stale, refreshing...");
            }
        }

        // Login and cache new token
        let token = self.login().await?;
        {
            let mut cache = self.token_cache.write().await;
            *cache = Some(TokenCache::new(token.clone()));
        }

        Ok(token)
    }

    /// Invalidate cached token (for retry logic)
    async fn invalidate_token(&self) {
        let mut cache = self.token_cache.write().await;
        *cache = None;
    }

    /// Create request builder with current token
    async fn create_request_builder(&self) -> TTSResult<AcapelaRequestBuilder> {
        let token = self.get_token().await?;
        Ok(AcapelaRequestBuilder::new(
            self.acapela_config.clone(),
            self.base_config.clone(),
            token,
        ))
    }

    /// Perform logout (optional cleanup)
    pub async fn logout(&self) -> TTSResult<()> {
        let token = {
            let cache = self.token_cache.read().await;
            match *cache {
                Some(ref cached) => cached.token.clone(),
                None => return Ok(()), // Not logged in
            }
        };

        let response = self
            .auth_client
            .get(super::ACAPELA_LOGOUT_URL)
            .header(AUTHORIZATION, format!("Token {}", token))
            .send()
            .await;

        // Invalidate cache regardless of response
        self.invalidate_token().await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                debug!("Successfully logged out from Acapela Cloud");
                Ok(())
            }
            Ok(resp) => {
                warn!("Logout returned status {}", resp.status());
                Ok(()) // Don't fail on logout errors
            }
            Err(e) => {
                warn!("Logout request failed: {}", e);
                Ok(()) // Don't fail on logout errors
            }
        }
    }

    /// Get account information
    ///
    /// Returns account details including available credits and voices.
    pub async fn get_account_info(&self) -> TTSResult<AcapelaAccountInfo> {
        let token = self.get_token().await?;

        let response = self
            .auth_client
            .get(super::ACAPELA_ACCOUNT_URL)
            .header(AUTHORIZATION, format!("Token {}", token))
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Account info request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(TTSError::ProviderError(format!(
                "Failed to get account info ({}): {}",
                status, error_body
            )));
        }

        let account_info: AcapelaAccountInfo = response.json().await.map_err(|e| {
            TTSError::InternalError(format!("Failed to parse account info: {}", e))
        })?;

        Ok(account_info)
    }

    /// List available voices from account
    pub async fn list_voices(&self) -> TTSResult<Vec<AcapelaVoice>> {
        let account_info = self.get_account_info().await?;

        // Convert voice IDs to AcapelaVoice objects
        let voices: Vec<AcapelaVoice> = account_info
            .voices
            .into_iter()
            .map(|id| AcapelaVoice {
                id: id.clone(),
                name: Some(id),
                language: None,
                gender: None,
                voice_type: None,
            })
            .collect();

        Ok(voices)
    }

    /// Get the Acapela-specific configuration
    pub fn acapela_config(&self) -> &AcapelaTtsConfig {
        &self.acapela_config
    }

    /// Get the command endpoint URL being used
    pub fn command_url(&self) -> &str {
        super::ACAPELA_COMMAND_URL
    }
}

#[async_trait]
impl BaseTTS for AcapelaTts {
    /// Create a new Acapela Cloud TTS provider
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        // Parse Acapela-specific configuration from base config
        let acapela_config = AcapelaTtsConfig::from_base(&config)?;

        info!(
            "Creating Acapela Cloud TTS provider: voice={}, format={}, sample_rate={}",
            acapela_config.voice_id, acapela_config.audio_format, acapela_config.sample_rate
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            acapela_config,
            base_config: config,
            token_cache: Arc::new(RwLock::new(None)),
            auth_client: reqwest::Client::new(),
        })
    }

    /// Get the underlying TTSProvider for HTTP-based providers
    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    /// Connect to the TTS provider
    ///
    /// Performs login to obtain authentication token and initializes
    /// the HTTP connection pool.
    async fn connect(&mut self) -> TTSResult<()> {
        debug!("Connecting Acapela Cloud TTS provider");

        // Perform login to get token
        let token = self.login().await?;
        {
            let mut cache = self.token_cache.write().await;
            *cache = Some(TokenCache::new(token));
        }

        // Initialize HTTP connection pool
        self.provider
            .generic_connect_with_config(super::ACAPELA_COMMAND_URL, &self.base_config)
            .await
    }

    /// Disconnect from the TTS provider
    ///
    /// Performs logout and closes the connection pool.
    async fn disconnect(&mut self) -> TTSResult<()> {
        debug!("Disconnecting Acapela Cloud TTS provider");

        // Logout (best effort)
        if let Err(e) = self.logout().await {
            warn!("Logout failed during disconnect: {}", e);
        }

        // Disconnect HTTP provider
        self.provider.generic_disconnect().await
    }

    /// Synthesize text to speech
    ///
    /// # Arguments
    /// * `text` - The text to synthesize
    /// * `flush` - Whether to interrupt current playback
    ///
    /// # Returns
    /// * `TTSResult<()>` - Success or error
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
        self.acapela_config.validate_text(text)?;

        debug!(
            "Acapela TTS speak: text='{}', flush={}, voice={}, format={}",
            text, flush, self.acapela_config.voice_id, self.acapela_config.audio_format
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
        match self.provider.generic_speak(request_builder, text, flush).await {
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
    use crate::core::tts::acapela::AcapelaAudioFormat;

    #[test]
    fn test_acapela_tts_creation() {
        // With credentials
        let config = TTSConfig {
            api_key: "user@example.com:password123".to_string(),
            voice_id: Some("alice".to_string()),
            ..Default::default()
        };

        let tts = AcapelaTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert_eq!(tts.acapela_config().voice_id, "alice");
        assert_eq!(
            tts.acapela_config().credentials.email,
            "user@example.com"
        );
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_acapela_tts_requires_credentials() {
        let config = TTSConfig::default();
        let result = AcapelaTts::new(config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(
                msg.contains("credentials") || msg.contains("ACAPELA"),
                "Error message should mention credentials: {}",
                msg
            );
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_acapela_tts_invalid_credentials_format() {
        let config = TTSConfig {
            api_key: "no-colon-here".to_string(),
            voice_id: Some("alice".to_string()),
            ..Default::default()
        };

        let result = AcapelaTts::new(config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(
                msg.contains("email:password"),
                "Error should mention format: {}",
                msg
            );
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_request_builder_url() {
        let config = TTSConfig {
            api_key: "user@example.com:password".to_string(),
            voice_id: Some("graham".to_string()),
            audio_format: Some("wav".to_string()),
            ..Default::default()
        };

        let acapela_config = AcapelaTtsConfig::from_base(&config).unwrap();
        let builder =
            AcapelaRequestBuilder::new(acapela_config, config, "test-token".to_string());

        // Test that command URL is correct
        assert!(builder.command_url().contains("acapela-cloud.com"));
        assert!(builder.command_url().contains("/api/command"));
    }

    #[test]
    fn test_request_builder_with_options() {
        let config = TTSConfig {
            api_key: "user@example.com:password".to_string(),
            voice_id: Some("lily".to_string()),
            audio_format: Some("ogg".to_string()),
            sample_rate: Some(24000),
            ..Default::default()
        };

        let acapela_config = AcapelaTtsConfig::from_base(&config).unwrap();
        let builder =
            AcapelaRequestBuilder::new(acapela_config, config, "test-token".to_string());

        assert_eq!(builder.acapela_config().voice_id, "lily");
        assert_eq!(builder.acapela_config().audio_format, AcapelaAudioFormat::Ogg);
        assert_eq!(builder.acapela_config().sample_rate, 24000);
    }

    #[test]
    fn test_pronunciation_replacer() {
        use crate::core::tts::base::Pronunciation;

        let config = TTSConfig {
            api_key: "user@example.com:password".to_string(),
            voice_id: Some("alice".to_string()),
            pronunciations: vec![
                Pronunciation {
                    word: "Acapela".to_string(),
                    pronunciation: "ah-kah-peh-lah".to_string(),
                },
            ],
            ..Default::default()
        };

        let acapela_config = AcapelaTtsConfig::from_base(&config).unwrap();
        let builder =
            AcapelaRequestBuilder::new(acapela_config, config, "test-token".to_string());

        assert!(builder.get_pronunciation_replacer().is_some());
    }

    #[tokio::test]
    async fn test_acapela_tts_not_connected() {
        let config = TTSConfig {
            api_key: "user@example.com:password".to_string(),
            voice_id: Some("alice".to_string()),
            ..Default::default()
        };

        let mut tts = AcapelaTts::new(config).unwrap();
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
    async fn test_acapela_tts_empty_text() {
        let config = TTSConfig {
            api_key: "user@example.com:password".to_string(),
            voice_id: Some("alice".to_string()),
            ..Default::default()
        };

        let mut tts = AcapelaTts::new(config).unwrap();

        // Empty text should succeed without making a request (even when not connected)
        // because we check for empty text before checking connection
        let result = tts.speak("", true).await;
        assert!(result.is_ok());

        let result = tts.speak("   ", true).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_default_command_url() {
        let config = TTSConfig {
            api_key: "user@example.com:password".to_string(),
            voice_id: Some("alice".to_string()),
            ..Default::default()
        };

        let tts = AcapelaTts::new(config).unwrap();
        assert_eq!(
            tts.command_url(),
            "https://www.acapela-cloud.com/api/command/"
        );
    }

    #[test]
    fn test_config_with_all_audio_formats() {
        let formats = vec![
            ("mp3", AcapelaAudioFormat::Mp3),
            ("ogg", AcapelaAudioFormat::Ogg),
            ("wav", AcapelaAudioFormat::Wav),
            ("flac", AcapelaAudioFormat::Flac),
            ("aac", AcapelaAudioFormat::Aac),
            ("opus", AcapelaAudioFormat::Opus),
        ];

        for (format_str, expected_format) in formats {
            let config = TTSConfig {
                api_key: "user@example.com:password".to_string(),
                voice_id: Some("alice".to_string()),
                audio_format: Some(format_str.to_string()),
                ..Default::default()
            };

            let tts = AcapelaTts::new(config).unwrap();
            assert_eq!(
                tts.acapela_config().audio_format,
                expected_format,
                "Format {} should parse to {:?}",
                format_str,
                expected_format
            );
        }
    }

    #[test]
    fn test_token_cache_staleness() {
        let cache = TokenCache::new("test-token".to_string());
        assert!(!cache.might_be_stale());

        // Can't easily test time-based staleness without mocking time
        // Just verify the method exists and returns a boolean
        let _ = cache.might_be_stale();
    }
}
