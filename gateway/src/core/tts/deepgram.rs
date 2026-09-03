use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::base::{AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult};
use super::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};
use crate::utils::req_manager::ReqManager;
use xxhash_rust::xxh3::xxh3_128;

/// Deepgram TTS endpoint
pub const DEEPGRAM_TTS_URL: &str = "https://api.deepgram.com/v1/speak";

fn validate_deepgram_tts_endpoint(source: &str, endpoint: &str) -> TTSResult<()> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }

    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES).map_err(
        |msg| TTSError::InvalidConfiguration(format!("{source} rejected (SSRF protection): {msg}")),
    )
}

fn validate_deepgram_callback_url<'a>(source: &str, callback: &'a str) -> TTSResult<&'a str> {
    let callback = callback.trim();
    if callback.is_empty() {
        return Err(TTSError::InvalidConfiguration(format!(
            "{source} rejected (SSRF protection): empty URL"
        )));
    }

    crate::core::net::validate_url_for_ssrf(callback, crate::core::net::HTTP_URL_SCHEMES).map_err(
        |msg| TTSError::InvalidConfiguration(format!("{source} rejected (SSRF protection): {msg}")),
    )?;

    Ok(callback)
}

fn normalize_deepgram_callback_method(method: &str) -> TTSResult<String> {
    let method = method.trim();
    if method.is_empty() {
        return Err(TTSError::InvalidConfiguration(
            "callback_method rejected: empty method".to_string(),
        ));
    }

    let method = method.to_ascii_uppercase();
    match method.as_str() {
        "POST" | "PUT" => Ok(method),
        _ => Err(TTSError::InvalidConfiguration(format!(
            "callback_method rejected: expected POST or PUT, got {method}"
        ))),
    }
}

/// Deepgram-specific request builder
#[derive(Clone)]
struct DeepgramRequestBuilder {
    config: TTSConfig,
    pronunciation_replacer: Option<PronunciationReplacer>,
    /// Deepgram `/v1/speak` advanced query parameters, populated from the standardized config.
    speak: DeepgramSpeakParams,
}

/// Advanced Deepgram `/v1/speak` query parameters that the flat `TTSConfig` cannot express.
///
/// All optional; absent fields are omitted from the URL so default behavior is preserved.
#[derive(Clone, Default)]
struct DeepgramSpeakParams {
    /// Speaking rate (`speed`, range 0.7–1.5, default 1.0). Audio-changing → part of cache key.
    speed: Option<f32>,
    /// Output audio bitrate in bits/sec (`bit_rate`). Audio-changing → part of cache key.
    bit_rate: Option<u32>,
    /// Async result callback URL (`callback`). Delivery-only → NOT part of cache key.
    callback: Option<String>,
    /// Callback HTTP method (`callback_method`, POST|PUT). Delivery-only → NOT part of cache key.
    callback_method: Option<String>,
    /// Endpoint override base (scheme+host) for the credential-free mock harness (W-T0); keeps the
    /// `/v1/speak` path + query. Not part of the cache key (it never changes the audio).
    endpoint_override: Option<String>,
}

impl TTSRequestBuilder for DeepgramRequestBuilder {
    /// Build the Deepgram-specific HTTP request with URL, headers and body
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build the URL with query parameters (honoring the W-T0 endpoint override, which swaps
        // scheme+host while keeping the `/v1/speak` path the mock serves on).
        let mut url = crate::core::tts::standard::override_rest_endpoint(
            DEEPGRAM_TTS_URL,
            self.speak.endpoint_override.as_deref(),
        );
        let mut params: Vec<(&str, String)> = Vec::new();

        // Use model field if provided, otherwise fall back to voice_id
        if !self.config.model.is_empty() {
            params.push(("model", self.config.model.clone()));
        } else if let Some(voice_id) = &self.config.voice_id {
            params.push(("model", voice_id.clone()));
        }

        // Encoding (default to raw linear PCM)
        let encoding = self.config.audio_format.as_deref().unwrap_or("linear16");
        params.push(("encoding", encoding.to_string()));

        // Ensure no container when requesting raw PCM to avoid WAV headers
        // Aligns with WS behavior which delivers raw binary frames without headers
        match encoding {
            "linear16" | "pcm" | "mulaw" | "ulaw" | "alaw" => {
                params.push(("container", "none".to_string()));
            }
            _ => {}
        }

        if let Some(sample_rate) = self.config.sample_rate {
            params.push(("sample_rate", sample_rate.to_string()));
        } else {
            // Use 24000 to match defaults elsewhere (e.g., WS path)
            params.push(("sample_rate", "24000".to_string()));
        }

        // Speaking speed/rate (`speed`, Deepgram range 0.7–1.5). Only emitted when explicitly set
        // via the standardized features, so existing default behavior (no `speed` param) is kept.
        if let Some(speed) = self.speak.speed {
            params.push(("speed", speed.to_string()));
        }

        // Output audio bitrate (`bit_rate`, bits/sec).
        if let Some(bit_rate) = self.speak.bit_rate {
            params.push(("bit_rate", bit_rate.to_string()));
        }

        // Async result callback URL (`callback`).
        if let Some(callback) = &self.speak.callback {
            params.push(("callback", callback.clone()));
        }

        // Callback HTTP method (`callback_method`, POST|PUT).
        if let Some(method) = &self.speak.callback_method {
            params.push(("callback_method", method.clone()));
        }

        if !params.is_empty() {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for (key, value) in &params {
                serializer.append_pair(key, value);
            }
            url.push('?');
            url.push_str(&serializer.finish());
        }

        // Determine Accept header based on encoding format
        let accept_header = match encoding {
            "linear16" | "pcm" => "audio/pcm",
            "mp3" => "audio/mpeg",
            "mulaw" | "ulaw" | "alaw" => "audio/basic",
            "opus" => "audio/opus",
            "flac" => "audio/flac",
            _ => "audio/pcm", // Default fallback
        };

        // Build the request with Deepgram-specific headers and body
        client
            .post(url)
            .header("Authorization", format!("Token {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", accept_header)
            .json(&json!({
                "text": text
            }))
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

fn compute_tts_config_hash(config: &TTSConfig, speak: &DeepgramSpeakParams) -> String {
    // Build a stable representation of config fields that impact audio output
    let mut s = String::new();
    s.push_str(config.provider.as_str());
    s.push('|');
    s.push_str(config.voice_id.as_deref().unwrap_or(""));
    s.push('|');
    s.push_str(&config.model);
    s.push('|');
    s.push_str(config.audio_format.as_deref().unwrap_or(""));
    s.push('|');
    if let Some(sr) = config.sample_rate {
        s.push_str(&sr.to_string());
    }
    s.push('|');
    if let Some(rate) = config.speaking_rate {
        s.push_str(&format!("{rate:.3}"));
    }
    // Audio-changing `/v1/speak` params: `speed` and `bit_rate` alter the synthesized bytes, so
    // they MUST participate in the cache key (prior review's collision bug class). `callback` /
    // `callback_method` are delivery-only and intentionally excluded (they don't change audio).
    s.push('|');
    if let Some(speed) = speak.speed {
        s.push_str(&format!("{speed:.3}"));
    }
    s.push('|');
    if let Some(bit_rate) = speak.bit_rate {
        s.push_str(&bit_rate.to_string());
    }
    let hash = xxh3_128(s.as_bytes());
    format!("{hash:032x}")
}

/// Deepgram TTS provider implementation using the Deepgram HTTP REST API
pub struct DeepgramTTS {
    /// Generic HTTP-based TTS provider
    provider: TTSProvider,
    /// Request builder
    request_builder: DeepgramRequestBuilder,
    /// Precomputed config hash for caching
    config_hash: String,
}

impl DeepgramTTS {
    /// Create a new Deepgram TTS instance
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        Self::new_with_speak(config, DeepgramSpeakParams::default())
    }

    /// Create a new Deepgram TTS instance with advanced `/v1/speak` query parameters.
    fn new_with_speak(config: TTSConfig, speak: DeepgramSpeakParams) -> TTSResult<Self> {
        Ok(Self::build_with_speak(config, speak))
    }

    fn build_with_speak(config: TTSConfig, speak: DeepgramSpeakParams) -> Self {
        let pronunciation_replacer = if !config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&config.pronunciations))
        } else {
            None
        };
        let hash = compute_tts_config_hash(&config, &speak);
        let request_builder = DeepgramRequestBuilder {
            config: config.clone(),
            pronunciation_replacer,
            speak,
        };
        Self {
            provider: TTSProvider::new(),
            request_builder,
            config_hash: hash,
        }
    }

    /// Build from the standardized config (W1 keystone). Deepgram's flat `TTSConfig` is the
    /// constructor input, so this maps the standardized features onto it (plus the advanced
    /// `/v1/speak` query params from `ProviderExtras`) before delegating to the constructor.
    ///
    /// Wired `/v1/speak` params:
    /// - `sample_rate` (typed) → `sample_rate=` (output rate)
    /// - `speed` (typed)       → `speed=` (speaking rate, Deepgram range 0.7–1.5; audio-changing)
    /// - `bit_rate` (extras)   → `bit_rate=` (output bitrate; audio-changing)
    /// - `callback` (extras)   → `callback=` (async result callback URL; delivery-only)
    /// - `callback_method` (extras) → `callback_method=` (POST|PUT; delivery-only)
    ///
    /// Pitch/volume, emotion, instructions, SSML, voice settings, word timestamps, streaming, seed
    /// and language have no Deepgram `/v1/speak` parameter and are skipped (capability gaps).
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let f = &std.features;
        let mut base = std.base.clone();
        if let Some(sr) = f.sample_rate {
            base.sample_rate = Some(sr);
        }

        if let Some(endpoint) = std.endpoint_override() {
            validate_deepgram_tts_endpoint("endpoint_override", endpoint)?;
        }

        let extras = &std.extras.0;
        let callback = match extras.get("callback").and_then(|v| v.as_str()) {
            Some(callback) => {
                Some(validate_deepgram_callback_url("callback", callback).map(str::to_string)?)
            }
            None => None,
        };
        let callback_method = match extras.get("callback_method").and_then(|v| v.as_str()) {
            Some(method) => Some(normalize_deepgram_callback_method(method)?),
            None => None,
        };
        let speak = DeepgramSpeakParams {
            // Typed speed → mirror onto `speaking_rate` (so the flat config stays consistent) and
            // emit `speed=` on the wire.
            speed: f.speed,
            bit_rate: extras
                .get("bit_rate")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32),
            callback,
            callback_method,
            endpoint_override: std.endpoint_override().map(String::from),
        };
        if let Some(speed) = f.speed {
            base.speaking_rate = Some(speed);
        }

        Self::new_with_speak(base, speak)
    }

    /// Set the request manager for this instance
    pub async fn set_req_manager(&mut self, req_manager: Arc<ReqManager>) {
        self.provider.set_req_manager(req_manager).await;
    }
}

impl Default for DeepgramTTS {
    fn default() -> Self {
        Self::build_with_speak(TTSConfig::default(), DeepgramSpeakParams::default())
    }
}

#[async_trait]
impl BaseTTS for DeepgramTTS {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        DeepgramTTS::new(config)
    }

    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        self.provider
            .generic_connect_with_config(DEEPGRAM_TTS_URL, &self.request_builder.config)
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
            tracing::info!("Deepgram TTS not ready, attempting to connect...");
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
            "provider": "deepgram",
            "version": "2.0.0",
            "api_type": "HTTP REST",
            "connection_pooling": true,
            "supported_formats": ["mp3", "wav", "pcm", "aac", "flac", "opus"],
            "supported_sample_rates": [8000, 16000, 22050, 24000, 44100, 48000],
            "supported_models": [
                "aura-asteria-en",
                "aura-luna-en",
                "aura-stella-en",
                "aura-athena-en",
                "aura-hera-en",
                "aura-orion-en",
                "aura-arcas-en",
                "aura-perseus-en",
                "aura-angus-en",
                "aura-orpheus-en",
                "aura-helios-en",
                "aura-zeus-en"
            ],
            "endpoint": DEEPGRAM_TTS_URL,
            "documentation": "https://developers.deepgram.com/reference/text-to-speech-api",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_deepgram_tts_creation() {
        let config = TTSConfig::default();
        let tts = DeepgramTTS::new(config).unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn deepgram_default_is_disconnected_and_does_not_depend_on_result_unwrap() {
        let tts = DeepgramTTS::default();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    // W1 keystone: Deepgram's narrow `/v1/speak` surface only expresses `sample_rate`; the
    // standardized feature reaches the flat config the request builder reads. Other features are
    // capability gaps (no Deepgram parameter) and are intentionally skipped.
    #[tokio::test]
    async fn from_standard_maps_sample_rate() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "deepgram".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                sample_rate: Some(48000),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = DeepgramTTS::from_standard(&std).unwrap();
        assert_eq!(tts.request_builder.config.sample_rate, Some(48000));
        // base carried through.
        assert_eq!(tts.request_builder.config.api_key, "k");
    }

    #[tokio::test]
    async fn from_standard_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mk = |endpoint: &str| {
            crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
                provider: "deepgram".into(),
                api_key: "k".into(),
                voice_id: Some("aura-asteria-en".into()),
                ..Default::default()
            })
            .with_endpoint_override(endpoint)
        };

        assert!(DeepgramTTS::from_standard(&mk("https://deepgram-proxy.example.com")).is_ok());

        let err = match DeepgramTTS::from_standard(&mk("http://127.0.0.1:9000")) {
            Ok(_) => panic!("loopback endpoint override should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("SSRF protection"),
            "unexpected error: {err}"
        );

        let err = match DeepgramTTS::from_standard(&mk("file:///tmp/socket")) {
            Ok(_) => panic!("file endpoint override should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("URL scheme"),
            "unexpected error: {err}"
        );

        let err = match DeepgramTTS::from_standard(&mk("ws://deepgram-proxy.example.com")) {
            Ok(_) => panic!("websocket endpoint override should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("URL scheme"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn test_http_request_building() {
        let config = TTSConfig {
            voice_id: Some("aura-asteria-en".to_string()),
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(24000),
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        let builder = DeepgramRequestBuilder {
            config,
            pronunciation_replacer: None,
            speak: DeepgramSpeakParams::default(),
        };
        let client = reqwest::Client::new();
        let request = builder.build_http_request(&client, "Test text");

        // Get the request as built
        let built_request = request.build().unwrap();
        let url = built_request.url().to_string();

        assert!(url.contains("model=aura-asteria-en"));
        assert!(url.contains("encoding=mp3"));
        assert!(url.contains("sample_rate=24000"));
        assert!(url.starts_with(DEEPGRAM_TTS_URL));
        // With default (empty) speak params, none of the advanced params appear.
        assert!(!url.contains("speed="));
        assert!(!url.contains("bit_rate="));
        assert!(!url.contains("callback"));
    }

    // WIRE-LEVEL: the standardized speed/bit_rate/callback/callback_method features must reach the
    // built `/v1/speak` request URL — not merely the config struct (the recurring "set but never
    // serialized" bug class). We build the real reqwest request and inspect its URL.
    #[tokio::test]
    async fn from_standard_features_reach_speak_url() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("bit_rate".into(), serde_json::json!(48000));
        extras.insert(
            "callback".into(),
            serde_json::json!("https://example.com/cb?x=1&y=two words"),
        );
        extras.insert("callback_method".into(), serde_json::json!("put"));

        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "deepgram".into(),
                api_key: "k".into(),
                voice_id: Some("aura-2-thalia-en".into()),
                audio_format: Some("mp3".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(0.9),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };

        let tts = DeepgramTTS::from_standard(&std).unwrap();
        let client = reqwest::Client::new();
        let url = tts
            .request_builder
            .build_http_request(&client, "hi")
            .build()
            .unwrap()
            .url()
            .to_string();

        assert!(url.contains("speed=0.9"), "speed missing from URL: {url}");
        assert!(
            url.contains("bit_rate=48000"),
            "bit_rate missing from URL: {url}"
        );
        let parsed = url::Url::parse(&url).unwrap();
        let callback = parsed
            .query_pairs()
            .find(|(key, _)| key == "callback")
            .map(|(_, value)| value.into_owned());
        assert_eq!(
            callback.as_deref(),
            Some("https://example.com/cb?x=1&y=two words"),
            "callback missing or corrupted in URL: {url}"
        );
        assert!(
            url.contains("callback=https%3A%2F%2F"),
            "callback URL was not form-encoded: {url}"
        );
        assert!(
            url.contains("callback_method=PUT"),
            "callback_method missing from URL: {url}"
        );
    }

    #[tokio::test]
    async fn from_standard_rejects_ssrf_callback_url() {
        let _env = crate::core::net::ssrf_env_lock();
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig};

        let mk = |callback: &str| {
            let mut extras = serde_json::Map::new();
            extras.insert("callback".into(), serde_json::json!(callback));
            StandardTTSConfig {
                base: TTSConfig {
                    provider: "deepgram".into(),
                    api_key: "k".into(),
                    voice_id: Some("aura-2-thalia-en".into()),
                    ..Default::default()
                },
                features: Default::default(),
                extras: ProviderExtras(extras),
            }
        };

        assert!(DeepgramTTS::from_standard(&mk("https://example.com/cb")).is_ok());

        let err = match DeepgramTTS::from_standard(&mk("http://127.0.0.1:9000/cb")) {
            Ok(_) => panic!("loopback callback URL should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("SSRF protection"),
            "unexpected error: {err}"
        );

        let err = match DeepgramTTS::from_standard(&mk("file:///tmp/cb")) {
            Ok(_) => panic!("file callback URL should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("URL scheme"),
            "unexpected error: {err}"
        );

        let err = match DeepgramTTS::from_standard(&mk("   ")) {
            Ok(_) => panic!("blank callback URL should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("empty URL"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn from_standard_rejects_invalid_callback_method() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig};

        let mk = |method: &str| {
            let mut extras = serde_json::Map::new();
            extras.insert("callback_method".into(), serde_json::json!(method));
            StandardTTSConfig {
                base: TTSConfig {
                    provider: "deepgram".into(),
                    api_key: "k".into(),
                    voice_id: Some("aura-2-thalia-en".into()),
                    ..Default::default()
                },
                features: Default::default(),
                extras: ProviderExtras(extras),
            }
        };

        assert!(DeepgramTTS::from_standard(&mk("post")).is_ok());

        let err = match DeepgramTTS::from_standard(&mk("DELETE")) {
            Ok(_) => panic!("unsupported callback_method should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("expected POST or PUT"),
            "unexpected error: {err}"
        );

        let err = match DeepgramTTS::from_standard(&mk("   ")) {
            Ok(_) => panic!("blank callback_method should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("empty method"),
            "unexpected error: {err}"
        );
    }

    // CACHE-KEY: audio-changing params (`speed`, `bit_rate`) must change the cache hash to avoid
    // serving stale/colliding audio (prior review's collision bug class). Delivery-only params
    // (`callback`, `callback_method`) must NOT change the hash.
    #[tokio::test]
    async fn speed_and_bit_rate_change_cache_key_but_callback_does_not() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};

        let base = || TTSConfig {
            provider: "deepgram".into(),
            api_key: "k".into(),
            voice_id: Some("aura-2-thalia-en".into()),
            ..Default::default()
        };
        let mk = |features: TtsFeatures, extras: serde_json::Map<String, serde_json::Value>| {
            DeepgramTTS::from_standard(&StandardTTSConfig {
                base: base(),
                features,
                extras: ProviderExtras(extras),
            })
            .unwrap()
            .config_hash
        };

        let baseline = mk(TtsFeatures::default(), serde_json::Map::new());

        // speed changes the hash.
        let with_speed = mk(
            TtsFeatures {
                speed: Some(1.2),
                ..Default::default()
            },
            serde_json::Map::new(),
        );
        assert_ne!(baseline, with_speed, "speed must change the cache key");

        // bit_rate changes the hash.
        let mut br = serde_json::Map::new();
        br.insert("bit_rate".into(), serde_json::json!(32000));
        let with_bitrate = mk(TtsFeatures::default(), br);
        assert_ne!(baseline, with_bitrate, "bit_rate must change the cache key");

        // callback / callback_method must NOT change the hash (delivery-only).
        let mut cb = serde_json::Map::new();
        cb.insert(
            "callback".into(),
            serde_json::json!("https://example.com/cb"),
        );
        cb.insert("callback_method".into(), serde_json::json!("POST"));
        let with_callback = mk(TtsFeatures::default(), cb);
        assert_eq!(
            baseline, with_callback,
            "callback must NOT change the cache key (delivery-only)"
        );
    }
}
