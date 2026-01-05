//! OpenAI Realtime API module.
//!
//! This module provides real-time audio-to-audio streaming using OpenAI's Realtime API.
//!
//! # Features
//!
//! - Bidirectional audio streaming
//! - Real-time speech-to-text transcription
//! - Text-to-speech synthesis
//! - Voice Activity Detection (VAD)
//! - Function calling support
//!
//! # Supported Models
//!
//! - `gpt-4o-realtime-preview` - GPT-4o Realtime Preview
//! - `gpt-4o-realtime-preview-2024-10-01` - October 2024 version
//! - `gpt-4o-realtime-preview-2024-12-17` - December 2024 version
//! - `gpt-4o-mini-realtime-preview` - Mini model for lower latency
//!
//! # Supported Voices
//!
//! alloy, ash, ballad, coral, echo, sage, shimmer, verse
//!
//! # Audio Format
//!
//! Input and output audio is PCM 16-bit signed little-endian at 24kHz.
//! G.711 u-law and a-law are also supported at 8kHz.
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
//!         model: "gpt-4o-realtime-preview".to_string(),
//!         voice: Some("alloy".to_string()),
//!         instructions: Some("You are a helpful assistant.".to_string()),
//!         ..Default::default()
//!     };
//!
//!     let mut realtime = OpenAIRealtime::new(config).unwrap();
//!
//!     // Register callbacks
//!     realtime.on_transcript(Arc::new(|t| Box::pin(async move {
//!         println!("[{}] {}", t.role, t.text);
//!     }))).unwrap();
//!
//!     realtime.on_audio(Arc::new(|audio| Box::pin(async move {
//!         // Play audio.data
//!     }))).unwrap();
//!
//!     // Connect
//!     realtime.connect().await.unwrap();
//!
//!     // Send audio
//!     realtime.send_audio(audio_bytes).await.unwrap();
//! }
//! ```

mod client;
mod config;
mod messages;

pub use client::OpenAIRealtime;
pub use config::{
    Modality, OPENAI_REALTIME_SAMPLE_RATE, OPENAI_REALTIME_URL, OpenAIRealtimeAudioFormat,
    OpenAIRealtimeModel, OpenAIRealtimeVoice,
};
pub use messages::{
    ClientEvent, ConversationItem, ResponseConfig, ServerEvent, SessionConfig, TurnDetection,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::realtime::base::{BaseRealtime, ConnectionState, RealtimeConfig};

    #[tokio::test]
    async fn test_openai_realtime_default_creation() {
        let realtime = OpenAIRealtime::default();
        assert!(!realtime.is_ready());
        assert_eq!(
            realtime.get_connection_state(),
            ConnectionState::Disconnected
        );
    }

    #[tokio::test]
    async fn test_openai_realtime_with_config() {
        let config = RealtimeConfig {
            api_key: "test_key".to_string(),
            model: "gpt-4o-mini-realtime-preview".to_string(),
            voice: Some("shimmer".to_string()),
            instructions: Some("Test instructions".to_string()),
            ..Default::default()
        };

        let realtime = OpenAIRealtime::new(config).unwrap();
        assert_eq!(
            realtime.model(),
            OpenAIRealtimeModel::Gpt4oMiniRealtimePreview
        );
        assert_eq!(realtime.voice(), OpenAIRealtimeVoice::Shimmer);
    }

    #[test]
    fn test_model_parsing() {
        assert_eq!(
            OpenAIRealtimeModel::from_str_or_default("gpt-4o-realtime-preview"),
            OpenAIRealtimeModel::Gpt4oRealtimePreview
        );
        assert_eq!(
            OpenAIRealtimeModel::from_str_or_default("gpt-4o-mini-realtime-preview"),
            OpenAIRealtimeModel::Gpt4oMiniRealtimePreview
        );
        assert_eq!(
            OpenAIRealtimeModel::from_str_or_default("unknown"),
            OpenAIRealtimeModel::Gpt4oRealtimePreview
        );
    }

    #[test]
    fn test_voice_parsing() {
        assert_eq!(
            OpenAIRealtimeVoice::from_str_or_default("alloy"),
            OpenAIRealtimeVoice::Alloy
        );
        assert_eq!(
            OpenAIRealtimeVoice::from_str_or_default("SHIMMER"),
            OpenAIRealtimeVoice::Shimmer
        );
        assert_eq!(
            OpenAIRealtimeVoice::from_str_or_default("unknown"),
            OpenAIRealtimeVoice::Alloy
        );
    }

    #[test]
    fn test_audio_format_sample_rate() {
        assert_eq!(OpenAIRealtimeAudioFormat::Pcm16.sample_rate(), 24000);
        assert_eq!(OpenAIRealtimeAudioFormat::G711Ulaw.sample_rate(), 8000);
    }

    #[test]
    fn test_realtime_url() {
        assert_eq!(OPENAI_REALTIME_URL, "wss://api.openai.com/v1/realtime");
    }

    #[test]
    fn test_sample_rate() {
        assert_eq!(OPENAI_REALTIME_SAMPLE_RATE, 24000);
    }
}
