//! Tests for Prosa.ai TTS provider.

use super::*;
use crate::core::tts::base::{AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

fn make_test_config() -> TTSConfig {
    TTSConfig {
        provider: "prosa-ai".to_string(),
        api_key: "test_key".to_string(),
        voice_id: Some("dimas".to_string()),
        ..Default::default()
    }
}

// =============================================================================
// ProsaTtsConfig Tests
// =============================================================================

#[test]
fn test_config_default() {
    let config = ProsaTtsConfig::default();
    assert!(config.api_key.is_empty());
    assert_eq!(config.voice, ProsaTtsVoice::DimasFormal);
    assert_eq!(config.audio_format, ProsaTtsAudioFormat::Opus);
    assert_eq!(config.pitch, DEFAULT_PITCH);
    assert!((config.tempo - DEFAULT_TEMPO).abs() < f32::EPSILON);
    assert!(config.wait);
    assert!(!config.as_signed_url);
}

#[test]
fn test_config_from_base_empty_key() {
    let config = TTSConfig {
        api_key: String::new(),
        ..Default::default()
    };

    let result = ProsaTtsConfig::from_base(config);
    assert!(result.is_err());

    match result {
        Err(TTSError::AuthenticationFailed(msg)) => {
            assert!(msg.contains("API key"));
        }
        _ => panic!("Expected AuthenticationFailed error"),
    }
}

#[test]
fn test_config_from_base_with_voice() {
    let config = TTSConfig {
        api_key: "test_key".to_string(),
        voice_id: Some("ocha-friendly".to_string()),
        ..Default::default()
    };

    let prosa_config = ProsaTtsConfig::from_base(config).unwrap();
    assert_eq!(prosa_config.voice, ProsaTtsVoice::OchaFriendly);
}

#[test]
fn test_config_from_base_with_model() {
    let config = TTSConfig {
        api_key: "test_key".to_string(),
        model: "tts-roger".to_string(),
        voice_id: None, // Explicitly None so model is used
        ..Default::default()
    };

    let prosa_config = ProsaTtsConfig::from_base(config).unwrap();
    assert_eq!(prosa_config.voice, ProsaTtsVoice::Roger);
}

#[test]
fn test_config_from_base_with_format() {
    let config = TTSConfig {
        api_key: "test_key".to_string(),
        audio_format: Some("mp3".to_string()),
        ..Default::default()
    };

    let prosa_config = ProsaTtsConfig::from_base(config).unwrap();
    assert_eq!(prosa_config.audio_format, ProsaTtsAudioFormat::Mp3);
}

#[test]
fn test_config_from_base_with_speed() {
    let config = TTSConfig {
        api_key: "test_key".to_string(),
        speaking_rate: Some(1.5),
        ..Default::default()
    };

    let prosa_config = ProsaTtsConfig::from_base(config).unwrap();
    assert!((prosa_config.tempo - 1.5).abs() < f32::EPSILON);
}

#[test]
fn test_config_from_base_speed_clamped() {
    // Test too low
    let config = TTSConfig {
        api_key: "test_key".to_string(),
        speaking_rate: Some(0.1),
        ..Default::default()
    };
    let prosa_config = ProsaTtsConfig::from_base(config).unwrap();
    assert!((prosa_config.tempo - MIN_TEMPO).abs() < f32::EPSILON);

    // Test too high
    let config = TTSConfig {
        api_key: "test_key".to_string(),
        speaking_rate: Some(5.0),
        ..Default::default()
    };
    let prosa_config = ProsaTtsConfig::from_base(config).unwrap();
    assert!((prosa_config.tempo - MAX_TEMPO).abs() < f32::EPSILON);
}

#[test]
fn test_config_validation_empty_key() {
    let config = ProsaTtsConfig::default();
    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_invalid_pitch() {
    let mut config = ProsaTtsConfig::default();
    config.api_key = "test_key".to_string();
    config.pitch = 20;
    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_invalid_tempo() {
    let mut config = ProsaTtsConfig::default();
    config.api_key = "test_key".to_string();
    config.tempo = 5.0;
    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_success() {
    let mut config = ProsaTtsConfig::default();
    config.api_key = "test_key".to_string();
    assert!(config.validate().is_ok());
}

// =============================================================================
// ProsaTtsVoice Tests
// =============================================================================

#[test]
fn test_voice_model_ids() {
    assert_eq!(ProsaTtsVoice::DimasFormal.model_id(), "tts-dimas-formal");
    assert_eq!(ProsaTtsVoice::DimasExpressive.model_id(), "tts-dimas-expressive");
    assert_eq!(ProsaTtsVoice::OchaFriendly.model_id(), "tts-ocha-friendly");
    assert_eq!(ProsaTtsVoice::Roger.model_id(), "tts-roger");
    assert_eq!(ProsaTtsVoice::Jennifer.model_id(), "tts-jennifer");
}

#[test]
fn test_voice_display_names() {
    assert!(ProsaTtsVoice::DimasFormal.display_name().contains("Dimas"));
    assert!(ProsaTtsVoice::OchaFriendly.display_name().contains("Ocha"));
    assert!(ProsaTtsVoice::Roger.display_name().contains("English"));
    assert!(ProsaTtsVoice::Jennifer.display_name().contains("English"));
}

#[test]
fn test_voice_language() {
    assert_eq!(ProsaTtsVoice::DimasFormal.language(), "id");
    assert_eq!(ProsaTtsVoice::OchaFriendly.language(), "id");
    assert_eq!(ProsaTtsVoice::Roger.language(), "en");
    assert_eq!(ProsaTtsVoice::Jennifer.language(), "en");
}

#[test]
fn test_voice_gender() {
    assert_eq!(ProsaTtsVoice::DimasFormal.gender(), "male");
    assert_eq!(ProsaTtsVoice::DimasExpressive.gender(), "male");
    assert_eq!(ProsaTtsVoice::OchaFriendly.gender(), "female");
    assert_eq!(ProsaTtsVoice::Dini.gender(), "female");
    assert_eq!(ProsaTtsVoice::Roger.gender(), "male");
    assert_eq!(ProsaTtsVoice::Jennifer.gender(), "female");
}

#[test]
fn test_voice_domain() {
    assert_eq!(ProsaTtsVoice::DimasFormal.domain(), "formal");
    assert_eq!(ProsaTtsVoice::DimasExpressive.domain(), "expressive");
    assert_eq!(ProsaTtsVoice::OchaFriendly.domain(), "friendly");
    assert_eq!(ProsaTtsVoice::Dini.domain(), "audiobook");
    assert_eq!(ProsaTtsVoice::Roger.domain(), "news");
}

#[test]
fn test_voice_from_str_default() {
    assert_eq!(ProsaTtsVoice::from_str_relaxed("dimas"), ProsaTtsVoice::DimasFormal);
    assert_eq!(ProsaTtsVoice::from_str_relaxed("default"), ProsaTtsVoice::DimasFormal);
}

#[test]
fn test_voice_from_str_variations() {
    assert_eq!(
        ProsaTtsVoice::from_str_relaxed("tts-dimas-formal"),
        ProsaTtsVoice::DimasFormal
    );
    assert_eq!(
        ProsaTtsVoice::from_str_relaxed("dimas_formal"),
        ProsaTtsVoice::DimasFormal
    );
    assert_eq!(
        ProsaTtsVoice::from_str_relaxed("ocha-friendly"),
        ProsaTtsVoice::OchaFriendly
    );
    assert_eq!(
        ProsaTtsVoice::from_str_relaxed("ocha"),
        ProsaTtsVoice::OchaFriendly
    );
}

#[test]
fn test_voice_from_str_english() {
    assert_eq!(ProsaTtsVoice::from_str_relaxed("roger"), ProsaTtsVoice::Roger);
    assert_eq!(ProsaTtsVoice::from_str_relaxed("english_male"), ProsaTtsVoice::Roger);
    assert_eq!(ProsaTtsVoice::from_str_relaxed("jennifer"), ProsaTtsVoice::Jennifer);
    assert_eq!(ProsaTtsVoice::from_str_relaxed("english_female"), ProsaTtsVoice::Jennifer);
}

#[test]
fn test_voice_from_str_custom() {
    let custom = ProsaTtsVoice::from_str_relaxed("some_custom_voice");
    match custom {
        ProsaTtsVoice::Custom(id) => assert_eq!(id, "some_custom_voice"),
        _ => panic!("Expected Custom variant"),
    }
}

#[test]
fn test_all_voices() {
    let voices = ProsaTtsVoice::all();
    assert_eq!(voices.len(), 9);
    assert!(voices.contains(&ProsaTtsVoice::DimasFormal));
    assert!(voices.contains(&ProsaTtsVoice::OchaFriendly));
    assert!(voices.contains(&ProsaTtsVoice::Roger));
    assert!(voices.contains(&ProsaTtsVoice::Jennifer));
}

#[test]
fn test_voice_display() {
    let voice = ProsaTtsVoice::DimasFormal;
    assert_eq!(format!("{}", voice), "tts-dimas-formal");
}

// =============================================================================
// ProsaTtsAudioFormat Tests
// =============================================================================

#[test]
fn test_audio_format_as_str() {
    assert_eq!(ProsaTtsAudioFormat::Opus.as_str(), "opus");
    assert_eq!(ProsaTtsAudioFormat::Mp3.as_str(), "mp3");
    assert_eq!(ProsaTtsAudioFormat::Wav.as_str(), "wav");
}

#[test]
fn test_audio_format_mime_type() {
    assert_eq!(ProsaTtsAudioFormat::Opus.mime_type(), "audio/opus");
    assert_eq!(ProsaTtsAudioFormat::Mp3.mime_type(), "audio/mpeg");
    assert_eq!(ProsaTtsAudioFormat::Wav.mime_type(), "audio/wav");
}

#[test]
fn test_audio_format_sample_rate() {
    assert_eq!(ProsaTtsAudioFormat::Opus.sample_rate(), DEFAULT_SAMPLE_RATE);
    assert_eq!(ProsaTtsAudioFormat::Mp3.sample_rate(), DEFAULT_SAMPLE_RATE);
    assert_eq!(ProsaTtsAudioFormat::Wav.sample_rate(), DEFAULT_SAMPLE_RATE);
}

#[test]
fn test_audio_format_from_str() {
    assert_eq!(ProsaTtsAudioFormat::from_str_relaxed("opus"), ProsaTtsAudioFormat::Opus);
    assert_eq!(ProsaTtsAudioFormat::from_str_relaxed("mp3"), ProsaTtsAudioFormat::Mp3);
    assert_eq!(ProsaTtsAudioFormat::from_str_relaxed("wav"), ProsaTtsAudioFormat::Wav);
    assert_eq!(ProsaTtsAudioFormat::from_str_relaxed("audio/mpeg"), ProsaTtsAudioFormat::Mp3);
    assert_eq!(ProsaTtsAudioFormat::from_str_relaxed("unknown"), ProsaTtsAudioFormat::Opus);
}

#[test]
fn test_audio_format_display() {
    let format = ProsaTtsAudioFormat::Mp3;
    assert_eq!(format!("{}", format), "mp3");
}

// =============================================================================
// Request/Response Tests
// =============================================================================

#[test]
fn test_request_serialization() {
    let request = ProsaTtsRequest {
        config: ProsaTtsRequestConfig {
            model: "tts-dimas-formal".to_string(),
            wait: Some(true),
            pitch: Some(0),
            tempo: Some(1.0),
            audio_format: Some("opus".to_string()),
            as_signed_url: None,
        },
        request: ProsaTtsRequestData {
            label: Some("test".to_string()),
            text: "Selamat pagi".to_string(),
        },
    };

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("tts-dimas-formal"));
    assert!(json.contains("Selamat pagi"));
    assert!(json.contains("\"wait\":true"));
}

#[test]
fn test_response_deserialization() {
    let json = r#"{
        "job_id": "test-job-123",
        "status": "complete",
        "result": {
            "data": "SGVsbG8gV29ybGQ=",
            "duration": 2.5,
            "sample_rate": 48000,
            "channels": 1
        },
        "model": {
            "id": "tts-dimas-formal",
            "language": "id",
            "gender": "male"
        }
    }"#;

    let response: ProsaTtsResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.job_id, "test-job-123");
    assert!(response.is_complete());
    assert!(!response.has_error());
    assert!(response.audio_data().is_some());
}

#[test]
fn test_response_is_complete() {
    let response = ProsaTtsResponse {
        job_id: "test".to_string(),
        status: "complete".to_string(),
        result: None,
        model: None,
        error: None,
    };
    assert!(response.is_complete());
    assert!(!response.is_in_progress());
    assert!(!response.has_error());
}

#[test]
fn test_response_is_in_progress() {
    let response = ProsaTtsResponse {
        job_id: "test".to_string(),
        status: "processing".to_string(),
        result: None,
        model: None,
        error: None,
    };
    assert!(response.is_in_progress());
    assert!(!response.is_complete());
}

#[test]
fn test_response_has_error() {
    let response = ProsaTtsResponse {
        job_id: "test".to_string(),
        status: "error".to_string(),
        result: None,
        model: None,
        error: Some(ProsaTtsError {
            code: "auth_invalid_api_key".to_string(),
            message: "Invalid API key".to_string(),
        }),
    };
    assert!(response.has_error());
    assert!(!response.is_complete());
}

#[test]
fn test_response_status_message_auth() {
    let response = ProsaTtsResponse {
        job_id: "test".to_string(),
        status: "error".to_string(),
        result: None,
        model: None,
        error: Some(ProsaTtsError {
            code: "auth_invalid_api_key".to_string(),
            message: String::new(),
        }),
    };
    assert_eq!(response.status_message(), "Invalid API key");
}

#[test]
fn test_response_status_message_custom() {
    let response = ProsaTtsResponse {
        job_id: "test".to_string(),
        status: "error".to_string(),
        result: None,
        model: None,
        error: Some(ProsaTtsError {
            code: "custom".to_string(),
            message: "Custom error message".to_string(),
        }),
    };
    assert_eq!(response.status_message(), "Custom error message");
}

// =============================================================================
// Text Validation Tests
// =============================================================================

#[test]
fn test_validate_text_empty() {
    let mut config = ProsaTtsConfig::default();
    config.api_key = "test_key".to_string();
    assert!(config.validate_text("").is_err());
}

#[test]
fn test_validate_text_too_long_sync() {
    let mut config = ProsaTtsConfig::default();
    config.api_key = "test_key".to_string();
    config.wait = true;
    let long_text = "a".repeat(MAX_SYNC_TEXT_LENGTH + 1);
    assert!(config.validate_text(&long_text).is_err());
}

#[test]
fn test_validate_text_too_long_async() {
    let mut config = ProsaTtsConfig::default();
    config.api_key = "test_key".to_string();
    config.wait = false;
    let long_text = "a".repeat(MAX_ASYNC_TEXT_LENGTH + 1);
    assert!(config.validate_text(&long_text).is_err());
}

#[test]
fn test_validate_text_valid_sync() {
    let mut config = ProsaTtsConfig::default();
    config.api_key = "test_key".to_string();
    config.wait = true;
    assert!(config.validate_text("Selamat pagi").is_ok());
}

#[test]
fn test_validate_text_valid_async() {
    let mut config = ProsaTtsConfig::default();
    config.api_key = "test_key".to_string();
    config.wait = false;
    let text = "a".repeat(MAX_SYNC_TEXT_LENGTH + 10); // Longer than sync limit
    assert!(config.validate_text(&text).is_ok());
}

// =============================================================================
// Client Tests
// =============================================================================

#[test]
fn test_new_valid_config() {
    let config = make_test_config();
    let result = ProsaTts::new(config);
    assert!(result.is_ok());

    let tts = result.unwrap();
    assert!(!tts.is_ready());
}

#[test]
fn test_new_empty_api_key() {
    let config = TTSConfig {
        provider: "prosa-ai".to_string(),
        api_key: String::new(),
        ..Default::default()
    };

    let result = ProsaTts::new(config);
    assert!(result.is_err());
}

#[test]
fn test_provider_info() {
    let tts = ProsaTts::new(make_test_config()).unwrap();
    let info = tts.get_provider_info();

    assert_eq!(info["provider"], "prosa-ai");
    assert!(info["voice"].as_str().unwrap().contains("dimas"));
    assert!(info["supported_languages"].as_array().unwrap().len() >= 2);
}

#[test]
fn test_default_state() {
    let tts = ProsaTts::default();
    assert!(!tts.is_ready());
    assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
}

#[test]
fn test_get_prosa_config() {
    let tts = ProsaTts::new(make_test_config()).unwrap();
    let config = tts.get_prosa_config();

    assert_eq!(config.api_key, "test_key");
    assert_eq!(config.voice, ProsaTtsVoice::DimasFormal);
}

#[test]
fn test_set_voice() {
    let mut tts = ProsaTts::new(make_test_config()).unwrap();
    assert_eq!(tts.get_prosa_config().voice, ProsaTtsVoice::DimasFormal);

    tts.set_voice(ProsaTtsVoice::OchaFriendly);
    assert_eq!(tts.get_prosa_config().voice, ProsaTtsVoice::OchaFriendly);
}

#[test]
fn test_set_audio_format() {
    let mut tts = ProsaTts::new(make_test_config()).unwrap();

    tts.set_audio_format(ProsaTtsAudioFormat::Mp3);
    assert_eq!(tts.get_prosa_config().audio_format, ProsaTtsAudioFormat::Mp3);
}

#[test]
fn test_set_pitch() {
    let mut tts = ProsaTts::new(make_test_config()).unwrap();

    tts.set_pitch(5);
    assert_eq!(tts.get_prosa_config().pitch, 5);

    // Test clamping
    tts.set_pitch(100);
    assert_eq!(tts.get_prosa_config().pitch, MAX_PITCH);

    tts.set_pitch(-100);
    assert_eq!(tts.get_prosa_config().pitch, MIN_PITCH);
}

#[test]
fn test_set_tempo() {
    let mut tts = ProsaTts::new(make_test_config()).unwrap();

    tts.set_tempo(1.5);
    assert!((tts.get_prosa_config().tempo - 1.5).abs() < f32::EPSILON);

    // Test clamping
    tts.set_tempo(10.0);
    assert!((tts.get_prosa_config().tempo - MAX_TEMPO).abs() < f32::EPSILON);

    tts.set_tempo(0.1);
    assert!((tts.get_prosa_config().tempo - MIN_TEMPO).abs() < f32::EPSILON);
}

#[tokio::test]
async fn test_connect_success() {
    let mut tts = ProsaTts::new(make_test_config()).unwrap();
    let result = tts.connect().await;

    assert!(result.is_ok());
    assert!(tts.is_ready());
    assert_eq!(tts.get_connection_state(), ConnectionState::Connected);
}

#[tokio::test]
async fn test_connect_empty_api_key() {
    let mut tts = ProsaTts::default();
    let result = tts.connect().await;

    assert!(result.is_err());
    match result {
        Err(TTSError::AuthenticationFailed(_)) => {}
        _ => panic!("Expected AuthenticationFailed error"),
    }
}

#[tokio::test]
async fn test_disconnect() {
    let mut tts = ProsaTts::new(make_test_config()).unwrap();
    tts.connect().await.unwrap();

    let result = tts.disconnect().await;
    assert!(result.is_ok());
    assert!(!tts.is_ready());
    assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
}

#[tokio::test]
async fn test_clear() {
    let mut tts = ProsaTts::new(make_test_config()).unwrap();
    let result = tts.clear().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_flush() {
    let tts = ProsaTts::new(make_test_config()).unwrap();
    let result = tts.flush().await;
    assert!(result.is_ok());
}

struct TestCallback;

impl AudioCallback for TestCallback {
    fn on_audio(&self, _data: AudioData) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }

    fn on_error(&self, _error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }

    fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

#[tokio::test]
async fn test_on_audio_callback() {
    let mut tts = ProsaTts::new(make_test_config()).unwrap();
    let callback = Arc::new(TestCallback);

    let result = tts.on_audio(callback);
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_remove_audio_callback() {
    let mut tts = ProsaTts::new(make_test_config()).unwrap();
    let callback = Arc::new(TestCallback);
    tts.on_audio(callback).unwrap();

    let result = tts.remove_audio_callback();
    assert!(result.is_ok());
}
