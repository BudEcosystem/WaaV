//! ElevenLabs Conversational AI ("ConvAI") — a base64+JSON speech-to-speech
//! realtime provider on WaaV's S2S scaffold (the OpenAI-family wire shape, NOT
//! Deepgram's raw binary frames).
//!
//! ConvAI is a single bidirectional WebSocket that runs the whole STT→LLM→TTS
//! loop server-side against a PRE-CREATED *agent* (the agent IS the model) with
//! server VAD + turn-taking + barge-in. All frames are JSON; user audio is
//! base64 INSIDE a JSON event (`{"user_audio_chunk": base64(pcm)}`) and agent
//! TTS comes back base64'd inside `{"type":"audio",…}`. An APP-level keepalive
//! (`ping`→`pong`) — distinct from the WS-protocol ping — is lowered to
//! [`S2sEvent::SendFrame`](crate::core::realtime::scaffold::S2sEvent::SendFrame),
//! which the driver sends on the outbound path.
//!
//! [`ElevenLabsRealtime`] is the thin `RealtimeSession<ElevenLabsProtocol>`
//! newtype: every [`BaseRealtime`] method delegates to the generic driver (which
//! owns reconnect, barge-in, resilience, callback dispatch).
//!
//! # API Reference (LIVE-probed oracle + docs)
//!
//! - Endpoint: `wss://api.elevenlabs.io/v1/convai/conversation?agent_id=<id>`
//! - Auth: `xi-api-key: <ELEVENLABS_API_KEY>` (NOT Bearer)
//! - Audio: `pcm_16000` both directions (16 kHz mono 16-bit ⇒ 32 B/ms)
//! - Wire: JSON only (audio base64'd inside JSON)
//! - Docs: <https://elevenlabs.io/docs/conversational-ai/api-reference/conversational-ai/websocket>

mod protocol;

pub use protocol::ElevenLabsProtocol;

use async_trait::async_trait;
use bytes::Bytes;

use crate::core::realtime::base::{
    AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback, RealtimeConfig,
    RealtimeErrorCallback, RealtimeResponseOverride, RealtimeResult, ReconnectionCallback,
    ReplayConversationItem, ResponseDoneCallback, SpeechEventCallback, TranscriptCallback,
};
use crate::core::realtime::scaffold::RealtimeSession;

/// ElevenLabs Conversational AI client — a newtype over the generic
/// [`RealtimeSession`] driver parameterized by [`ElevenLabsProtocol`]. Every
/// [`BaseRealtime`] method delegates to the driver.
pub struct ElevenLabsRealtime(RealtimeSession<ElevenLabsProtocol>);

impl ElevenLabsRealtime {
    /// The shared circuit breaker this session feeds, if injected (for metrics/tests).
    pub fn resilience_breaker(
        &self,
    ) -> Option<&std::sync::Arc<crate::core::resilience::CircuitBreaker>> {
        self.0.resilience_breaker()
    }

    /// Get the session ID (ConvAI `conversation_id`) if connected.
    pub async fn session_id(&self) -> Option<String> {
        self.0.session_id().await
    }
}

#[async_trait]
impl BaseRealtime for ElevenLabsRealtime {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self> {
        Ok(Self(RealtimeSession::<ElevenLabsProtocol>::new(config)?))
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
            "provider": "elevenlabs",
            "api_type": "WebSocket Conversational AI (speech-to-speech)",
            "version": "1.0.0",
            "wire": "JSON only (audio base64 inside JSON, both ways)",
            "auth": "xi-api-key",
            "endpoint": "wss://api.elevenlabs.io/v1/convai/conversation",
            "audio_format": "pcm_16000",
            "features": {
                "bidirectional_audio": true,
                "server_vad": true,
                "barge_in": true,
                "function_calling": true,
                "transcription": true,
                "managed_stt_llm_tts": true,
                "pre_created_agent": true
            },
            "documentation": "https://elevenlabs.io/docs/conversational-ai/api-reference/conversational-ai/websocket"
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
            provider: "elevenlabs".into(),
            api_key: "xikey".into(),
            model: "agent_abc123".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn creates_and_reports_provider() {
        let rt = ElevenLabsRealtime::new(cfg()).unwrap();
        assert!(!rt.is_ready());
        assert_eq!(rt.get_connection_state(), ConnectionState::Disconnected);
        assert_eq!(rt.get_provider_info()["provider"], "elevenlabs");
    }

    #[tokio::test]
    async fn requires_api_key() {
        let bad = RealtimeConfig {
            api_key: String::new(),
            ..cfg()
        };
        assert!(matches!(
            ElevenLabsRealtime::new(bad),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    #[tokio::test]
    async fn requires_agent_id() {
        let bad = RealtimeConfig {
            model: String::new(),
            ..cfg()
        };
        assert!(matches!(
            ElevenLabsRealtime::new(bad),
            Err(RealtimeError::InvalidConfiguration(_))
        ));
    }

    /// Factory create-by-name via the registry: canonical + aliases + case-insensitive.
    #[tokio::test]
    async fn factory_creates_by_name_case_insensitive() {
        use crate::core::realtime::create_realtime_provider;
        for name in ["elevenlabs", "ELEVENLABS", "ElevenLabs", "elevenlabs-convai", "11labs"] {
            let provider = create_realtime_provider(name, cfg())
                .unwrap_or_else(|e| panic!("registry failed for {name}: {e:?}"));
            assert_eq!(
                provider.get_provider_info()["provider"],
                "elevenlabs",
                "name {name} ⇒ elevenlabs"
            );
            assert!(!provider.is_ready());
        }
    }
}
