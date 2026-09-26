//! # Deepgram Aura Streaming TTS (WebSocket) — P1.1
//!
//! [`DeepgramAuraTTS`] implements [`BaseTTS`] over the generic
//! [`WebSocketTtsClient`], speaking Deepgram's `/v1/speak` WebSocket protocol:
//!
//! - **Connect**: `wss://api.deepgram.com/v1/speak?model=<voice>&encoding=<fmt>`
//!   `&sample_rate=<rate>&container=none` with `Authorization: Token <api_key>`
//!   (the same auth header convention as the Deepgram STT WS and TTS REST paths).
//! - **Send** (JSON text frames): `{"type":"Speak","text":…}`, `{"type":"Flush"}`,
//!   `{"type":"Clear"}`, `{"type":"Close"}`.
//! - **Receive**: binary audio frames plus JSON `{"type":"Flushed","sequence_id":…}`,
//!   `{"type":"Cleared"}`, `{"type":"Metadata",…}`, `{"type":"Warning",…}`.
//!
//! `speak(text, flush=false)` buffers client-side; `flush=true` sends ONE `Speak`
//! (buffer + text) followed by `Flush` — sentence-boundary flushing preserves Aura's
//! prosody. `clear()` cancels all in-flight synthesis (barge-in actually stops audio).
//!
//! Selected by `create_tts_standard("deepgram", …)` when
//! `features.streaming == Some(true)`; the HTTP [`DeepgramTTS`] remains the default.
//!
//! An `endpoint_override` from the standardized config extras is honored for the
//! credential-free mock harness, but only after passing the DAG's SSRF rules
//! (loopback requires `WAAV_ALLOW_LOOPBACK_ENDPOINTS=1`) — enforced inside
//! [`WebSocketTtsClient::connect`].
//!
//! [`DeepgramTTS`]: super::deepgram::DeepgramTTS

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use super::base::{AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSResult};
use super::websocket::{WebSocketTtsClient, WsTtsConnectSpec, WsTtsEvent, WsTtsProtocol};

/// Deepgram streaming TTS (Aura) WebSocket endpoint.
pub const DEEPGRAM_TTS_WS_URL: &str = "wss://api.deepgram.com/v1/speak";

/// Default raw-PCM encoding (matches the HTTP request builder).
const DEFAULT_ENCODING: &str = "linear16";

/// Deepgram `/v1/speak` WS wire protocol for the generic client.
struct DeepgramAuraProtocol {
    sample_rate: u32,
    audio_format: String,
}

impl WsTtsProtocol for DeepgramAuraProtocol {
    fn provider_name(&self) -> &'static str {
        "deepgram"
    }

    fn speak_frame(&self, text: &str) -> String {
        json!({"type": "Speak", "text": text}).to_string()
    }

    fn flush_frame(&self) -> Option<String> {
        Some(json!({"type": "Flush"}).to_string())
    }

    fn clear_frame(&self) -> Option<String> {
        Some(json!({"type": "Clear"}).to_string())
    }

    fn close_frame(&self) -> Option<String> {
        Some(json!({"type": "Close"}).to_string())
    }

    fn classify_text_frame(&self, raw: &str) -> WsTtsEvent {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            return WsTtsEvent::Ignored;
        };
        match value.get("type").and_then(|t| t.as_str()) {
            Some("Flushed") => {
                tracing::debug!(
                    sequence_id = value.get("sequence_id").and_then(|s| s.as_i64()),
                    "Deepgram Aura Flushed"
                );
                WsTtsEvent::Flushed
            }
            Some("Cleared") => WsTtsEvent::Cleared,
            Some("Metadata") => WsTtsEvent::Metadata,
            Some("Warning") => WsTtsEvent::Warning(
                value
                    .get("description")
                    .or_else(|| value.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or(raw)
                    .to_string(),
            ),
            Some("Error") => WsTtsEvent::Error(
                value
                    .get("description")
                    .or_else(|| value.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or(raw)
                    .to_string(),
            ),
            // Frames carrying an error payload without a recognized type.
            None if value.get("error").is_some() => WsTtsEvent::Error(value["error"].to_string()),
            _ => WsTtsEvent::Ignored,
        }
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn audio_format(&self) -> String {
        self.audio_format.clone()
    }
}

/// The Aura model to request — the same answer as the HTTP request builder: the voice, else
/// `model`, else none (Deepgram then applies its own default).
fn effective_model(config: &TTSConfig) -> Option<&str> {
    super::deepgram::deepgram_model(config)
}

/// Deepgram's name for the requested encoding. The socket streams bare frames, so a WAV request
/// is linear16 here, and WaaV's `pcm`/`ulaw` become Deepgram's `linear16`/`mulaw` — it refuses
/// the others.
fn ws_encoding(config: &TTSConfig) -> &str {
    let requested = config.audio_format.as_deref().unwrap_or(DEFAULT_ENCODING);
    super::deepgram::deepgram_encoding_and_container(requested).0
}

/// The rate the audio arrives at: the one requested, else Deepgram's default for the encoding.
fn output_sample_rate(config: &TTSConfig) -> u32 {
    config
        .sample_rate
        .unwrap_or_else(|| super::deepgram::deepgram_default_sample_rate(ws_encoding(config)))
}

/// Normalize an endpoint override base for a WS dial: `http(s)://` → `ws(s)://`.
fn normalize_ws_override(base: &str) -> String {
    let trimmed = base.trim();
    if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        trimmed.to_string()
    }
}

/// Build the full `/v1/speak` WS URL with query parameters, honoring an override base
/// (scheme+host swap, path kept — same mechanics as the REST override helper).
fn build_ws_url(config: &TTSConfig, endpoint_override: Option<&str>) -> String {
    let normalized = endpoint_override.map(normalize_ws_override);
    let mut url = crate::core::tts::standard::override_rest_endpoint(
        DEEPGRAM_TTS_WS_URL,
        normalized.as_deref(),
    );

    let mut params = Vec::new();
    if let Some(model) = effective_model(config) {
        params.push(format!("model={model}"));
    }

    let encoding = ws_encoding(config);
    params.push(format!("encoding={encoding}"));

    // Only a rate someone chose. It is optional, and the fixed 24000 that stood here is
    // invalid for mulaw/alaw (8000 or 16000 only).
    if let Some(rate) = config.sample_rate {
        params.push(format!("sample_rate={rate}"));
    }

    // Raw PCM-family encodings must not be wrapped in a container (WS delivers raw
    // binary frames) — mirrors the HTTP builder's container handling.
    if matches!(encoding, "linear16" | "mulaw" | "alaw") {
        params.push("container=none".to_string());
    }

    url.push('?');
    url.push_str(&params.join("&"));
    url
}

/// Deepgram Aura streaming TTS over WebSocket (see module docs).
pub struct DeepgramAuraTTS {
    client: WebSocketTtsClient,
    config: TTSConfig,
    ws_url: String,
}

impl DeepgramAuraTTS {
    /// Create a disconnected Aura WS provider from the flat config.
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        Self::new_with_override(config, None)
    }

    /// Create with an optional client-supplied endpoint override (SSRF-validated at
    /// connect time by the generic client).
    fn new_with_override(config: TTSConfig, endpoint_override: Option<&str>) -> TTSResult<Self> {
        let ws_url = build_ws_url(&config, endpoint_override);
        let protocol: Arc<dyn WsTtsProtocol> = Arc::new(DeepgramAuraProtocol {
            sample_rate: output_sample_rate(&config),
            audio_format: ws_encoding(&config).to_string(),
        });
        let spec = WsTtsConnectSpec {
            url: ws_url.clone(),
            headers: vec![(
                // Repo-wide Deepgram auth convention (STT WS + TTS REST): Token <key>.
                "Authorization".to_string(),
                format!("Token {}", config.api_key),
            )],
            url_is_override: endpoint_override.is_some(),
            connect_timeout: config.connection_timeout.map(Duration::from_secs),
        };
        Ok(Self {
            client: WebSocketTtsClient::new(protocol, spec),
            config,
            ws_url,
        })
    }

    /// Build from the standardized config (the dispatch path used by
    /// `create_tts_standard("deepgram", …)` when `features.streaming == Some(true)`).
    ///
    /// Mapped features: `sample_rate` (output rate). The `endpoint_override` extra is
    /// honored for the mock harness but must pass the DAG SSRF rules at connect.
    /// Other features have no Aura WS parameter and stay capability gaps.
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let mut base = std.base.clone();
        if let Some(sr) = std.features.sample_rate {
            base.sample_rate = Some(sr);
        }
        Self::new_with_override(base, std.endpoint_override())
    }
}

#[async_trait]
impl BaseTTS for DeepgramAuraTTS {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        DeepgramAuraTTS::new(config)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        self.client.connect().await
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.client.disconnect().await
    }

    fn is_ready(&self) -> bool {
        self.client.is_ready()
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.client.connection_state()
    }

    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        self.speak_with_context(text, flush, None).await
    }

    async fn speak_with_context(
        &mut self,
        text: &str,
        flush: bool,
        context_id: Option<&str>,
    ) -> TTSResult<()> {
        // Lazy (re)connect mirrors the HTTP provider's speak-time reconnect: after a
        // socket failure the next speak dials fresh (full supervision is a follow-up).
        if !self.client.is_ready() {
            tracing::info!("Deepgram Aura TTS not ready, attempting to connect...");
            self.client.connect().await?;
        }
        self.client
            .speak_with_context(text, flush, context_id)
            .await
    }

    async fn clear(&mut self) -> TTSResult<()> {
        self.client.clear().await
    }

    async fn flush(&self) -> TTSResult<()> {
        self.client.flush_buffered().await
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.client.set_audio_callback(callback);
        Ok(())
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        self.client.remove_audio_callback();
        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "deepgram",
            "version": "1.0.0",
            "api_type": "WebSocket",
            "transport": "websocket",
            "streaming": true,
            "model": effective_model(&self.config),
            "encoding": ws_encoding(&self.config),
            "sample_rate": output_sample_rate(&self.config),
            "endpoint": self.ws_url,
            "in_flight": self.client.in_flight(),
            "last_ttfb_ms": self.client.last_ttfb_ns().map(|ns| ns / 1_000_000),
            "documentation": "https://developers.deepgram.com/docs/tts-websocket",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};

    fn cfg() -> TTSConfig {
        TTSConfig {
            provider: "deepgram".into(),
            api_key: "test-key".into(),
            voice_id: Some("aura-2-thalia-en".into()),
            audio_format: Some("linear16".into()),
            sample_rate: Some(24000),
            ..Default::default()
        }
    }

    #[test]
    fn ws_url_carries_model_encoding_rate_container() {
        let url = build_ws_url(&cfg(), None);
        assert!(url.starts_with(DEEPGRAM_TTS_WS_URL), "{url}");
        assert!(url.contains("model=aura-2-thalia-en"), "{url}");
        assert!(url.contains("encoding=linear16"), "{url}");
        assert!(url.contains("sample_rate=24000"), "{url}");
        assert!(url.contains("container=none"), "{url}");
    }

    /// Deepgram's voice id IS the model, so a named voice wins over a deployment's family name.
    /// The reverse order sent `model=aura-2` and Deepgram refused it.
    #[test]
    fn ws_url_voice_wins_over_a_family_model_and_containers_skip_none() {
        let mut config = cfg();
        config.model = "aura-2".into();
        config.audio_format = Some("mp3".into());
        let url = build_ws_url(&config, None);
        assert!(url.contains("model=aura-2-thalia-en"), "{url}");
        assert!(!url.contains("model=aura-2&"), "{url}");
        assert!(url.contains("encoding=mp3"), "{url}");
        assert!(!url.contains("container=none"), "{url}");
    }

    #[test]
    fn ws_url_uses_the_model_when_no_voice_is_named() {
        let mut config = cfg();
        config.voice_id = None;
        config.model = "aura-asteria-en".into();
        let url = build_ws_url(&config, None);
        assert!(url.contains("model=aura-asteria-en"), "{url}");
    }

    /// WaaV's `ulaw`/`pcm` go out under Deepgram's names, no rate is sent that nobody chose, and
    /// the audio is labelled with the rate Deepgram then produces (8000 for G.711, not 24000).
    #[test]
    fn ws_url_uses_deepgram_encodings_and_sends_no_unchosen_rate() {
        let mut config = cfg();
        config.audio_format = Some("ulaw".into());
        config.sample_rate = None;
        let url = build_ws_url(&config, None);
        assert!(url.contains("encoding=mulaw"), "{url}");
        assert!(!url.contains("sample_rate"), "{url}");
        assert_eq!(output_sample_rate(&config), 8000);

        config.audio_format = Some("pcm".into());
        let url = build_ws_url(&config, None);
        assert!(url.contains("encoding=linear16"), "{url}");
        assert_eq!(output_sample_rate(&config), 24000);
    }

    /// Nothing named: no `model` at all, so Deepgram applies its own default voice.
    #[test]
    fn ws_url_omits_model_when_nothing_is_named() {
        let mut config = cfg();
        config.voice_id = None;
        let url = build_ws_url(&config, None);
        assert!(!url.contains("model="), "{url}");
    }

    #[test]
    fn ws_url_override_swaps_host_keeps_path_and_normalizes_scheme() {
        let url = build_ws_url(&cfg(), Some("ws://127.0.0.1:9123"));
        assert!(url.starts_with("ws://127.0.0.1:9123/v1/speak?"), "{url}");
        // http(s) override bases normalize to ws(s) for the WS dial.
        let url = build_ws_url(&cfg(), Some("http://127.0.0.1:9123"));
        assert!(url.starts_with("ws://127.0.0.1:9123/v1/speak?"), "{url}");
        let url = build_ws_url(&cfg(), Some("https://mock.example.com"));
        assert!(url.starts_with("wss://mock.example.com/v1/speak?"), "{url}");
    }

    #[test]
    fn from_standard_maps_sample_rate_and_override() {
        let std = StandardTTSConfig {
            base: cfg(),
            features: TtsFeatures {
                streaming: Some(true),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: Default::default(),
        }
        .with_endpoint_override("ws://127.0.0.1:9999");
        let tts = DeepgramAuraTTS::from_standard(&std).unwrap();
        assert_eq!(tts.config.sample_rate, Some(16000));
        assert!(tts.ws_url.starts_with("ws://127.0.0.1:9999/v1/speak?"));
        assert!(tts.ws_url.contains("sample_rate=16000"));
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn provider_info_identifies_websocket_transport() {
        let tts = DeepgramAuraTTS::new(cfg()).unwrap();
        let info = tts.get_provider_info();
        assert_eq!(info["provider"], "deepgram");
        assert_eq!(info["api_type"], "WebSocket");
        assert_eq!(info["transport"], "websocket");
        assert_eq!(info["streaming"], true);
    }

    #[test]
    fn classify_maps_deepgram_event_frames() {
        let p = DeepgramAuraProtocol {
            sample_rate: 24000,
            audio_format: "linear16".into(),
        };
        assert_eq!(
            p.classify_text_frame(r#"{"type":"Flushed","sequence_id":3}"#),
            WsTtsEvent::Flushed
        );
        assert_eq!(
            p.classify_text_frame(r#"{"type":"Cleared"}"#),
            WsTtsEvent::Cleared
        );
        assert_eq!(
            p.classify_text_frame(r#"{"type":"Metadata","request_id":"r"}"#),
            WsTtsEvent::Metadata
        );
        assert!(matches!(
            p.classify_text_frame(r#"{"type":"Warning","description":"slow"}"#),
            WsTtsEvent::Warning(m) if m == "slow"
        ));
        assert!(matches!(
            p.classify_text_frame(r#"{"type":"Error","description":"bad"}"#),
            WsTtsEvent::Error(m) if m == "bad"
        ));
        assert_eq!(p.classify_text_frame("not json"), WsTtsEvent::Ignored);
        assert_eq!(
            p.classify_text_frame(r#"{"type":"SomethingNew"}"#),
            WsTtsEvent::Ignored
        );
    }

    #[test]
    fn speak_and_flush_frames_match_deepgram_protocol() {
        let p = DeepgramAuraProtocol {
            sample_rate: 24000,
            audio_format: "linear16".into(),
        };
        let speak: serde_json::Value = serde_json::from_str(&p.speak_frame("Hello world")).unwrap();
        assert_eq!(speak["type"], "Speak");
        assert_eq!(speak["text"], "Hello world");
        assert_eq!(p.flush_frame().unwrap(), r#"{"type":"Flush"}"#);
        assert_eq!(p.clear_frame().unwrap(), r#"{"type":"Clear"}"#);
        assert_eq!(p.close_frame().unwrap(), r#"{"type":"Close"}"#);
    }
}
