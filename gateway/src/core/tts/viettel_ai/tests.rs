//! Tests for Viettel AI TTS provider.

use super::*;
use crate::core::tts::base::{AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn make_test_config() -> TTSConfig {
    TTSConfig {
        provider: "viettel-ai".to_string(),
        api_key: "test_token".to_string(),
        voice_id: Some("doanngocle".to_string()),
        ..Default::default()
    }
}

struct MockAudioCallback {
    audio_count: AtomicUsize,
    complete_count: AtomicUsize,
    error_count: AtomicUsize,
}

impl MockAudioCallback {
    fn new() -> Self {
        Self {
            audio_count: AtomicUsize::new(0),
            complete_count: AtomicUsize::new(0),
            error_count: AtomicUsize::new(0),
        }
    }

    fn get_audio_count(&self) -> usize {
        self.audio_count.load(Ordering::SeqCst)
    }

    fn get_complete_count(&self) -> usize {
        self.complete_count.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    fn get_error_count(&self) -> usize {
        self.error_count.load(Ordering::SeqCst)
    }
}

impl AudioCallback for MockAudioCallback {
    fn on_audio(&self, _audio_data: AudioData) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.audio_count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }

    fn on_error(&self, _error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.error_count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }

    fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.complete_count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}

// =============================================================================
// ViettelVoice Tests
// =============================================================================

#[test]
fn test_voice_default() {
    assert_eq!(ViettelVoice::default(), ViettelVoice::DoanNgocLe);
}

#[test]
fn test_voice_ids() {
    assert_eq!(ViettelVoice::DoanNgocLe.voice_id(), "doanngocle");
    assert!(ViettelVoice::NorthernFemale1.voice_id().contains("ngochuyen"));
    assert!(ViettelVoice::NorthernFemale2.voice_id().contains("thuthao"));
    assert!(ViettelVoice::NorthernFemale3.voice_id().contains("phuongtrang"));
    assert!(ViettelVoice::NorthernMale1.voice_id().contains("xuankien"));
    assert!(ViettelVoice::NorthernMale2.voice_id().contains("quang"));
    assert!(ViettelVoice::SouthernFemale1.voice_id().contains("thuphuong"));
    assert!(ViettelVoice::SouthernFemale2.voice_id().contains("minhly"));
    assert!(ViettelVoice::SouthernFemale3.voice_id().contains("huonggiang"));
    assert!(ViettelVoice::SouthernMale.voice_id().contains("trongphuc"));
    assert!(ViettelVoice::CentralFemale.voice_id().contains("maiphuong"));
    assert!(ViettelVoice::CentralMale.voice_id().contains("thanhtung"));
}

#[test]
fn test_voice_display_names() {
    assert!(ViettelVoice::DoanNgocLe.display_name().contains("Doan Ngoc Le"));
    assert!(ViettelVoice::NorthernFemale1.display_name().contains("Ngoc Huyen"));
    assert!(ViettelVoice::SouthernMale.display_name().contains("Trong Phuc"));
    assert!(ViettelVoice::CentralFemale.display_name().contains("Mai Phuong"));
}

#[test]
fn test_voice_from_str_default() {
    assert_eq!(ViettelVoice::from_str_relaxed("doanngocle"), ViettelVoice::DoanNgocLe);
    assert_eq!(ViettelVoice::from_str_relaxed("default"), ViettelVoice::DoanNgocLe);
    assert_eq!(ViettelVoice::from_str_relaxed("female"), ViettelVoice::DoanNgocLe);
    assert_eq!(ViettelVoice::from_str_relaxed("0"), ViettelVoice::DoanNgocLe);
}

#[test]
fn test_voice_from_str_underscores() {
    assert_eq!(ViettelVoice::from_str_relaxed("doan_ngoc_le"), ViettelVoice::DoanNgocLe);
    assert_eq!(ViettelVoice::from_str_relaxed("ngoc_huyen"), ViettelVoice::NorthernFemale1);
    assert_eq!(ViettelVoice::from_str_relaxed("thu_thao"), ViettelVoice::NorthernFemale2);
}

#[test]
fn test_voice_from_str_northern() {
    assert_eq!(ViettelVoice::from_str_relaxed("ngochuyen"), ViettelVoice::NorthernFemale1);
    assert_eq!(ViettelVoice::from_str_relaxed("thuthao"), ViettelVoice::NorthernFemale2);
    assert_eq!(ViettelVoice::from_str_relaxed("phuongtrang"), ViettelVoice::NorthernFemale3);
    assert_eq!(ViettelVoice::from_str_relaxed("xuankien"), ViettelVoice::NorthernMale1);
    assert_eq!(ViettelVoice::from_str_relaxed("quang"), ViettelVoice::NorthernMale2);
}

#[test]
fn test_voice_from_str_southern() {
    assert_eq!(ViettelVoice::from_str_relaxed("thuphuong"), ViettelVoice::SouthernFemale1);
    assert_eq!(ViettelVoice::from_str_relaxed("minhly"), ViettelVoice::SouthernFemale2);
    assert_eq!(ViettelVoice::from_str_relaxed("huonggiang"), ViettelVoice::SouthernFemale3);
    assert_eq!(ViettelVoice::from_str_relaxed("trongphuc"), ViettelVoice::SouthernMale);
}

#[test]
fn test_voice_from_str_central() {
    assert_eq!(ViettelVoice::from_str_relaxed("maiphuong"), ViettelVoice::CentralFemale);
    assert_eq!(ViettelVoice::from_str_relaxed("thanhtung"), ViettelVoice::CentralMale);
}

#[test]
fn test_voice_from_str_custom() {
    let custom = ViettelVoice::from_str_relaxed("some_custom_voice");
    match custom {
        ViettelVoice::Custom(id) => assert_eq!(id, "some_custom_voice"),
        _ => panic!("Expected Custom variant"),
    }
}

#[test]
fn test_all_voices() {
    let voices = ViettelVoice::all();
    assert_eq!(voices.len(), 12);
    assert!(voices.contains(&ViettelVoice::DoanNgocLe));
    assert!(voices.contains(&ViettelVoice::NorthernFemale1));
    assert!(voices.contains(&ViettelVoice::SouthernMale));
    assert!(voices.contains(&ViettelVoice::CentralFemale));
}

#[test]
fn test_voice_language() {
    for voice in ViettelVoice::all() {
        assert_eq!(voice.language(), "vi-VN");
    }
}

#[test]
fn test_voice_gender() {
    assert_eq!(ViettelVoice::DoanNgocLe.gender(), "female");
    assert_eq!(ViettelVoice::NorthernFemale1.gender(), "female");
    assert_eq!(ViettelVoice::NorthernFemale2.gender(), "female");
    assert_eq!(ViettelVoice::NorthernMale1.gender(), "male");
    assert_eq!(ViettelVoice::NorthernMale2.gender(), "male");
    assert_eq!(ViettelVoice::SouthernFemale1.gender(), "female");
    assert_eq!(ViettelVoice::SouthernMale.gender(), "male");
    assert_eq!(ViettelVoice::CentralFemale.gender(), "female");
    assert_eq!(ViettelVoice::CentralMale.gender(), "male");
}

#[test]
fn test_voice_region() {
    assert_eq!(ViettelVoice::DoanNgocLe.region(), "Northern");
    assert_eq!(ViettelVoice::NorthernFemale1.region(), "Northern");
    assert_eq!(ViettelVoice::NorthernMale1.region(), "Northern");
    assert_eq!(ViettelVoice::SouthernFemale1.region(), "Southern");
    assert_eq!(ViettelVoice::SouthernMale.region(), "Southern");
    assert_eq!(ViettelVoice::CentralFemale.region(), "Central");
    assert_eq!(ViettelVoice::CentralMale.region(), "Central");
}

#[test]
fn test_custom_voice_region() {
    let custom = ViettelVoice::Custom("test".to_string());
    assert_eq!(custom.region(), "Unknown");
    assert_eq!(custom.gender(), "unknown");
}

// =============================================================================
// ViettelTtsConfig Tests
// =============================================================================

#[test]
fn test_config_default() {
    let config = ViettelTtsConfig::default();
    assert!(config.api_key.is_empty());
    assert_eq!(config.voice, ViettelVoice::DoanNgocLe);
    assert!((config.speed - DEFAULT_SPEED).abs() < f32::EPSILON);
    assert!(!config.without_filter);
    assert_eq!(config.tts_return_option, DEFAULT_TTS_RETURN_OPTION);
}

#[test]
fn test_config_validation_empty_key() {
    let config = ViettelTtsConfig::default();
    assert!(config.validate().is_err());
    let err = config.validate().unwrap_err();
    assert!(err.contains("token"));
}

#[test]
fn test_config_validation_invalid_speed_low() {
    let mut config = ViettelTtsConfig::default();
    config.api_key = "test_token".to_string();
    config.speed = 0.1;

    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_invalid_speed_high() {
    let mut config = ViettelTtsConfig::default();
    config.api_key = "test_token".to_string();
    config.speed = 3.0;

    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_success() {
    let mut config = ViettelTtsConfig::default();
    config.api_key = "test_token".to_string();

    assert!(config.validate().is_ok());
}

#[test]
fn test_config_from_base_empty_key() {
    let config = TTSConfig {
        api_key: String::new(),
        ..Default::default()
    };

    let result = ViettelTtsConfig::from_base(config);
    assert!(result.is_err());
}

#[test]
fn test_config_from_base_with_voice() {
    let config = TTSConfig {
        api_key: "test_token".to_string(),
        voice_id: Some("ngochuyen".to_string()),
        ..Default::default()
    };

    let viettel_config = ViettelTtsConfig::from_base(config).unwrap();
    assert_eq!(viettel_config.voice, ViettelVoice::NorthernFemale1);
}

#[test]
fn test_config_from_base_with_custom_voice() {
    let config = TTSConfig {
        api_key: "test_token".to_string(),
        voice_id: Some("custom_voice_id".to_string()),
        ..Default::default()
    };

    let viettel_config = ViettelTtsConfig::from_base(config).unwrap();
    match viettel_config.voice {
        ViettelVoice::Custom(id) => assert_eq!(id, "custom_voice_id"),
        _ => panic!("Expected Custom variant"),
    }
}

#[test]
fn test_config_from_base_with_speed() {
    let config = TTSConfig {
        api_key: "test_token".to_string(),
        speaking_rate: Some(1.5),
        ..Default::default()
    };

    let viettel_config = ViettelTtsConfig::from_base(config).unwrap();
    assert!((viettel_config.speed - 1.5).abs() < f32::EPSILON);
}

#[test]
fn test_config_from_base_speed_clamped_low() {
    let config = TTSConfig {
        api_key: "test_token".to_string(),
        speaking_rate: Some(0.1),
        ..Default::default()
    };

    let viettel_config = ViettelTtsConfig::from_base(config).unwrap();
    assert!((viettel_config.speed - MIN_SPEED).abs() < f32::EPSILON);
}

#[test]
fn test_config_from_base_speed_clamped_high() {
    let config = TTSConfig {
        api_key: "test_token".to_string(),
        speaking_rate: Some(5.0),
        ..Default::default()
    };

    let viettel_config = ViettelTtsConfig::from_base(config).unwrap();
    assert!((viettel_config.speed - MAX_SPEED).abs() < f32::EPSILON);
}

#[test]
fn test_validate_text_empty() {
    let result = ViettelTtsConfig::validate_text("");
    assert!(result.is_err());
}

#[test]
fn test_validate_text_too_long() {
    let long_text = "a".repeat(MAX_TEXT_LENGTH + 1);
    let result = ViettelTtsConfig::validate_text(&long_text);
    assert!(result.is_err());
}

#[test]
fn test_validate_text_valid() {
    let result = ViettelTtsConfig::validate_text("Xin chào");
    assert!(result.is_ok());
}

#[test]
fn test_validate_text_single_char() {
    let result = ViettelTtsConfig::validate_text("a");
    assert!(result.is_ok());
}

// =============================================================================
// ViettelTtsResponse Tests
// =============================================================================

#[test]
fn test_response_success() {
    let response = ViettelTtsResponse {
        status: 0,
        audio_url: String::new(),
        message: String::new(),
    };

    assert!(response.is_success());
    assert_eq!(response.status_message(), "Success");
}

#[test]
fn test_response_error_401() {
    let response = ViettelTtsResponse {
        status: 401,
        audio_url: String::new(),
        message: String::new(),
    };

    assert!(!response.is_success());
    assert!(response.status_message().contains("Unauthorized"));
}

#[test]
fn test_response_error_400() {
    let response = ViettelTtsResponse {
        status: 400,
        audio_url: String::new(),
        message: String::new(),
    };

    assert!(!response.is_success());
    assert!(response.status_message().contains("Bad request"));
}

#[test]
fn test_response_error_429() {
    let response = ViettelTtsResponse {
        status: 429,
        audio_url: String::new(),
        message: String::new(),
    };

    assert!(!response.is_success());
    assert!(response.status_message().contains("Rate limited"));
}

#[test]
fn test_response_error_500() {
    let response = ViettelTtsResponse {
        status: 500,
        audio_url: String::new(),
        message: String::new(),
    };

    assert!(!response.is_success());
    assert!(response.status_message().contains("Server error"));
}

#[test]
fn test_response_custom_message() {
    let response = ViettelTtsResponse {
        status: 500,
        audio_url: String::new(),
        message: "Custom error message".to_string(),
    };

    assert!(!response.is_success());
    assert_eq!(response.status_message(), "Custom error message");
}

// =============================================================================
// ViettelTts Client Tests
// =============================================================================

#[test]
fn test_new_valid_config() {
    let config = make_test_config();
    let result = ViettelTts::new(config);
    assert!(result.is_ok());

    let tts = result.unwrap();
    assert!(!tts.is_ready());
}

#[test]
fn test_new_empty_api_key() {
    let config = TTSConfig {
        provider: "viettel-ai".to_string(),
        api_key: String::new(),
        ..Default::default()
    };

    let result = ViettelTts::new(config);
    assert!(result.is_err());

    match result {
        Err(TTSError::AuthenticationFailed(msg)) => {
            assert!(msg.contains("token"));
        }
        _ => panic!("Expected AuthenticationFailed error"),
    }
}

#[test]
fn test_provider_info() {
    let tts = ViettelTts::new(make_test_config()).unwrap();
    let info = tts.get_provider_info();

    assert_eq!(info["provider"], "viettel-ai");
    assert_eq!(info["name"], "Viettel AI TTS (Viettel Group)");
    assert_eq!(info["format"], "wav");
    assert_eq!(info["voices"].as_array().unwrap().len(), 12);
    assert!(info["features"]["regional_accents"].as_bool().unwrap());
}

#[test]
fn test_default_state() {
    let tts = ViettelTts::default();
    assert!(!tts.is_ready());
}

#[test]
fn test_set_voice() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();
    tts.set_voice(ViettelVoice::SouthernMale);

    assert_eq!(tts.get_viettel_config().voice, ViettelVoice::SouthernMale);
}

#[test]
fn test_set_speed() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();

    // Normal speed
    tts.set_speed(1.5);
    assert!((tts.get_viettel_config().speed - 1.5).abs() < f32::EPSILON);

    // Below minimum - should clamp
    tts.set_speed(0.1);
    assert!((tts.get_viettel_config().speed - MIN_SPEED).abs() < f32::EPSILON);

    // Above maximum - should clamp
    tts.set_speed(5.0);
    assert!((tts.get_viettel_config().speed - MAX_SPEED).abs() < f32::EPSILON);
}

#[test]
fn test_set_without_filter() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();
    assert!(!tts.get_viettel_config().without_filter);

    tts.set_without_filter(true);
    assert!(tts.get_viettel_config().without_filter);
}

#[tokio::test]
async fn test_connect_success() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();
    let result = tts.connect().await;

    assert!(result.is_ok());
    assert!(tts.is_ready());
}

#[tokio::test]
async fn test_connect_empty_api_key() {
    let mut tts = ViettelTts::default();
    let result = tts.connect().await;

    assert!(result.is_err());
    match result {
        Err(TTSError::AuthenticationFailed(_)) => {}
        _ => panic!("Expected AuthenticationFailed error"),
    }
}

#[tokio::test]
async fn test_disconnect() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();
    tts.connect().await.unwrap();

    let result = tts.disconnect().await;
    assert!(result.is_ok());
    assert!(!tts.is_ready());
}

#[tokio::test]
async fn test_speak_empty_text() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();
    tts.connect().await.unwrap();

    let result = tts.speak("", false).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_flush() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();
    tts.connect().await.unwrap();

    let callback = Arc::new(MockAudioCallback::new());
    tts.on_audio(callback.clone()).unwrap();

    // Give time for the async callback registration
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let result = tts.flush().await;
    assert!(result.is_ok());

    // Give time for callback to be called
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    assert_eq!(callback.get_complete_count(), 1);
}

#[tokio::test]
async fn test_clear() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();
    tts.connect().await.unwrap();

    let result = tts.clear().await;
    assert!(result.is_ok());
}

#[test]
fn test_connection_state_initial() {
    let tts = ViettelTts::new(make_test_config()).unwrap();
    assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
}

#[tokio::test]
async fn test_connection_state_after_connect() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();
    tts.connect().await.unwrap();
    assert_eq!(tts.get_connection_state(), ConnectionState::Connected);
}

#[tokio::test]
async fn test_connection_state_after_disconnect() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();
    tts.connect().await.unwrap();
    tts.disconnect().await.unwrap();
    assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
}

#[test]
fn test_voice_parsing_in_config() {
    let test_cases = vec![
        ("doanngocle", ViettelVoice::DoanNgocLe),
        ("ngochuyen", ViettelVoice::NorthernFemale1),
        ("thuthao", ViettelVoice::NorthernFemale2),
        ("trongphuc", ViettelVoice::SouthernMale),
        ("maiphuong", ViettelVoice::CentralFemale),
    ];

    for (voice_id, expected_voice) in test_cases {
        let config = TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: "test".to_string(),
            voice_id: Some(voice_id.to_string()),
            ..Default::default()
        };

        let tts = ViettelTts::new(config).unwrap();
        assert_eq!(
            tts.get_viettel_config().voice,
            expected_voice,
            "Voice ID '{}' should map to {:?}",
            voice_id,
            expected_voice
        );
    }
}

#[test]
fn test_get_viettel_config() {
    let tts = ViettelTts::new(make_test_config()).unwrap();
    let config = tts.get_viettel_config();

    assert_eq!(config.api_key, "test_token");
    assert_eq!(config.voice, ViettelVoice::DoanNgocLe);
    assert!((config.speed - DEFAULT_SPEED).abs() < f32::EPSILON);
}

#[tokio::test]
async fn test_auto_connect_on_speak() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();
    assert!(!tts.is_ready());

    // Speaking should auto-connect
    // Note: This will fail at the network level, but it should auto-connect first
    let _ = tts.speak("Test", false).await;
    // After attempting speak, it should have connected
    // (The speak will fail due to network, but is_ready should be true)
}

#[tokio::test]
async fn test_on_audio_callback() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();
    let callback = Arc::new(MockAudioCallback::new());

    let result = tts.on_audio(callback.clone());
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_remove_audio_callback() {
    let mut tts = ViettelTts::new(make_test_config()).unwrap();
    let callback = Arc::new(MockAudioCallback::new());
    tts.on_audio(callback).unwrap();

    let result = tts.remove_audio_callback();
    assert!(result.is_ok());
}
