//! Gnani.ai TTS Provider Implementation
//!
//! Implements the BaseTTS trait for Gnani's Text-to-Speech REST API.

use async_trait::async_trait;
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
            GnaniTTSConfig::from_base(config).map_err(TTSError::InvalidConfiguration)?;

        Ok(Self {
            config: Some(gnani_config),
            ..Default::default()
        })
    }

    /// Build from the standardized config (W1 keystone), mirroring `DeepgramTTS::from_standard`.
    /// Gnani's expressible advanced features are `language` (-> `language_code`) and `sample_rate`
    /// (-> `output_sample_rate`), plus the provider-specific `voice_name` extra; these are mapped
    /// by [`GnaniTTSConfig::from_standard`] and reach the live synthesis request — previously
    /// unreachable through the flat factory. Speed/pitch/volume/stability/emotion/instructions/
    /// SSML-text-mode have no Gnani field and are skipped (capability gaps).
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let gnani_config =
            GnaniTTSConfig::from_standard(std).map_err(TTSError::InvalidConfiguration)?;

        Ok(Self {
            config: Some(gnani_config),
            ..Default::default()
        })
    }

    /// Build HTTP client
    fn build_client(config: &GnaniTTSConfig) -> TTSResult<reqwest::Client> {
        let builder =
            crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
                .timeout(std::time::Duration::from_secs(config.request_timeout_secs));

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

        let request_body = GnaniTTSRequest::from_config(config, text);

        debug!(
            text_len = text.len(),
            model = config.model.as_str(),
            voice = %request_body.voice,
            language = request_body.language,
            "Gnani TTS synthesis request"
        );

        let response = client
            .post(crate::core::tts::standard::override_rest_endpoint(
                config.endpoint(),
                config.endpoint_override.as_deref(),
            ))
            .header("X-API-Key-ID", &config.api_key)
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

        // `container: raw` with `linear_pcm`: the body is 16-bit PCM, no header.
        let audio_bytes = response
            .bytes()
            .await
            .map_err(|e| TTSError::ProviderError(format!("Failed to read audio: {}", e)))?
            .to_vec();

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
            "description": "Indic TTS (Timbre v3.0 and v2.5) in ten languages",
            "api_type": "HTTP REST",
            "endpoint": super::config::GNANI_TTS_ENDPOINT,
            "features": ["indic-languages"],
            "models": ["timbre-v3.0", "timbre-v2.5"],
            "supported_languages": [
                "hi-IN", "en-IN", "ta-IN", "te-IN", "kn-IN",
                "ml-IN", "mr-IN", "pa-IN", "bn-IN", "gu-IN"
            ]
        })
    }
}

/// The `/api/v1/tts/inference` body (docs.gnani.ai, "Text-to-Speech (REST)").
#[derive(Debug, Clone, serde::Serialize)]
struct GnaniTTSRequest {
    text: String,
    voice: String,
    model: &'static str,
    language: &'static str,
    speed: f32,
    audio_config: GnaniAudioConfig,
}

impl GnaniTTSRequest {
    fn from_config(config: &GnaniTTSConfig, text: &str) -> Self {
        Self {
            text: text.to_string(),
            voice: config.resolved_voice(),
            model: config.model.as_str(),
            // `validate` refuses a language without an API code; `auto` is never reached.
            language: config.language_code.api_code().unwrap_or("auto"),
            speed: config.speed.clamp(0.85, 1.15),
            audio_config: GnaniAudioConfig {
                sample_rate: config.output_sample_rate,
                num_channels: 1,
                sample_width: 2,
                encoding: "linear_pcm",
                container: "raw",
            },
        }
    }
}

/// Audio output configuration: raw 16-bit mono PCM at the session's rate.
#[derive(Debug, Clone, serde::Serialize)]
struct GnaniAudioConfig {
    sample_rate: u32,
    num_channels: u8,
    sample_width: u8,
    encoding: &'static str,
    container: &'static str,
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
            api_base: None,
        }
    }

    #[test]
    fn test_gnani_tts_creation() {
        let config = create_test_config();
        let result = GnaniTTS::create(config);
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn gnani_tts_redirect_policy_rejects_private_hop() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local redirect test server");
        let addr = listener.local_addr().expect("local addr");

        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: http://127.0.0.1:9/metadata\r\n",
                "Content-Length: 0\r\n",
                "\r\n"
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
        });

        let mut config = GnaniTTSConfig::default();
        config.api_key = "test-api-key".to_string();
        let client = GnaniTTS::build_client(&config).expect("construct Gnani TTS client");
        let err = client
            .get(format!("http://{addr}/start"))
            .send()
            .await
            .expect_err("private redirect target must be rejected");
        let mut error_chain = err.to_string();
        let mut source = std::error::Error::source(&err);
        while let Some(err) = source {
            error_chain.push_str(": ");
            error_chain.push_str(&err.to_string());
            source = err.source();
        }
        assert!(error_chain.contains("SSRF protection"), "{error_chain}");
    }

    // W1 keystone (struct-level, mirrors DeepgramTTS::from_standard): the standardized `language`
    // and `sample_rate` features and the `voice_name` extra reach the live `GnaniTTSConfig` through
    // the provider STRUCT's `from_standard` — the path the dispatch helper constructs.
    #[test]
    fn from_standard_struct_maps_language_and_sample_rate() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("voice_name".into(), serde_json::json!("speaker-3"));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "gnani".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                language: Some("Ta-IN".into()),
                sample_rate: Some(16000),
                ssml: Some(true), // capability gap: must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let tts = GnaniTTS::from_standard(&std).unwrap();
        let cfg = tts.config.as_ref().unwrap();
        assert_eq!(
            cfg.language_code,
            crate::core::tts::gnani::GnaniTTSLanguage::Tamil
        );
        assert_eq!(cfg.output_sample_rate, 16000);
        assert_eq!(cfg.voice_name, Some("speaker-3".to_string()));
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
        assert!(!info["features"].as_array().unwrap().is_empty());
    }

    /// The body is the documented `/api/v1/tts/inference` shape: an old language-code voice
    /// becomes `language`, the default voice is Timbre v3.0's for the gender, audio is raw PCM.
    #[test]
    fn test_gnani_tts_request_serialization() {
        let mut config = GnaniTTSConfig::from_base(create_test_config()).unwrap();
        config.api_key = "test-api-key".to_string();
        let body = serde_json::to_value(GnaniTTSRequest::from_config(&config, "नमस्ते")).unwrap();
        assert_eq!(body["text"], "नमस्ते");
        assert_eq!(body["model"], "timbre-v3.0");
        assert_eq!(body["voice"], "Shivani");
        assert_eq!(body["language"], "hi-IN");
        assert_eq!(body["speed"], 1.0);
        assert_eq!(body["audio_config"]["sample_rate"], 8000);
        assert_eq!(body["audio_config"]["encoding"], "linear_pcm");
        assert_eq!(body["audio_config"]["container"], "raw");

        // A Timbre v2.5 voice selects its model (a v2.5 voice sent with v3.0 is a 400).
        let mut base = create_test_config();
        base.voice_id = Some("nalini".to_string());
        let config = GnaniTTSConfig::from_base(base).unwrap();
        let body = serde_json::to_value(GnaniTTSRequest::from_config(&config, "x")).unwrap();
        assert_eq!(
            (body["model"].as_str(), body["voice"].as_str()),
            (Some("timbre-v2.5"), Some("Nalini"))
        );
    }

    /// End to end against a local server: the request reaches the documented endpoint path with
    /// `X-API-Key-ID` and the JSON body, and the raw PCM response is the audio.
    #[tokio::test(flavor = "current_thread")]
    async fn synthesis_sends_the_documented_request_and_returns_the_pcm() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        struct RestoreLoopbackFlag(Option<String>);
        impl Drop for RestoreLoopbackFlag {
            fn drop(&mut self) {
                // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
                unsafe {
                    if let Some(previous) = self.0.take() {
                        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous);
                    } else {
                        std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
                    }
                }
            }
        }
        let _restore_loopback =
            RestoreLoopbackFlag(std::env::var("WAAV_ALLOW_LOOPBACK_ENDPOINTS").ok());
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1") };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local Gnani mock");
        let addr = listener.local_addr().expect("local addr");
        let (seen_tx, seen_rx) = tokio::sync::oneshot::channel::<String>();
        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            // Headers, then the body (`Content-Length`).
            loop {
                let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
                    .await
                    .unwrap_or(0);
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if request.len() >= head_end + 4 + length {
                        break;
                    }
                }
            }
            let _ = seen_tx.send(String::from_utf8_lossy(&request).into_owned());
            let pcm = [1u8, 0, 2, 0];
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
                pcm.len()
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, head.as_bytes()).await;
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, &pcm).await;
        });

        let mut base = create_test_config();
        base.api_key = "test-api-key".to_string();
        let mut tts = GnaniTTS::create(base).unwrap();
        if let Some(config) = tts.config.as_mut() {
            config.endpoint_override = Some(format!("http://{addr}"));
        }
        tts.connect().await.expect("connect");
        let audio = tts.synthesize("नमस्ते").await.expect("synthesize");
        assert_eq!(audio, vec![1u8, 0, 2, 0]);

        let request = seen_rx.await.expect("the mock saw the request");
        assert!(
            request.starts_with("POST /api/v1/tts/inference "),
            "{request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("x-api-key-id: test-api-key"),
            "{request}"
        );
        assert!(request.contains(r#""model":"timbre-v3.0""#), "{request}");
        assert!(request.contains(r#""language":"hi-IN""#), "{request}");
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
