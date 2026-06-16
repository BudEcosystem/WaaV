//! Speechmatics Flow Realtime (speech-to-speech) — the conversational API layered
//! on Speechmatics' streaming STT, on WaaV's S2S scaffold.
//!
//! Speechmatics Flow ("Voice AI") is a single bidirectional WebSocket that runs
//! the entire STT→LLM→TTS loop server-side with server VAD + turn-taking +
//! barge-in. The agent's language/prompt/LLM/TTS-voice/dictionary are bound to a
//! *template* (created in the Speechmatics Portal) selected by `template_id`.
//! Like Deepgram (and unlike the OpenAI family), [`SpeechmaticsProtocol`] streams
//! user mic audio as RAW PCM BINARY frames and receives agent TTS as RAW PCM
//! BINARY frames; only the `StartConversation` config + transcript / lifecycle /
//! control messages are JSON.
//!
//! [`SpeechmaticsRealtime`] is the thin `RealtimeSession<SpeechmaticsProtocol>`
//! newtype: every [`BaseRealtime`] method delegates to the generic driver (which
//! owns reconnect, barge-in, resilience, callback dispatch).
//!
//! # API Reference (authoritative wire — RESEARCHED from the official
//! `speechmatics/speechmatics-flow` Python SDK + Flow API-reference docs; NO
//! Speechmatics Flow key held, so unit-validated — see the field-by-field `[FLAG]`
//! notes in [`protocol`] + the implementation report)
//!
//! - Endpoint: `wss://flow.api.speechmatics.com/v1/flow` (host confirmed; path is
//!   the documented connect path; overridable via `cfg.endpoint`).
//! - Auth: `Authorization: Bearer <token>` — `cfg.api_key` is passed AS the bearer
//!   token (a Speechmatics JWT / temporary token). The in-gateway management-
//!   platform token EXCHANGE is NOT performed.
//! - `StartConversation`: `{message, audio_format:{type:"raw", encoding:"pcm_s16le",
//!   sample_rate:16000}, conversation_config:{template_id, template_variables?}}`.
//! - Audio: RAW PCM BINARY both ways (input acked by `AudioAdded`; output acked by
//!   the client with `AudioReceived`).
//! - Server events: `ConversationStarted`, `AddTranscript`/`AddPartialTranscript`
//!   (USER), `ResponseStarted`/`ResponseCompleted`/`ResponseInterrupted` (AGENT),
//!   `ToolInvoke`, `AudioAdded`, `ConversationEnding`/`ConversationEnded`,
//!   `Info`/`Warning`/`Error`/`Debug`/`prompt`.
//! - Docs: <https://docs.speechmatics.com/flow-api-ref>,
//!   <https://github.com/speechmatics/speechmatics-flow>

mod protocol;

pub use protocol::SpeechmaticsProtocol;

use async_trait::async_trait;
use bytes::Bytes;

use crate::core::realtime::base::{
    AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback, RealtimeConfig,
    RealtimeErrorCallback, RealtimeResponseOverride, RealtimeResult, ReconnectionCallback,
    ReplayConversationItem, ResponseDoneCallback, SpeechEventCallback, TranscriptCallback,
};
use crate::core::realtime::scaffold::RealtimeSession;

/// Speechmatics Flow Realtime client — a newtype over the generic
/// [`RealtimeSession`] driver parameterized by [`SpeechmaticsProtocol`]. Every
/// [`BaseRealtime`] method delegates to the driver.
pub struct SpeechmaticsRealtime(RealtimeSession<SpeechmaticsProtocol>);

impl SpeechmaticsRealtime {
    /// The shared circuit breaker this session feeds, if injected (for metrics/tests).
    pub fn resilience_breaker(
        &self,
    ) -> Option<&std::sync::Arc<crate::core::resilience::CircuitBreaker>> {
        self.0.resilience_breaker()
    }

    /// Get the session ID if connected (the `ConversationStarted` id, if any).
    pub async fn session_id(&self) -> Option<String> {
        self.0.session_id().await
    }
}

#[async_trait]
impl BaseRealtime for SpeechmaticsRealtime {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self> {
        Ok(Self(RealtimeSession::<SpeechmaticsProtocol>::new(config)?))
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
            "provider": "speechmatics",
            "api_type": "Flow Voice AI WebSocket (speech-to-speech)",
            "version": "1.0.0",
            "wire": "RAW PCM binary audio (both ways) + JSON control",
            "auth": "Authorization: Bearer <token> (JWT / temporary token)",
            "endpoint": "wss://flow.api.speechmatics.com/v1/flow",
            "audio": "16 kHz pcm_s16le PCM (input + output)",
            "features": {
                "bidirectional_audio": true,
                "server_vad": true,
                "barge_in": true,
                "function_calling": true,
                "transcription": true,
                "managed_stt_llm_tts": true,
                "template_based": true
            },
            "documentation": "https://docs.speechmatics.com/flow-api-ref"
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
            provider: "speechmatics".into(),
            api_key: "sm-token".into(),
            model: "flow-service-assistant-amelia".into(),
            voice: Some("amelia".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn creates_and_reports_provider() {
        let rt = SpeechmaticsRealtime::new(cfg()).unwrap();
        assert!(!rt.is_ready());
        assert_eq!(rt.get_connection_state(), ConnectionState::Disconnected);
        assert_eq!(rt.get_provider_info()["provider"], "speechmatics");
    }

    #[tokio::test]
    async fn requires_api_key() {
        let bad = RealtimeConfig {
            api_key: String::new(),
            ..cfg()
        };
        assert!(matches!(
            SpeechmaticsRealtime::new(bad),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    /// Factory create-by-name via the registry: canonical + alias + case-insensitive.
    #[tokio::test]
    async fn factory_creates_by_name_case_insensitive() {
        use crate::core::realtime::create_realtime_provider;
        for name in ["speechmatics", "SPEECHMATICS", "Speechmatics", "flow"] {
            let provider = create_realtime_provider(name, cfg())
                .unwrap_or_else(|e| panic!("registry failed for {name}: {e:?}"));
            assert_eq!(
                provider.get_provider_info()["provider"],
                "speechmatics",
                "name {name} ⇒ speechmatics"
            );
            assert!(!provider.is_ready());
        }
    }
}
