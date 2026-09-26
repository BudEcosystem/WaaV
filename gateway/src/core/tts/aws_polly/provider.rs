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
//! - Output formats: pcm (16-bit signed little-endian), mp3, ogg_vorbis, ogg_opus
//! - Sample rates: mp3/ogg_vorbis (8000, 16000, 22050, 24000, 44100, 48000), pcm (8000, 16000),
//!   ogg_opus (48000)
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
//!         engine: Some(PollyEngine::Neural),
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
use aws_sdk_polly::operation::synthesize_speech::builders::SynthesizeSpeechInputBuilder;
use aws_sdk_polly::primitives::ByteStream;
use aws_sdk_polly::types::{
    Engine, OutputFormat, SpeechMarkType, TextType as PollyTextType, VoiceId,
};
use bytes::Bytes;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::config::{
    AwsPollyTTSConfig, MAX_TEXT_LENGTH, PollyEngine, PollyOutputFormat, PollyVoice, TextType,
};
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
        PollyOutputFormat::OggOpus => OutputFormat::OggOpus,
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

/// The engine as a log field: the name, or what Polly applies when none is sent.
fn engine_label(engine: Option<PollyEngine>) -> &'static str {
    engine.map_or("(unset: Polly default, standard)", |e| e.as_str())
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
/// - Multiple output formats (pcm, mp3, ogg_vorbis, ogg_opus)
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
    /// Resolves voice, engine (`model`), output format and sample rate exactly as the
    /// standardized path does ([`AwsPollyTTSConfig::from_base`]): an unsupported format or an
    /// unknown engine is a configuration error, and an out-of-range pcm rate is sent — and
    /// reported — as 16000.
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        let polly_config =
            AwsPollyTTSConfig::from_base(config).map_err(TTSError::InvalidConfiguration)?;
        Self::new_from_polly_config(polly_config)
    }

    /// Build the provider from the standardized config (W1 keystone), mirroring
    /// `DeepgramTTS::from_standard`. Delegates the feature mapping to
    /// [`AwsPollyTTSConfig::from_standard`] (SSML input type, language override, output
    /// sample_rate + voice/engine and the `region` extra) so advanced features reach the live
    /// SynthesizeSpeech request through the standardized dispatch instead of being dropped at the
    /// flat boundary.
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let polly_config =
            AwsPollyTTSConfig::from_standard(std).map_err(TTSError::InvalidConfiguration)?;
        Self::new_from_polly_config(polly_config)
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
    ///
    /// The region is the configured name passed straight to the SDK (`Region::new`), so any
    /// AWS-shaped region works — not only the 16 an old enum listed (every other one used to
    /// become us-east-1).
    async fn init_client(&self) -> TTSResult<PollyClient> {
        let region: Region = self.config.region.to_sdk();

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

                let mut builder = PollyConfigBuilder::new()
                    // A behavior major version is mandatory; without it the SDK panics at request
                    // time. The default-credential branch sets it via `aws_config::defaults`, but
                    // this explicit-credential `Builder::new()` path must set it itself.
                    .behavior_version(BehaviorVersion::latest())
                    .region(region)
                    .credentials_provider(credentials);
                // Honor the raw endpoint base (e.g. http://127.0.0.1:PORT for a mock e2e harness);
                // the SDK appends the operation path, so this is NOT override_rest_endpoint.
                if let Some(ep) = self.config.endpoint_override.as_deref() {
                    builder = builder.endpoint_url(ep);
                }
                let polly_config = builder.build();

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

    /// Build the `SynthesizeSpeech` request input from the resolved config.
    ///
    /// Factored out of [`Self::synthesize`] so the exact request that reaches the AWS wire can be
    /// asserted in a unit test without a live API call (the recurring "config struct set but never
    /// emitted" bug class). Maps every audio-affecting field — voice, engine, output format, text
    /// type, sample rate, language, lexicons — and the `speech_mark_types` knob onto the SDK input
    /// builder. When `speech_mark_types` is non-empty, Polly returns a JSON marks stream rather
    /// than audio, so we force `OutputFormat::Json` on that request (AWS rejects audio + marks).
    fn build_synthesize_input(&self, text: &str) -> TTSResult<SynthesizeSpeechInputBuilder> {
        let speech_marks: Vec<SpeechMarkType> = self
            .config
            .speech_mark_types
            .iter()
            .map(|s| SpeechMarkType::from(s.as_str()))
            .collect();

        // Speech marks require the JSON output format (mutually exclusive with audio bytes).
        let output_format = if speech_marks.is_empty() {
            output_format_to_sdk(self.config.output_format)
        } else {
            OutputFormat::Json
        };

        let mut input =
            aws_sdk_polly::operation::synthesize_speech::SynthesizeSpeechInput::builder()
                .text(text)
                .voice_id(voice_to_sdk(&self.config.voice))
                // Engine is optional on SynthesizeSpeech: with none Polly applies `standard`.
                // `None` (an empty model) therefore sends nothing rather than a guessed engine.
                .set_engine(self.config.engine.map(engine_to_sdk))
                .output_format(output_format)
                .text_type(text_type_to_sdk(self.config.text_type));

        // The resolved rate (see `PollyOutputFormat::resolve_sample_rate`): always set for pcm and
        // ogg_opus, set for mp3/ogg_vorbis only when the caller chose a rate Polly accepts.
        if let Some(sample_rate) = self.config.base.sample_rate {
            input = input.sample_rate(sample_rate.to_string());
        }

        // Add language code if specified
        if let Some(ref lang_code) = self.config.language_code {
            input = input.language_code(lang_code.parse().map_err(|_| {
                TTSError::InvalidConfiguration(format!("Invalid language code: {}", lang_code))
            })?);
        }

        // Add lexicons
        for lexicon in &self.config.lexicon_names {
            input = input.lexicon_names(lexicon.clone());
        }

        // Speech marks (word / sentence / viseme / ssml timing & metadata).
        if !speech_marks.is_empty() {
            input = input.set_speech_mark_types(Some(speech_marks));
        }

        Ok(input)
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
            engine = engine_label(self.config.engine),
            "Synthesizing text with Amazon Polly"
        );

        // Build the request input (shared with the wire-assert test path).
        let input = self.build_synthesize_input(text)?;
        let request = client
            .synthesize_speech()
            .set_text(input.get_text().clone())
            .set_voice_id(input.get_voice_id().clone())
            .set_engine(input.get_engine().clone())
            .set_output_format(input.get_output_format().clone())
            .set_text_type(input.get_text_type().clone())
            .set_sample_rate(input.get_sample_rate().clone())
            .set_language_code(input.get_language_code().clone())
            .set_lexicon_names(input.get_lexicon_names().clone())
            .set_speech_mark_types(input.get_speech_mark_types().clone());

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
        // The rate Polly produced: the one sent, else its documented default for this format and
        // engine. The OpenAI route wraps pcm in a WAV header / `x-sample-rate` at this rate.
        let sample_rate = self.config.output_sample_rate();

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
            PollyOutputFormat::Mp3 | PollyOutputFormat::OggVorbis | PollyOutputFormat::OggOpus => {
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

    /// Get the configured engine (`None`: none is sent and Polly applies `standard`)
    pub fn engine(&self) -> Option<PollyEngine> {
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
            engine = engine_label(self.config.engine),
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
            "supported_formats": ["mp3", "ogg_vorbis", "ogg_opus", "pcm"],
            "supported_engines": ["standard", "neural", "long-form", "generative"],
            "supported_sample_rates": {
                "mp3": PollyOutputFormat::Mp3.supported_sample_rates(),
                "ogg_vorbis": PollyOutputFormat::OggVorbis.supported_sample_rates(),
                "ogg_opus": PollyOutputFormat::OggOpus.supported_sample_rates(),
                "pcm": PollyOutputFormat::Pcm.supported_sample_rates()
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
        assert_eq!(tts.engine(), Some(PollyEngine::Neural));
        assert_eq!(tts.output_format(), PollyOutputFormat::Pcm);
    }

    // W1 keystone: a StandardTTSConfig advanced feature Polly supports (SSML input type, language
    // override, output sample_rate) reaches the provider's resolved `polly_config` through the
    // provider struct's `from_standard`, mirroring `DeepgramTTS::from_standard`.
    #[tokio::test]
    async fn from_standard_reaches_provider_config() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "aws-polly".into(),
                voice_id: Some("Matthew".into()),
                model: "neural".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                ssml: Some(true),
                language: Some("en-GB".into()),
                sample_rate: Some(16000), // valid for the default PCM output format
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = AwsPollyTTS::from_standard(&std).unwrap();
        assert_eq!(tts.polly_config().text_type, TextType::Ssml);
        assert_eq!(tts.polly_config().language_code, Some("en-GB".to_string()));
        assert_eq!(tts.polly_config().base.sample_rate, Some(16000));
        assert_eq!(tts.voice(), PollyVoice::Matthew);
        assert_eq!(tts.engine(), Some(PollyEngine::Neural));
    }

    #[tokio::test]
    async fn from_standard_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let std = crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
            provider: "aws-polly".into(),
            voice_id: Some("Joanna".to_string()),
            sample_rate: Some(16000),
            ..Default::default()
        })
        .with_endpoint_override("http://127.0.0.1:9000");

        match AwsPollyTTS::from_standard(&std) {
            Ok(_) => panic!("AWS Polly provider construction must reject unsafe endpoint_override"),
            Err(err) => assert!(err.to_string().contains("SSRF protection")),
        }
    }

    // WIRE-LEVEL: the `speech_mark_types` knob must reach the actual `SynthesizeSpeech` request
    // input (not merely sit on the config struct — the recurring "set but never emitted" bug). We
    // build the exact SDK input the live path sends and assert the marks are present, in order,
    // and that the output format was flipped to JSON (AWS forbids audio + marks in one request).
    #[tokio::test]
    async fn speech_mark_types_reach_synthesize_request() {
        use crate::core::stt::standard::ProviderExtras;
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert(
            "speech_mark_types".into(),
            serde_json::json!(["word", "sentence", "viseme", "ssml", "bogus"]),
        );
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "aws-polly".into(),
                voice_id: Some("Joanna".into()),
                model: "neural".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                // PCM-valid rate so config validation (default output format = PCM) passes.
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let tts = AwsPollyTTS::from_standard(&std).unwrap();
        // The invalid "bogus" name is filtered; only the 4 valid AWS marks survive.
        assert_eq!(
            tts.polly_config().speech_mark_types,
            vec!["word", "sentence", "viseme", "ssml"]
        );

        let input = tts.build_synthesize_input("Hello").unwrap();
        let marks = input.get_speech_mark_types().clone().unwrap();
        assert_eq!(
            marks,
            vec![
                SpeechMarkType::Word,
                SpeechMarkType::Sentence,
                SpeechMarkType::Viseme,
                SpeechMarkType::Ssml,
            ],
            "speech_mark_types must reach the SynthesizeSpeech request input"
        );
        // Marks force the JSON metadata stream, not audio bytes.
        assert_eq!(input.get_output_format().clone(), Some(OutputFormat::Json));
    }

    // Without the knob, no marks are requested and the request stays an audio call.
    #[tokio::test]
    async fn no_speech_mark_types_keeps_audio_output() {
        let config = TTSConfig {
            provider: "aws-polly".into(),
            voice_id: Some("Joanna".into()),
            audio_format: Some("pcm".into()),
            sample_rate: Some(16000),
            ..Default::default()
        };
        let tts = AwsPollyTTS::new(config).unwrap();
        let input = tts.build_synthesize_input("Hello").unwrap();
        assert!(input.get_speech_mark_types().is_none());
        assert_eq!(input.get_output_format().clone(), Some(OutputFormat::Pcm));
    }

    #[tokio::test]
    async fn test_aws_polly_from_polly_config() {
        let config = AwsPollyTTSConfig {
            voice: PollyVoice::Matthew,
            engine: Some(PollyEngine::Neural),
            output_format: PollyOutputFormat::Mp3,
            ..Default::default()
        };

        let tts = AwsPollyTTS::new_from_polly_config(config).unwrap();
        assert_eq!(tts.voice(), PollyVoice::Matthew);
        assert_eq!(tts.engine(), Some(PollyEngine::Neural));
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
        // No model → no engine sent; Polly applies `standard`. (This was `neural`, a choice
        // nobody made.)
        assert_eq!(tts.engine(), None);
        // No format → pcm, WaaV's canonical linear16 — the same answer the standardized path
        // gives. (The flat path used to pick mp3 here and the standardized one pcm.)
        assert_eq!(tts.output_format(), PollyOutputFormat::Pcm);
        // TTSConfig's default 24000 is not a pcm rate: 16000 is sent and reported.
        assert_eq!(tts.polly_config().base.sample_rate, Some(16000));
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
            output_format_to_sdk(PollyOutputFormat::OggOpus),
            OutputFormat::OggOpus
        ));
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

    // =========================================================================
    // WIRE-LEVEL: what the resolved config puts on the actual SynthesizeSpeech
    // input, and what the provider REPORTS for the audio it delivers. The
    // OpenAI route labels/wraps pcm with the reported rate, so reported must
    // equal sent.
    // =========================================================================

    /// Records what `deliver_audio` reports on each chunk.
    #[derive(Default)]
    struct ReportedAudio {
        chunks: std::sync::Mutex<Vec<(u32, String)>>,
    }

    impl AudioCallback for ReportedAudio {
        fn on_audio(
            &self,
            audio: AudioData,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            self.chunks
                .lock()
                .unwrap()
                .push((audio.sample_rate, audio.format));
            Box::pin(async {})
        }

        fn on_error(
            &self,
            _: TTSError,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async {})
        }

        fn on_complete(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async {})
        }
    }

    /// Build the provider the way the OpenAI route does: flat fields in a standardized config.
    fn route_like(audio_format: &str, model: &str, sample_rate: Option<u32>) -> AwsPollyTTS {
        let std = crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
            provider: "aws-polly".into(),
            voice_id: Some("Joanna".into()),
            model: model.into(),
            audio_format: Some(audio_format.into()),
            sample_rate,
            ..Default::default()
        });
        AwsPollyTTS::from_standard(&std)
            .unwrap_or_else(|e| panic!("{audio_format}/{model}/{sample_rate:?}: {e}"))
    }

    /// The rate the provider reports for delivered audio.
    async fn reported_rate(tts: &AwsPollyTTS) -> (u32, String) {
        let seen = Arc::new(ReportedAudio::default());
        *tts.audio_callback.write().await = Some(seen.clone() as Arc<dyn AudioCallback>);
        tts.deliver_audio(Bytes::from(vec![0u8; 640]))
            .await
            .unwrap();
        let chunks = seen.chunks.lock().unwrap().clone();
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|c| *c == chunks[0]), "one rate per clip");
        chunks[0].clone()
    }

    /// Each OpenAI format the route sends (`as_waav_format`) reaches the request as the matching
    /// Polly OutputFormat. `from_standard` used to send pcm for all of them.
    #[tokio::test]
    async fn requested_format_reaches_the_synthesize_request() {
        for (requested, expected) in [
            ("linear16", OutputFormat::Pcm),
            ("wav", OutputFormat::Pcm),
            ("mp3", OutputFormat::Mp3),
            ("opus", OutputFormat::OggOpus),
            ("ogg_vorbis", OutputFormat::OggVorbis),
        ] {
            let input = route_like(requested, "", None)
                .build_synthesize_input("Hello")
                .unwrap();
            assert_eq!(
                input.get_output_format().clone(),
                Some(expected),
                "{requested}"
            );
        }
    }

    /// `aac` / `flac` cannot be produced: refused at construction, before any vendor call.
    #[tokio::test]
    async fn unsupported_formats_are_refused_at_construction() {
        for bad in ["aac", "flac"] {
            let std = crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
                provider: "aws-polly".into(),
                audio_format: Some(bad.into()),
                ..Default::default()
            });
            match AwsPollyTTS::from_standard(&std) {
                Err(TTSError::InvalidConfiguration(m)) => assert!(m.contains(bad), "{m}"),
                Err(other) => panic!("{bad}: expected InvalidConfiguration, got {other:?}"),
                Ok(_) => panic!("{bad} must not be synthesised as another format"),
            }
            assert!(
                AwsPollyTTS::new(std.base.clone()).is_err(),
                "flat path, {bad}"
            );
        }
    }

    /// The route asks for 24 kHz pcm. Polly pcm takes 8000/16000 only, so 16000 is SENT and the
    /// provider REPORTS 16000 — the rate the route then writes into `x-sample-rate` / the WAV
    /// header. Every such request used to fail validation; had it passed, a 24000 label on 16 kHz
    /// samples would have played at 1.5x.
    #[tokio::test]
    async fn pcm_rate_sent_and_reported_agree() {
        for (format, requested, sent) in [
            ("linear16", Some(24000), 16000),
            ("linear16", Some(16000), 16000),
            ("linear16", Some(8000), 8000),
            ("wav", None, 16000),
        ] {
            let tts = route_like(format, "", requested);
            let input = tts.build_synthesize_input("Hello").unwrap();
            assert_eq!(
                input.get_sample_rate().as_deref(),
                Some(sent.to_string().as_str()),
                "{format} {requested:?}"
            );
            assert_eq!(
                reported_rate(&tts).await,
                (sent, "pcm".to_string()),
                "{format} {requested:?}"
            );
        }
    }

    /// ogg_opus: 48000 is sent and reported. mp3 with no rate: none is sent and the reported rate
    /// is Polly's default for the engine (22050 standard / none, 24000 neural).
    #[tokio::test]
    async fn compressed_formats_report_the_rate_polly_produces() {
        let tts = route_like("opus", "", None);
        let input = tts.build_synthesize_input("Hello").unwrap();
        assert_eq!(input.get_sample_rate().as_deref(), Some("48000"));
        assert_eq!(reported_rate(&tts).await, (48000, "ogg_opus".to_string()));

        let tts = route_like("mp3", "", None);
        assert_eq!(
            tts.build_synthesize_input("Hello")
                .unwrap()
                .get_sample_rate(),
            &None
        );
        assert_eq!(reported_rate(&tts).await, (22050, "mp3".to_string()));

        let tts = route_like("mp3", "neural", None);
        assert_eq!(reported_rate(&tts).await, (24000, "mp3".to_string()));

        let tts = route_like("mp3", "", Some(44100));
        let input = tts.build_synthesize_input("Hello").unwrap();
        assert_eq!(input.get_sample_rate().as_deref(), Some("44100"));
        assert_eq!(reported_rate(&tts).await, (44100, "mp3".to_string()));
    }

    /// Engine: an empty model sends NO engine (Polly applies `standard`); a catalog name sends
    /// that engine; an unknown model is refused. All three used to send `neural`.
    #[tokio::test]
    async fn engine_is_sent_only_when_the_model_names_one() {
        let input = route_like("mp3", "", None)
            .build_synthesize_input("Hello")
            .unwrap();
        assert_eq!(input.get_engine(), &None, "empty model sends no engine");

        for (model, engine) in [
            ("standard", Engine::Standard),
            ("neural", Engine::Neural),
            ("long-form", Engine::LongForm),
            ("generative", Engine::Generative),
        ] {
            let input = route_like("mp3", model, None)
                .build_synthesize_input("Hello")
                .unwrap();
            assert_eq!(input.get_engine(), &Some(engine), "{model}");
        }

        let std = crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
            provider: "aws-polly".into(),
            model: "polly-turbo".into(),
            ..Default::default()
        });
        match AwsPollyTTS::from_standard(&std) {
            Err(TTSError::InvalidConfiguration(m)) => assert!(m.contains("polly-turbo"), "{m}"),
            Err(other) => panic!("expected InvalidConfiguration, got {other:?}"),
            Ok(_) => panic!("an unknown model must not become neural"),
        }
    }

    /// The region reaches the SDK client verbatim — including regions the old enum turned into
    /// us-east-1. Explicit credentials, so building the client touches no default chain.
    #[tokio::test]
    async fn region_reaches_the_sdk_client() {
        // Building the SDK client may build its default TLS client; make the provider unambiguous.
        let _ = rustls::crypto::ring::default_provider().install_default();
        for name in ["eu-north-1", "ap-northeast-3", "us-east-1"] {
            let mut std = crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
                provider: "aws-polly".into(),
                ..Default::default()
            });
            for (k, v) in [
                ("aws_access_key_id", "AKIAIOSFODNN7EXAMPLE"),
                (
                    "aws_secret_access_key",
                    "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
                ),
                ("region", name),
            ] {
                std.extras.0.insert(k.into(), serde_json::json!(v));
            }
            let tts = AwsPollyTTS::from_standard(&std).unwrap();
            let client = tts.init_client().await.unwrap();
            assert_eq!(
                client.config().region().map(|r| r.as_ref()),
                Some(name),
                "{name}"
            );
            assert_eq!(tts.get_provider_info()["region"], name);
        }
    }
}
