//! Tests for Prosa.ai STT provider.

use super::*;
use crate::core::stt::base::{BaseSTT, STTConfig, STTError, STTResult};
use bytes::Bytes;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn make_test_config() -> STTConfig {
    STTConfig {
        provider: "prosa-ai".to_string(),
        api_key: "test_key".to_string(),
        language: "id".to_string(),
        sample_rate: 16000,
        channels: 1,
        ..Default::default()
    }
}

// =============================================================================
// ProsaSttConfig Tests
// =============================================================================

#[test]
fn test_config_default() {
    let config = ProsaSttConfig::default();
    assert!(config.api_key.is_empty());
    assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
    assert_eq!(config.channels, DEFAULT_CHANNELS);
    assert_eq!(config.model, ProsaSttModel::General);
    assert!(config.wait);
    assert!(config.include_partial);
    assert!(config.auto_punctuation);
}

#[test]
fn test_config_validation_empty_key() {
    let config = ProsaSttConfig::default();
    assert!(config.validate().is_err());
    let err = config.validate().unwrap_err();
    assert!(err.contains("API key"));
}

// W1 keystone: advanced features Prosa.ai can express (interim_results -> include_partial,
// filler_words -> include_filler, smart_format -> auto_punctuation) survive through the
// provider-struct `new_standard` method to the provider-specific config.
#[test]
fn test_prosa_new_standard_unlocks_advanced_features() {
    use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
    let std = StandardSTTConfig {
        base: STTConfig {
            provider: "prosa-ai".into(),
            api_key: "test_key".into(),
            ..Default::default()
        },
        features: SttFeatures {
            interim_results: Some(false),
            filler_words: Some(true),
            smart_format: Some(false),
            ..Default::default()
        },
        extras: ProviderExtras::default(),
    };
    let stt = ProsaStt::new_standard(&std).expect("new_standard must succeed");
    let cfg = stt.get_prosa_config();
    assert!(!cfg.include_partial); // interim_results
    assert!(cfg.include_filler); // filler_words
    assert!(!cfg.auto_punctuation); // smart_format

    // Missing api_key is rejected through the standardized path too.
    let bad = StandardSTTConfig::from_base(STTConfig {
        provider: "prosa-ai".into(),
        api_key: String::new(),
        ..Default::default()
    });
    assert!(ProsaStt::new_standard(&bad).is_err());
}

#[test]
fn test_config_validation_zero_sample_rate() {
    let mut config = ProsaSttConfig::default();
    config.api_key = "test_key".to_string();
    config.sample_rate = 0;

    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_zero_channels() {
    let mut config = ProsaSttConfig::default();
    config.api_key = "test_key".to_string();
    config.channels = 0;

    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_success() {
    let mut config = ProsaSttConfig::default();
    config.api_key = "test_key".to_string();

    assert!(config.validate().is_ok());
}

#[test]
fn test_config_from_base_empty_key() {
    let config = STTConfig {
        api_key: String::new(),
        ..Default::default()
    };

    let result = ProsaSttConfig::from_base(&config);
    assert!(result.is_err());
}

#[test]
fn test_config_from_base_with_params() {
    let config = STTConfig {
        api_key: "test_key".to_string(),
        sample_rate: 8000,
        channels: 2,
        ..Default::default()
    };

    let prosa_config = ProsaSttConfig::from_base(&config).unwrap();
    assert_eq!(prosa_config.sample_rate, 8000);
    assert_eq!(prosa_config.channels, 2);
}

#[test]
fn test_config_from_base_default_sample_rate() {
    let config = STTConfig {
        api_key: "test_key".to_string(),
        sample_rate: 0,
        ..Default::default()
    };

    let prosa_config = ProsaSttConfig::from_base(&config).unwrap();
    assert_eq!(prosa_config.sample_rate, DEFAULT_SAMPLE_RATE);
}

#[test]
fn test_config_from_base_default_channels() {
    let config = STTConfig {
        api_key: "test_key".to_string(),
        channels: 0,
        ..Default::default()
    };

    let prosa_config = ProsaSttConfig::from_base(&config).unwrap();
    assert_eq!(prosa_config.channels, DEFAULT_CHANNELS);
}

// =============================================================================
// ProsaSttModel Tests
// =============================================================================

#[test]
fn test_model_as_str() {
    assert_eq!(ProsaSttModel::General.as_str(), "stt-general");
    assert_eq!(ProsaSttModel::GeneralOnline.as_str(), "stt-general-online");
}

#[test]
fn test_model_supports_streaming() {
    assert!(!ProsaSttModel::General.supports_streaming());
    assert!(ProsaSttModel::GeneralOnline.supports_streaming());
}

#[test]
fn test_model_from_str_general() {
    assert_eq!(
        ProsaSttModel::from_str_relaxed("stt-general"),
        Some(ProsaSttModel::General)
    );
    assert_eq!(
        ProsaSttModel::from_str_relaxed("general"),
        Some(ProsaSttModel::General)
    );
    assert_eq!(
        ProsaSttModel::from_str_relaxed("STT-GENERAL"),
        Some(ProsaSttModel::General)
    );
}

#[test]
fn test_model_from_str_online() {
    assert_eq!(
        ProsaSttModel::from_str_relaxed("stt-general-online"),
        Some(ProsaSttModel::GeneralOnline)
    );
    assert_eq!(
        ProsaSttModel::from_str_relaxed("streaming"),
        Some(ProsaSttModel::GeneralOnline)
    );
    assert_eq!(
        ProsaSttModel::from_str_relaxed("online"),
        Some(ProsaSttModel::GeneralOnline)
    );
}

#[test]
fn test_model_from_str_invalid() {
    assert_eq!(ProsaSttModel::from_str_relaxed("invalid"), None);
    assert_eq!(ProsaSttModel::from_str_relaxed(""), None);
}

#[test]
fn test_model_display() {
    assert_eq!(format!("{}", ProsaSttModel::General), "stt-general");
    assert_eq!(
        format!("{}", ProsaSttModel::GeneralOnline),
        "stt-general-online"
    );
}

#[test]
fn test_model_from_str_trait() {
    

    let model: ProsaSttModel = "general".parse().unwrap();
    assert_eq!(model, ProsaSttModel::General);

    let result: Result<ProsaSttModel, _> = "invalid".parse();
    assert!(result.is_err());
}

// =============================================================================
// ProsaAudioFormat Tests
// =============================================================================

#[test]
fn test_audio_format_as_str() {
    assert_eq!(ProsaAudioFormat::Wav.as_str(), "wav");
    assert_eq!(ProsaAudioFormat::Mp3.as_str(), "mp3");
    assert_eq!(ProsaAudioFormat::Ogg.as_str(), "ogg");
    assert_eq!(ProsaAudioFormat::Flac.as_str(), "flac");
    assert_eq!(ProsaAudioFormat::WebM.as_str(), "webm");
}

#[test]
fn test_audio_format_mime_type() {
    assert_eq!(ProsaAudioFormat::Wav.mime_type(), "audio/wav");
    assert_eq!(ProsaAudioFormat::Mp3.mime_type(), "audio/mpeg");
    assert_eq!(ProsaAudioFormat::Ogg.mime_type(), "audio/ogg");
    assert_eq!(ProsaAudioFormat::Flac.mime_type(), "audio/flac");
    assert_eq!(ProsaAudioFormat::WebM.mime_type(), "audio/webm");
}

#[test]
fn test_audio_format_display() {
    assert_eq!(format!("{}", ProsaAudioFormat::Wav), "wav");
    assert_eq!(format!("{}", ProsaAudioFormat::Mp3), "mp3");
}

// =============================================================================
// ProsaSttResponse Tests
// =============================================================================

#[test]
fn test_response_is_complete() {
    let response = ProsaSttResponse {
        job_id: "test".to_string(),
        status: "complete".to_string(),
        result: None,
        model: None,
        error: None,
    };
    assert!(response.is_complete());
}

#[test]
fn test_response_is_not_complete() {
    let response = ProsaSttResponse {
        job_id: "test".to_string(),
        status: "processing".to_string(),
        result: None,
        model: None,
        error: None,
    };
    assert!(!response.is_complete());
}

#[test]
fn test_response_is_in_progress_created() {
    let response = ProsaSttResponse {
        job_id: "test".to_string(),
        status: "created".to_string(),
        result: None,
        model: None,
        error: None,
    };
    assert!(response.is_in_progress());
}

#[test]
fn test_response_is_in_progress_processing() {
    let response = ProsaSttResponse {
        job_id: "test".to_string(),
        status: "processing".to_string(),
        result: None,
        model: None,
        error: None,
    };
    assert!(response.is_in_progress());
}

#[test]
fn test_response_has_error_status() {
    let response = ProsaSttResponse {
        job_id: "test".to_string(),
        status: "error".to_string(),
        result: None,
        model: None,
        error: None,
    };
    assert!(response.has_error());
}

#[test]
fn test_response_has_error_object() {
    let response = ProsaSttResponse {
        job_id: "test".to_string(),
        status: "complete".to_string(),
        result: None,
        model: None,
        error: Some(ProsaSttError {
            code: "test".to_string(),
            message: "Test error".to_string(),
        }),
    };
    assert!(response.has_error());
}

#[test]
fn test_response_transcription() {
    let response = ProsaSttResponse {
        job_id: "test".to_string(),
        status: "complete".to_string(),
        result: Some(ProsaSttResult {
            data: vec![
                ProsaSttSegment {
                    transcript: "Selamat".to_string(),
                    time_start: 0.0,
                    time_end: 0.5,
                    channel: 0,
                    speaker: None,
                },
                ProsaSttSegment {
                    transcript: "pagi".to_string(),
                    time_start: 0.5,
                    time_end: 1.0,
                    channel: 0,
                    speaker: None,
                },
            ],
        }),
        model: None,
        error: None,
    };
    assert_eq!(response.transcription(), Some("Selamat pagi".to_string()));
}

#[test]
fn test_response_transcription_empty() {
    let response = ProsaSttResponse {
        job_id: "test".to_string(),
        status: "complete".to_string(),
        result: None,
        model: None,
        error: None,
    };
    assert_eq!(response.transcription(), None);
}

#[test]
fn test_response_status_message_custom() {
    let response = ProsaSttResponse {
        job_id: "test".to_string(),
        status: "error".to_string(),
        result: None,
        model: None,
        error: Some(ProsaSttError {
            code: "".to_string(),
            message: "Custom error message".to_string(),
        }),
    };
    assert_eq!(response.status_message(), "Custom error message");
}

#[test]
fn test_response_status_message_auth_invalid() {
    let response = ProsaSttResponse {
        job_id: "test".to_string(),
        status: "error".to_string(),
        result: None,
        model: None,
        error: Some(ProsaSttError {
            code: "auth_invalid_api_key".to_string(),
            message: String::new(),
        }),
    };
    assert_eq!(response.status_message(), "Invalid API key");
}

#[test]
fn test_response_status_message_quota_insufficient() {
    let response = ProsaSttResponse {
        job_id: "test".to_string(),
        status: "error".to_string(),
        result: None,
        model: None,
        error: Some(ProsaSttError {
            code: "quota_insufficient".to_string(),
            message: String::new(),
        }),
    };
    assert_eq!(response.status_message(), "Insufficient quota");
}

#[test]
fn test_response_status_message_model_not_found() {
    let response = ProsaSttResponse {
        job_id: "test".to_string(),
        status: "error".to_string(),
        result: None,
        model: None,
        error: Some(ProsaSttError {
            code: "asr_model_not_found".to_string(),
            message: String::new(),
        }),
    };
    assert_eq!(response.status_message(), "ASR model not found");
}

// =============================================================================
// ProsaStt Client Tests
// =============================================================================

#[test]
fn test_new_valid_config() {
    let config = make_test_config();
    let result = ProsaStt::new(config);
    assert!(result.is_ok());

    let stt = result.unwrap();
    assert!(!stt.is_ready());
}

#[test]
fn test_new_empty_api_key() {
    let config = STTConfig {
        provider: "prosa-ai".to_string(),
        api_key: String::new(),
        ..Default::default()
    };

    let result = ProsaStt::new(config);
    assert!(result.is_err());

    match result {
        Err(STTError::AuthenticationFailed(msg)) => {
            assert!(msg.contains("API key"));
        }
        _ => panic!("Expected AuthenticationFailed error"),
    }
}

#[test]
fn test_provider_info() {
    let stt = ProsaStt::new(make_test_config()).unwrap();
    let info = stt.get_provider_info();

    assert!(info.contains("Prosa.ai"));
    assert!(info.contains("Indonesian"));
}

#[test]
fn test_default_state() {
    let stt = ProsaStt::default();
    assert!(!stt.is_ready());
}

#[test]
fn test_get_config() {
    let config = make_test_config();
    let stt = ProsaStt::new(config.clone()).unwrap();

    let retrieved_config = stt.get_config();
    assert!(retrieved_config.is_some());
    assert_eq!(retrieved_config.unwrap().api_key, config.api_key);
}

#[test]
fn test_get_prosa_config() {
    let stt = ProsaStt::new(make_test_config()).unwrap();
    let config = stt.get_prosa_config();

    assert_eq!(config.api_key, "test_key");
    assert_eq!(config.sample_rate, 16000);
    assert_eq!(config.channels, 1);
    assert_eq!(config.model, ProsaSttModel::General);
}

#[tokio::test]
async fn test_connect_success() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();
    let result = stt.connect().await;

    assert!(result.is_ok());
    assert!(stt.is_ready());
}

#[tokio::test]
async fn test_connect_empty_api_key() {
    let mut stt = ProsaStt::default();
    let result = stt.connect().await;

    assert!(result.is_err());
    match result {
        Err(STTError::AuthenticationFailed(_)) => {}
        _ => panic!("Expected AuthenticationFailed error"),
    }
}

#[tokio::test]
async fn test_disconnect() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    let result = stt.disconnect().await;
    assert!(result.is_ok());
    assert!(!stt.is_ready());
}

#[tokio::test]
async fn test_send_audio_empty() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    let result = stt.send_audio(Bytes::new()).await;
    assert!(result.is_ok());
    assert_eq!(stt.buffer_size().await, 0);
}

#[tokio::test]
async fn test_send_audio_buffers() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    let audio_data = Bytes::from(vec![0u8; 1000]);
    let result = stt.send_audio(audio_data).await;

    assert!(result.is_ok());
    assert_eq!(stt.buffer_size().await, 1000);
}

#[tokio::test]
async fn test_send_audio_multiple() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
    stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
    stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();

    assert_eq!(stt.buffer_size().await, 1500);
}

#[tokio::test]
async fn test_flush_empty_buffer() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    let result = stt.flush().await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[tokio::test]
async fn test_disconnect_clears_buffer() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    stt.send_audio(Bytes::from(vec![0u8; 1000])).await.unwrap();
    assert!(stt.buffer_size().await > 0);

    stt.disconnect().await.unwrap();
    assert_eq!(stt.buffer_size().await, 0);
}

#[tokio::test]
async fn test_on_result_callback() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();
    let callback_count = Arc::new(AtomicUsize::new(0));
    let callback_count_clone = callback_count.clone();

    let callback: crate::core::stt::base::STTResultCallback =
        Arc::new(move |_result: STTResult| {
            callback_count_clone.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        });

    let result = stt.on_result(callback).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_on_error_callback() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();
    let callback_count = Arc::new(AtomicUsize::new(0));
    let callback_count_clone = callback_count.clone();

    let callback: crate::core::stt::base::STTErrorCallback = Arc::new(move |_error: STTError| {
        callback_count_clone.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    });

    let result = stt.on_error(callback).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_update_config() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();

    let new_config = STTConfig {
        provider: "prosa-ai".to_string(),
        api_key: "new_key".to_string(),
        sample_rate: 8000,
        channels: 2,
        ..Default::default()
    };

    let result = stt.update_config(new_config).await;
    assert!(result.is_ok());

    assert_eq!(stt.get_prosa_config().sample_rate, 8000);
    assert_eq!(stt.get_prosa_config().channels, 2);
    assert_eq!(stt.get_prosa_config().api_key, "new_key");
}

#[tokio::test]
async fn test_auto_connect_on_send_audio() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();
    assert!(!stt.is_ready());

    // Should auto-connect
    let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
    assert!(result.is_ok());
    assert!(stt.is_ready());
}

#[tokio::test]
async fn test_buffer_size() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    assert_eq!(stt.buffer_size().await, 0);

    stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
    assert_eq!(stt.buffer_size().await, 500);

    stt.send_audio(Bytes::from(vec![0u8; 300])).await.unwrap();
    assert_eq!(stt.buffer_size().await, 800);
}

#[tokio::test]
async fn test_connect_clears_previous_buffer() {
    let mut stt = ProsaStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    // Add some audio
    stt.send_audio(Bytes::from(vec![0u8; 1000])).await.unwrap();
    assert_eq!(stt.buffer_size().await, 1000);

    // Disconnect
    stt.disconnect().await.unwrap();
    assert_eq!(stt.buffer_size().await, 0);

    // Reconnect - buffer should be clear
    stt.connect().await.unwrap();
    assert_eq!(stt.buffer_size().await, 0);
}

// =============================================================================
// Serialization Tests
// =============================================================================

#[test]
fn test_request_serialization() {
    use super::config::{ProsaSttRequest, ProsaSttRequestConfig, ProsaSttRequestData};

    let request = ProsaSttRequest {
        config: ProsaSttRequestConfig {
            engine: "stt-general".to_string(),
            wait: Some(true),
            speaker_count: None,
            include_filler: Some(false),
            auto_punctuation: Some(true),
            enable_spoken_numerals: Some(true),
        },
        request: ProsaSttRequestData {
            label: Some("test".to_string()),
            data: Some("base64data".to_string()),
            uri: None,
        },
    };

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("stt-general"));
    assert!(json.contains("base64data"));
    assert!(json.contains("auto_punctuation"));
}

#[test]
fn test_stream_config_serialization() {
    use super::config::{ProsaSttAudioConfig, ProsaSttStreamConfig};

    let config = ProsaSttStreamConfig {
        model: "stt-general-online".to_string(),
        label: Some("test".to_string()),
        include_partial: Some(true),
        audio: Some(ProsaSttAudioConfig {
            format: "wav".to_string(),
            channels: Some(1),
            sample_rate: Some(16000),
        }),
    };

    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("stt-general-online"));
    assert!(json.contains("wav"));
    assert!(json.contains("16000"));
}

#[test]
fn test_response_deserialization() {
    let json = r#"{
        "job_id": "abc123",
        "status": "complete",
        "result": {
            "data": [
                {
                    "transcript": "Selamat pagi",
                    "time_start": 0.0,
                    "time_end": 1.5,
                    "channel": 0
                }
            ]
        },
        "model": {
            "id": "stt-general",
            "language": "id"
        }
    }"#;

    let response: ProsaSttResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.job_id, "abc123");
    assert!(response.is_complete());
    assert_eq!(response.transcription(), Some("Selamat pagi".to_string()));
}

#[test]
fn test_error_response_deserialization() {
    let json = r#"{
        "job_id": "",
        "status": "error",
        "error": {
            "code": "auth_invalid_api_key",
            "message": "Invalid API key provided"
        }
    }"#;

    let response: ProsaSttResponse = serde_json::from_str(json).unwrap();
    assert!(response.has_error());
    assert_eq!(response.status_message(), "Invalid API key provided");
}
