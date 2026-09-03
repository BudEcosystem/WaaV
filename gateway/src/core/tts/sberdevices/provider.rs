//! SberDevices SaluteSpeech TTS Provider Implementation
//!
//! Implements the BaseTTS trait for SberDevices' REST-based Text-to-Speech service.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Notify, RwLock};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::config::{MAX_TEXT_LENGTH, SberTtsConfig, TOKEN_REFRESH_THRESHOLD_SECS};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};

fn sber_tts_http_client(
    connection_timeout_secs: u64,
    request_timeout_secs: u64,
) -> Result<reqwest::Client, reqwest::Error> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
        .connect_timeout(Duration::from_secs(connection_timeout_secs))
        .timeout(Duration::from_secs(request_timeout_secs))
        .build()
}

fn default_sber_tts_http_client() -> Option<reqwest::Client> {
    match sber_tts_http_client(30, 60) {
        Ok(client) => Some(client),
        Err(err) => {
            warn!(
                error = %err,
                "failed to create default SberDevices TTS HTTP client; default instance is inert until rebuilt with a valid config"
            );
            None
        }
    }
}

fn sber_tts_http_client_unavailable_error() -> TTSError {
    TTSError::InvalidConfiguration(
        "SberDevices TTS default HTTP client is unavailable; construct with SberDevicesTts::new or from_standard"
            .to_string(),
    )
}

// =============================================================================
// OAuth Token Manager
// =============================================================================

/// Manages OAuth token lifecycle with automatic refresh
#[derive(Debug)]
struct TokenManager {
    access_token: Option<String>,
    expires_at: u64,
}

impl TokenManager {
    fn new() -> Self {
        Self {
            access_token: None,
            expires_at: 0,
        }
    }

    fn is_token_valid(&self) -> bool {
        if self.access_token.is_none() {
            return false;
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let threshold_ms = TOKEN_REFRESH_THRESHOLD_SECS * 1000;
        self.expires_at > now_ms + threshold_ms
    }

    fn set_token(&mut self, token: String, expires_at: u64) {
        self.access_token = Some(token);
        self.expires_at = expires_at;
    }

    fn get_token(&self) -> Option<&str> {
        self.access_token.as_deref()
    }

    fn clear(&mut self) {
        self.access_token = None;
        self.expires_at = 0;
    }
}

// =============================================================================
// SberDevices TTS Provider
// =============================================================================

/// SberDevices SaluteSpeech TTS provider
pub struct SberDevicesTts {
    config: Option<SberTtsConfig>,
    state: ConnectionState,
    state_notify: Arc<Notify>,
    client: Option<reqwest::Client>,
    token_manager: Arc<RwLock<TokenManager>>,
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
    connected: AtomicBool,
}

impl Default for SberDevicesTts {
    fn default() -> Self {
        Self {
            config: None,
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            client: default_sber_tts_http_client(),
            token_manager: Arc::new(RwLock::new(TokenManager::new())),
            audio_callback: Arc::new(RwLock::new(None)),
            connected: AtomicBool::new(false),
        }
    }
}

impl SberDevicesTts {
    /// Build from the standardized TTS config (W1 keystone).
    ///
    /// Mirrors `DeepgramTTS::from_standard`: maps the standardized features onto SberDevices'
    /// provider config via [`SberTtsConfig::from_standard`] (sample_rate + the timeout extras),
    /// then constructs the provider mirroring [`BaseTTS::new`] (HTTP client built from the
    /// resolved timeouts). SberDevices SaluteSpeech exposes no prosody knobs, so speed/pitch/
    /// emotion/instructions/ssml/seed/… are capability gaps and stay at their defaults.
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let sber_config =
            SberTtsConfig::from_standard(std).map_err(TTSError::InvalidConfiguration)?;

        let client = sber_tts_http_client(
            sber_config.connection_timeout_secs,
            sber_config.request_timeout_secs,
        )
        .map_err(|e| TTSError::InternalError(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            config: Some(sber_config),
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            client: Some(client),
            token_manager: Arc::new(RwLock::new(TokenManager::new())),
            audio_callback: Arc::new(RwLock::new(None)),
            connected: AtomicBool::new(false),
        })
    }

    /// Obtain OAuth access token
    async fn obtain_token(&self) -> TTSResult<String> {
        let config = self.config.as_ref().ok_or_else(|| {
            TTSError::InvalidConfiguration("No configuration available".to_string())
        })?;

        // Check if existing token is valid
        {
            let token_guard = self.token_manager.read().await;
            if token_guard.is_token_valid()
                && let Some(token) = token_guard.get_token()
            {
                return Ok(token.to_string());
            }
        }

        // Need to refresh token
        debug!("Obtaining new OAuth token for SberDevices TTS");

        let client = self
            .client
            .as_ref()
            .ok_or_else(sber_tts_http_client_unavailable_error)?;

        let rq_uid = Uuid::new_v4().to_string();

        let response = client
            .post(config.oauth_url())
            .header("Authorization", config.oauth_auth_header())
            .header("RqUID", &rq_uid)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!("scope={}", config.scope.as_str()))
            .timeout(Duration::from_secs(config.connection_timeout_secs))
            .send()
            .await
            .map_err(|e| TTSError::ConnectionFailed(format!("OAuth request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(TTSError::AuthenticationFailed(format!(
                "OAuth failed with status {}: {}",
                status, body
            )));
        }

        #[derive(serde::Deserialize)]
        struct OAuthResponse {
            access_token: String,
            expires_at: u64,
        }

        let oauth_response: OAuthResponse = response.json().await.map_err(|e| {
            TTSError::ConnectionFailed(format!("Failed to parse OAuth response: {}", e))
        })?;

        // Store the token
        let token = oauth_response.access_token.clone();
        {
            let mut token_guard = self.token_manager.write().await;
            token_guard.set_token(oauth_response.access_token, oauth_response.expires_at);
        }

        debug!("Successfully obtained OAuth token for SberDevices TTS");
        Ok(token)
    }

    /// Build the `text:synthesize` POST request (URL, auth, body, and — load-bearing — the
    /// `Content-Type` that selects plain-text vs SSML input mode). Kept as an inspectable seam so
    /// the wire-level test asserts the exact request the live synth path sends.
    fn build_synthesis_request(
        client: &reqwest::Client,
        config: &SberTtsConfig,
        url: &str,
        token: &str,
        text: &str,
    ) -> reqwest::RequestBuilder {
        client
            .post(url)
            .header("Authorization", format!("Bearer {}", token))
            // SSML input mode flips this to `application/ssml`, putting SaluteSpeech into SSML
            // parsing for the same `text:synthesize` endpoint.
            .header("Content-Type", config.synthesis_content_type())
            .body(text.to_string())
            .timeout(Duration::from_secs(config.request_timeout_secs))
    }

    /// Synthesize text using REST API
    async fn synthesize(&self, text: &str) -> TTSResult<Vec<u8>> {
        let config = self.config.as_ref().ok_or_else(|| {
            TTSError::InvalidConfiguration("No configuration available".to_string())
        })?;

        // Check text length
        if text.len() > MAX_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text exceeds maximum length of {} characters",
                MAX_TEXT_LENGTH
            )));
        }

        // Get OAuth token
        let token = self.obtain_token().await?;
        let client = self
            .client
            .as_ref()
            .ok_or_else(sber_tts_http_client_unavailable_error)?;

        // Build synthesis URL
        let url = config.synthesis_url();

        debug!(
            text_len = text.len(),
            voice = config.voice.as_str(),
            format = config.audio_format.as_str(),
            sample_rate = config.sample_rate,
            "Synthesizing text with SberDevices TTS"
        );

        let response = Self::build_synthesis_request(client, config, &url, &token, text)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Synthesis request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();

            // Handle specific error codes
            if status.as_u16() == 401 || status.as_u16() == 403 {
                // Clear cached token on auth failure
                self.token_manager.write().await.clear();
                return Err(TTSError::AuthenticationFailed(format!(
                    "Authentication failed: {}",
                    body
                )));
            }

            if status.as_u16() == 429 {
                return Err(TTSError::RateLimited {
                    retry_after_secs: None,
                    message: "Rate limit exceeded".to_string(),
                });
            }

            return Err(TTSError::AudioGenerationFailed(format!(
                "Synthesis failed with status {}: {}",
                status, body
            )));
        }

        let audio_bytes = response.bytes().await.map_err(|e| {
            TTSError::AudioGenerationFailed(format!("Failed to read audio response: {}", e))
        })?;

        debug!(
            audio_size = audio_bytes.len(),
            "Received audio from SberDevices TTS"
        );
        Ok(audio_bytes.to_vec())
    }
}

#[async_trait]
impl BaseTTS for SberDevicesTts {
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        let sber_config =
            SberTtsConfig::from_base(config).map_err(TTSError::InvalidConfiguration)?;

        // Build HTTP client with timeouts
        let client = sber_tts_http_client(
            sber_config.connection_timeout_secs,
            sber_config.request_timeout_secs,
        )
        .map_err(|e| TTSError::InternalError(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            config: Some(sber_config),
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            client: Some(client),
            token_manager: Arc::new(RwLock::new(TokenManager::new())),
            audio_callback: Arc::new(RwLock::new(None)),
            connected: AtomicBool::new(false),
        })
    }

    async fn connect(&mut self) -> TTSResult<()> {
        let config = self.config.clone().ok_or_else(|| {
            TTSError::InvalidConfiguration("No configuration available".to_string())
        })?;

        // Validate configuration
        config.validate().map_err(TTSError::InvalidConfiguration)?;

        self.state = ConnectionState::Connecting;
        self.state_notify.notify_waiters();

        // Obtain initial OAuth token to verify credentials
        match self.obtain_token().await {
            Ok(_) => {
                self.state = ConnectionState::Connected;
                self.connected.store(true, Ordering::SeqCst);
                self.state_notify.notify_waiters();
                info!("Connected to SberDevices SaluteSpeech TTS");
                Ok(())
            }
            Err(e) => {
                self.state = ConnectionState::Error(e.to_string());
                self.state_notify.notify_waiters();
                Err(e)
            }
        }
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.token_manager.write().await.clear();
        *self.audio_callback.write().await = None;
        self.connected.store(false, Ordering::SeqCst);

        self.state = ConnectionState::Disconnected;
        self.state_notify.notify_waiters();

        info!("Disconnected from SberDevices SaluteSpeech TTS");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst) && self.config.is_some()
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.state.clone()
    }

    async fn speak(&mut self, text: &str, _flush: bool) -> TTSResult<()> {
        if !self.is_ready() {
            // Try to reconnect
            self.connect().await?;
        }

        if text.is_empty() {
            return Ok(());
        }

        let config = self.config.as_ref().ok_or_else(|| {
            TTSError::InvalidConfiguration("No configuration available".to_string())
        })?;

        // Synthesize audio
        let audio_bytes = self.synthesize(text).await?;

        // Create AudioData and deliver via callback
        let audio_data = AudioData {
            data: audio_bytes,
            sample_rate: config.sample_rate,
            format: config.audio_format.mime_type().to_string(),
            duration_ms: None,
        };

        let callback_guard = self.audio_callback.read().await;
        if let Some(callback) = callback_guard.as_ref() {
            callback.on_audio(audio_data).await;
            callback.on_complete().await;
        }

        Ok(())
    }

    async fn clear(&mut self) -> TTSResult<()> {
        // SberDevices TTS is request-based, no queue to clear
        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        // SberDevices TTS processes requests immediately, no buffering
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        let callback_ref = self.audio_callback.clone();
        tokio::spawn(async move {
            *callback_ref.write().await = Some(callback);
        });
        Ok(())
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        let callback_ref = self.audio_callback.clone();
        tokio::spawn(async move {
            *callback_ref.write().await = None;
        });
        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        let config = self.config.as_ref();

        serde_json::json!({
            "provider": "sberdevices",
            "name": "SberDevices SaluteSpeech TTS",
            "version": "1.0.0",
            "endpoint": super::SBER_TTS_SYNTHESIZE_URL,
            "languages": ["ru-RU", "en-US"],
            "voices": [
                {
                    "id": "Nec",
                    "name": "Natalia",
                    "gender": "female",
                    "language": "ru-RU"
                },
                {
                    "id": "Bys",
                    "name": "Boris",
                    "gender": "male",
                    "language": "ru-RU"
                },
                {
                    "id": "May",
                    "name": "Martha",
                    "gender": "female",
                    "language": "ru-RU"
                },
                {
                    "id": "Tur",
                    "name": "Taras",
                    "gender": "male",
                    "language": "ru-RU"
                },
                {
                    "id": "Ost",
                    "name": "Alexandra",
                    "gender": "female",
                    "language": "ru-RU"
                },
                {
                    "id": "Pon",
                    "name": "Sergey",
                    "gender": "male",
                    "language": "ru-RU"
                },
                {
                    "id": "Kin",
                    "name": "Kira",
                    "gender": "female",
                    "language": "en-US"
                }
            ],
            "formats": ["wav", "opus", "mp3"],
            "sample_rates": [8000, 24000],
            "features": {
                "ssml": true,
                "max_text_length": 4000
            },
            "current_config": config.map(|c| serde_json::json!({
                "voice": c.voice.as_str(),
                "format": c.audio_format.as_str(),
                "sample_rate": c.sample_rate,
                "scope": c.scope.as_str()
            }))
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
    use std::io::ErrorKind;
    use std::pin::Pin;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            provider: "sberdevices".to_string(),
            api_key: "test_client:test_secret".to_string(),
            voice_id: Some("Nec".to_string()),
            audio_format: Some("wav".to_string()),
            sample_rate: Some(24000),
            ..Default::default()
        }
    }

    // The provider struct's `from_standard` (mirroring `DeepgramTTS::from_standard`) maps a
    // standardized advanced feature (sample_rate) all the way onto the provider config the
    // synthesis request reads — proving the standardized dispatch path reaches the struct, not
    // just the config-level method.
    #[test]
    fn from_standard_maps_sample_rate_onto_provider_config() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                sample_rate: Some(8000),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = SberDevicesTts::from_standard(&std).unwrap();
        assert_eq!(tts.config.as_ref().unwrap().sample_rate, 8000);
    }

    // WIRE-LEVEL (S1/S5 recurring bug class): the typed `ssml` feature must flip the SYNTH REQUEST's
    // `Content-Type` header to `application/ssml` on the actual POST that hits text:synthesize — not
    // just a config bool. Build the exact request the live synth path sends and inspect its header.
    #[test]
    fn ssml_feature_sets_application_ssml_content_type_on_synth_request() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                ssml: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = SberDevicesTts::from_standard(&std).unwrap();
        let config = tts.config.as_ref().unwrap();
        // (1) config carries the flag …
        assert!(config.ssml_input);

        // (2) … AND the built synth request carries Content-Type: application/ssml on the real
        // text:synthesize URL.
        let url = config.synthesis_url();
        let request = SberDevicesTts::build_synthesis_request(
            tts.client
                .as_ref()
                .expect("strict constructor builds an HTTP client"),
            config,
            &url,
            "tok",
            "<speak/>",
        )
        .build()
        .unwrap();
        assert!(
            request
                .url()
                .as_str()
                .starts_with(super::super::SBER_TTS_SYNTHESIZE_URL)
        );
        assert_eq!(
            request
                .headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap(),
            "application/ssml",
            "ssml feature must set Content-Type: application/ssml on the synth request"
        );
    }

    // Default path: without the ssml feature the synth request keeps Content-Type: application/text.
    #[test]
    fn plain_text_content_type_when_ssml_not_requested() {
        let tts = SberDevicesTts::new(create_test_config()).unwrap();
        let config = tts.config.as_ref().unwrap();
        assert!(!config.ssml_input);
        let url = config.synthesis_url();
        let request = SberDevicesTts::build_synthesis_request(
            tts.client
                .as_ref()
                .expect("strict constructor builds an HTTP client"),
            config,
            &url,
            "tok",
            "hi",
        )
        .build()
        .unwrap();
        assert_eq!(
            request
                .headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap(),
            "application/text"
        );
    }

    // Documented capability gap: `language` is NOT a parameter of the REST text:synthesize endpoint
    // (only voice/format/sample_rate appear in its URL), so a language feature must NOT fabricate a
    // `language=` query param. This guard locks the gap in place so a future change can't silently
    // inject one without updating the cited comment in `from_standard`.
    #[test]
    fn language_feature_does_not_reach_synth_url() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                language: Some("en-US".into()),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = SberDevicesTts::from_standard(&std).unwrap();
        let url = tts.config.as_ref().unwrap().synthesis_url();
        assert!(
            !url.contains("language="),
            "REST text:synthesize has no language query param; must not fabricate one: {url}"
        );
    }

    #[tokio::test]
    async fn sberdevices_tts_redirect_policy_rejects_private_hop() {
        let _env = crate::core::net::ssrf_env_lock();
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(err) => {
                if err.kind() == ErrorKind::PermissionDenied {
                    eprintln!(
                        "Skipping sberdevices_tts_redirect_policy_rejects_private_hop: {err}"
                    );
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

        let err = sber_tts_http_client(1, 1)
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
    fn test_sberdevices_tts_new() {
        let config = create_test_config();
        let tts = SberDevicesTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert!(!tts.is_ready());
        assert!(tts.config.is_some());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_sberdevices_tts_new_requires_credentials() {
        let config = TTSConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = SberDevicesTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_sberdevices_tts_default() {
        let tts = SberDevicesTts::default();
        assert!(!tts.is_ready());
        assert!(tts.config.is_none());
    }

    #[tokio::test]
    async fn default_without_http_client_returns_typed_error_without_panic() {
        let mut tts = SberDevicesTts::default();
        tts.config = Some(SberTtsConfig::from_base(create_test_config()).unwrap());
        tts.client = None;

        let err = tts
            .synthesize("Privet")
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
    fn test_sberdevices_tts_get_provider_info() {
        let config = create_test_config();
        let tts = SberDevicesTts::new(config).unwrap();

        let info = tts.get_provider_info();
        assert_eq!(info["provider"], "sberdevices");
        assert_eq!(info["name"], "SberDevices SaluteSpeech TTS");

        let voices = info["voices"].as_array().unwrap();
        assert_eq!(voices.len(), 7);

        // Verify first voice
        assert_eq!(voices[0]["id"], "Nec");
        assert_eq!(voices[0]["name"], "Natalia");
        assert_eq!(voices[0]["gender"], "female");
        assert_eq!(voices[0]["language"], "ru-RU");

        // Verify English voice
        assert_eq!(voices[6]["id"], "Kin");
        assert_eq!(voices[6]["language"], "en-US");

        let formats = info["formats"].as_array().unwrap();
        assert_eq!(formats.len(), 3);

        let sample_rates = info["sample_rates"].as_array().unwrap();
        assert_eq!(sample_rates.len(), 2);
    }

    #[tokio::test]
    async fn test_sberdevices_tts_speak_not_connected() {
        let config = create_test_config();
        let mut tts = SberDevicesTts::new(config).unwrap();

        // Should fail because connect() will fail without real credentials
        let result = tts.speak("Привет", false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sberdevices_tts_speak_empty_text() {
        let config = create_test_config();
        let mut tts = SberDevicesTts::new(config).unwrap();

        // Manually set connected state for testing empty text handling
        tts.connected.store(true, Ordering::SeqCst);

        let result = tts.speak("", false).await;
        assert!(result.is_ok()); // Empty text should return Ok
    }

    #[tokio::test]
    async fn test_sberdevices_tts_disconnect() {
        let config = create_test_config();
        let mut tts = SberDevicesTts::new(config).unwrap();

        // Disconnect should succeed even when not connected
        let result = tts.disconnect().await;
        assert!(result.is_ok());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert!(!tts.is_ready());
    }

    #[tokio::test]
    async fn test_sberdevices_tts_clear() {
        let config = create_test_config();
        let mut tts = SberDevicesTts::new(config).unwrap();

        // Clear should always succeed (no-op for request-based TTS)
        let result = tts.clear().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sberdevices_tts_flush() {
        let config = create_test_config();
        let tts = SberDevicesTts::new(config).unwrap();

        // Flush should always succeed (no buffering)
        let result = tts.flush().await;
        assert!(result.is_ok());
    }

    struct MockAudioCallback {
        audio_count: AtomicUsize,
        error_count: AtomicUsize,
        complete_count: AtomicUsize,
    }

    impl MockAudioCallback {
        fn new() -> Self {
            Self {
                audio_count: AtomicUsize::new(0),
                error_count: AtomicUsize::new(0),
                complete_count: AtomicUsize::new(0),
            }
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
            self.error_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }

        fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.complete_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn test_sberdevices_tts_callback_registration() {
        let config = create_test_config();
        let mut tts = SberDevicesTts::new(config).unwrap();

        let callback = Arc::new(MockAudioCallback::new());
        let result = tts.on_audio(callback);
        assert!(result.is_ok());

        // Give the async task time to complete
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Verify callback was registered
        let callback_guard = tts.audio_callback.read().await;
        assert!(callback_guard.is_some());
    }

    #[tokio::test]
    async fn test_sberdevices_tts_callback_removal() {
        let config = create_test_config();
        let mut tts = SberDevicesTts::new(config).unwrap();

        let callback = Arc::new(MockAudioCallback::new());
        tts.on_audio(callback).unwrap();

        // Give the async task time to complete
        tokio::time::sleep(Duration::from_millis(10)).await;

        let result = tts.remove_audio_callback();
        assert!(result.is_ok());

        // Give the async task time to complete
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Verify callback was removed
        let callback_guard = tts.audio_callback.read().await;
        assert!(callback_guard.is_none());
    }

    #[test]
    fn test_sberdevices_tts_config_with_different_voices() {
        let mut config = create_test_config();
        config.voice_id = Some("Bys".to_string());

        let tts = SberDevicesTts::new(config).unwrap();
        let sber_config = tts.config.as_ref().unwrap();

        assert_eq!(sber_config.voice.as_str(), "Bys");
        assert_eq!(sber_config.voice.gender(), "male");
        assert_eq!(sber_config.voice.display_name(), "Boris");
    }

    #[test]
    fn test_sberdevices_tts_config_with_different_formats() {
        let mut config = create_test_config();
        config.audio_format = Some("opus".to_string());

        let tts = SberDevicesTts::new(config).unwrap();
        let sber_config = tts.config.as_ref().unwrap();

        assert_eq!(sber_config.audio_format.as_str(), "opus");
        assert_eq!(sber_config.audio_format.mime_type(), "audio/opus");
    }

    #[test]
    fn test_sberdevices_tts_config_invalid_voice() {
        let mut config = create_test_config();
        config.voice_id = Some("invalid_voice".to_string());

        let result = SberDevicesTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_sberdevices_tts_config_invalid_format() {
        let mut config = create_test_config();
        config.audio_format = Some("invalid_format".to_string());

        let result = SberDevicesTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_sberdevices_tts_config_invalid_sample_rate() {
        let mut config = create_test_config();
        config.sample_rate = Some(16000); // Invalid - only 8000 or 24000

        let result = SberDevicesTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_sberdevices_tts_config_with_english_voice() {
        let mut config = create_test_config();
        config.voice_id = Some("Kin".to_string());

        let tts = SberDevicesTts::new(config).unwrap();
        let sber_config = tts.config.as_ref().unwrap();

        assert_eq!(sber_config.voice.as_str(), "Kin");
        assert_eq!(sber_config.voice.language(), "en-US");
        assert_eq!(sber_config.voice.display_name(), "Kira");
    }

    #[test]
    fn test_token_manager() {
        let mut manager = TokenManager::new();

        // Initially no token
        assert!(!manager.is_token_valid());
        assert!(manager.get_token().is_none());

        // Set a token that expires in 5 minutes
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        manager.set_token("test_token".to_string(), now_ms + 300_000);

        assert!(manager.is_token_valid());
        assert_eq!(manager.get_token(), Some("test_token"));

        // Clear the token
        manager.clear();
        assert!(!manager.is_token_valid());
        assert!(manager.get_token().is_none());
    }

    #[test]
    fn test_token_manager_expired() {
        let mut manager = TokenManager::new();

        // Set an already expired token
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        manager.set_token("expired_token".to_string(), now_ms - 1000);

        assert!(!manager.is_token_valid());
    }
}
