//! Ultravox Realtime (speech-to-speech) — the FIRST `RestThenWebSocket` realtime
//! provider on WaaV's S2S scaffold.
//!
//! Ultravox is the hosted realtime API for the open-weight Ultravox S2S model
//! (`fixie-ai/ultravox`): a single WebSocket runs the entire STT→LLM→TTS loop
//! server-side with server VAD + turn-taking + barge-in. Like Deepgram (and
//! unlike the OpenAI family), [`UltravoxProtocol`] streams user mic audio as RAW
//! PCM BINARY frames and receives agent TTS as RAW PCM BINARY frames; only the
//! transcript / state / barge-in / tool control messages are JSON.
//!
//! Unique on the scaffold is the CONNECT handshake: you can't open the WS
//! directly — a REST `POST https://api.ultravox.ai/api/calls` (auth `X-API-Key`)
//! mints a single-use, pre-authed `joinUrl` that the WS then connects. That
//! two-step lives entirely in the
//! [`RestHandshakeWsTransportFactory`](crate::core::realtime::scaffold::RestHandshakeWsTransportFactory)
//! (the protocol's `transport_factory()`); the generic driver is unchanged.
//!
//! [`UltravoxRealtime`] is the thin `RealtimeSession<UltravoxProtocol>` newtype:
//! every [`BaseRealtime`] method delegates to the generic driver (which owns
//! reconnect, barge-in, resilience, callback dispatch).
//!
//! # API Reference (authoritative wire; verified against Pipecat's
//! `services/ultravox/llm.py` — NO Ultravox key held, so unit-validated)
//!
//! - Create call: `POST https://api.ultravox.ai/api/calls`, header
//!   `X-API-Key: <ULTRAVOX_API_KEY>`, JSON body
//!   `{systemPrompt, model, voice, medium.serverWebSocket.{inputSampleRate,outputSampleRate}}`
//!   ⇒ `{"joinUrl":"wss://…"}`.
//! - Then connect `joinUrl` (a plain WS, pre-authed, NO extra headers).
//! - Audio: RAW PCM BINARY both ways — input @ 16 kHz, output @ 24 kHz.
//! - Wire: RAW PCM binary audio + JSON control (`transcript`, `state`,
//!   `playback_clear_buffer`, `client_tool_invocation` / `client_tool_result`,
//!   `user_text_message`).
//! - Docs: <https://docs.ultravox.ai/>

mod protocol;

pub use protocol::UltravoxProtocol;

use async_trait::async_trait;
use bytes::Bytes;

use crate::core::realtime::base::{
    AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback, RealtimeConfig,
    RealtimeErrorCallback, RealtimeResponseOverride, RealtimeResult, ReconnectionCallback,
    ReplayConversationItem, ResponseDoneCallback, SpeechEventCallback, TranscriptCallback,
};
use crate::core::realtime::scaffold::RealtimeSession;

/// Ultravox Realtime client — a newtype over the generic [`RealtimeSession`]
/// driver parameterized by [`UltravoxProtocol`]. Every [`BaseRealtime`] method
/// delegates to the driver.
pub struct UltravoxRealtime(RealtimeSession<UltravoxProtocol>);

impl UltravoxRealtime {
    /// The shared circuit breaker this session feeds, if injected (for metrics/tests).
    pub fn resilience_breaker(
        &self,
    ) -> Option<&std::sync::Arc<crate::core::resilience::CircuitBreaker>> {
        self.0.resilience_breaker()
    }

    /// Get the session ID if connected (Ultravox surfaces none over the WS ⇒ `None`).
    pub async fn session_id(&self) -> Option<String> {
        self.0.session_id().await
    }
}

#[async_trait]
impl BaseRealtime for UltravoxRealtime {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self> {
        Ok(Self(RealtimeSession::<UltravoxProtocol>::new(config)?))
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

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "ultravox",
            "api_type": "REST-create-call + WebSocket (speech-to-speech)",
            "version": "1.0.0",
            "wire": "RAW PCM binary audio (both ways) + JSON control",
            "auth": "X-API-Key (create-call); pre-authed joinUrl WS",
            "endpoint": "https://api.ultravox.ai/api/calls",
            "audio": "16 kHz PCM input / 24 kHz PCM output",
            "features": {
                "bidirectional_audio": true,
                "server_vad": true,
                "barge_in": true,
                "function_calling": true,
                "transcription": true,
                "managed_stt_llm_tts": true
            },
            "documentation": "https://docs.ultravox.ai/"
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

    async fn replay_conversation(&mut self, items: &[ReplayConversationItem]) -> RealtimeResult<()> {
        self.0.replay_conversation(items).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::realtime::base::RealtimeError;

    fn cfg() -> RealtimeConfig {
        RealtimeConfig {
            provider: "ultravox".into(),
            api_key: "uvkey".into(),
            model: "fixie-ai/ultravox".into(),
            voice: Some("Mark".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn creates_and_reports_provider() {
        let rt = UltravoxRealtime::new(cfg()).unwrap();
        assert!(!rt.is_ready());
        assert_eq!(rt.get_connection_state(), ConnectionState::Disconnected);
        assert_eq!(rt.get_provider_info()["provider"], "ultravox");
    }

    #[tokio::test]
    async fn requires_api_key() {
        let bad = RealtimeConfig {
            api_key: String::new(),
            ..cfg()
        };
        assert!(matches!(
            UltravoxRealtime::new(bad),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    /// Factory create-by-name via the registry: canonical + aliases + case-insensitive.
    #[tokio::test]
    async fn factory_creates_by_name_case_insensitive() {
        use crate::core::realtime::create_realtime_provider;
        for name in ["ultravox", "ULTRAVOX", "Ultravox", "fixie"] {
            let provider = create_realtime_provider(name, cfg())
                .unwrap_or_else(|e| panic!("registry failed for {name}: {e:?}"));
            assert_eq!(
                provider.get_provider_info()["provider"],
                "ultravox",
                "name {name} ⇒ ultravox"
            );
            assert!(!provider.is_ready());
        }
    }
}
