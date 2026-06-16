//! Realtime audio-to-audio provider module.
//!
//! This module provides abstractions and implementations for real-time
//! bidirectional audio streaming with transcription and TTS.
//!
//! # Supported Providers
//!
//! - **OpenAI Realtime API** - Full duplex audio with GPT-4o
//! - **Hume EVI** - Empathic Voice Interface with 48-dimension emotion analysis
//!
//! # Architecture
//!
//! The realtime module follows the same patterns as STT and TTS:
//! - `BaseRealtime` trait for provider abstraction
//! - Factory functions for dynamic provider creation
//! - Callback-based event handling
//!
//! # Audio Format
//!
//! - OpenAI: PCM 16-bit signed little-endian at 24kHz
//! - Hume EVI: Linear16 PCM at 44.1kHz or WebM
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::realtime::{create_realtime_provider, RealtimeConfig};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = RealtimeConfig {
//!         api_key: "sk-...".to_string(),
//!         provider: "openai".to_string(),
//!         model: "gpt-4o-realtime-preview".to_string(),
//!         voice: Some("alloy".to_string()),
//!         ..Default::default()
//!     };
//!
//!     let mut provider = create_realtime_provider("openai", config).unwrap();
//!     provider.connect().await.unwrap();
//!
//!     provider.on_transcript(Arc::new(|t| Box::pin(async move {
//!         println!("[{}] {}", t.role, t.text);
//!     }))).unwrap();
//!
//!     provider.send_audio(audio_bytes).await.unwrap();
//! }
//! ```

mod base;
pub mod azure;
pub mod deepgram;
pub mod elevenlabs;
pub mod gemini;
pub mod grok;
pub mod hume;
pub mod inworld;
pub mod nova_sonic;
pub mod openai;
pub mod scaffold;
pub mod speechmatics;
pub mod ultravox;
pub mod yandex;

pub use base::{
    AudioOutputCallback, BaseRealtime, BoxedRealtime, ConnectionState, FunctionCallCallback,
    FunctionCallRequest, FunctionDefinition, InputTranscriptionConfig, RealtimeAudioData,
    RealtimeConfig, RealtimeError, RealtimeErrorCallback, RealtimeFactory, RealtimeResponseOverride,
    RealtimeResult, ReconnectionCallback, ReconnectionEvent, ReplayConversationItem,
    ResponseDoneCallback,
    SpeechEvent, SpeechEventCallback, ToolDefinition, TranscriptCallback, TranscriptResult,
    TranscriptRole, TurnDetectionConfig, clamp_truncate_ms, run_barge_in_sequence,
};
pub use azure::{AzureProtocol, AzureRealtime};
pub use deepgram::{DeepgramProtocol, DeepgramRealtime};
pub use elevenlabs::{ElevenLabsProtocol, ElevenLabsRealtime};
pub use gemini::{GeminiProtocol, GeminiRealtime};
pub use grok::{GrokProtocol, GrokRealtime};
pub use hume::{
    EVIVersion, HUME_EVI_DEFAULT_SAMPLE_RATE, HUME_EVI_WEBSOCKET_URL, HumeEVI, HumeEVIConfig,
    ProsodyScores,
};
pub use inworld::{InworldProtocol, InworldRealtime};
pub use nova_sonic::{NovaSonicProtocol, NovaSonicRealtime};
pub use openai::{
    Modality, OPENAI_REALTIME_SAMPLE_RATE, OPENAI_REALTIME_URL, OpenAIRealtime,
    OpenAIRealtimeAudioFormat, OpenAIRealtimeModel, OpenAIRealtimeVoice,
};
pub use speechmatics::{SpeechmaticsProtocol, SpeechmaticsRealtime};
pub use ultravox::{UltravoxProtocol, UltravoxRealtime};
pub use yandex::{YandexProtocol, YandexRealtime};

/// Supported realtime providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeProvider {
    /// OpenAI Realtime API
    OpenAI,
    /// Hume EVI (Empathic Voice Interface)
    Hume,
}

impl RealtimeProvider {
    /// Parse provider from string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "openai" => Some(RealtimeProvider::OpenAI),
            "hume" | "hume_evi" | "hume-evi" | "evi" => Some(RealtimeProvider::Hume),
            _ => None,
        }
    }
}

impl std::fmt::Display for RealtimeProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RealtimeProvider::OpenAI => write!(f, "openai"),
            RealtimeProvider::Hume => write!(f, "hume"),
        }
    }
}

/// Factory function to create a realtime provider.
///
/// # Supported Providers
///
/// - `"openai"` - OpenAI Realtime API (gpt-4o-realtime-preview)
/// - `"hume"` / `"evi"` - Hume EVI (Empathic Voice Interface)
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::realtime::{create_realtime_provider, RealtimeConfig};
///
/// let config = RealtimeConfig {
///     api_key: "sk-...".to_string(),
///     model: "gpt-4o-realtime-preview".to_string(),
///     ..Default::default()
/// };
///
/// let provider = create_realtime_provider("openai", config)?;
/// ```
pub fn create_realtime_provider(
    provider_type: &str,
    config: RealtimeConfig,
) -> RealtimeResult<Box<dyn BaseRealtime>> {
    // Delegate to the plugin registry for provider creation
    // This enables extensibility: new providers can be registered without modifying this function
    crate::plugin::global_registry().create_realtime(provider_type, config)
}

/// Create a realtime provider from enum.
pub fn create_realtime_provider_from_enum(
    provider: RealtimeProvider,
    config: RealtimeConfig,
) -> RealtimeResult<Box<dyn BaseRealtime>> {
    // Delegate to the plugin registry using provider's string representation
    crate::plugin::global_registry().create_realtime(&provider.to_string(), config)
}

/// Get list of supported realtime providers.
///
/// `azure`/`grok`/`inworld` are OpenAI-PROTOCOL CLONES (they reuse the GA wire via
/// the embedded [`OpenAiProtocol`](openai::OpenAiProtocol) by delegation).
/// `deepgram` is the Voice Agent (S2S) provider — the first RAW-binary-frame
/// provider on the scaffold (its own [`DeepgramProtocol`], not an OpenAI clone).
/// `elevenlabs` is Conversational AI (S2S) — base64+JSON on its own
/// [`ElevenLabsProtocol`] (the OpenAI-family wire shape, not a clone).
/// `gemini` is Google Gemini Live (BidiGenerateContent, S2S) — base64+JSON on its
/// own [`GeminiProtocol`]; the MULTI-FRAME `serverContent` + session-resumption
/// provider the scaffold was designed for.
/// `ultravox` is the hosted Ultravox S2S model API — raw-PCM binary on its own
/// [`UltravoxProtocol`]; the FIRST `RestThenWebSocket` provider (a REST
/// create-call mints a pre-authed `joinUrl` the WS then connects).
/// `nova_sonic` is AWS Nova Sonic (`amazon.nova-sonic-v1:0`) — base64-PCM + JSON
/// events on its own [`NovaSonicProtocol`]; the FIRST `BedrockBidi` provider (an
/// Amazon Bedrock `InvokeModelWithBidirectionalStream` HTTP/2 event stream, NOT a
/// WebSocket; auth is AWS SigV4 via the `aws-config` default chain — NO api-key).
/// `speechmatics` is Speechmatics Flow (Voice AI) — raw-PCM binary on its own
/// [`SpeechmaticsProtocol`] (Deepgram-shaped wire: binary audio in/out + JSON
/// control); template-driven agents, auth via `Authorization: Bearer <token>`
/// (a Speechmatics JWT / temporary token).
/// `yandex` is Yandex Cloud AI Studio's Realtime API — an OpenAI-PROTOCOL CLONE
/// (GA wire reused via the embedded [`OpenAiProtocol`](openai::OpenAiProtocol) by
/// delegation, like azure/grok/inworld); it differs only in the host
/// (`wss://ai.api.cloud.yandex.net/v1/realtime?model=gpt://<folder>/<model>`) +
/// Bearer auth (a Yandex IAM token / static API key).
pub fn get_supported_realtime_providers() -> Vec<&'static str> {
    vec![
        "openai",
        "hume",
        "azure",
        "grok",
        "inworld",
        "deepgram",
        "elevenlabs",
        "gemini",
        "ultravox",
        "nova_sonic",
        "speechmatics",
        "yandex",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_realtime_provider() {
        let config = RealtimeConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        let result = create_realtime_provider("openai", config);
        assert!(result.is_ok());

        let invalid_result = create_realtime_provider("invalid", RealtimeConfig::default());
        assert!(invalid_result.is_err());
    }

    #[tokio::test]
    async fn test_create_realtime_provider_case_insensitive() {
        let config = RealtimeConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        assert!(create_realtime_provider("openai", config.clone()).is_ok());
        assert!(create_realtime_provider("OPENAI", config.clone()).is_ok());
        assert!(create_realtime_provider("OpenAI", config).is_ok());
    }

    #[test]
    fn test_get_supported_providers() {
        let providers = get_supported_realtime_providers();
        assert!(providers.contains(&"openai"));
        assert!(providers.contains(&"hume"));
        // OpenAI-protocol clones.
        assert!(providers.contains(&"azure"));
        assert!(providers.contains(&"grok"));
        assert!(providers.contains(&"inworld"));
        // Deepgram Voice Agent (S2S, raw-binary frames; not a clone).
        assert!(providers.contains(&"deepgram"));
        // ElevenLabs Conversational AI (S2S, base64+JSON; not a clone).
        assert!(providers.contains(&"elevenlabs"));
        // Google Gemini Live (S2S, base64+JSON, MULTI-FRAME serverContent; not a clone).
        assert!(providers.contains(&"gemini"));
        // Ultravox (S2S, raw-PCM binary; the first RestThenWebSocket provider).
        assert!(providers.contains(&"ultravox"));
        // AWS Nova Sonic (S2S, base64-PCM+JSON; the first BedrockBidi provider).
        assert!(providers.contains(&"nova_sonic"));
        // Speechmatics Flow (S2S, raw-PCM binary + JSON control; template-driven).
        assert!(providers.contains(&"speechmatics"));
        // Yandex Cloud AI Studio Realtime (OpenAI-protocol clone; GA wire, Bearer).
        assert!(providers.contains(&"yandex"));
        assert_eq!(providers.len(), 12);
    }

    #[test]
    fn test_provider_parse() {
        assert_eq!(
            RealtimeProvider::parse("openai"),
            Some(RealtimeProvider::OpenAI)
        );
        assert_eq!(
            RealtimeProvider::parse("OPENAI"),
            Some(RealtimeProvider::OpenAI)
        );
        assert_eq!(
            RealtimeProvider::parse("hume"),
            Some(RealtimeProvider::Hume)
        );
        assert_eq!(
            RealtimeProvider::parse("HUME"),
            Some(RealtimeProvider::Hume)
        );
        assert_eq!(RealtimeProvider::parse("evi"), Some(RealtimeProvider::Hume));
        assert_eq!(
            RealtimeProvider::parse("hume-evi"),
            Some(RealtimeProvider::Hume)
        );
        assert_eq!(RealtimeProvider::parse("invalid"), None);
    }

    #[test]
    fn test_provider_display() {
        assert_eq!(RealtimeProvider::OpenAI.to_string(), "openai");
        assert_eq!(RealtimeProvider::Hume.to_string(), "hume");
    }

    #[test]
    fn test_invalid_provider_error_message() {
        let config = RealtimeConfig::default();
        let result = create_realtime_provider("invalid_provider", config);

        match result {
            Err(RealtimeError::InvalidConfiguration(msg)) => {
                assert!(
                    msg.contains("openai"),
                    "Error message should mention openai as supported"
                );
                assert!(
                    msg.contains("hume"),
                    "Error message should mention hume as supported"
                );
            }
            _ => panic!("Expected InvalidConfiguration error"),
        }
    }

    /// Provider matrix: EVERY supported realtime provider must be constructible
    /// when a server-config `realtime_endpoint_override` is present — including
    /// the non-WebSocket transports (Ultravox `RestThenWebSocket`, Nova Sonic
    /// `BedrockBidi`). Bug class: a provider whose `from_config` (or registration)
    /// regresses, or that rejects/ignores the override field at construction so it
    /// would never be dialable behind a proxy / self-hosted / gov-cloud endpoint.
    /// Construction is offline (no connect), so this is credential-free.
    #[test]
    fn every_provider_constructs_with_endpoint_override() {
        for provider in get_supported_realtime_providers() {
            let config = RealtimeConfig {
                // A non-empty key satisfies the providers that validate one at
                // `from_config`; Nova Sonic ignores `api_key` (AWS creds).
                api_key: "test-key".to_string(),
                // A model/deployment name — every provider gets one in production
                // (the session config / node config supplies it); Azure REQUIRES
                // it as the deployment name.
                model: "gpt-realtime".to_string(),
                // The SERVER-CONFIG-ONLY upstream override (SSRF-safe) — a proxy
                // URL a deployment would legitimately set.
                realtime_endpoint_override: Some(
                    "wss://proxy.internal.example/realtime".to_string(),
                ),
                // Azure OpenAI Realtime additionally REQUIRES the resource
                // `endpoint` at construction (a distinct, server-injected field —
                // the handler sets `azure_openai_endpoint`). Supply it for azure so
                // the matrix mirrors production wiring; harmless for the rest.
                endpoint: Some("https://my-resource.openai.azure.com".to_string()),
                ..Default::default()
            };
            let result = create_realtime_provider(provider, config);
            assert!(
                result.is_ok(),
                "provider `{provider}` must construct with an endpoint override set, got {:?}",
                result.err()
            );
        }
    }

    /// Best-effort SEND on a NOT-connected session: a freshly-created provider
    /// (never `connect()`ed) must NOT panic when frames are pushed at it — it
    /// returns a graceful error (or Ok for providers that buffer), never an
    /// abort. Bug class: a send path that unwraps an absent transport / channel
    /// and crashes the task instead of surfacing `NotConnected`. Covers the
    /// WS, RestThenWebSocket, and BedrockBidi families via the matrix.
    #[tokio::test]
    async fn send_on_unconnected_session_is_best_effort_no_panic() {
        for provider in get_supported_realtime_providers() {
            let config = RealtimeConfig {
                api_key: "test-key".to_string(),
                // Azure needs a deployment name + resource endpoint to construct;
                // the rest ignore the extra fields. Keeps every provider in the
                // matrix (this test is about the SEND path, not construction).
                model: "gpt-realtime".to_string(),
                endpoint: Some("https://my-resource.openai.azure.com".to_string()),
                ..Default::default()
            };
            let mut rt = create_realtime_provider(provider, config)
                .unwrap_or_else(|e| panic!("construct `{provider}`: {e}"));
            // None of these may panic on an un-connected provider. The result is
            // intentionally ignored: graceful Err (typically NotConnected) — or
            // an Ok that merely buffers preroll — are both acceptable; a panic is
            // not. `is_ready()` must report not-ready.
            assert!(!rt.is_ready(), "`{provider}` must not be ready before connect");
            let _ = rt.send_audio(bytes::Bytes::from_static(&[0u8; 320])).await;
            let _ = rt.send_text("hello").await;
            let _ = rt.create_response().await;
            let _ = rt.commit_audio_buffer().await;
            // Disconnect on an un-connected session must also be a safe no-op.
            let _ = rt.disconnect().await;
        }
    }
}
