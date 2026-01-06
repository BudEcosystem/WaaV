//! IBM Watson Text-to-Speech provider implementation.
//!
//! This module provides a real-time text-to-speech integration with IBM Watson
//! Text-to-Speech API using HTTP REST calls.
//!
//! # Features
//!
//! - Multiple V3 neural voices across 15+ languages
//! - Multiple audio output formats (wav, mp3, ogg, flac, webm, pcm)
//! - IAM token-based authentication with automatic refresh
//! - SSML support for fine-grained control
//! - Rate and pitch adjustment
//! - Custom pronunciation dictionaries
//! - Connection pooling for efficient HTTP requests
//!
//! # IBM Watson Regions
//!
//! IBM Watson Text-to-Speech is available in the following regions:
//!
//! | Region | Location |
//! |--------|----------|
//! | `us-south` | Dallas, Texas (Default) |
//! | `us-east` | Washington, D.C. |
//! | `eu-de` | Frankfurt, Germany |
//! | `eu-gb` | London, UK |
//! | `au-syd` | Sydney, Australia |
//! | `jp-tok` | Tokyo, Japan |
//! | `kr-seo` | Seoul, South Korea |
//!
//! # Available Voices
//!
//! IBM Watson provides V3 neural voices optimized for natural speech:
//!
//! - **US English**: Allison, Emily, Henry, Kevin, Lisa, Michael, Olivia
//! - **UK English**: Charlotte, James, Kate
//! - **Australian English**: Craig, Madison
//! - **German**: Birgit, Dieter, Erika
//! - **Spanish**: Enrique, Laura, Sofia (Castilian, Latin American, North American)
//! - **French**: Nicolas, Renee, Louise (Canadian)
//! - **Italian**: Francesca
//! - **Japanese**: Emi
//! - **Korean**: Hyunjun, Siwoo, Youngmi, Yuna
//! - **Dutch**: Emma, Liam
//! - **Portuguese**: Isabela
//! - **Chinese**: LiNa, WangWei, ZhangJing
//!
//! # Configuration
//!
//! ## Environment Variables
//!
//! ```bash
//! export IBM_WATSON_API_KEY="your-api-key"
//! export IBM_WATSON_INSTANCE_ID="your-instance-id"
//! export IBM_WATSON_REGION="us-south"  # Optional, defaults to us-south
//! ```
//!
//! ## WebSocket Configuration Message
//!
//! ```json
//! {
//!   "type": "config",
//!   "config": {
//!     "tts_provider": "ibm-watson",
//!     "ibm_watson_instance_id": "your-instance-id",
//!     "ibm_watson_region": "us-south",
//!     "ibm_watson_voice": "en-US_AllisonV3Voice",
//!     "ibm_watson_format": "ogg-opus"
//!   }
//! }
//! ```
//!
//! # Example Usage
//!
//! ```rust,no_run
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig};
//! use waav_gateway::core::tts::ibm_watson::{IbmWatsonTTS, IbmVoice, IbmOutputFormat};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create base configuration
//!     let config = TTSConfig {
//!         api_key: std::env::var("IBM_WATSON_API_KEY")?,
//!         voice_id: Some("en-US_AllisonV3Voice".to_string()),
//!         audio_format: Some("ogg-opus".to_string()),
//!         ..Default::default()
//!     };
//!
//!     // Create IBM Watson TTS instance
//!     let mut tts = IbmWatsonTTS::new(config)?;
//!
//!     // Configure IBM-specific settings
//!     tts.set_instance_id(std::env::var("IBM_WATSON_INSTANCE_ID")?);
//!     tts.set_voice(IbmVoice::EnUsAllisonV3Voice);
//!     tts.set_output_format(IbmOutputFormat::OggOpus);
//!
//!     // Connect to IBM Watson
//!     tts.connect().await?;
//!
//!     // Synthesize text
//!     tts.speak("Hello, world! This is IBM Watson TTS.", true).await?;
//!
//!     // Disconnect when done
//!     tts.disconnect().await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Advanced Features
//!
//! ## Rate and Pitch Control
//!
//! ```rust,ignore
//! // Speed up speech by 25%
//! tts.set_rate_percentage(25)?;
//!
//! // Lower pitch by 15%
//! tts.set_pitch_percentage(-15)?;
//! ```
//!
//! ## Custom Pronunciations
//!
//! Custom pronunciation dictionaries can be applied by specifying customization IDs
//! in the configuration.
//!
//! # Audio Format Details
//!
//! | Format | Extension | Description |
//! |--------|-----------|-------------|
//! | WAV | .wav | PCM in WAV container (default) |
//! | MP3 | .mp3 | MPEG Layer-3 compressed |
//! | OGG Opus | .ogg | Opus codec in OGG (best quality/size) |
//! | OGG Vorbis | .ogg | Vorbis codec in OGG |
//! | FLAC | .flac | Free Lossless Audio Codec |
//! | WebM | .webm | WebM container with Opus |
//! | L16 | .raw | Raw 16-bit PCM |
//! | μ-law | .raw | μ-law 8kHz telephony |
//! | A-law | .raw | A-law 8kHz telephony |
//!
//! # References
//!
//! - [IBM Watson TTS Documentation](https://cloud.ibm.com/docs/text-to-speech)
//! - [API Reference](https://cloud.ibm.com/apidocs/text-to-speech)
//! - [Voices Documentation](https://cloud.ibm.com/docs/text-to-speech?topic=text-to-speech-voices)

pub mod config;
mod provider;

#[cfg(test)]
mod tests;

pub use config::{
    IbmOutputFormat, IbmVoice, IbmWatsonTTSConfig, DEFAULT_SAMPLE_RATE, DEFAULT_VOICE,
    IBM_IAM_URL, MAX_TEXT_LENGTH,
};
pub use provider::{IbmWatsonTTS, IBM_WATSON_TTS_URL};
