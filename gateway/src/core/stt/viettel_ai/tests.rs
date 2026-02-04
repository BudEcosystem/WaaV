//! Tests for Viettel AI STT provider.

use super::*;
use crate::core::stt::base::{BaseSTT, STTConfig, STTError, STTResult};
use bytes::Bytes;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn make_test_config() -> STTConfig {
    STTConfig {
        provider: "viettel-ai".to_string(),
        api_key: "test_token".to_string(),
        language: "vi".to_string(),
        sample_rate: 16000,
        channels: 1,
        ..Default::default()
    }
}

// =============================================================================
// ViettelSttConfig Tests
// =============================================================================

#[test]
fn test_config_default() {
    let config = ViettelSttConfig::default();
    assert!(config.api_key.is_empty());
    assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
    assert_eq!(config.channels, DEFAULT_CHANNELS);
    assert_eq!(config.format, PCM_FORMAT_S16LE);
    assert!(config.asr_model.is_none());
}

#[test]
fn test_config_validation_empty_key() {
    let config = ViettelSttConfig::default();
    assert!(config.validate().is_err());
    let err = config.validate().unwrap_err();
    assert!(err.contains("token"));
}

#[test]
fn test_config_validation_zero_sample_rate() {
    let mut config = ViettelSttConfig::default();
    config.api_key = "test_token".to_string();
    config.sample_rate = 0;

    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_zero_channels() {
    let mut config = ViettelSttConfig::default();
    config.api_key = "test_token".to_string();
    config.channels = 0;

    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_success() {
    let mut config = ViettelSttConfig::default();
    config.api_key = "test_token".to_string();

    assert!(config.validate().is_ok());
}

#[test]
fn test_config_from_base_empty_key() {
    let config = STTConfig {
        api_key: String::new(),
        ..Default::default()
    };

    let result = ViettelSttConfig::from_base(&config);
    assert!(result.is_err());
}

#[test]
fn test_config_from_base_with_params() {
    let config = STTConfig {
        api_key: "test_token".to_string(),
        sample_rate: 8000,
        channels: 1,
        ..Default::default()
    };

    let viettel_config = ViettelSttConfig::from_base(&config).unwrap();
    assert_eq!(viettel_config.sample_rate, 8000);
    assert_eq!(viettel_config.channels, 1);
}

#[test]
fn test_config_from_base_default_sample_rate() {
    let config = STTConfig {
        api_key: "test_token".to_string(),
        sample_rate: 0,
        ..Default::default()
    };

    let viettel_config = ViettelSttConfig::from_base(&config).unwrap();
    assert_eq!(viettel_config.sample_rate, DEFAULT_SAMPLE_RATE);
}

#[test]
fn test_config_from_base_default_channels() {
    let config = STTConfig {
        api_key: "test_token".to_string(),
        channels: 0,
        ..Default::default()
    };

    let viettel_config = ViettelSttConfig::from_base(&config).unwrap();
    assert_eq!(viettel_config.channels, DEFAULT_CHANNELS);
}

// =============================================================================
// ViettelSttResponse Tests
// =============================================================================

#[test]
fn test_response_success() {
    let response = ViettelSttResponse {
        status: 0,
        result: "Xin chào Việt Nam".to_string(),
        message: String::new(),
    };

    assert!(response.is_success());
    assert_eq!(response.transcription(), Some("Xin chào Việt Nam"));
    assert_eq!(response.status_message(), "Success");
}

#[test]
fn test_response_no_voice() {
    let response = ViettelSttResponse {
        status: 1,
        result: String::new(),
        message: String::new(),
    };

    assert!(!response.is_success());
    assert_eq!(response.transcription(), None);
    assert_eq!(response.status_message(), "No voice detected");
}

#[test]
fn test_response_unauthorized() {
    let response = ViettelSttResponse {
        status: 401,
        result: String::new(),
        message: String::new(),
    };

    assert!(!response.is_success());
    assert!(response.status_message().contains("Unauthorized"));
}

#[test]
fn test_response_bad_request() {
    let response = ViettelSttResponse {
        status: 400,
        result: String::new(),
        message: String::new(),
    };

    assert!(!response.is_success());
    assert!(response.status_message().contains("Bad request"));
}

#[test]
fn test_response_server_error() {
    let response = ViettelSttResponse {
        status: 500,
        result: String::new(),
        message: String::new(),
    };

    assert!(!response.is_success());
    assert!(response.status_message().contains("Server error"));
}

#[test]
fn test_response_custom_message() {
    let response = ViettelSttResponse {
        status: 500,
        result: String::new(),
        message: "Custom error message".to_string(),
    };

    assert!(!response.is_success());
    assert_eq!(response.status_message(), "Custom error message");
}

#[test]
fn test_response_empty_result_still_success() {
    let response = ViettelSttResponse {
        status: 0,
        result: String::new(),
        message: String::new(),
    };

    // Status is 0 but result is empty
    assert!(response.is_success());
    assert_eq!(response.transcription(), None);
}

// =============================================================================
// ViettelStt Client Tests
// =============================================================================

#[test]
fn test_new_valid_config() {
    let config = make_test_config();
    let result = ViettelStt::new(config);
    assert!(result.is_ok());

    let stt = result.unwrap();
    assert!(!stt.is_ready());
}

#[test]
fn test_new_empty_api_key() {
    let config = STTConfig {
        provider: "viettel-ai".to_string(),
        api_key: String::new(),
        ..Default::default()
    };

    let result = ViettelStt::new(config);
    assert!(result.is_err());

    match result {
        Err(STTError::AuthenticationFailed(msg)) => {
            assert!(msg.contains("token"));
        }
        _ => panic!("Expected AuthenticationFailed error"),
    }
}

#[test]
fn test_provider_info() {
    let stt = ViettelStt::new(make_test_config()).unwrap();
    let info = stt.get_provider_info();

    assert!(info.contains("Viettel"));
    assert!(info.contains("Vietnamese"));
    assert!(info.contains("96%"));
}

#[test]
fn test_default_state() {
    let stt = ViettelStt::default();
    assert!(!stt.is_ready());
}

#[test]
fn test_get_config() {
    let config = make_test_config();
    let stt = ViettelStt::new(config.clone()).unwrap();

    let retrieved_config = stt.get_config();
    assert!(retrieved_config.is_some());
    assert_eq!(retrieved_config.unwrap().api_key, config.api_key);
}

#[test]
fn test_get_viettel_config() {
    let stt = ViettelStt::new(make_test_config()).unwrap();
    let config = stt.get_viettel_config();

    assert_eq!(config.api_key, "test_token");
    assert_eq!(config.sample_rate, 16000);
    assert_eq!(config.channels, 1);
}

#[tokio::test]
async fn test_connect_success() {
    let mut stt = ViettelStt::new(make_test_config()).unwrap();
    let result = stt.connect().await;

    assert!(result.is_ok());
    assert!(stt.is_ready());
}

#[tokio::test]
async fn test_connect_empty_api_key() {
    let mut stt = ViettelStt::default();
    let result = stt.connect().await;

    assert!(result.is_err());
    match result {
        Err(STTError::AuthenticationFailed(_)) => {}
        _ => panic!("Expected AuthenticationFailed error"),
    }
}

#[tokio::test]
async fn test_disconnect() {
    let mut stt = ViettelStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    let result = stt.disconnect().await;
    assert!(result.is_ok());
    assert!(!stt.is_ready());
}

#[tokio::test]
async fn test_send_audio_empty() {
    let mut stt = ViettelStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    let result = stt.send_audio(Bytes::new()).await;
    assert!(result.is_ok());
    assert_eq!(stt.buffer_size().await, 0);
}

#[tokio::test]
async fn test_send_audio_buffers() {
    let mut stt = ViettelStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    let audio_data = Bytes::from(vec![0u8; 1000]);
    let result = stt.send_audio(audio_data).await;

    assert!(result.is_ok());
    assert_eq!(stt.buffer_size().await, 1000);
}

#[tokio::test]
async fn test_send_audio_multiple() {
    let mut stt = ViettelStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
    stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
    stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();

    assert_eq!(stt.buffer_size().await, 1500);
}

#[tokio::test]
async fn test_flush_empty_buffer() {
    let mut stt = ViettelStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    let result = stt.flush().await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[tokio::test]
async fn test_disconnect_clears_buffer() {
    let mut stt = ViettelStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    stt.send_audio(Bytes::from(vec![0u8; 1000])).await.unwrap();
    assert!(stt.buffer_size().await > 0);

    stt.disconnect().await.unwrap();
    assert_eq!(stt.buffer_size().await, 0);
}

#[tokio::test]
async fn test_on_result_callback() {
    let mut stt = ViettelStt::new(make_test_config()).unwrap();
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
    let mut stt = ViettelStt::new(make_test_config()).unwrap();
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
    let mut stt = ViettelStt::new(make_test_config()).unwrap();

    let new_config = STTConfig {
        provider: "viettel-ai".to_string(),
        api_key: "new_token".to_string(),
        sample_rate: 8000,
        channels: 1,
        ..Default::default()
    };

    let result = stt.update_config(new_config).await;
    assert!(result.is_ok());

    assert_eq!(stt.get_viettel_config().sample_rate, 8000);
    assert_eq!(stt.get_viettel_config().api_key, "new_token");
}

#[tokio::test]
async fn test_auto_connect_on_send_audio() {
    let mut stt = ViettelStt::new(make_test_config()).unwrap();
    assert!(!stt.is_ready());

    // Should auto-connect
    let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
    assert!(result.is_ok());
    assert!(stt.is_ready());
}

#[tokio::test]
async fn test_buffer_size() {
    let mut stt = ViettelStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    assert_eq!(stt.buffer_size().await, 0);

    stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
    assert_eq!(stt.buffer_size().await, 500);

    stt.send_audio(Bytes::from(vec![0u8; 300])).await.unwrap();
    assert_eq!(stt.buffer_size().await, 800);
}

#[tokio::test]
async fn test_connect_clears_previous_buffer() {
    let mut stt = ViettelStt::new(make_test_config()).unwrap();
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
