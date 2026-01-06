//! Amazon Polly TTS provider implementation.
//!
//! This module provides the Amazon Polly TTS provider that implements the `BaseTTS` trait
//! using Amazon Polly's SynthesizeSpeech API via the AWS SDK for Rust.
//!
//! # API Reference
//!
//! - Service: Amazon Polly
//! - Operation: SynthesizeSpeech
//! - Engines: standard, neural, long-form, generative
//! - Voices: 60+ voices across 30+ languages
//! - Output formats: mp3, ogg_vorbis, pcm (16-bit signed little-endian)
//! - Sample rates: mp3/ogg (8000, 16000, 22050, 24000), pcm (8000, 16000)
//!
//! # Authentication
//!
//! AWS credentials can be provided via:
//! 1. `aws_access_key_id` and `aws_secret_access_key` fields in config
//! 2. Environment variables: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`
//! 3. AWS credentials file (`~/.aws/credentials`)
//! 4. IAM instance profile (for EC2/ECS/Lambda)
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::{BaseTTS, TTSConfig};
//! use waav_gateway::core::tts::aws_polly::{AwsPollyTTS, AwsPollyTTSConfig, PollyVoice, PollyEngine};
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = AwsPollyTTSConfig {
//!         voice: PollyVoice::Joanna,
//!         engine: PollyEngine::Neural,
//!         ..Default::default()
//!     };
//!
//!     let mut tts = AwsPollyTTS::new_from_polly_config(config).unwrap();
//!     tts.connect().await.unwrap();
//!
//!     // Register audio callback
//!     // tts.on_audio(Arc::new(MyCallback)).unwrap();
//!
//!     // Synthesize text
//!     tts.speak("Hello, world!", true).await.unwrap();
//! }
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_polly::Client as PollyClient;
use aws_sdk_polly::config::Builder as PollyConfigBuilder;
use aws_sdk_polly::primitives::ByteStream;
use aws_sdk_polly::types::{Engine, OutputFormat, TextType as PollyTextType, VoiceId};
use bytes::Bytes;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::config::{
    AwsPollyTTSConfig, MAX_TEXT_LENGTH, PollyEngine, PollyOutputFormat, PollyVoice, TextType,
};
use crate::core::stt::aws_transcribe::AwsRegion;
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use crate::utils::req_manager::ReqManager;

/// Amazon Polly TTS API base URL (for documentation purposes)
pub const AWS_POLLY_TTS_URL: &str = "https://polly.{region}.amazonaws.com/v1/speech";

// =============================================================================
// Helper Functions
// =============================================================================

/// Convert PollyEngine to AWS SDK Engine type
fn engine_to_sdk(engine: PollyEngine) -> Engine {
    match engine {
        PollyEngine::Standard => Engine::Standard,
        PollyEngine::Neural => Engine::Neural,
        PollyEngine::LongForm => Engine::LongForm,
        PollyEngine::Generative => Engine::Generative,
    }
}

/// Convert PollyOutputFormat to AWS SDK OutputFormat type
fn output_format_to_sdk(format: PollyOutputFormat) -> OutputFormat {
    match format {
        PollyOutputFormat::Mp3 => OutputFormat::Mp3,
        PollyOutputFormat::OggVorbis => OutputFormat::OggVorbis,
        PollyOutputFormat::Pcm => OutputFormat::Pcm,
    }
}

/// Convert TextType to AWS SDK TextType
fn text_type_to_sdk(text_type: TextType) -> PollyTextType {
    match text_type {
        TextType::Text => PollyTextType::Text,
        TextType::Ssml => PollyTextType::Ssml,
    }
}

/// Convert PollyVoice to AWS SDK VoiceId
fn voice_to_sdk(voice: &PollyVoice) -> VoiceId {
    match voice {
        // US English
        PollyVoice::Joanna => VoiceId::Joanna,
        PollyVoice::Matthew => VoiceId::Matthew,
        PollyVoice::Salli => VoiceId::Salli,
        PollyVoice::Kendra => VoiceId::Kendra,
        PollyVoice::Kimberly => VoiceId::Kimberly,
        PollyVoice::Joey => VoiceId::Joey,
        PollyVoice::Ruth => VoiceId::Ruth,
        PollyVoice::Stephen => VoiceId::Stephen,
        PollyVoice::Kevin => VoiceId::Kevin,
        PollyVoice::Ivy => VoiceId::Ivy,
        PollyVoice::Justin => VoiceId::Justin,
        // UK English
        PollyVoice::Amy => VoiceId::Amy,
        PollyVoice::Emma => VoiceId::Emma,
        PollyVoice::Brian => VoiceId::Brian,
        PollyVoice::Arthur => VoiceId::Arthur,
        // Australian
        PollyVoice::Olivia => VoiceId::Olivia,
        // French
        PollyVoice::Lea => VoiceId::Lea,
        // German
        PollyVoice::Hans => VoiceId::Hans,
        PollyVoice::Vicki => VoiceId::Vicki,
        // Japanese
        PollyVoice::Mizuki => VoiceId::Mizuki,
        PollyVoice::Takumi => VoiceId::Takumi,
        // Korean
        PollyVoice::Seoyeon => VoiceId::Seoyeon,
        // Chinese
        PollyVoice::Zhiyu => VoiceId::Zhiyu,
        // Portuguese
        PollyVoice::Camila => VoiceId::Camila,
        PollyVoice::Vitoria => VoiceId::Vitoria,
        // Spanish
        PollyVoice::Lupe => VoiceId::Lupe,
        PollyVoice::Pedro => VoiceId::Pedro,
        PollyVoice::Lucia => VoiceId::Lucia,
        PollyVoice::Enrique => VoiceId::Enrique,
        PollyVoice::Mia => VoiceId::Mia,
        // Italian
        PollyVoice::Bianca => VoiceId::Bianca,
        PollyVoice::Adriano => VoiceId::Adriano,
        // Polish
        PollyVoice::Ola => VoiceId::Ola,
        // Hindi
        PollyVoice::Kajal => VoiceId::Kajal,
        // New Zealand
        PollyVoice::Aria => VoiceId::Aria,
        // Custom - use from string
        PollyVoice::Custom(id) => VoiceId::from(id.as_str()),
    }
}

/// Convert AwsRegion to AWS SDK Region
fn region_to_sdk(region: AwsRegion) -> Region {
    Region::new(region.as_str())
}

// =============================================================================
// Amazon Polly TTS Provider
// =============================================================================

/// Amazon Polly TTS provider implementation using AWS SDK.
///
/// This provider uses the AWS SDK for Rust to communicate with Amazon Polly's
/// SynthesizeSpeech API. It supports:
/// - Multiple voices (60+ across 30+ languages)
/// - Multiple engines (standard, neural, long-form, generative)
/// - Multiple output formats (mp3, ogg_vorbis, pcm)
/// - SSML input for fine-grained control
/// - AWS credential management (explicit keys, IAM roles, etc.)
///
/// Unlike HTTP-based providers, this implementation directly uses the AWS SDK
/// which handles request signing, credential management, and streaming.
pub struct AwsPollyTTS {
    /// Polly configuration
    config: AwsPollyTTSConfig,
    /// AWS Polly client (lazily initialized on connect)
    client: Arc<RwLock<Option<PollyClient>>>,
    /// Connection state
    connected: Arc<AtomicBool>,
    /// Audio callback
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
    /// Request counter for logging (atomic for lock-free access)
    request_counter: Arc<std::sync::atomic::AtomicU64>,
}

impl AwsPollyTTS {
    /// Create a new Amazon Polly TTS instance from base TTSConfig.
    ///
    /// This creates a default Polly configuration and overrides voice and
    /// sample rate from the base config if provided.
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        // Parse voice from config
        let voice = config
            .voice_id
            .as_ref()
            .map(|v| PollyVoice::from_str_or_default(v))
            .unwrap_or_default();

        // Parse engine from model field
        let engine = if config.model.is_empty() {
            PollyEngine::default()
        } else {
            PollyEngine::from_str_or_default(&config.model)
        };

        // Parse output format
        let output_format = config
            .audio_format
            .as_ref()
            .map(|f| PollyOutputFormat::from_str_or_default(f))
            .unwrap_or_default();

        // Adjust sample rate if not supported by the output format
        // AWS Polly PCM only supports 8000 and 16000 Hz
        let mut base_config = config;
        if let Some(rate) = base_config.sample_rate {
            if !output_format.supported_sample_rates().contains(&rate) {
                debug!(
                    "Sample rate {} not supported for {} format, using default {}",
                    rate,
                    output_format.as_str(),
                    output_format.default_sample_rate()
                );
                base_config.sample_rate = Some(output_format.default_sample_rate());
            }
        } else {
            base_config.sample_rate = Some(output_format.default_sample_rate());
        }

        // Build Polly-specific config from base config
        let polly_config = AwsPollyTTSConfig {
            base: base_config,
            voice,
            engine,
            output_format,
            ..Default::default()
        };

        // Validate configuration
        polly_config
            .validate()
            .map_err(TTSError::InvalidConfiguration)?;

        Ok(Self {
            config: polly_config,
            client: Arc::new(RwLock::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
            audio_callback: Arc::new(RwLock::new(None)),
            request_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    /// Create a new Amazon Polly TTS instance from AwsPollyTTSConfig.
    ///
    /// Use this when you want full control over Polly-specific settings.
    pub fn new_from_polly_config(config: AwsPollyTTSConfig) -> TTSResult<Self> {
        // Validate configuration
        config.validate().map_err(TTSError::InvalidConfiguration)?;

        Ok(Self {
            config,
            client: Arc::new(RwLock::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
            audio_callback: Arc::new(RwLock::new(None)),
            request_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    /// Initialize AWS Polly client with credentials.
    async fn init_client(&self) -> TTSResult<PollyClient> {
        let region = region_to_sdk(self.config.region);

        // Build AWS config
        let aws_config =
            if self.config.has_explicit_credentials() {
                // Use explicit credentials from config
                let access_key = self.config.aws_access_key_id.as_ref().ok_or_else(|| {
                    TTSError::InvalidConfiguration("Missing AWS access key".into())
                })?;
                let secret_key = self.config.aws_secret_access_key.as_ref().ok_or_else(|| {
                    TTSError::InvalidConfiguration("Missing AWS secret key".into())
                })?;

                let credentials = if let Some(ref session_token) = self.config.aws_session_token {
                    Credentials::new(
                        access_key,
                        secret_key,
                        Some(session_token.clone()),
                        None,
                        "waav",
                    )
                } else {
                    Credentials::new(access_key, secret_key, None, None, "waav")
                };

                let polly_config = PollyConfigBuilder::new()
                    .region(region)
                    .credentials_provider(credentials)
                    .build();

                return Ok(PollyClient::from_conf(polly_config));
            } else {
                // Use default credential chain (environment, IAM roles, etc.)
                aws_config::defaults(BehaviorVersion::latest())
                    .region(region)
                    .load()
                    .await
            };

        Ok(PollyClient::new(&aws_config))
    }

    /// Synthesize text to audio using Amazon Polly.
    async fn synthesize(&self, text: &str) -> TTSResult<Bytes> {
        let client = {
            let client_guard = self.client.read().await;
            client_guard
                .clone()
                .ok_or_else(|| TTSError::ProviderNotReady("Polly client not initialized".into()))?
        };

        // Validate text length
        if text.len() > MAX_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text length {} exceeds maximum {} characters",
                text.len(),
                MAX_TEXT_LENGTH
            )));
        }

        // Increment request counter (lock-free atomic operation)
        let request_id = self.request_counter.fetch_add(1, Ordering::Relaxed) + 1;

        debug!(
            request_id = request_id,
            text_len = text.len(),
            voice = %self.config.voice,
            engine = %self.config.engine,
            "Synthesizing text with Amazon Polly"
        );

        // Build request
        let mut request = client
            .synthesize_speech()
            .text(text)
            .voice_id(voice_to_sdk(&self.config.voice))
            .engine(engine_to_sdk(self.config.engine))
            .output_format(output_format_to_sdk(self.config.output_format))
            .text_type(text_type_to_sdk(self.config.text_type));

        // Add sample rate if specified and supported
        if let Some(sample_rate) = self.config.base.sample_rate {
            request = request.sample_rate(sample_rate.to_string());
        }

        // Add language code if specified
        if let Some(ref lang_code) = self.config.language_code {
            request = request.language_code(lang_code.parse().map_err(|_| {
                TTSError::InvalidConfiguration(format!("Invalid language code: {}", lang_code))
            })?);
        }

        // Add lexicons
        for lexicon in &self.config.lexicon_names {
            request = request.lexicon_names(lexicon.clone());
        }

        // Send request
        let response = request.send().await.map_err(|e| {
            error!(request_id = request_id, error = %e, "Polly API error");
            TTSError::ProviderError(format!("Polly API error: {}", e))
        })?;

        // Read audio stream
        let audio_stream: ByteStream = response.audio_stream;
        let audio_bytes = audio_stream.collect().await.map_err(|e| {
            error!(request_id = request_id, error = %e, "Failed to read audio stream");
            TTSError::AudioGenerationFailed(format!("Failed to read audio stream: {}", e))
        })?;

        let bytes = audio_bytes.into_bytes();

        debug!(
            request_id = request_id,
            audio_bytes = bytes.len(),
            "Successfully synthesized audio"
        );

        Ok(bytes)
    }

    /// Process audio and deliver to callback with proper chunking.
    async fn deliver_audio(&self, audio_bytes: Bytes) -> TTSResult<()> {
        let callback = self.audio_callback.read().await.clone();

        let Some(cb) = callback else {
            debug!("No audio callback registered, discarding audio");
            return Ok(());
        };

        let format = self.config.output_format.as_str().to_string();
        let sample_rate = self
            .config
            .base
            .sample_rate
            .unwrap_or_else(|| self.config.output_format.default_sample_rate());

        // For PCM, chunk the audio for streaming delivery
        // For compressed formats (MP3, OGG), deliver as single chunk
        match self.config.output_format {
            PollyOutputFormat::Pcm => {
                // PCM: 16-bit signed little-endian, mono
                // Chunk into ~10ms segments for streaming
                let bytes_per_sample = 2; // 16-bit
                let samples_per_chunk = sample_rate / 100; // 10ms worth
                let chunk_size = (samples_per_chunk * bytes_per_sample) as usize;

                let audio_vec = audio_bytes.to_vec();
                let mut offset = 0;

                while offset < audio_vec.len() {
                    let end = (offset + chunk_size).min(audio_vec.len());
                    let chunk = audio_vec[offset..end].to_vec();
                    let chunk_len = chunk.len();

                    let duration_ms =
                        Some(((chunk_len / bytes_per_sample as usize) as u32 * 1000) / sample_rate);

                    let audio_data = AudioData {
                        data: chunk,
                        sample_rate,
                        format: format.clone(),
                        duration_ms,
                    };

                    cb.on_audio(audio_data).await;
                    offset = end;
                }
            }
            PollyOutputFormat::Mp3 | PollyOutputFormat::OggVorbis => {
                // Compressed formats: deliver as single chunk
                // Duration calculation is complex for compressed audio
                let audio_data = AudioData {
                    data: audio_bytes.to_vec(),
                    sample_rate,
                    format,
                    duration_ms: None,
                };

                cb.on_audio(audio_data).await;
            }
        }

        // Notify completion
        cb.on_complete().await;

        Ok(())
    }

    /// Get the configured voice
    pub fn voice(&self) -> PollyVoice {
        self.config.voice.clone()
    }

    /// Get the configured engine
    pub fn engine(&self) -> PollyEngine {
        self.config.engine
    }

    /// Get the configured output format
    pub fn output_format(&self) -> PollyOutputFormat {
        self.config.output_format
    }

    /// Get the Polly configuration
    pub fn polly_config(&self) -> &AwsPollyTTSConfig {
        &self.config
    }
}

#[async_trait]
impl BaseTTS for AwsPollyTTS {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        AwsPollyTTS::new(config)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        if self.connected.load(Ordering::Acquire) {
            debug!("Amazon Polly TTS already connected");
            return Ok(());
        }

        info!(
            region = %self.config.region,
            voice = %self.config.voice,
            engine = %self.config.engine,
            "Connecting to Amazon Polly"
        );

        // Initialize client
        let client = self.init_client().await?;
        *self.client.write().await = Some(client);

        self.connected.store(true, Ordering::Release);

        info!("Amazon Polly TTS connected successfully");
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        if !self.connected.load(Ordering::Acquire) {
            debug!("Amazon Polly TTS already disconnected");
            return Ok(());
        }

        info!("Disconnecting from Amazon Polly");

        // Clear client
        *self.client.write().await = None;

        // Clear callback
        *self.audio_callback.write().await = None;

        self.connected.store(false, Ordering::Release);

        info!("Amazon Polly TTS disconnected");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    fn get_connection_state(&self) -> ConnectionState {
        if self.connected.load(Ordering::Acquire) {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    async fn speak(&mut self, text: &str, _flush: bool) -> TTSResult<()> {
        // Auto-connect if needed
        if !self.is_ready() {
            warn!("Amazon Polly TTS not ready, attempting to connect...");
            self.connect().await?;
        }

        // Skip empty text
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }

        // Synthesize
        let audio_bytes = self.synthesize(text).await?;

        // Deliver to callback
        self.deliver_audio(audio_bytes).await?;

        Ok(())
    }

    async fn clear(&mut self) -> TTSResult<()> {
        // Amazon Polly is synchronous (one request at a time)
        // Nothing to clear
        debug!("Amazon Polly clear (no-op for synchronous API)");
        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        // Amazon Polly is synchronous, no buffering
        debug!("Amazon Polly flush (no-op for synchronous API)");
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        // Use try_write which doesn't block - safe in both sync and async contexts
        if let Ok(mut guard) = self.audio_callback.try_write() {
            *guard = Some(callback);
            Ok(())
        } else {
            Err(TTSError::InternalError(
                "Failed to register audio callback - lock contention".into(),
            ))
        }
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        // Use try_write which doesn't block - safe in both sync and async contexts
        if let Ok(mut guard) = self.audio_callback.try_write() {
            *guard = None;
            Ok(())
        } else {
            Err(TTSError::InternalError(
                "Failed to remove audio callback - lock contention".into(),
            ))
        }
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "aws-polly",
            "version": "1.0.0",
            "api_type": "AWS SDK",
            "connection_pooling": false,
            "region": self.config.region.as_str(),
            "supported_formats": ["mp3", "ogg_vorbis", "pcm"],
            "supported_engines": ["standard", "neural", "long-form", "generative"],
            "supported_sample_rates": {
                "mp3": [8000, 16000, 22050, 24000],
                "ogg_vorbis": [8000, 16000, 22050, 24000],
                "pcm": [8000, 16000]
            },
            "max_text_length": MAX_TEXT_LENGTH,
            "supported_voices": [
                // US English
                "Joanna", "Matthew", "Salli", "Kendra", "Kimberly",
                "Joey", "Ruth", "Stephen", "Kevin", "Ivy", "Justin",
                // UK English
                "Amy", "Emma", "Brian", "Arthur",
                // Australian
                "Olivia",
                // Other languages
                "Lea", "Hans", "Vicki", "Mizuki", "Takumi", "Seoyeon",
                "Zhiyu", "Camila", "Vitoria", "Lupe", "Pedro", "Lucia",
                "Enrique", "Mia", "Bianca", "Adriano", "Ola", "Kajal", "Aria"
            ],
            "documentation": "https://docs.aws.amazon.com/polly/latest/dg/API_SynthesizeSpeech.html"
        })
    }

    async fn set_req_manager(&mut self, _req_manager: Arc<ReqManager>) {
        // Amazon Polly uses AWS SDK, not HTTP request manager
        // This is a no-op
        debug!("Amazon Polly does not use ReqManager (uses AWS SDK)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_aws_polly_creation() {
        let config = TTSConfig {
            provider: "aws-polly".to_string(),
            api_key: String::new(), // Not used for AWS
            voice_id: Some("Joanna".to_string()),
            model: "neural".to_string(),
            audio_format: Some("pcm".to_string()),
            sample_rate: Some(16000),
            ..Default::default()
        };

        let tts = AwsPollyTTS::new(config).unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert_eq!(tts.voice(), PollyVoice::Joanna);
        assert_eq!(tts.engine(), PollyEngine::Neural);
        assert_eq!(tts.output_format(), PollyOutputFormat::Pcm);
    }

    #[tokio::test]
    async fn test_aws_polly_from_polly_config() {
        let config = AwsPollyTTSConfig {
            voice: PollyVoice::Matthew,
            engine: PollyEngine::Neural,
            output_format: PollyOutputFormat::Mp3,
            ..Default::default()
        };

        let tts = AwsPollyTTS::new_from_polly_config(config).unwrap();
        assert_eq!(tts.voice(), PollyVoice::Matthew);
        assert_eq!(tts.engine(), PollyEngine::Neural);
        assert_eq!(tts.output_format(), PollyOutputFormat::Mp3);
    }

    #[tokio::test]
    async fn test_aws_polly_default_values() {
        // Clear voice_id to test Polly defaults
        let config = TTSConfig {
            provider: "aws-polly".to_string(),
            voice_id: None,     // Clear to use Polly default
            audio_format: None, // Clear to use Polly default
            ..Default::default()
        };

        let tts = AwsPollyTTS::new(config).unwrap();
        // With no voice_id, should use Polly defaults
        assert_eq!(tts.voice(), PollyVoice::Joanna); // Default Polly voice
        assert_eq!(tts.engine(), PollyEngine::Neural);
        // Default output format is Mp3 (based on PollyOutputFormat::default())
        assert_eq!(tts.output_format(), PollyOutputFormat::Mp3);
    }

    #[tokio::test]
    async fn test_aws_polly_invalid_sample_rate() {
        let mut config = AwsPollyTTSConfig::default();
        config.output_format = PollyOutputFormat::Pcm;
        config.base.sample_rate = Some(44100); // Invalid for PCM

        let result = AwsPollyTTS::new_from_polly_config(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_voice_conversion() {
        assert!(matches!(voice_to_sdk(&PollyVoice::Joanna), VoiceId::Joanna));
        assert!(matches!(
            voice_to_sdk(&PollyVoice::Matthew),
            VoiceId::Matthew
        ));
        assert!(matches!(voice_to_sdk(&PollyVoice::Amy), VoiceId::Amy));
    }

    #[test]
    fn test_engine_conversion() {
        assert!(matches!(engine_to_sdk(PollyEngine::Neural), Engine::Neural));
        assert!(matches!(
            engine_to_sdk(PollyEngine::Standard),
            Engine::Standard
        ));
        assert!(matches!(
            engine_to_sdk(PollyEngine::LongForm),
            Engine::LongForm
        ));
    }

    #[test]
    fn test_output_format_conversion() {
        assert!(matches!(
            output_format_to_sdk(PollyOutputFormat::Pcm),
            OutputFormat::Pcm
        ));
        assert!(matches!(
            output_format_to_sdk(PollyOutputFormat::Mp3),
            OutputFormat::Mp3
        ));
        assert!(matches!(
            output_format_to_sdk(PollyOutputFormat::OggVorbis),
            OutputFormat::OggVorbis
        ));
    }

    #[test]
    fn test_provider_info() {
        let config = TTSConfig::default();
        let tts = AwsPollyTTS::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "aws-polly");
        assert_eq!(info["api_type"], "AWS SDK");
        assert!(
            info["supported_formats"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("pcm"))
        );
        assert!(
            info["supported_engines"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("neural"))
        );
    }

    #[test]
    fn test_region_conversion() {
        let region = region_to_sdk(AwsRegion::UsEast1);
        assert_eq!(region.to_string(), "us-east-1");

        let region = region_to_sdk(AwsRegion::EuWest1);
        assert_eq!(region.to_string(), "eu-west-1");
    }
}
