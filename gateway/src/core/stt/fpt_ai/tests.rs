//! Tests for FPT.AI STT provider.

use super::*;
use crate::core::stt::base::{BaseSTT, STTConfig, STTError, STTResult};
use bytes::Bytes;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn make_test_config() -> STTConfig {
    STTConfig {
        provider: "fpt-ai".to_string(),
        api_key: "test_api_key".to_string(),
        language: "vi".to_string(),
        sample_rate: 16000,
        channels: 1,
        ..Default::default()
    }
}

struct MockResultCallback {
    result_count: AtomicUsize,
    last_transcript: Arc<tokio::sync::RwLock<String>>,
}

impl MockResultCallback {
    fn new() -> Self {
        Self {
            result_count: AtomicUsize::new(0),
            last_transcript: Arc::new(tokio::sync::RwLock::new(String::new())),
        }
    }

    fn get_result_count(&self) -> usize {
        self.result_count.load(Ordering::SeqCst)
    }
}

#[test]
fn test_new_valid_config() {
    let config = make_test_config();
    let result = FptStt::new(config);
    assert!(result.is_ok());

    let stt = result.unwrap();
    assert!(!stt.is_ready());
}

#[test]
fn test_new_empty_api_key() {
    let config = STTConfig {
        provider: "fpt-ai".to_string(),
        api_key: String::new(),
        ..Default::default()
    };

    let result = FptStt::new(config);
    assert!(result.is_err());

    match result {
        Err(STTError::AuthenticationFailed(msg)) => {
            assert!(msg.contains("API key"));
        }
        _ => panic!("Expected AuthenticationFailed error"),
    }
}

// W1 keystone: FPT.AI exposes no advanced-feature surface, so `new_standard` is a uniform
// standardized entry point that carries the base transport config (api_key/sample_rate/channels)
// through to the provider-specific config unchanged — even when advanced features are requested.
#[test]
fn new_standard_carries_base_through() {
    use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
    let std = StandardSTTConfig {
        base: STTConfig {
            provider: "fpt_ai".into(),
            api_key: "test_key".into(),
            sample_rate: 8000,
            channels: 1,
            ..Default::default()
        },
        features: SttFeatures {
            diarization: Some(true),
            word_timestamps: Some(true),
            ..Default::default()
        },
        extras: ProviderExtras::default(),
        translation: None,
    };
    let stt = FptStt::new_standard(&std).expect("new_standard should succeed");
    let cfg = stt.get_fpt_config();
    assert_eq!(cfg.api_key, "test_key");
    assert_eq!(cfg.sample_rate, 8000);
    assert_eq!(cfg.channels, 1);

    // Missing key is rejected through the standardized path too.
    let bad = StandardSTTConfig::from_base(STTConfig {
        provider: "fpt_ai".into(),
        api_key: String::new(),
        ..Default::default()
    });
    assert!(FptStt::new_standard(&bad).is_err());
}

#[test]
fn test_provider_info() {
    let stt = FptStt::new(make_test_config()).unwrap();
    let info = stt.get_provider_info();

    assert!(info.contains("FPT.AI"));
    assert!(info.contains("Vietnamese"));
}

#[test]
fn test_default_state() {
    let stt = FptStt::default();
    assert!(!stt.is_ready());
}

#[test]
fn test_get_config() {
    let config = make_test_config();
    let stt = FptStt::new(config.clone()).unwrap();

    let retrieved_config = stt.get_config();
    assert!(retrieved_config.is_some());
    assert_eq!(retrieved_config.unwrap().api_key, config.api_key);
}

#[tokio::test]
async fn test_connect_success() {
    let mut stt = FptStt::new(make_test_config()).unwrap();
    let result = stt.connect().await;

    assert!(result.is_ok());
    assert!(stt.is_ready());
}

#[tokio::test]
async fn test_connect_empty_api_key() {
    let mut stt = FptStt::default();
    let result = stt.connect().await;

    assert!(result.is_err());
    match result {
        Err(STTError::AuthenticationFailed(_)) => {}
        _ => panic!("Expected AuthenticationFailed error"),
    }
}

#[tokio::test]
async fn test_disconnect() {
    let mut stt = FptStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    let result = stt.disconnect().await;
    assert!(result.is_ok());
    assert!(!stt.is_ready());
}

#[tokio::test]
async fn test_send_audio_empty() {
    let mut stt = FptStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    let result = stt.send_audio(Bytes::new()).await;
    assert!(result.is_ok());
    assert_eq!(stt.buffer_size().await, 0);
}

#[tokio::test]
async fn test_send_audio_buffers() {
    let mut stt = FptStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    let audio_data = Bytes::from(vec![0u8; 1000]);
    let result = stt.send_audio(audio_data).await;

    assert!(result.is_ok());
    assert_eq!(stt.buffer_size().await, 1000);
}

#[tokio::test]
async fn test_send_audio_multiple() {
    let mut stt = FptStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
    stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
    stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();

    assert_eq!(stt.buffer_size().await, 1500);
}

#[tokio::test]
async fn test_flush_empty_buffer() {
    let mut stt = FptStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    let result = stt.flush().await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[tokio::test]
async fn test_disconnect_clears_buffer() {
    let mut stt = FptStt::new(make_test_config()).unwrap();
    stt.connect().await.unwrap();

    stt.send_audio(Bytes::from(vec![0u8; 1000])).await.unwrap();
    assert!(stt.buffer_size().await > 0);

    stt.disconnect().await.unwrap();
    assert_eq!(stt.buffer_size().await, 0);
}

#[tokio::test]
async fn test_on_result_callback() {
    let mut stt = FptStt::new(make_test_config()).unwrap();
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
    let mut stt = FptStt::new(make_test_config()).unwrap();
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
    let mut stt = FptStt::new(make_test_config()).unwrap();

    let new_config = STTConfig {
        provider: "fpt-ai".to_string(),
        api_key: "new_api_key".to_string(),
        sample_rate: 8000,
        channels: 1,
        ..Default::default()
    };

    let result = stt.update_config(new_config).await;
    assert!(result.is_ok());

    // Verify config was updated via get_fpt_config
    assert_eq!(stt.get_fpt_config().sample_rate, 8000);
}

#[test]
fn test_fpt_config_from_base() {
    let config = make_test_config();
    let fpt_config = config::FptSttConfig::from_base(&config).unwrap();

    assert_eq!(fpt_config.api_key, "test_api_key");
    assert_eq!(fpt_config.sample_rate, 16000);
    assert_eq!(fpt_config.channels, 1);
}

#[test]
fn test_fpt_config_validation() {
    let mut config = config::FptSttConfig::default();
    config.api_key = "test".to_string();

    assert!(config.validate().is_ok());

    config.sample_rate = 44100;
    assert!(config.validate().is_err());
}

#[test]
fn test_response_parsing() {
    let response = config::FptSttResponse {
        status: 0,
        hypotheses: vec![config::FptSttHypothesis {
            utterance: "Hello world".to_string(),
        }],
        id: "test-123".to_string(),
    };

    assert!(response.is_success());
    assert_eq!(response.transcription(), Some("Hello world"));
}

#[test]
fn test_response_no_voice() {
    let response = config::FptSttResponse {
        status: 1,
        hypotheses: vec![],
        id: "test-123".to_string(),
    };

    assert!(!response.is_success());
    assert_eq!(response.status_message(), "No voice detected");
}

#[tokio::test]
async fn test_auto_connect_on_send_audio() {
    let mut stt = FptStt::new(make_test_config()).unwrap();
    assert!(!stt.is_ready());

    // Should auto-connect
    let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
    assert!(result.is_ok());
    assert!(stt.is_ready());
}
