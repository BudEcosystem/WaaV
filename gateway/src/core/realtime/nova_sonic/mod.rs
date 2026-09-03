//! AWS Nova Sonic Realtime (speech-to-speech) — the FIRST `BedrockBidi` realtime
//! provider on WaaV's S2S scaffold (an Amazon Bedrock
//! `InvokeModelWithBidirectionalStream` HTTP/2 event stream, NOT a WebSocket).
//!
//! Amazon Nova Sonic (`amazon.nova-sonic-v1:0`) is AWS's speech-to-speech model: a
//! single bidirectional Bedrock stream runs the entire ASR→LLM→TTS loop
//! server-side with server VAD + turn-taking + barge-in. Audio is base64 PCM
//! carried INSIDE event JSON (the Bedrock stream is event-framed; there are no
//! separate binary frames). Auth is AWS SigV4 via the `aws-config` default
//! credential chain (env / shared config / IAM role) — there is NO api-key.
//!
//! Unique on the scaffold is the TRANSPORT: the Bedrock bidi stream lives entirely
//! in the
//! [`BedrockBidiTransportFactory`](crate::core::realtime::scaffold::BedrockBidiTransportFactory)
//! (the protocol's `transport_factory()`), which mirrors the in-tree
//! `AwsTranscribeTransport` (another non-WS bidi HTTP/2 event stream behind a
//! transport trait). The generic driver (`session.rs`) is unchanged.
//!
//! [`NovaSonicRealtime`] is the thin `RealtimeSession<NovaSonicProtocol>` newtype:
//! every [`BaseRealtime`] method delegates to the generic driver (which owns
//! reconnect, barge-in, resilience, callback dispatch).
//!
//! # API Reference (authoritative wire — verified VERBATIM against the AWS Bedrock
//! / Nova Sonic docs; NO AWS key held, so unit-validated)
//!
//! - Bedrock op `InvokeModelWithBidirectionalStream`, modelId
//!   `amazon.nova-sonic-v1:0`; every event is `{"event": { … }}` JSON.
//! - Client→server: `sessionStart` → `promptStart` → SYSTEM
//!   `contentStart`/`textInput`/`contentEnd` → USER AUDIO `contentStart` +
//!   `audioInput`(base64 PCM) …
//! - Server→client: `completionStart` / `contentStart` / `textOutput` /
//!   `audioOutput`(base64 PCM) / `toolUse` / `contentEnd` / `completionEnd`.
//! - Audio: PCM 16-bit mono — 16 kHz input / 24 kHz output.
//! - Docs: <https://docs.aws.amazon.com/nova/latest/userguide/speech.html>

mod protocol;

pub use protocol::NovaSonicProtocol;

use async_trait::async_trait;
use bytes::Bytes;

use crate::core::realtime::base::{
    AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback, RealtimeConfig,
    RealtimeErrorCallback, RealtimeResponseOverride, RealtimeResult, ReconnectionCallback,
    ReplayConversationItem, ResponseDoneCallback, SpeechEventCallback, TranscriptCallback,
};
use crate::core::realtime::scaffold::RealtimeSession;

/// AWS Nova Sonic Realtime client — a newtype over the generic [`RealtimeSession`]
/// driver parameterized by [`NovaSonicProtocol`]. Every [`BaseRealtime`] method
/// delegates to the driver.
pub struct NovaSonicRealtime(RealtimeSession<NovaSonicProtocol>);

impl NovaSonicRealtime {
    /// The shared circuit breaker this session feeds, if injected (for metrics/tests).
    pub fn resilience_breaker(
        &self,
    ) -> Option<&std::sync::Arc<crate::core::resilience::CircuitBreaker>> {
        self.0.resilience_breaker()
    }

    /// Get the session ID if connected (Nova Sonic surfaces it on `completionStart`
    /// / `completionEnd`; the driver tracks none over this seam ⇒ `None`).
    pub async fn session_id(&self) -> Option<String> {
        self.0.session_id().await
    }
}

#[async_trait]
impl BaseRealtime for NovaSonicRealtime {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self> {
        Ok(Self(RealtimeSession::<NovaSonicProtocol>::new(config)?))
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
            "provider": "nova_sonic",
            "api_type": "Amazon Bedrock InvokeModelWithBidirectionalStream (speech-to-speech)",
            "version": "1.0.0",
            "wire": "base64 PCM audio + JSON events (event-framed bidi HTTP/2 stream)",
            "auth": "AWS SigV4 via aws-config default credential chain (NO api-key)",
            "model": "amazon.nova-sonic-v1:0",
            "audio": "16 kHz PCM input / 24 kHz PCM output",
            "features": {
                "bidirectional_audio": true,
                "server_vad": true,
                "barge_in": true,
                "function_calling": true,
                "transcription": true,
                "managed_asr_llm_tts": true
            },
            "documentation": "https://docs.aws.amazon.com/nova/latest/userguide/speech.html"
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RealtimeConfig {
        RealtimeConfig {
            provider: "nova_sonic".into(),
            api_key: String::new(), // Nova Sonic uses AWS creds, NOT an api-key.
            model: "amazon.nova-sonic-v1:0".into(),
            voice: Some("matthew".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn creates_and_reports_provider() {
        let rt = NovaSonicRealtime::new(cfg()).unwrap();
        assert!(!rt.is_ready());
        assert_eq!(rt.get_connection_state(), ConnectionState::Disconnected);
        assert_eq!(rt.get_provider_info()["provider"], "nova_sonic");
    }

    /// UNLIKE every other realtime provider, Nova Sonic does NOT require an api-key
    /// (it auths via the AWS credential chain) — an empty api_key still constructs.
    #[tokio::test]
    async fn constructs_without_api_key() {
        let no_key = RealtimeConfig {
            api_key: String::new(),
            ..cfg()
        };
        assert!(NovaSonicRealtime::new(no_key).is_ok());
    }

    /// Factory create-by-name via the registry: canonical + aliases + case-insensitive.
    #[tokio::test]
    async fn factory_creates_by_name_case_insensitive() {
        use crate::core::realtime::create_realtime_provider;
        for name in ["nova_sonic", "NOVA_SONIC", "nova-sonic", "aws"] {
            let provider = create_realtime_provider(name, cfg())
                .unwrap_or_else(|e| panic!("registry failed for {name}: {e:?}"));
            assert_eq!(
                provider.get_provider_info()["provider"],
                "nova_sonic",
                "name {name} ⇒ nova_sonic"
            );
            assert!(!provider.is_ready());
        }
    }
}
