//! Azure OpenAI Realtime — an OpenAI-PROTOCOL CLONE on the shared S2S scaffold.
//!
//! Azure OpenAI's Realtime API speaks the EXACT OpenAI GA wire; it differs ONLY
//! in the endpoint (`<resource>.openai.azure.com/openai/realtime?…`) and auth
//! (the `api-key` HEADER). [`AzureProtocol`] therefore embeds the live-validated
//! [`OpenAiProtocol`](crate::core::realtime::openai) and delegates every wire
//! method to it (byte-identical GA wire); [`AzureRealtime`] is the thin
//! `RealtimeSession<AzureProtocol>` newtype, identical to `OpenAIRealtime` modulo
//! the provider id + `get_provider_info()`.
//!
//! # API Reference
//!
//! - Endpoint: `wss://<resource>.openai.azure.com/openai/realtime?api-version=<v>&deployment=<deployment>`
//! - Auth: `api-key: <key>` request header
//! - Wire: identical to OpenAI GA (`gpt-realtime`)

mod protocol;

pub use protocol::AzureProtocol;

use async_trait::async_trait;
use bytes::Bytes;

use crate::core::realtime::base::{
    AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback, RealtimeConfig,
    RealtimeErrorCallback, RealtimeResponseOverride, RealtimeResult, ReconnectionCallback,
    ReplayConversationItem, ResponseDoneCallback, SpeechEventCallback, TranscriptCallback,
};
use crate::core::realtime::openai::{OpenAIRealtimeAudioFormat, OpenAIRealtimeModel, OpenAIRealtimeVoice};
use crate::core::realtime::scaffold::RealtimeSession;

/// Azure OpenAI Realtime client — a newtype over the generic [`RealtimeSession`]
/// driver parameterized by [`AzureProtocol`]. Every [`BaseRealtime`] method
/// delegates to the driver.
pub struct AzureRealtime(RealtimeSession<AzureProtocol>);

impl AzureRealtime {
    /// The shared circuit breaker this session feeds, if injected (for metrics/tests).
    pub fn resilience_breaker(
        &self,
    ) -> Option<&std::sync::Arc<crate::core::resilience::CircuitBreaker>> {
        self.0.resilience_breaker()
    }

    /// Get the configured model (the OpenAI-derived enum the embedded protocol parsed).
    pub fn model(&self) -> OpenAIRealtimeModel {
        self.0.protocol().inner().model()
    }

    /// Get the configured voice.
    pub fn voice(&self) -> OpenAIRealtimeVoice {
        self.0.protocol().inner().voice()
    }

    /// Get the configured audio format.
    pub fn audio_format(&self) -> OpenAIRealtimeAudioFormat {
        self.0.protocol().inner().audio_format()
    }

    /// Get the session ID if connected.
    pub async fn session_id(&self) -> Option<String> {
        self.0.session_id().await
    }
}

#[async_trait]
impl BaseRealtime for AzureRealtime {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self> {
        Ok(Self(RealtimeSession::<AzureProtocol>::new(config)?))
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
            "provider": "azure",
            "api_type": "WebSocket Realtime (Azure OpenAI)",
            "version": "1.0.0",
            "wire": "OpenAI GA (gpt-realtime) — byte-identical",
            "auth": "api-key header",
            "features": {
                "bidirectional_audio": true,
                "vad": true,
                "function_calling": true,
                "text_and_audio": true,
                "transcription": true
            },
            "documentation": "https://learn.microsoft.com/azure/ai-services/openai/realtime-audio-reference"
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
            provider: "azure".into(),
            api_key: "k".into(),
            model: "my-deployment".into(),
            endpoint: Some("my-resource".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn creates_and_reports_provider() {
        let rt = AzureRealtime::new(cfg()).unwrap();
        assert!(!rt.is_ready());
        assert_eq!(rt.get_provider_info()["provider"], "azure");
    }

    #[tokio::test]
    async fn requires_api_key() {
        let bad = RealtimeConfig {
            api_key: String::new(),
            ..cfg()
        };
        assert!(matches!(
            AzureRealtime::new(bad),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }
}
