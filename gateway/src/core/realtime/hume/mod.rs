//! Hume AI Realtime Module - Empathic Voice Interface (EVI).
//!
//! This module provides real-time bidirectional audio streaming with
//! Hume's Empathic Voice Interface (EVI), which offers emotional intelligence
//! through 48-dimension prosody analysis.
//!
//! # Features
//!
//! - **Full-duplex audio streaming**: Send and receive audio simultaneously
//! - **Prosody analysis**: 48 emotion dimensions detected in speech
//! - **Empathic responses**: AI responses adapted to user's emotional state
//! - **Function calling**: Tool use support for extending capabilities
//! - **Conversation continuity**: Resume conversations with chat group IDs
//!
//! # EVI Versions
//!
//! - **EVI 1/2**: Deprecated, sunset August 30, 2025
//! - **EVI 3**: Current version, English only
//! - **EVI 4-mini**: Multilingual (11 languages), lower latency
//!
//! # Audio Format
//!
//! - **Input**: Linear16 PCM (44.1kHz, mono) or WebM
//! - **Output**: Base64-encoded WAV
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::realtime::hume::{HumeEVI, HumeEVIConfig, EVIVersion};
//! use waav_gateway::core::realtime::BaseRealtime;
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = HumeEVIConfig::new("your-api-key")
//!         .with_config_id("your-config-id")
//!         .with_version(EVIVersion::V3)
//!         .with_voice("kora");
//!
//!     let mut evi = HumeEVI::from_hume_config(config)?;
//!
//!     // Register callbacks
//!     evi.on_transcript(Arc::new(|t| Box::pin(async move {
//!         println!("[{}] {}", t.role, t.text);
//!     })))?;
//!
//!     evi.on_audio(Arc::new(|audio| Box::pin(async move {
//!         println!("Received {} bytes of audio", audio.data.len());
//!     })))?;
//!
//!     // Connect
//!     evi.connect().await?;
//!
//!     // Send audio
//!     let audio_chunk = vec![0u8; 4410]; // 100ms of audio at 44.1kHz
//!     evi.send_audio(audio_chunk.into()).await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Prosody Analysis
//!
//! EVI provides 48 emotion dimensions in the prosody scores:
//!
//! - **Primary emotions**: Joy, Sadness, Anger, Fear, Surprise, Disgust
//! - **Social emotions**: Admiration, Gratitude, Pride, Shame, Guilt
//! - **Cognitive states**: Concentration, Confusion, Realization, Interest
//! - **And many more**: See [`ProsodyScores`] for the full list
//!
//! ```rust,ignore
//! use waav_gateway::core::realtime::hume::ProsodyScores;
//!
//! fn analyze_emotions(scores: &ProsodyScores) {
//!     // Get the dominant emotion
//!     if let Some((name, score)) = scores.dominant_emotion() {
//!         println!("Dominant emotion: {} ({:.2})", name, score);
//!     }
//!
//!     // Get top 3 emotions
//!     for (name, score) in scores.top_emotions(3) {
//!         println!("  {}: {:.2}", name, score);
//!     }
//! }
//! ```
//!
//! # Function Calling
//!
//! EVI supports function calling for extending capabilities:
//!
//! ```rust,ignore
//! evi.on_function_call(Arc::new(|call| Box::pin(async move {
//!     println!("Function call: {} with args: {}", call.name, call.arguments);
//!     // Handle the function call
//! })))?;
//!
//! // Submit function result
//! evi.submit_function_result(&call_id, r#"{"result": "success"}"#).await?;
//! ```
//!
//! # Conversation Continuity
//!
//! Resume a previous conversation using chat group ID:
//!
//! ```rust,ignore
//! // First session
//! let mut evi = HumeEVI::from_hume_config(config)?;
//! evi.connect().await?;
//! let chat_group_id = evi.get_chat_group_id().await.unwrap();
//!
//! // Later session - resume the conversation
//! let config = HumeEVIConfig::new("your-api-key")
//!     .with_chat_group(&chat_group_id);
//! let mut evi = HumeEVI::from_hume_config(config)?;
//! evi.connect().await?;
//! ```

mod client;
mod config;
pub mod messages;

pub use client::HumeEVI;
pub use config::{EVIVersion, HumeEVIConfig};
pub use messages::{
    AudioEncoding, AudioInput, AudioOutput, AudioSettings, EVIClientMessage, EVIServerMessage,
    ProsodyScores, SessionSettings, TextInput, ToolResponse,
    HUME_EVI_DEFAULT_CHANNELS, HUME_EVI_DEFAULT_SAMPLE_RATE, HUME_EVI_MAX_SESSION_DURATION,
    HUME_EVI_WEBSOCKET_URL,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_exports() {
        // Verify all expected types are accessible
        let _version = EVIVersion::V3;
        let _config = HumeEVIConfig::default();
        let _encoding = AudioEncoding::Linear16;

        // Constants accessible
        assert_eq!(HUME_EVI_DEFAULT_SAMPLE_RATE, 44100);
        assert_eq!(HUME_EVI_DEFAULT_CHANNELS, 1);
        assert_eq!(HUME_EVI_MAX_SESSION_DURATION, 1800);
    }

    #[test]
    fn test_prosody_scores_accessible() {
        let scores = ProsodyScores::default();
        assert_eq!(scores.joy, 0.0);
        assert_eq!(scores.anger, 0.0);

        let top = scores.top_emotions(3);
        assert_eq!(top.len(), 3);
    }

    #[test]
    fn test_client_accessible() {
        let config = HumeEVIConfig::new("test-key");
        let result = HumeEVI::from_hume_config(config);
        assert!(result.is_ok());
    }
}
