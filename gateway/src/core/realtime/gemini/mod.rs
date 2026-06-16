//! Google Gemini Live ("BidiGenerateContent") — a base64+JSON speech-to-speech
//! realtime provider on WaaV's S2S scaffold.
//!
//! Gemini Live is a single bidirectional WebSocket that runs the whole
//! STT→LLM→TTS loop server-side (the model IS the agent) with server VAD,
//! turn-taking, and barge-in. It is the provider the scaffold's MULTI-FRAME
//! `Vec<S2sEvent>`, session RESUMPTION, and SendFrame features were designed for:
//!
//! - **MULTI-FRAME**: one `serverContent` wire message bundles a
//!   `modelTurn.parts[]` array (audio `inlineData` + `text` parts) plus turn
//!   flags; [`GeminiProtocol::map_server_event`] lowers it to ONE
//!   [`S2sEvent`](crate::core::realtime::scaffold::S2sEvent) PER part.
//! - **RESUMPTION**: `sessionResumptionUpdate` ⇒
//!   [`ResumptionHandle`](crate::core::realtime::scaffold::S2sEvent::ResumptionHandle);
//!   the driver stores it and feeds it back into `build_session_config` on
//!   reconnect (⇒ `setup.sessionResumption.handle`).
//!
//! [`GeminiRealtime`] is the thin `RealtimeSession<GeminiProtocol>` newtype:
//! every [`BaseRealtime`] method delegates to the generic driver (which owns
//! reconnect, barge-in, resilience, callback dispatch).
//!
//! # API Reference (authoritative wire; verified against Pipecat's
//! `services/google/gemini_live/llm.py` — NO Gemini key held, so unit-validated)
//!
//! - Endpoint: `wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key=<API_KEY>`
//! - Auth: `?key=<GEMINI_API_KEY>` QUERY param (NOT a header)
//! - Audio: 16 kHz PCM input, 24 kHz PCM output (base64 inside JSON, both ways)
//! - Wire: JSON only
//! - Docs: <https://ai.google.dev/gemini-api/docs/live>

mod protocol;

pub use protocol::GeminiProtocol;

use async_trait::async_trait;
use bytes::Bytes;

use crate::core::realtime::base::{
    AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback, RealtimeConfig,
    RealtimeErrorCallback, RealtimeResponseOverride, RealtimeResult, ReconnectionCallback,
    ReplayConversationItem, ResponseDoneCallback, SpeechEventCallback, TranscriptCallback,
};
use crate::core::realtime::scaffold::RealtimeSession;

/// Google Gemini Live client — a newtype over the generic [`RealtimeSession`]
/// driver parameterized by [`GeminiProtocol`]. Every [`BaseRealtime`] method
/// delegates to the driver.
pub struct GeminiRealtime(RealtimeSession<GeminiProtocol>);

impl GeminiRealtime {
    /// The shared circuit breaker this session feeds, if injected (for metrics/tests).
    pub fn resilience_breaker(
        &self,
    ) -> Option<&std::sync::Arc<crate::core::resilience::CircuitBreaker>> {
        self.0.resilience_breaker()
    }

    /// Get the session ID if connected (Gemini does not surface one ⇒ `None`).
    pub async fn session_id(&self) -> Option<String> {
        self.0.session_id().await
    }
}

#[async_trait]
impl BaseRealtime for GeminiRealtime {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self> {
        Ok(Self(RealtimeSession::<GeminiProtocol>::new(config)?))
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
            "provider": "gemini",
            "api_type": "WebSocket BidiGenerateContent (speech-to-speech)",
            "version": "1.0.0",
            "wire": "JSON only (audio base64 inside JSON, both ways)",
            "auth": "?key= query param",
            "endpoint": "wss://generativelanguage.googleapis.com/ws/.../BidiGenerateContent",
            "input_audio_format": "audio/pcm;rate=16000",
            "output_audio_format": "audio/pcm;rate=24000",
            "features": {
                "bidirectional_audio": true,
                "server_vad": true,
                "barge_in": true,
                "function_calling": true,
                "transcription": true,
                "session_resumption": true,
                "multi_frame_server_content": true
            },
            "documentation": "https://ai.google.dev/gemini-api/docs/live"
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
            provider: "gemini".into(),
            api_key: "gkey".into(),
            model: "gemini-2.0-flash-live-001".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn creates_and_reports_provider() {
        let rt = GeminiRealtime::new(cfg()).unwrap();
        assert!(!rt.is_ready());
        assert_eq!(rt.get_connection_state(), ConnectionState::Disconnected);
        assert_eq!(rt.get_provider_info()["provider"], "gemini");
    }

    #[tokio::test]
    async fn requires_api_key() {
        let bad = RealtimeConfig {
            api_key: String::new(),
            ..cfg()
        };
        assert!(matches!(
            GeminiRealtime::new(bad),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    /// Factory create-by-name via the registry: canonical + aliases + case-insensitive.
    #[tokio::test]
    async fn factory_creates_by_name_case_insensitive() {
        use crate::core::realtime::create_realtime_provider;
        for name in ["gemini", "GEMINI", "Gemini", "gemini-live", "google"] {
            let provider = create_realtime_provider(name, cfg())
                .unwrap_or_else(|e| panic!("registry failed for {name}: {e:?}"));
            assert_eq!(
                provider.get_provider_info()["provider"],
                "gemini",
                "name {name} ⇒ gemini"
            );
            assert!(!provider.is_ready());
        }
    }
}
