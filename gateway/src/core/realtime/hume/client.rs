//! Hume EVI WebSocket client — a thin newtype over the shared S2S scaffold.
//!
//! All the provider-specific wire mapping (EVI session settings, server-event
//! lowering, base64 audio input, the dual text+binary inbound path) lives in
//! [`HumeProtocol`](super::protocol::HumeProtocol); all the shared machinery
//! (reconnect supervisor + backoff + breaker/governor storm control, barge-in,
//! conversation replay, callback dispatch) now lives ONCE in the generic
//! [`RealtimeSession`](crate::core::realtime::scaffold::RealtimeSession) driver.
//!
//! `HumeEVI` is therefore just `RealtimeSession<HumeProtocol>` plus the
//! Hume-specific inherent accessors (`get_chat_id`/`get_chat_group_id`/
//! `resilience_breaker`) and the rich `get_provider_info()` JSON the public API
//! guarantees. The ~580-line bespoke struct + single-connection message loop +
//! `handle_server_message` are DELETED — Hume GAINS automatic reconnect + the
//! shared barge-in path for free.
//!
//! # Features
//!
//! - Real-time bidirectional audio streaming (base64 JSON in, JSON+binary out)
//! - 48-dimension prosody (emotion) analysis (deserialized in `messages.rs`)
//! - Empathic response generation
//! - Function calling support
//! - Automatic reconnection (NEW — inherited from the scaffold)
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::realtime::hume::{HumeEVI, HumeEVIConfig};
//! use waav_gateway::core::realtime::BaseRealtime;
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = HumeEVIConfig::new("your-api-key")
//!         .with_config_id("your-config-id");
//!
//!     let mut evi = HumeEVI::from_hume_config(config).unwrap();
//!     evi.connect().await.unwrap();
//!
//!     // Register callbacks
//!     evi.on_transcript(Arc::new(|t| Box::pin(async move {
//!         println!("[{}] {}", t.role, t.text);
//!     }))).unwrap();
//!
//!     // Send audio
//!     evi.send_audio(audio_bytes).await.unwrap();
//! }
//! ```

use async_trait::async_trait;
use bytes::Bytes;

use super::config::HumeEVIConfig;
use super::protocol::HumeProtocol;
use crate::core::realtime::base::{
    AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback, RealtimeConfig,
    RealtimeErrorCallback, RealtimeResult, ReconnectionCallback, ReplayConversationItem,
    ResponseDoneCallback, SpeechEventCallback, TranscriptCallback,
};
use crate::core::realtime::scaffold::RealtimeSession;

// =============================================================================
// HumeEVI Client
// =============================================================================

/// Hume EVI (Empathic Voice Interface) realtime client.
///
/// A newtype over the generic [`RealtimeSession`] driver parameterized by
/// [`HumeProtocol`]. Every [`BaseRealtime`] method delegates to the driver; the
/// inherent accessors (`get_chat_id`/`get_chat_group_id`/`resilience_breaker`)
/// and the rich `get_provider_info()` are preserved for the existing public API +
/// tests.
pub struct HumeEVI(RealtimeSession<HumeProtocol>);

impl HumeEVI {
    /// Create a new HumeEVI client from a [`HumeEVIConfig`].
    ///
    /// Validates the config (api key + EVI version + audio params) EXACTLY as
    /// before, then builds the scaffold session around the parsed config.
    pub fn from_hume_config(config: HumeEVIConfig) -> RealtimeResult<Self> {
        let protocol = HumeProtocol::from_hume_config(config.clone())?;
        // The scaffold's generic config drives only reconnection + the (overridden)
        // generic provider_info; carry the EVI config's reconnection + api key so
        // reconnect behavior is config-honored.
        let generic = RealtimeConfig {
            provider: "hume".to_string(),
            api_key: config.api_key.clone(),
            voice: config.voice_id.clone(),
            instructions: config.system_prompt.clone(),
            reconnection: config.reconnection.clone(),
            ..Default::default()
        };
        Ok(Self(RealtimeSession::from_parts(protocol, generic)?))
    }

    /// The shared circuit breaker this session feeds, if injected (for metrics/tests).
    pub fn resilience_breaker(
        &self,
    ) -> Option<&std::sync::Arc<crate::core::resilience::CircuitBreaker>> {
        self.0.resilience_breaker()
    }

    /// Get the chat ID for the current session.
    ///
    /// Sourced from the EVI `chat_metadata` event (lowered to the scaffold's
    /// session id). `None` until the server sends it post-connect.
    pub async fn get_chat_id(&self) -> Option<String> {
        self.0.session_id().await
    }

    /// Get the chat group ID for resuming conversations.
    ///
    /// NOTE: the shared scaffold carries a single session identity (the chat id,
    /// surfaced by [`get_chat_id`](Self::get_chat_id)); the EVI `chat_group_id` is
    /// not separately tracked by the generic driver. Resuming a prior conversation
    /// is still driven config-side via
    /// [`HumeEVIConfig::with_chat_group`](super::config::HumeEVIConfig::with_chat_group)
    /// (the `resumed_chat_group_id` query param), which is unaffected.
    pub async fn get_chat_group_id(&self) -> Option<String> {
        None
    }
}

#[async_trait]
impl BaseRealtime for HumeEVI {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self> {
        Ok(Self(RealtimeSession::<HumeProtocol>::new(config)?))
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

    /// Rich provider info (preserved verbatim — the existing provider-info test
    /// asserts this exact shape; the generic driver's minimal info is intentionally
    /// overridden here).
    fn get_provider_info(&self) -> serde_json::Value {
        let c = self.0.protocol().evi_config();
        serde_json::json!({
            "provider": "hume",
            "product": "evi",
            "evi_version": c.evi_version.as_str(),
            "sample_rate": c.sample_rate,
            "channels": c.channels,
            "encoding": format!("{:?}", c.input_encoding),
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::realtime::base::{RealtimeError, ReconnectionConfig};
    use std::sync::Arc;

    #[test]
    fn test_hume_evi_creation() {
        let config = HumeEVIConfig::new("test-api-key");
        let evi = HumeEVI::from_hume_config(config);
        assert!(evi.is_ok());
    }

    #[test]
    fn test_hume_evi_creation_empty_key() {
        let config = HumeEVIConfig::default();
        let evi = HumeEVI::from_hume_config(config);
        assert!(evi.is_err());
    }

    #[test]
    fn test_hume_evi_from_realtime_config() {
        let config = RealtimeConfig {
            api_key: "test-key".to_string(),
            voice: Some("kora".to_string()),
            instructions: Some("Be helpful".to_string()),
            ..Default::default()
        };

        let evi = HumeEVI::new(config);
        assert!(evi.is_ok());

        // The bespoke struct's private `config` fields moved into the protocol;
        // assert the SAME values through the scaffold's protocol accessor (the
        // EVI config the protocol parsed from the RealtimeConfig).
        let evi = evi.unwrap();
        let c = evi.0.protocol().evi_config();
        assert_eq!(c.api_key, "test-key");
        assert_eq!(c.voice_id, Some("kora".to_string()));
        assert_eq!(c.system_prompt, Some("Be helpful".to_string()));
    }

    #[test]
    fn test_hume_evi_initial_state() {
        let config = HumeEVIConfig::new("test-key");
        let evi = HumeEVI::from_hume_config(config).unwrap();

        assert_eq!(evi.get_connection_state(), ConnectionState::Disconnected);
        assert!(!evi.is_ready());
    }

    #[test]
    fn test_hume_evi_provider_info() {
        let config =
            HumeEVIConfig::new("test-key").with_version(super::super::config::EVIVersion::V4Mini);
        let evi = HumeEVI::from_hume_config(config).unwrap();

        let info = evi.get_provider_info();
        assert_eq!(info["provider"], "hume");
        assert_eq!(info["product"], "evi");
        assert_eq!(info["evi_version"], "4-mini");
    }

    #[tokio::test]
    async fn test_hume_evi_callbacks() {
        let config = HumeEVIConfig::new("test-key");
        let mut evi = HumeEVI::from_hume_config(config).unwrap();

        // Test setting callbacks (should not error)
        let result = evi.on_transcript(Arc::new(|_| Box::pin(async {})));
        assert!(result.is_ok());

        let result = evi.on_audio(Arc::new(|_| Box::pin(async {})));
        assert!(result.is_ok());

        let result = evi.on_error(Arc::new(|_| Box::pin(async {})));
        assert!(result.is_ok());

        let result = evi.on_function_call(Arc::new(|_| Box::pin(async {})));
        assert!(result.is_ok());

        let result = evi.on_speech_event(Arc::new(|_| Box::pin(async {})));
        assert!(result.is_ok());

        let result = evi.on_response_done(Arc::new(|_| Box::pin(async {})));
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_hume_evi_send_without_connection() {
        let config = HumeEVIConfig::new("test-key");
        let mut evi = HumeEVI::from_hume_config(config).unwrap();

        let result = evi.send_audio(Bytes::from_static(&[1, 2, 3])).await;
        assert!(matches!(result, Err(RealtimeError::NotConnected)));

        let result = evi.send_text("Hello").await;
        assert!(matches!(result, Err(RealtimeError::NotConnected)));
    }

    #[tokio::test]
    async fn test_hume_evi_noop_methods() {
        let config = HumeEVIConfig::new("test-key");
        let mut evi = HumeEVI::from_hume_config(config).unwrap();

        // These are no-ops for EVI
        let result = evi.create_response().await;
        assert!(result.is_ok());

        let result = evi.commit_audio_buffer().await;
        assert!(result.is_ok());

        let result = evi.clear_audio_buffer().await;
        assert!(result.is_ok());
    }

    /// NEW: Hume GAINS automatic reconnection for free from the scaffold (the
    /// bespoke single-connection loop had NONE). Prove the reconnection config is
    /// honored end-to-end: a custom `ReconnectionConfig` set on the source config
    /// reaches the scaffold's reconnect machinery (the same wiring the supervisor
    /// reconnect loop reads), and `set_resilience` (the shared breaker/governor
    /// storm-control path the reconnect dials consult) is wired through.
    #[tokio::test]
    async fn test_hume_evi_gains_reconnect_and_resilience() {
        // Custom reconnection config flows from HumeEVIConfig → scaffold.
        let config = HumeEVIConfig::new("test-key").with_config_id("cfg");
        let custom = ReconnectionConfig {
            enabled: true,
            max_attempts: 9,
            initial_delay_ms: 250,
            max_delay_ms: 30_000,
            backoff_multiplier: 2.0,
            jitter: false,
        };
        let mut with_recon = config.clone();
        with_recon.reconnection = Some(custom);

        let mut evi = HumeEVI::from_hume_config(with_recon).unwrap();
        // The scaffold honors the reconnection config (Hume previously could not
        // reconnect at all).
        let cfg = evi.0.config().reconnection.clone().unwrap();
        assert!(cfg.enabled, "reconnect is enabled");
        assert_eq!(cfg.max_attempts, 9, "custom max_attempts honored");
        assert!(cfg.should_retry(0), "retry path is wired");
        assert!(!cfg.should_retry(9), "respects the attempt cap");

        // The shared resilience handles (breaker + governor) the reconnect dials
        // consult are wired through `set_resilience` — Hume now participates in the
        // fleet storm-control path it gained from the scaffold.
        assert!(
            evi.resilience_breaker().is_none(),
            "no breaker until injected"
        );
        let handles = crate::core::resilience::ResilienceRegistry::new(4).handles_for("hume");
        evi.set_resilience(handles);
        assert!(
            evi.resilience_breaker().is_some(),
            "set_resilience wires the shared breaker"
        );

        // A default-config Hume defaults reconnection ON (scaffold default) —
        // another capability the bespoke client lacked.
        let default_evi = HumeEVI::from_hume_config(HumeEVIConfig::new("k")).unwrap();
        let dcfg = default_evi
            .0
            .config()
            .reconnection
            .clone()
            .unwrap_or_default();
        assert!(dcfg.enabled, "scaffold default reconnection is ON for Hume");
    }
}
