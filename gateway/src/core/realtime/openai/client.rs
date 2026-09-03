//! OpenAI Realtime API client — a thin newtype over the shared S2S scaffold.
//!
//! All the provider-specific wire mapping (session config, server-event lowering,
//! audio base64, truncate, replay) lives in
//! [`OpenAiProtocol`](super::protocol::OpenAiProtocol); all the shared machinery
//! (reconnect supervisor + backoff + breaker/governor storm control, barge-in /
//! truncate / preroll, conversation replay, callback dispatch) lives in the
//! generic [`RealtimeSession`](crate::core::realtime::scaffold::RealtimeSession)
//! driver. `OpenAIRealtime` is therefore just
//! `RealtimeSession<OpenAiProtocol>` plus the OpenAI-specific inherent accessors
//! and the rich `get_provider_info()` JSON the public API guarantees.
//!
//! # API Reference
//!
//! - Endpoint: `wss://api.openai.com/v1/realtime?model=<model>`
//! - Protocol: WebSocket with JSON events (GA `gpt-realtime`)
//! - Audio: PCM 16-bit, 24kHz, mono, little-endian, base64 encoded
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::realtime::{BaseRealtime, RealtimeConfig, OpenAIRealtime};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = RealtimeConfig {
//!         api_key: "sk-...".to_string(),
//!         model: "gpt-realtime".to_string(),
//!         voice: Some("alloy".to_string()),
//!         ..Default::default()
//!     };
//!
//!     let mut realtime = OpenAIRealtime::new(config).unwrap();
//!     realtime.connect().await.unwrap();
//!
//!     realtime.on_transcript(Arc::new(|t| Box::pin(async move {
//!         println!("{}: {}", t.role, t.text);
//!     }))).unwrap();
//!
//!     realtime.send_audio(audio_bytes).await.unwrap();
//! }
//! ```

use async_trait::async_trait;
use bytes::Bytes;

use super::config::{
    OPENAI_REALTIME_URL, OpenAIRealtimeAudioFormat, OpenAIRealtimeModel, OpenAIRealtimeVoice,
};
use super::messages::SessionConfig;
use super::protocol::OpenAiProtocol;
use crate::core::realtime::base::{
    AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback, RealtimeConfig,
    RealtimeErrorCallback, RealtimeResponseOverride, RealtimeResult, ReconnectionCallback,
    ReplayConversationItem, ResponseDoneCallback, SpeechEventCallback, TranscriptCallback,
};
use crate::core::realtime::scaffold::RealtimeSession;

// =============================================================================
// OpenAI Realtime Client
// =============================================================================

/// OpenAI Realtime API client.
///
/// A newtype over the generic [`RealtimeSession`] driver parameterized by
/// [`OpenAiProtocol`]. Every [`BaseRealtime`] method delegates to the driver; the
/// inherent accessors (`model`/`voice`/`audio_format`/`session_id`/`build_ws_url`/
/// `build_session_config`) and the rich `get_provider_info()` are preserved for
/// the existing public API + tests.
pub struct OpenAIRealtime(RealtimeSession<OpenAiProtocol>);

impl OpenAIRealtime {
    /// The shared circuit breaker this session feeds, if injected (for metrics/tests).
    pub fn resilience_breaker(
        &self,
    ) -> Option<&std::sync::Arc<crate::core::resilience::CircuitBreaker>> {
        self.0.resilience_breaker()
    }

    /// Get the configured model.
    pub fn model(&self) -> OpenAIRealtimeModel {
        self.0.protocol().model()
    }

    /// Get the configured voice.
    pub fn voice(&self) -> OpenAIRealtimeVoice {
        self.0.protocol().voice()
    }

    /// Get the configured audio format.
    pub fn audio_format(&self) -> OpenAIRealtimeAudioFormat {
        self.0.protocol().audio_format()
    }

    /// Get the session ID if connected.
    pub async fn session_id(&self) -> Option<String> {
        self.0.session_id().await
    }

    /// Build the WebSocket URL with model parameter (delegates to the protocol's
    /// connect spec; retained as an inherent method for the existing wire test).
    fn build_ws_url(&self) -> String {
        format!("{}?model={}", OPENAI_REALTIME_URL, self.model().as_str())
    }

    /// Build the initial session configuration (delegates to the protocol;
    /// retained as an inherent method for the existing GA wire-shape tests).
    fn build_session_config(&self) -> SessionConfig {
        self.0.protocol().session_config(self.0.config())
    }
}

#[async_trait]
impl BaseRealtime for OpenAIRealtime {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self> {
        Ok(Self(RealtimeSession::<OpenAiProtocol>::new(config)?))
    }

    async fn connect(&mut self) -> RealtimeResult<()> {
        self.0.connect().await
    }

    async fn disconnect(&mut self) -> RealtimeResult<()> {
        self.0.disconnect().await
    }

    fn is_ready(&self) -> bool {
        self.0.is_ready()
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.0.get_connection_state()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> RealtimeResult<()> {
        self.0.send_audio(audio_data).await
    }

    async fn send_text(&mut self, text: &str) -> RealtimeResult<()> {
        self.0.send_text(text).await
    }

    async fn create_response(&mut self) -> RealtimeResult<()> {
        self.0.create_response().await
    }

    async fn create_response_with(
        &mut self,
        overrides: RealtimeResponseOverride,
    ) -> RealtimeResult<()> {
        self.0.create_response_with(overrides).await
    }

    async fn cancel_response(&mut self) -> RealtimeResult<()> {
        self.0.cancel_response().await
    }

    async fn commit_audio_buffer(&mut self) -> RealtimeResult<()> {
        self.0.commit_audio_buffer().await
    }

    async fn clear_audio_buffer(&mut self) -> RealtimeResult<()> {
        self.0.clear_audio_buffer().await
    }

    fn on_transcript(&mut self, callback: TranscriptCallback) -> RealtimeResult<()> {
        self.0.on_transcript(callback)
    }

    fn on_audio(&mut self, callback: AudioOutputCallback) -> RealtimeResult<()> {
        self.0.on_audio(callback)
    }

    fn on_error(&mut self, callback: RealtimeErrorCallback) -> RealtimeResult<()> {
        self.0.on_error(callback)
    }

    fn on_function_call(&mut self, callback: FunctionCallCallback) -> RealtimeResult<()> {
        self.0.on_function_call(callback)
    }

    fn on_speech_event(&mut self, callback: SpeechEventCallback) -> RealtimeResult<()> {
        self.0.on_speech_event(callback)
    }

    fn on_response_done(&mut self, callback: ResponseDoneCallback) -> RealtimeResult<()> {
        self.0.on_response_done(callback)
    }

    fn on_reconnection(&mut self, callback: ReconnectionCallback) -> RealtimeResult<()> {
        self.0.on_reconnection(callback)
    }

    async fn update_session(&mut self, config: RealtimeConfig) -> RealtimeResult<()> {
        self.0.update_session(config).await
    }

    async fn submit_function_result(&mut self, call_id: &str, result: &str) -> RealtimeResult<()> {
        self.0.submit_function_result(call_id, result).await
    }

    /// Rich provider info (preserved verbatim — the public-API + integration-test
    /// contract asserts this exact shape; the generic driver's minimal info is
    /// intentionally overridden here).
    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "openai",
            "api_type": "WebSocket Realtime",
            "version": "1.0.0",
            "endpoint": OPENAI_REALTIME_URL,
            "supported_models": [
                "gpt-4o-realtime-preview",
                "gpt-4o-realtime-preview-2024-10-01",
                "gpt-4o-realtime-preview-2024-12-17",
                "gpt-4o-mini-realtime-preview",
                "gpt-4o-mini-realtime-preview-2024-12-17"
            ],
            "supported_voices": [
                "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"
            ],
            "supported_audio_formats": [
                "pcm16", "g711_ulaw", "g711_alaw"
            ],
            "default_sample_rate": 24000,
            "features": {
                "bidirectional_audio": true,
                "vad": true,
                "function_calling": true,
                "text_and_audio": true,
                "transcription": true
            },
            "documentation": "https://platform.openai.com/docs/guides/realtime"
        })
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        self.0.set_resilience(resilience)
    }

    fn emits_user_turn_frames(&self) -> bool {
        self.0.emits_user_turn_frames()
    }

    async fn truncate_response(&mut self, item_id: &str, audio_end_ms: u64) -> RealtimeResult<()> {
        self.0.truncate_response(item_id, audio_end_ms).await
    }

    async fn truncate_current_response(&mut self) -> RealtimeResult<Option<(String, u64)>> {
        self.0.truncate_current_response().await
    }

    async fn replay_user_audio_preroll(&mut self) -> RealtimeResult<()> {
        self.0.replay_user_audio_preroll().await
    }

    async fn replay_conversation(
        &mut self,
        items: &[ReplayConversationItem],
    ) -> RealtimeResult<()> {
        self.0.replay_conversation(items).await
    }
}

impl Default for OpenAIRealtime {
    fn default() -> Self {
        // `RealtimeConfig::default()` has an empty api_key → `new` errors; fall
        // back to a never-connectable session built from a dummy key so the
        // accessors (model/voice/provider_info) still work, matching the old
        // Default which built an unusable-but-introspectable client.
        Self::new(RealtimeConfig::default()).unwrap_or_else(|_| {
            Self::new(RealtimeConfig {
                api_key: "placeholder".to_string(),
                ..Default::default()
            })
            .expect("placeholder config builds")
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::realtime::base::{RealtimeError, ReconnectionConfig};

    #[tokio::test]
    async fn emits_user_turn_frames_tracks_server_vad_default() {
        use crate::core::realtime::TurnDetectionConfig;
        let mk = |td: Option<TurnDetectionConfig>| {
            OpenAIRealtime::new(RealtimeConfig {
                provider: "openai".into(),
                api_key: "k".into(),
                model: "gpt-4o-realtime-preview".into(),
                turn_detection: td,
                ..Default::default()
            })
            .unwrap()
        };
        // Explicit server VAD → server produces turn frames.
        assert!(mk(Some(TurnDetectionConfig::default())).emits_user_turn_frames());
        // review wf_d43814c3 #7: OMITTED turn_detection is serialized away, so
        // OpenAI keeps its SERVER-VAD DEFAULT (on) — frames still come from the
        // server.
        assert!(
            mk(None).emits_user_turn_frames(),
            "omitted turn_detection ⇒ OpenAI server-VAD default is ON"
        );
        // ONLY the explicit manual variant flips it off.
        assert!(
            !mk(Some(TurnDetectionConfig::None)).emits_user_turn_frames(),
            "explicit None (manual) ⇒ the gateway drives turns"
        );
    }

    #[test]
    fn ga_build_session_config_shape_omits_temperature_and_reasoning() {
        use crate::core::llm::ReasoningEffort;
        // GA `gpt-realtime` has no session-level temperature/reasoning. Even with
        // both set on the config, build_session_config must emit the GA NESTED
        // shape and leak NEITHER field (either 400s session.update).
        let c = OpenAIRealtime::new(RealtimeConfig {
            provider: "openai".into(),
            api_key: "k".into(),
            model: "gpt-realtime".into(),
            temperature: Some(0.7),
            reasoning_effort: Some(ReasoningEffort::Low),
            ..Default::default()
        })
        .unwrap();
        let sc = c.build_session_config();
        assert_eq!(sc.session_type, "realtime", "GA requires session.type");
        assert_eq!(
            sc.output_modalities,
            Some(vec!["audio".to_string()]),
            "GA renamed modalities ⇒ output_modalities"
        );
        let audio = sc.audio.as_ref().expect("GA nests audio.input/output");
        assert!(
            audio.output.as_ref().unwrap().voice.is_some(),
            "voice nests under audio.output"
        );
        assert_eq!(
            audio
                .input
                .as_ref()
                .unwrap()
                .format
                .as_ref()
                .unwrap()
                .format_type,
            "audio/pcm",
            "PCM16 ⇒ {{type: audio/pcm, rate: 24000}}"
        );
        let json = serde_json::to_value(&sc).unwrap();
        assert!(
            json.get("temperature").is_none(),
            "GA: no session temperature"
        );
        assert!(json.get("reasoning").is_none(), "GA: no session reasoning");
    }

    #[test]
    fn audio_format_bytes_per_ms_matches_rate() {
        // review wf_d43814c3 #6: telephony g711 is 1 byte/sample @8kHz = 8
        // B/ms; hardcoding PCM16's 48 over-truncated 6×.
        assert_eq!(OpenAIRealtimeAudioFormat::Pcm16.bytes_per_ms(), 48);
        assert_eq!(OpenAIRealtimeAudioFormat::G711Ulaw.bytes_per_ms(), 8);
        assert_eq!(OpenAIRealtimeAudioFormat::G711Alaw.bytes_per_ms(), 8);
        // 200ms of g711 = 1600 bytes (not 1600/48 ≈ 33ms).
        assert_eq!(
            1600 / OpenAIRealtimeAudioFormat::G711Ulaw.bytes_per_ms(),
            200
        );
    }

    #[tokio::test]
    async fn test_openai_realtime_creation() {
        let config = RealtimeConfig {
            api_key: "test_key".to_string(),
            model: "gpt-4o-realtime-preview".to_string(),
            voice: Some("shimmer".to_string()),
            ..Default::default()
        };

        let realtime = OpenAIRealtime::new(config).unwrap();
        assert!(!realtime.is_ready());
        assert_eq!(
            realtime.get_connection_state(),
            ConnectionState::Disconnected
        );
        assert_eq!(realtime.model(), OpenAIRealtimeModel::Gpt4oRealtimePreview);
        assert_eq!(realtime.voice(), OpenAIRealtimeVoice::Shimmer);
    }

    #[tokio::test]
    async fn test_api_key_required() {
        let config = RealtimeConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = OpenAIRealtime::new(config);
        assert!(result.is_err());
        match result {
            Err(RealtimeError::AuthenticationFailed(_)) => {}
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_send_audio_requires_connection() {
        let config = RealtimeConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        let mut realtime = OpenAIRealtime::new(config).unwrap();
        let result = realtime.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_err());
        match result {
            Err(RealtimeError::NotConnected) => {}
            _ => panic!("Expected NotConnected error"),
        }
    }

    #[test]
    fn test_provider_info() {
        let realtime = OpenAIRealtime::default();
        let info = realtime.get_provider_info();

        assert_eq!(info["provider"], "openai");
        assert_eq!(info["api_type"], "WebSocket Realtime");
        assert!(info["features"]["bidirectional_audio"].as_bool().unwrap());
        assert!(info["features"]["vad"].as_bool().unwrap());
    }

    #[test]
    fn test_build_ws_url() {
        let config = RealtimeConfig {
            api_key: "test".to_string(),
            model: "gpt-4o-realtime-preview".to_string(),
            ..Default::default()
        };

        let realtime = OpenAIRealtime::new(config).unwrap();
        let url = realtime.build_ws_url();
        assert!(url.contains("wss://api.openai.com"));
        assert!(url.contains("gpt-4o-realtime-preview"));
    }

    #[test]
    fn test_default_reconnection_config() {
        let config = RealtimeConfig {
            api_key: "test".to_string(),
            ..Default::default()
        };

        let realtime = OpenAIRealtime::new(config).unwrap();

        // Default reconnection should be enabled.
        let cfg = realtime.0.config().reconnection.clone().unwrap_or_default();
        assert!(cfg.enabled);
        assert_eq!(cfg.max_attempts, 5);
    }

    #[test]
    fn test_custom_reconnection_config() {
        let config = RealtimeConfig {
            api_key: "test".to_string(),
            reconnection: Some(ReconnectionConfig {
                enabled: true,
                max_attempts: 10,
                initial_delay_ms: 500,
                max_delay_ms: 60000,
                backoff_multiplier: 1.5,
                jitter: false,
            }),
            ..Default::default()
        };

        let realtime = OpenAIRealtime::new(config).unwrap();

        let cfg = realtime.0.config().reconnection.clone().unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.max_attempts, 10);
        assert_eq!(cfg.initial_delay_ms, 500);
        assert_eq!(cfg.max_delay_ms, 60000);
        assert_eq!(cfg.backoff_multiplier, 1.5);
        assert!(!cfg.jitter);
    }

    #[test]
    fn test_reconnection_disabled() {
        let config = RealtimeConfig {
            api_key: "test".to_string(),
            reconnection: Some(ReconnectionConfig::disabled()),
            ..Default::default()
        };

        let realtime = OpenAIRealtime::new(config).unwrap();

        let cfg = realtime.0.config().reconnection.clone().unwrap();
        assert!(!cfg.enabled);
        assert!(!cfg.should_retry(0));
    }
}
