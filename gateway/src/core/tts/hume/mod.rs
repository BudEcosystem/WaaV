//! Hume AI Octave Text-to-Speech provider.
//!
//! This module provides integration with Hume AI's Octave TTS REST API for
//! high-quality speech synthesis with natural language emotion control.
//!
//! # Architecture
//!
//! The Hume TTS provider follows WaaV Gateway's HTTP-based TTS pattern:
//!
//! ```text
//! HumeTTS
//!     │
//!     └── TTSProvider (generic HTTP infrastructure)
//!             │
//!             ├── ReqManager (connection pooling)
//!             ├── Dispatcher (ordered audio delivery)
//!             └── QueueWorker (sequential request processing)
//! ```
//!
//! The module is organized into:
//! - **config**: Hume-specific configuration types
//!   - `HumeTTSConfig` - Provider configuration
//!   - `HumeAudioFormat` - Audio format enum
//!   - `HumeOutputFormat` - Output format specification
//!   - `HumeVoice` - Voice enum
//!
//! - **messages**: API request/response types
//!   - `HumeTTSRequest` - Complete request body
//!   - `HumeUtterance` - Single utterance with voice/emotion
//!   - `HumeVoiceSpec` - Voice specification (by name or ID)
//!   - `HumeRequestFormat` - Output format for requests
//!
//! - **provider**: Request builder and main provider
//!   - `HumeRequestBuilder` - HTTP request construction
//!   - `HumeTTS` - Main provider implementing `BaseTTS`
//!
//! # Key Features
//!
//! - **Natural Language Emotions**: Control emotion via `description` field
//!   - Examples: "happy, energetic", "sad, melancholic", "whispered fearfully"
//!   - Max 100 characters
//! - **Speed Control**: 0.5 to 2.0 range
//! - **Instant Mode**: Low-latency streaming (default: enabled)
//! - **Context Continuity**: Maintain voice consistency via generation_id
//! - **Voice Cloning**: Custom voices via cloned voice IDs
//!
//! # Quick Start
//!
//! ## Via Factory Function (Recommended)
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::{create_tts_provider, TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     provider: "hume".to_string(),
//!     api_key: std::env::var("HUME_API_KEY").unwrap(),
//!     voice_id: Some("Kora".to_string()),
//!     audio_format: Some("linear16".to_string()),
//!     sample_rate: Some(24000),
//!     ..Default::default()
//! };
//!
//! let mut tts = create_tts_provider("hume", config)?;
//! tts.connect().await?;
//! tts.speak("Hello, world!", true).await?;
//! ```
//!
//! ## Direct Instantiation with Emotion Control
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::hume::{HumeTTS, HumeTTSConfig};
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let base = TTSConfig {
//!     api_key: std::env::var("HUME_API_KEY").unwrap(),
//!     voice_id: Some("Kora".to_string()),
//!     ..Default::default()
//! };
//!
//! let config = HumeTTSConfig::from_base(base)
//!     .with_description("warm, friendly, inviting")
//!     .with_speed(1.0);
//!
//! let mut tts = HumeTTS::with_config(config)?;
//! tts.connect().await?;
//! tts.speak("Hello! How can I help you today?", true).await?;
//! tts.disconnect().await?;
//! ```
//!
//! ## Dynamic Emotion Changes
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::hume::HumeTTS;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let mut tts = HumeTTS::new(TTSConfig {
//!     api_key: "your-api-key".to_string(),
//!     voice_id: Some("Kora".to_string()),
//!     ..Default::default()
//! })?;
//!
//! tts.connect().await?;
//!
//! // Happy greeting
//! tts.set_description("happy, energetic, welcoming");
//! tts.speak("Welcome to our service!", true).await?;
//!
//! // Empathetic response
//! tts.set_description("calm, empathetic, understanding");
//! tts.speak("I understand that can be frustrating.", true).await?;
//!
//! // Clear emotion for neutral speech
//! tts.clear_description();
//! tts.speak("Your account balance is $100.", true).await?;
//!
//! tts.disconnect().await?;
//! ```
//!
//! # Authentication
//!
//! Hume uses API key authentication:
//!
//! ```http
//! X-Hume-Api-Key: <HUME_API_KEY>
//! ```
//!
//! Set the API key via:
//! - Environment variable: `HUME_API_KEY`
//! - YAML config: `providers.hume_api_key`
//! - Direct: `TTSConfig.api_key`
//!
//! # Supported Audio Formats
//!
//! | Format | Description | Use Case |
//! |--------|-------------|----------|
//! | `pcm16` | 16-bit PCM | Real-time streaming (default) |
//! | `mp3` | MP3 compressed | Bandwidth optimization |
//! | `wav` | WAV with headers | File storage |
//! | `mulaw` | μ-law companding | US telephony |
//! | `alaw` | A-law companding | European telephony |
//!
//! # Emotion Control
//!
//! Hume's key differentiator is natural language emotion control. Instead of
//! SSML tags or predefined emotion enums, you describe the desired emotion
//! and speaking style in plain language:
//!
//! ## Examples
//!
//! | Description | Effect |
//! |-------------|--------|
//! | "happy, energetic" | Upbeat, enthusiastic delivery |
//! | "sad, melancholic" | Slow, somber tone |
//! | "whispered fearfully" | Quiet, nervous speech |
//! | "warm, inviting, professional" | Friendly business tone |
//! | "sarcastic, dry" | Deadpan humor |
//! | "excited, rushed" | Fast, breathless speech |
//!
//! ## Best Practices
//!
//! 1. Keep descriptions concise (max 100 chars)
//! 2. Combine emotion with delivery style: "calm, measured, authoritative"
//! 3. Use commas to separate attributes
//! 4. Test different phrasings to find optimal results
//!
//! # API Reference
//!
//! - Streaming Endpoint: `https://api.hume.ai/v0/tts/stream/file`
//! - Sync Endpoint: `https://api.hume.ai/v0/tts/file`
//! - Authentication: X-Hume-Api-Key header
//! - Documentation: <https://dev.hume.ai/docs/text-to-speech-tts/overview>
//!
//! # See Also
//!
//! - [`crate::core::tts::BaseTTS`] - Base trait for TTS providers
//! - [`crate::core::tts::create_tts_provider`] - Factory function
//! - [`crate::core::realtime::hume`] - Hume EVI (Audio-to-Audio) provider

mod config;
mod messages;
mod provider;

// =============================================================================
// Public Re-exports
// =============================================================================

// Configuration types
pub use config::{
    HumeAudioFormat, HumeOutputFormat, HumeTTSConfig, HumeVoice,
    HUME_TTS_STREAM_URL, HUME_TTS_SYNC_URL,
    DEFAULT_SPEED, DEFAULT_VOICE, MAX_DESCRIPTION_LENGTH,
    MAX_SPEED, MIN_SPEED, SUPPORTED_SAMPLE_RATES,
};

// Message types
pub use messages::{
    HumeContext, HumeErrorResponse, HumeRequestFormat, HumeTTSMetadata,
    HumeTTSRequest, HumeUtterance, HumeVoiceSpec,
};

// Provider types
pub use provider::{HumeRequestBuilder, HumeTTS};
