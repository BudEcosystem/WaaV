//! Realtime audio-to-audio provider module.
//!
//! This module provides abstractions and implementations for real-time
//! bidirectional audio streaming with transcription and TTS.
//!
//! # Supported Providers
//!
//! - **OpenAI Realtime API** - Full duplex audio with GPT-4o
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
//! All providers use PCM 16-bit signed little-endian at 24kHz by default.
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
pub mod openai;

pub use base::{
    AudioOutputCallback, BaseRealtime, BoxedRealtime, ConnectionState, FunctionCallCallback,
    FunctionCallRequest, FunctionDefinition, InputTranscriptionConfig, RealtimeAudioData,
    RealtimeConfig, RealtimeError, RealtimeErrorCallback, RealtimeFactory, RealtimeResult,
    ReconnectionCallback, ReconnectionEvent, ResponseDoneCallback, SpeechEvent,
    SpeechEventCallback, ToolDefinition, TranscriptCallback, TranscriptResult, TranscriptRole,
    TurnDetectionConfig,
};
pub use openai::{
    Modality, OPENAI_REALTIME_SAMPLE_RATE, OPENAI_REALTIME_URL, OpenAIRealtime,
    OpenAIRealtimeAudioFormat, OpenAIRealtimeModel, OpenAIRealtimeVoice,
};

/// Supported realtime providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeProvider {
    /// OpenAI Realtime API
    OpenAI,
}

impl RealtimeProvider {
    /// Parse provider from string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "openai" => Some(RealtimeProvider::OpenAI),
            _ => None,
        }
    }
}

impl std::fmt::Display for RealtimeProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RealtimeProvider::OpenAI => write!(f, "openai"),
        }
    }
}

/// Factory function to create a realtime provider.
///
/// # Supported Providers
///
/// - `"openai"` - OpenAI Realtime API (gpt-4o-realtime-preview)
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
    match provider_type.to_lowercase().as_str() {
        "openai" => Ok(Box::new(OpenAIRealtime::new(config)?)),
        _ => Err(RealtimeError::InvalidConfiguration(format!(
            "Unsupported realtime provider: {provider_type}. Supported providers: openai"
        ))),
    }
}

/// Create a realtime provider from enum.
pub fn create_realtime_provider_from_enum(
    provider: RealtimeProvider,
    config: RealtimeConfig,
) -> RealtimeResult<Box<dyn BaseRealtime>> {
    match provider {
        RealtimeProvider::OpenAI => Ok(Box::new(OpenAIRealtime::new(config)?)),
    }
}

/// Get list of supported realtime providers.
pub fn get_supported_realtime_providers() -> Vec<&'static str> {
    vec!["openai"]
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
        assert_eq!(providers.len(), 1);
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
        assert_eq!(RealtimeProvider::parse("invalid"), None);
    }

    #[test]
    fn test_provider_display() {
        assert_eq!(RealtimeProvider::OpenAI.to_string(), "openai");
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
            }
            _ => panic!("Expected InvalidConfiguration error"),
        }
    }
}
