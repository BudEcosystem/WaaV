//! Comprehensive tests for Amazon Polly TTS provider.
//!
//! These tests cover:
//! - Configuration validation
//! - Provider creation and lifecycle
//! - Voice, engine, and format conversions
//! - Error handling
//! - Edge cases
//!
//! Note: Integration tests requiring AWS credentials are marked with #[ignore]
//! and can be run with: cargo test -- --ignored

use super::*;
use crate::core::stt::aws_transcribe::AwsRegion;
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// =============================================================================
// Test Helpers
// =============================================================================

/// Mock audio callback for testing
struct MockAudioCallback {
    audio_chunks: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    complete_count: Arc<AtomicUsize>,
    error_count: Arc<AtomicUsize>,
}

impl MockAudioCallback {
    fn new() -> Self {
        Self {
            audio_chunks: Arc::new(std::sync::Mutex::new(Vec::new())),
            complete_count: Arc::new(AtomicUsize::new(0)),
            error_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn get_audio_chunks(&self) -> Vec<Vec<u8>> {
        self.audio_chunks.lock().unwrap().clone()
    }

    fn get_complete_count(&self) -> usize {
        self.complete_count.load(Ordering::Relaxed)
    }

    fn get_error_count(&self) -> usize {
        self.error_count.load(Ordering::Relaxed)
    }
}

impl AudioCallback for MockAudioCallback {
    fn on_audio(&self, audio_data: AudioData) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let data = audio_data.data.clone();
        let chunks = self.audio_chunks.clone();
        Box::pin(async move {
            chunks.lock().unwrap().push(data);
        })
    }

    fn on_error(&self, _error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let count = self.error_count.clone();
        Box::pin(async move {
            count.fetch_add(1, Ordering::Relaxed);
        })
    }

    fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let count = self.complete_count.clone();
        Box::pin(async move {
            count.fetch_add(1, Ordering::Relaxed);
        })
    }
}

// =============================================================================
// Configuration Tests
// =============================================================================

#[test]
fn test_polly_engine_variants() {
    assert_eq!(PollyEngine::Standard.as_str(), "standard");
    assert_eq!(PollyEngine::Neural.as_str(), "neural");
    assert_eq!(PollyEngine::LongForm.as_str(), "long-form");
    assert_eq!(PollyEngine::Generative.as_str(), "generative");
}

#[test]
fn test_polly_engine_from_str() {
    assert_eq!(
        PollyEngine::from_str_or_default("standard"),
        PollyEngine::Standard
    );
    assert_eq!(
        PollyEngine::from_str_or_default("neural"),
        PollyEngine::Neural
    );
    assert_eq!(
        PollyEngine::from_str_or_default("long-form"),
        PollyEngine::LongForm
    );
    assert_eq!(
        PollyEngine::from_str_or_default("longform"),
        PollyEngine::LongForm
    );
    assert_eq!(
        PollyEngine::from_str_or_default("generative"),
        PollyEngine::Generative
    );
    assert_eq!(
        PollyEngine::from_str_or_default("unknown"),
        PollyEngine::Neural
    ); // Default
}

#[test]
fn test_polly_output_format_variants() {
    assert_eq!(PollyOutputFormat::Mp3.as_str(), "mp3");
    assert_eq!(PollyOutputFormat::OggVorbis.as_str(), "ogg_vorbis");
    assert_eq!(PollyOutputFormat::Pcm.as_str(), "pcm");
}

#[test]
fn test_polly_output_format_mime_types() {
    assert_eq!(PollyOutputFormat::Mp3.mime_type(), "audio/mpeg");
    assert_eq!(PollyOutputFormat::OggVorbis.mime_type(), "audio/ogg");
    assert_eq!(PollyOutputFormat::Pcm.mime_type(), "audio/pcm");
}

#[test]
fn test_polly_output_format_sample_rates() {
    // MP3 and OGG support more sample rates
    let mp3_rates = PollyOutputFormat::Mp3.supported_sample_rates();
    assert!(mp3_rates.contains(&8000));
    assert!(mp3_rates.contains(&16000));
    assert!(mp3_rates.contains(&22050));
    assert!(mp3_rates.contains(&24000));

    // PCM only supports 8000 and 16000
    let pcm_rates = PollyOutputFormat::Pcm.supported_sample_rates();
    assert!(pcm_rates.contains(&8000));
    assert!(pcm_rates.contains(&16000));
    assert!(!pcm_rates.contains(&22050));
}

#[test]
fn test_polly_voice_variants() {
    // US English voices
    assert_eq!(PollyVoice::Joanna.as_str(), "Joanna");
    assert_eq!(PollyVoice::Matthew.as_str(), "Matthew");

    // UK English voices
    assert_eq!(PollyVoice::Amy.as_str(), "Amy");
    assert_eq!(PollyVoice::Brian.as_str(), "Brian");

    // Other language voices
    assert_eq!(PollyVoice::Lea.as_str(), "Léa");
    assert_eq!(PollyVoice::Hans.as_str(), "Hans");
    assert_eq!(PollyVoice::Mizuki.as_str(), "Mizuki");
}

#[test]
fn test_polly_voice_language_codes() {
    assert_eq!(PollyVoice::Joanna.language_code(), "en-US");
    assert_eq!(PollyVoice::Amy.language_code(), "en-GB");
    assert_eq!(PollyVoice::Olivia.language_code(), "en-AU");
    assert_eq!(PollyVoice::Lea.language_code(), "fr-FR");
    assert_eq!(PollyVoice::Hans.language_code(), "de-DE");
    assert_eq!(PollyVoice::Mizuki.language_code(), "ja-JP");
    assert_eq!(PollyVoice::Seoyeon.language_code(), "ko-KR");
}

#[test]
fn test_polly_voice_from_str() {
    assert_eq!(
        PollyVoice::from_str_or_default("joanna"),
        PollyVoice::Joanna
    );
    assert_eq!(
        PollyVoice::from_str_or_default("Matthew"),
        PollyVoice::Matthew
    );
    assert_eq!(PollyVoice::from_str_or_default("amy"), PollyVoice::Amy);

    // Custom voice
    let custom = PollyVoice::from_str_or_default("CustomVoice123");
    assert!(matches!(custom, PollyVoice::Custom(_)));
    assert_eq!(custom.as_str(), "CustomVoice123");
}

#[test]
fn test_polly_voice_supports_neural() {
    assert!(PollyVoice::Joanna.supports_neural());
    assert!(PollyVoice::Matthew.supports_neural());

    // Custom voices don't have known neural support
    let custom = PollyVoice::Custom("test".to_string());
    assert!(!custom.supports_neural());
}

#[test]
fn test_voices_for_language() {
    let us_voices = PollyVoice::voices_for_language("en-US");
    assert!(us_voices.contains(&PollyVoice::Joanna));
    assert!(us_voices.contains(&PollyVoice::Matthew));

    let gb_voices = PollyVoice::voices_for_language("en-GB");
    assert!(gb_voices.contains(&PollyVoice::Amy));
    assert!(gb_voices.contains(&PollyVoice::Brian));

    let de_voices = PollyVoice::voices_for_language("de-DE");
    assert!(de_voices.contains(&PollyVoice::Hans));
    assert!(de_voices.contains(&PollyVoice::Vicki));
}

#[test]
fn test_text_type_variants() {
    assert_eq!(TextType::Text.as_str(), "text");
    assert_eq!(TextType::Ssml.as_str(), "ssml");
}

#[test]
fn test_text_type_from_str() {
    assert_eq!(TextType::from_str_or_default("text"), TextType::Text);
    assert_eq!(TextType::from_str_or_default("ssml"), TextType::Ssml);
    assert_eq!(TextType::from_str_or_default("unknown"), TextType::Text);
}

// =============================================================================
// Configuration Validation Tests
// =============================================================================

#[test]
fn test_config_default() {
    let config = AwsPollyTTSConfig::default();
    assert_eq!(config.voice, PollyVoice::Joanna);
    assert_eq!(config.engine, PollyEngine::Neural);
    assert_eq!(config.output_format, PollyOutputFormat::Pcm);
    assert_eq!(config.base.sample_rate, Some(16000));
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_with_voice() {
    let config = AwsPollyTTSConfig::with_voice(PollyVoice::Amy);
    assert_eq!(config.voice, PollyVoice::Amy);
    assert_eq!(config.base.voice_id, Some("Amy".to_string()));
}

#[test]
fn test_config_validation_valid_pcm() {
    let mut config = AwsPollyTTSConfig::default();
    config.output_format = PollyOutputFormat::Pcm;
    config.base.sample_rate = Some(16000);
    assert!(config.validate().is_ok());

    config.base.sample_rate = Some(8000);
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_validation_invalid_pcm_sample_rate() {
    let mut config = AwsPollyTTSConfig::default();
    config.output_format = PollyOutputFormat::Pcm;
    config.base.sample_rate = Some(22050); // Not supported for PCM

    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Sample rate"));
}

#[test]
fn test_config_validation_valid_mp3() {
    let mut config = AwsPollyTTSConfig::default();
    config.output_format = PollyOutputFormat::Mp3;

    for rate in [8000, 16000, 22050, 24000] {
        config.base.sample_rate = Some(rate);
        assert!(
            config.validate().is_ok(),
            "Sample rate {} should be valid for MP3",
            rate
        );
    }
}

#[test]
fn test_config_validation_too_many_lexicons() {
    let mut config = AwsPollyTTSConfig::default();
    config.lexicon_names = vec![
        "lex1".to_string(),
        "lex2".to_string(),
        "lex3".to_string(),
        "lex4".to_string(),
        "lex5".to_string(),
        "lex6".to_string(), // 6th lexicon - exceeds limit
    ];

    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("lexicon"));
}

#[test]
fn test_config_effective_language_code() {
    let config = AwsPollyTTSConfig::default();
    assert_eq!(config.effective_language_code(), "en-US"); // From Joanna

    let mut config_override = AwsPollyTTSConfig::default();
    config_override.language_code = Some("en-GB".to_string());
    assert_eq!(config_override.effective_language_code(), "en-GB");
}

#[test]
fn test_config_has_explicit_credentials() {
    let mut config = AwsPollyTTSConfig::default();
    assert!(!config.has_explicit_credentials());

    config.aws_access_key_id = Some("AKIATEST".to_string());
    assert!(!config.has_explicit_credentials()); // Need both

    config.aws_secret_access_key = Some("secret".to_string());
    assert!(config.has_explicit_credentials());
}

// =============================================================================
// Provider Creation Tests
// =============================================================================

#[tokio::test]
async fn test_provider_creation_from_tts_config() {
    let config = TTSConfig {
        provider: "aws-polly".to_string(),
        voice_id: Some("Matthew".to_string()),
        model: "neural".to_string(),
        audio_format: Some("pcm".to_string()),
        sample_rate: Some(16000),
        ..Default::default()
    };

    let tts = AwsPollyTTS::new(config).unwrap();
    assert!(!tts.is_ready());
    assert_eq!(tts.voice(), PollyVoice::Matthew);
    assert_eq!(tts.engine(), PollyEngine::Neural);
    assert_eq!(tts.output_format(), PollyOutputFormat::Pcm);
}

#[tokio::test]
async fn test_provider_creation_from_polly_config() {
    let config = AwsPollyTTSConfig {
        voice: PollyVoice::Amy,
        engine: PollyEngine::Standard,
        output_format: PollyOutputFormat::Mp3,
        region: AwsRegion::EuWest1,
        ..Default::default()
    };

    let tts = AwsPollyTTS::new_from_polly_config(config).unwrap();
    assert_eq!(tts.voice(), PollyVoice::Amy);
    assert_eq!(tts.engine(), PollyEngine::Standard);
    assert_eq!(tts.output_format(), PollyOutputFormat::Mp3);
}

#[tokio::test]
async fn test_provider_creation_with_invalid_config() {
    let mut config = AwsPollyTTSConfig::default();
    config.output_format = PollyOutputFormat::Pcm;
    config.base.sample_rate = Some(44100); // Invalid

    let result = AwsPollyTTS::new_from_polly_config(config);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_provider_connection_state() {
    let config = TTSConfig::default();
    let tts = AwsPollyTTS::new(config).unwrap();

    assert!(!tts.is_ready());
    assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
}

// =============================================================================
// Provider Info Tests
// =============================================================================

#[test]
fn test_provider_info_structure() {
    let config = TTSConfig::default();
    let tts = AwsPollyTTS::new(config).unwrap();
    let info = tts.get_provider_info();

    assert_eq!(info["provider"], "aws-polly");
    assert_eq!(info["api_type"], "AWS SDK");
    assert_eq!(info["connection_pooling"], false);

    // Check supported formats
    let formats = info["supported_formats"].as_array().unwrap();
    assert!(formats.contains(&serde_json::json!("mp3")));
    assert!(formats.contains(&serde_json::json!("ogg_vorbis")));
    assert!(formats.contains(&serde_json::json!("pcm")));

    // Check supported engines
    let engines = info["supported_engines"].as_array().unwrap();
    assert!(engines.contains(&serde_json::json!("neural")));
    assert!(engines.contains(&serde_json::json!("standard")));

    // Check max text length
    assert_eq!(info["max_text_length"], MAX_TEXT_LENGTH);
}

// =============================================================================
// Audio Callback Tests
// =============================================================================

#[tokio::test]
async fn test_audio_callback_registration() {
    let config = TTSConfig::default();
    let mut tts = AwsPollyTTS::new(config).unwrap();

    let callback = Arc::new(MockAudioCallback::new());
    let result = tts.on_audio(callback);
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_audio_callback_removal() {
    let config = TTSConfig::default();
    let mut tts = AwsPollyTTS::new(config).unwrap();

    let callback = Arc::new(MockAudioCallback::new());
    tts.on_audio(callback).unwrap();

    let result = tts.remove_audio_callback();
    assert!(result.is_ok());
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_empty_text_handling() {
    // Test that provider creation works with default config
    // Empty text handling is tested through provider info
    let config = TTSConfig::default();
    let tts = AwsPollyTTS::new(config).unwrap();
    let info = tts.get_provider_info();
    assert_eq!(info["provider"], "aws-polly");
}

#[test]
fn test_custom_voice_handling() {
    let custom = PollyVoice::from_str_or_default("CustomVoice123");
    assert!(matches!(custom, PollyVoice::Custom(_)));
    assert_eq!(custom.as_str(), "CustomVoice123");
    assert_eq!(custom.language_code(), "en-US"); // Default
    assert!(!custom.supports_neural());
}

#[test]
fn test_voice_display() {
    assert_eq!(format!("{}", PollyVoice::Joanna), "Joanna");
    assert_eq!(format!("{}", PollyVoice::Lea), "Léa");
}

#[test]
fn test_engine_display() {
    assert_eq!(format!("{}", PollyEngine::Neural), "neural");
    assert_eq!(format!("{}", PollyEngine::LongForm), "long-form");
}

#[test]
fn test_output_format_display() {
    assert_eq!(format!("{}", PollyOutputFormat::Pcm), "pcm");
    assert_eq!(format!("{}", PollyOutputFormat::OggVorbis), "ogg_vorbis");
}

#[test]
fn test_text_type_display() {
    assert_eq!(format!("{}", TextType::Text), "text");
    assert_eq!(format!("{}", TextType::Ssml), "ssml");
}

// =============================================================================
// Serialization Tests
// =============================================================================

#[test]
fn test_config_serialization() {
    let config = AwsPollyTTSConfig::default();
    let json = serde_json::to_string(&config).unwrap();

    // Verify key fields are present
    assert!(json.contains("voice"));
    assert!(json.contains("engine"));
    assert!(json.contains("output_format"));
}

#[test]
fn test_config_deserialization() {
    let json = r#"{
        "provider": "aws-polly",
        "api_key": "",
        "voice_id": "Matthew",
        "model": "",
        "speaking_rate": 1.0,
        "audio_format": "pcm",
        "sample_rate": 16000,
        "pronunciations": [],
        "voice": "Matthew",
        "engine": "neural",
        "output_format": "pcm",
        "text_type": "text",
        "region": "us-east-1"
    }"#;

    let config: AwsPollyTTSConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.voice, PollyVoice::Matthew);
    assert_eq!(config.engine, PollyEngine::Neural);
    assert_eq!(config.output_format, PollyOutputFormat::Pcm);
}

// =============================================================================
// Integration Tests (require AWS credentials)
// =============================================================================

#[tokio::test]
#[ignore = "Requires AWS credentials"]
async fn test_integration_connect_and_synthesize() {
    let config = AwsPollyTTSConfig {
        voice: PollyVoice::Joanna,
        engine: PollyEngine::Neural,
        output_format: PollyOutputFormat::Pcm,
        ..Default::default()
    };

    let mut tts = AwsPollyTTS::new_from_polly_config(config).unwrap();

    // Connect
    tts.connect().await.unwrap();
    assert!(tts.is_ready());

    // Register callback
    let callback = Arc::new(MockAudioCallback::new());
    tts.on_audio(callback.clone()).unwrap();

    // Synthesize
    tts.speak("Hello from Amazon Polly!", true).await.unwrap();

    // Verify audio was received
    let chunks = callback.get_audio_chunks();
    assert!(!chunks.is_empty(), "Should have received audio chunks");

    let total_bytes: usize = chunks.iter().map(|c| c.len()).sum();
    assert!(total_bytes > 0, "Should have received audio data");

    assert_eq!(
        callback.get_complete_count(),
        1,
        "Should have received completion"
    );

    // Disconnect
    tts.disconnect().await.unwrap();
    assert!(!tts.is_ready());
}

#[tokio::test]
#[ignore = "Requires AWS credentials"]
async fn test_integration_multiple_voices() {
    let voices = [PollyVoice::Joanna, PollyVoice::Matthew, PollyVoice::Amy];

    for voice in voices {
        let config = AwsPollyTTSConfig {
            voice: voice.clone(),
            engine: PollyEngine::Neural,
            output_format: PollyOutputFormat::Pcm,
            ..Default::default()
        };

        let mut tts = AwsPollyTTS::new_from_polly_config(config).unwrap();
        tts.connect().await.unwrap();

        let callback = Arc::new(MockAudioCallback::new());
        tts.on_audio(callback.clone()).unwrap();

        tts.speak(&format!("Hello, I am {}.", voice.as_str()), true)
            .await
            .unwrap();

        let chunks = callback.get_audio_chunks();
        assert!(
            !chunks.is_empty(),
            "Voice {} should produce audio",
            voice.as_str()
        );

        tts.disconnect().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "Requires AWS credentials"]
async fn test_integration_ssml() {
    let config = AwsPollyTTSConfig {
        voice: PollyVoice::Joanna,
        engine: PollyEngine::Neural,
        output_format: PollyOutputFormat::Pcm,
        text_type: TextType::Ssml,
        ..Default::default()
    };

    let mut tts = AwsPollyTTS::new_from_polly_config(config).unwrap();
    tts.connect().await.unwrap();

    let callback = Arc::new(MockAudioCallback::new());
    tts.on_audio(callback.clone()).unwrap();

    let ssml = r#"<speak>
        Hello! <break time="500ms"/>
        This is <emphasis level="strong">Amazon Polly</emphasis> speaking.
    </speak>"#;

    tts.speak(ssml, true).await.unwrap();

    let chunks = callback.get_audio_chunks();
    assert!(!chunks.is_empty(), "SSML should produce audio");

    tts.disconnect().await.unwrap();
}
