//! Amazon Polly TTS provider module.
//!
//! This module provides text-to-speech synthesis using Amazon Polly.
//! It supports:
//!
//! - 60+ voices across 30+ languages
//! - Multiple engines (standard, neural, long-form, generative)
//! - Output formats: mp3, ogg_vorbis, pcm
//! - SSML input for fine-grained control
//! - AWS credential management (explicit keys, IAM roles, etc.)
//!
//! # Architecture
//!
//! The provider uses the AWS SDK for Rust to communicate with Amazon Polly's
//! SynthesizeSpeech API. Unlike HTTP-based TTS providers, this implementation
//! uses the AWS SDK which handles request signing and credential management.
//!
//! # Authentication
//!
//! AWS credentials can be provided via:
//! 1. `aws_access_key_id` and `aws_secret_access_key` in config
//! 2. Environment variables: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`
//! 3. AWS credentials file (`~/.aws/credentials`)
//! 4. IAM instance profile (for EC2/ECS/Lambda)
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig};
//! use waav_gateway::core::tts::aws_polly::{AwsPollyTTS, PollyVoice, PollyEngine};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = TTSConfig {
//!         provider: "aws-polly".to_string(),
//!         voice_id: Some("Joanna".to_string()),
//!         model: "neural".to_string(),
//!         audio_format: Some("pcm".to_string()),
//!         sample_rate: Some(16000),
//!         ..Default::default()
//!     };
//!
//!     let mut tts = AwsPollyTTS::new(config)?;
//!     tts.connect().await?;
//!
//!     // Register result callback
//!     let callback = std::sync::Arc::new(MyAudioCallback);
//!     tts.on_audio(callback)?;
//!
//!     // Synthesize text
//!     tts.speak("Hello from Amazon Polly!", true).await?;
//!
//!     tts.disconnect().await?;
//!     Ok(())
//! }
//!
//! # struct MyAudioCallback;
//! # impl waav_gateway::core::tts::AudioCallback for MyAudioCallback {
//! #     fn on_audio(&self, _: waav_gateway::core::tts::AudioData) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
//! #         Box::pin(async {})
//! #     }
//! #     fn on_error(&self, _: waav_gateway::core::tts::TTSError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
//! #         Box::pin(async {})
//! #     }
//! #     fn on_complete(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
//! #         Box::pin(async {})
//! #     }
//! # }
//! ```
//!
//! # Best Practices
//!
//! 1. **Engine Selection**: Use neural for best quality, standard for lowest cost
//! 2. **Output Format**: Use PCM for lowest latency, MP3 for bandwidth efficiency
//! 3. **Text Length**: Keep under 3000 characters per request
//! 4. **SSML**: Use SSML for fine-grained control over pronunciation
//! 5. **Caching**: Cache synthesized audio for repeated phrases
//!
//! # Pricing Considerations
//!
//! - Neural voices cost more than standard voices
//! - Pricing is per character synthesized
//! - Free tier: 5 million characters/month for standard, 1 million for neural

mod config;
mod provider;

#[cfg(test)]
mod tests;

pub use config::{
    AwsPollyTTSConfig, MAX_TEXT_LENGTH, MAX_TOTAL_LENGTH, PollyEngine, PollyOutputFormat,
    PollyVoice, TextType,
};
pub use provider::{AWS_POLLY_TTS_URL, AwsPollyTTS};
