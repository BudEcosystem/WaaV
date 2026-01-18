//! SberDevices SaluteSpeech STT Module
//!
//! Provides speech-to-text functionality using SberDevices SaluteSpeech API.
//!
//! ## Features
//!
//! - OAuth 2.0 authentication with automatic token refresh
//! - Synchronous REST API for recognition
//! - Pseudo-streaming through audio buffering
//! - Support for Russian, English, Kazakh, Kyrgyz, Uzbek
//! - Multiple audio formats: PCM, OPUS, MP3, FLAC, A-law, Mu-law
//!
//! ## Usage
//!
//! ```ignore
//! use crate::core::stt::sberdevices::SberDevicesSTT;
//! use crate::core::stt::base::{BaseSTT, STTConfig};
//!
//! let config = STTConfig {
//!     provider: "sberdevices".to_string(),
//!     api_key: "client_id:client_secret".to_string(),  // Or Base64 encoded
//!     language: "ru-RU".to_string(),
//!     sample_rate: 16000,
//!     encoding: "pcm".to_string(),
//!     model: "SALUTE_SPEECH_PERS".to_string(),  // Scope
//!     ..Default::default()
//! };
//!
//! let mut stt = SberDevicesSTT::new(config)?;
//! stt.connect().await?;
//! stt.send_audio(audio_bytes).await?;
//! stt.disconnect().await?;
//! ```
//!
//! ## Authentication
//!
//! SberDevices uses OAuth 2.0 with client credentials. The API key should be
//! provided in one of two formats:
//!
//! 1. Raw: `client_id:client_secret` - will be Base64 encoded automatically
//! 2. Pre-encoded: Already Base64 encoded credentials
//!
//! ## Scopes
//!
//! The model field is used to specify the OAuth scope:
//!
//! - `SALUTE_SPEECH_PERS` - Personal (individuals, 5 concurrent streams)
//! - `SALUTE_SPEECH_CORP` - Corporate (organizations, 10 concurrent streams)
//! - `SALUTE_SPEECH_B2B` - B2B (prepaid)
//! - `SBER_SPEECH` - Legacy enterprise

mod client;
mod config;
mod messages;

// Re-export public types
pub use client::SberDevicesSTT;
pub use config::{
    SberSTTAudioFormat, SberSTTConfig, SberSTTLanguage, SberScope,
    // Constants
    DEFAULT_SAMPLE_RATE, MAX_SYNC_AUDIO_DURATION_SECS, MAX_SYNC_AUDIO_SIZE, OAUTH_ENDPOINT,
    STT_RECOGNIZE_ENDPOINT, TOKEN_REFRESH_THRESHOLD_SECS, TOKEN_VALIDITY_SECS,
};
pub use messages::{
    OAuthTokenRequest, OAuthTokenResponse, SberApiError, SberRecognitionResponse, SberStatusCode,
};
