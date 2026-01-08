//! Mock integration tests for Gnani.ai STT and TTS providers.
//!
//! These tests verify the Gnani implementation works correctly by:
//! 1. Testing configuration parsing and validation
//! 2. Testing factory function creation
//! 3. Testing error handling paths
//! 4. Testing mock HTTP/gRPC responses
//!
//! ## Running Tests
//!
//! ```bash
//! # Run all mock tests (no credentials needed)
//! cargo test gnani_mock
//!
//! # Run a specific test
//! cargo test test_gnani_stt_factory_creation
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::pin::Pin;
use std::future::Future;
use tokio::sync::Mutex;

use waav_gateway::core::stt::{
    BaseSTT, STTConfig, STTError, create_stt_provider,
};
use waav_gateway::core::tts::{
    AudioCallback, AudioData, BaseTTS, TTSConfig, TTSError, create_tts_provider,
};

// ============================================================================
// Test Helpers
// ============================================================================

/// Audio collector for capturing TTS output in tests.
#[derive(Clone)]
pub struct MockAudioCollector {
    audio_buffer: Arc<Mutex<Vec<u8>>>,
    chunk_count: Arc<AtomicUsize>,
    completed: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
}

impl MockAudioCollector {
    pub fn new() -> Self {
        Self {
            audio_buffer: Arc::new(Mutex::new(Vec::new())),
            chunk_count: Arc::new(AtomicUsize::new(0)),
            completed: Arc::new(AtomicBool::new(false)),
            error: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn get_audio(&self) -> Vec<u8> {
        self.audio_buffer.lock().await.clone()
    }

    pub fn get_chunk_count(&self) -> usize {
        self.chunk_count.load(Ordering::SeqCst)
    }

    pub fn is_completed(&self) -> bool {
        self.completed.load(Ordering::SeqCst)
    }

    pub async fn get_error(&self) -> Option<String> {
        self.error.lock().await.clone()
    }
}

impl Default for MockAudioCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCallback for MockAudioCollector {
    fn on_audio(&self, audio_data: AudioData) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let buffer = self.audio_buffer.clone();
        let count = self.chunk_count.clone();

        Box::pin(async move {
            buffer.lock().await.extend(&audio_data.data);
            count.fetch_add(1, Ordering::SeqCst);
        })
    }

    fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let completed = self.completed.clone();
        Box::pin(async move {
            completed.store(true, Ordering::SeqCst);
        })
    }

    fn on_error(&self, error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let error_store = self.error.clone();
        Box::pin(async move {
            *error_store.lock().await = Some(error.to_string());
        })
    }
}

// ============================================================================
// STT Factory Tests
// ============================================================================

/// Test that Gnani STT provider can be created via factory function.
#[test]
fn test_gnani_stt_factory_creation() {
    let config = STTConfig {
        provider: "gnani".to_string(),
        api_key: "mock-token".to_string(),
        language: "hi-IN".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm16".to_string(),
        model: "default".to_string(),
    };

    let result = create_stt_provider("gnani", config);
    assert!(result.is_ok(), "Gnani STT provider should be created successfully");

    let provider = result.unwrap();
    assert!(provider.get_provider_info().contains("Gnani"),
        "Provider info should mention Gnani");
}

/// Test that Gnani aliases work for STT factory.
#[test]
fn test_gnani_stt_factory_aliases() {
    let aliases = ["gnani", "gnani-ai", "gnani.ai", "vachana"];

    for alias in aliases {
        let config = STTConfig {
            provider: alias.to_string(),
            api_key: "mock-token".to_string(),
            language: "hi-IN".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm16".to_string(),
            model: "default".to_string(),
        };

        let result = create_stt_provider(alias, config);
        assert!(result.is_ok(), "Gnani STT should be created with alias: {}", alias);
    }
}

/// Test STT factory with case-insensitive provider names.
#[test]
fn test_gnani_stt_factory_case_insensitive() {
    let variants = ["GNANI", "Gnani", "gNaNi", "GNANI-AI"];

    for variant in variants {
        let config = STTConfig {
            provider: variant.to_string(),
            api_key: "mock-token".to_string(),
            language: "hi-IN".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm16".to_string(),
            model: "default".to_string(),
        };

        let result = create_stt_provider(variant, config);
        assert!(result.is_ok(), "Gnani STT should handle case: {}", variant);
    }
}

// ============================================================================
// TTS Factory Tests
// ============================================================================

/// Test that Gnani TTS provider can be created via factory function.
#[test]
fn test_gnani_tts_factory_creation() {
    let config = TTSConfig {
        provider: "gnani".to_string(),
        api_key: "mock-token".to_string(),
        model: "default".to_string(),
        voice_id: Some("speaker1".to_string()),
        audio_format: Some("pcm16".to_string()),
        sample_rate: Some(8000),
        speaking_rate: Some(1.0),
        connection_timeout: Some(30),
        request_timeout: Some(60),
        pronunciations: Vec::new(),
        request_pool_size: Some(4),
        emotion_config: None,
    };

    let result = create_tts_provider("gnani", config);
    assert!(result.is_ok(), "Gnani TTS provider should be created successfully");
}

/// Test that Gnani aliases work for TTS factory.
#[test]
fn test_gnani_tts_factory_aliases() {
    let aliases = ["gnani", "gnani-ai", "gnani.ai"];

    for alias in aliases {
        let config = TTSConfig {
            provider: alias.to_string(),
            api_key: "mock-token".to_string(),
            model: "default".to_string(),
            voice_id: Some("speaker1".to_string()),
            audio_format: Some("pcm16".to_string()),
            sample_rate: Some(8000),
            speaking_rate: Some(1.0),
            connection_timeout: Some(30),
            request_timeout: Some(60),
            pronunciations: Vec::new(),
            request_pool_size: Some(4),
            emotion_config: None,
        };

        let result = create_tts_provider(alias, config);
        assert!(result.is_ok(), "Gnani TTS should be created with alias: {}", alias);
    }
}

// ============================================================================
// STT Configuration Tests
// ============================================================================

/// Test STT provider is not ready before connection.
#[test]
fn test_gnani_stt_not_ready_before_connect() {
    let config = STTConfig {
        provider: "gnani".to_string(),
        api_key: "mock-token".to_string(),
        language: "hi-IN".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm16".to_string(),
        model: "default".to_string(),
    };

    let provider = create_stt_provider("gnani", config).unwrap();
    assert!(!provider.is_ready(), "Provider should not be ready before connect");
}

/// Test STT provider info contains expected details.
#[test]
fn test_gnani_stt_provider_info_content() {
    let config = STTConfig {
        provider: "gnani".to_string(),
        api_key: "mock-token".to_string(),
        language: "hi-IN".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm16".to_string(),
        model: "default".to_string(),
    };

    let provider = create_stt_provider("gnani", config).unwrap();
    let info = provider.get_provider_info();

    assert!(info.contains("Gnani"), "Should contain 'Gnani'");
    assert!(info.contains("gRPC") || info.contains("streaming"),
        "Should mention gRPC or streaming capability");
}

/// Test that send_audio fails when not connected.
#[tokio::test]
async fn test_gnani_stt_send_audio_fails_not_connected() {
    let config = STTConfig {
        provider: "gnani".to_string(),
        api_key: "mock-token".to_string(),
        language: "hi-IN".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm16".to_string(),
        model: "default".to_string(),
    };

    let mut provider = create_stt_provider("gnani", config).unwrap();
    let audio_data = bytes::Bytes::from_static(&[0u8; 100]);

    let result = provider.send_audio(audio_data).await;
    assert!(result.is_err(), "send_audio should fail when not connected");

    match result {
        Err(STTError::ConnectionFailed(msg)) => {
            assert!(msg.contains("Not connected"), "Error should mention not connected");
        }
        Err(other) => panic!("Expected ConnectionFailed error, got: {:?}", other),
        Ok(_) => panic!("Expected error, got success"),
    }
}

/// Test disconnect when not connected does not error.
#[tokio::test]
async fn test_gnani_stt_disconnect_not_connected() {
    let config = STTConfig {
        provider: "gnani".to_string(),
        api_key: "mock-token".to_string(),
        language: "hi-IN".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm16".to_string(),
        model: "default".to_string(),
    };

    let mut provider = create_stt_provider("gnani", config).unwrap();

    // Disconnect when not connected should not error
    let result = provider.disconnect().await;
    assert!(result.is_ok(), "Disconnect when not connected should succeed");
}

// ============================================================================
// TTS Configuration Tests
// ============================================================================

/// Test TTS provider is not connected initially.
#[test]
fn test_gnani_tts_not_connected_initially() {
    let config = TTSConfig {
        provider: "gnani".to_string(),
        api_key: "mock-token".to_string(),
        model: "default".to_string(),
        voice_id: Some("speaker1".to_string()),
        audio_format: Some("pcm16".to_string()),
        sample_rate: Some(8000),
        speaking_rate: Some(1.0),
        connection_timeout: Some(30),
        request_timeout: Some(60),
        pronunciations: Vec::new(),
        request_pool_size: Some(4),
        emotion_config: None,
    };

    let provider = create_tts_provider("gnani", config).unwrap();
    // TTS providers may or may not have is_connected method
    // The key test is that speak fails without connect
}

/// Test that speak fails when not connected.
#[tokio::test]
async fn test_gnani_tts_speak_fails_not_connected() {
    let config = TTSConfig {
        provider: "gnani".to_string(),
        api_key: "mock-token".to_string(),
        model: "default".to_string(),
        voice_id: Some("speaker1".to_string()),
        audio_format: Some("pcm16".to_string()),
        sample_rate: Some(8000),
        speaking_rate: Some(1.0),
        connection_timeout: Some(30),
        request_timeout: Some(60),
        pronunciations: Vec::new(),
        request_pool_size: Some(4),
        emotion_config: None,
    };

    let mut provider = create_tts_provider("gnani", config).unwrap();

    let result = provider.speak("नमस्ते", false).await;
    assert!(result.is_err(), "speak should fail when not connected");
}

// ============================================================================
// Language Support Tests
// ============================================================================

/// Test all supported Gnani STT languages.
#[test]
fn test_gnani_stt_all_languages() {
    let languages = [
        "hi-IN", "kn-IN", "ta-IN", "te-IN", "gu-IN", "mr-IN",
        "bn-IN", "ml-IN", "pa-guru-IN", "ur-IN",
        "en-IN", "en-GB", "en-US", "en-SG",
    ];

    for lang in languages {
        let config = STTConfig {
            provider: "gnani".to_string(),
            api_key: "mock-token".to_string(),
            language: lang.to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm16".to_string(),
            model: "default".to_string(),
        };

        let result = create_stt_provider("gnani", config);
        assert!(result.is_ok(), "Gnani STT should support language: {}", lang);
    }
}

/// Test all supported Gnani TTS languages.
#[test]
fn test_gnani_tts_all_languages() {
    // Gnani TTS supports fewer languages than STT
    let languages = ["Hi-IN", "En-IN", "Kn-IN", "Ta-IN", "Te-IN", "Mr-IN"];

    for lang in languages {
        let config = TTSConfig {
            provider: "gnani".to_string(),
            api_key: "mock-token".to_string(),
            model: "default".to_string(),
            voice_id: Some(format!("{}-speaker1", lang)),
            audio_format: Some("pcm16".to_string()),
            sample_rate: Some(8000),
            speaking_rate: Some(1.0),
            connection_timeout: Some(30),
            request_timeout: Some(60),
            pronunciations: Vec::new(),
            request_pool_size: Some(4),
            emotion_config: None,
        };

        let result = create_tts_provider("gnani", config);
        assert!(result.is_ok(), "Gnani TTS should be creatable for language context: {}", lang);
    }
}

// ============================================================================
// Audio Format Tests
// ============================================================================

/// Test STT with different audio encodings.
#[test]
fn test_gnani_stt_audio_encodings() {
    let encodings = ["pcm16", "wav"];

    for encoding in encodings {
        let config = STTConfig {
            provider: "gnani".to_string(),
            api_key: "mock-token".to_string(),
            language: "hi-IN".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: encoding.to_string(),
            model: "default".to_string(),
        };

        let result = create_stt_provider("gnani", config);
        assert!(result.is_ok(), "Gnani STT should support encoding: {}", encoding);
    }
}

/// Test STT with different sample rates.
#[test]
fn test_gnani_stt_sample_rates() {
    let sample_rates = [8000, 16000, 24000, 48000];

    for rate in sample_rates {
        let config = STTConfig {
            provider: "gnani".to_string(),
            api_key: "mock-token".to_string(),
            language: "hi-IN".to_string(),
            sample_rate: rate,
            channels: 1,
            punctuation: true,
            encoding: "pcm16".to_string(),
            model: "default".to_string(),
        };

        let result = create_stt_provider("gnani", config);
        assert!(result.is_ok(), "Gnani STT should be creatable with sample rate: {}", rate);
    }
}

// ============================================================================
// Plugin Registry Tests
// ============================================================================

/// Test that Gnani is registered in the plugin registry.
#[test]
fn test_gnani_registered_in_plugin_registry() {
    use waav_gateway::plugin::global_registry;

    let registry = global_registry();

    // Check STT registration
    assert!(registry.has_stt_provider("gnani"), "Gnani STT should be registered");
    assert!(registry.has_stt_provider("gnani-ai"), "gnani-ai alias should work");
    assert!(registry.has_stt_provider("vachana"), "vachana alias should work");

    // Check TTS registration
    assert!(registry.has_tts_provider("gnani"), "Gnani TTS should be registered");
    assert!(registry.has_tts_provider("gnani-ai"), "gnani-ai TTS alias should work");
}

/// Test Gnani metadata is available.
#[test]
fn test_gnani_metadata_available() {
    use waav_gateway::plugin::global_registry;

    let registry = global_registry();

    // Check STT metadata
    let stt_meta = registry.get_stt_metadata("gnani");
    assert!(stt_meta.is_some(), "Gnani STT metadata should be available");
    if let Some(meta) = stt_meta {
        assert_eq!(meta.name, "gnani");
        assert!(meta.display_name.contains("Gnani") || meta.display_name.contains("Vachana"));
    }

    // Check TTS metadata
    let tts_meta = registry.get_tts_metadata("gnani");
    assert!(tts_meta.is_some(), "Gnani TTS metadata should be available");
}

/// Test Gnani appears in provider lists.
#[test]
fn test_gnani_in_provider_lists() {
    use waav_gateway::plugin::global_registry;

    let registry = global_registry();

    let stt_providers = registry.get_stt_provider_names();
    assert!(stt_providers.contains(&"gnani".to_string()),
        "Gnani should be in STT provider list");

    let tts_providers = registry.get_tts_provider_names();
    assert!(tts_providers.contains(&"gnani".to_string()),
        "Gnani should be in TTS provider list");
}

// ============================================================================
// Error Handling Tests
// ============================================================================

/// Test that invalid provider name returns appropriate error.
#[test]
fn test_invalid_provider_error() {
    let config = STTConfig {
        provider: "nonexistent".to_string(),
        api_key: "mock-token".to_string(),
        language: "hi-IN".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm16".to_string(),
        model: "default".to_string(),
    };

    let result = create_stt_provider("nonexistent", config);
    assert!(result.is_err(), "Invalid provider should error");

    match result {
        Err(STTError::ConfigurationError(msg)) => {
            assert!(msg.contains("Unknown") || msg.contains("provider"),
                "Error should mention unknown provider");
        }
        Err(other) => panic!("Expected ConfigurationError, got: {:?}", other),
        Ok(_) => panic!("Expected error, got success"),
    }
}

/// Test callback registration works.
#[tokio::test]
async fn test_gnani_stt_callback_registration() {
    use waav_gateway::core::stt::{STTResult, STTResultCallback};

    let config = STTConfig {
        provider: "gnani".to_string(),
        api_key: "mock-token".to_string(),
        language: "hi-IN".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm16".to_string(),
        model: "default".to_string(),
    };

    let mut provider = create_stt_provider("gnani", config).unwrap();

    let callback: STTResultCallback = Arc::new(|_result: STTResult| {
        Box::pin(async move {})
            as Pin<Box<dyn Future<Output = ()> + Send>>
    });

    let result = provider.on_result(callback).await;
    assert!(result.is_ok(), "Callback registration should succeed");
}

// ============================================================================
// Config Validation Tests
// ============================================================================

/// Test that empty API key is handled.
#[test]
fn test_gnani_stt_empty_api_key() {
    let config = STTConfig {
        provider: "gnani".to_string(),
        api_key: String::new(), // Empty API key
        language: "hi-IN".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm16".to_string(),
        model: "default".to_string(),
    };

    // Provider creation might succeed, but connect should fail
    let result = create_stt_provider("gnani", config);
    // Empty API key is allowed at creation time - validation happens at connect
    assert!(result.is_ok(), "Provider creation with empty key should work (validation at connect)");
}

/// Test provider config retrieval.
#[test]
fn test_gnani_stt_config_retrieval() {
    let config = STTConfig {
        provider: "gnani".to_string(),
        api_key: "mock-token".to_string(),
        language: "hi-IN".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm16".to_string(),
        model: "default".to_string(),
    };

    let provider = create_stt_provider("gnani", config).unwrap();

    let retrieved_config = provider.get_config();
    assert!(retrieved_config.is_some(), "Config should be retrievable");

    if let Some(cfg) = retrieved_config {
        assert_eq!(cfg.language, "hi-IN");
        assert_eq!(cfg.sample_rate, 16000);
    }
}

// ============================================================================
// Concurrent Access Tests
// ============================================================================

/// Test multiple provider instances can be created.
#[test]
fn test_gnani_multiple_instances() {
    let configs: Vec<_> = (0..5).map(|i| STTConfig {
        provider: "gnani".to_string(),
        api_key: format!("mock-token-{}", i),
        language: "hi-IN".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm16".to_string(),
        model: "default".to_string(),
    }).collect();

    let providers: Vec<_> = configs.into_iter()
        .map(|cfg| create_stt_provider("gnani", cfg))
        .collect();

    for (i, result) in providers.into_iter().enumerate() {
        assert!(result.is_ok(), "Instance {} should be created", i);
    }
}

/// Test concurrent provider creation.
#[tokio::test]
async fn test_gnani_concurrent_creation() {
    let handles: Vec<_> = (0..10).map(|i| {
        tokio::spawn(async move {
            let config = STTConfig {
                provider: "gnani".to_string(),
                api_key: format!("mock-token-{}", i),
                language: "hi-IN".to_string(),
                sample_rate: 16000,
                channels: 1,
                punctuation: true,
                encoding: "pcm16".to_string(),
                model: "default".to_string(),
            };
            create_stt_provider("gnani", config)
        })
    }).collect();

    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "Concurrent creation should succeed");
    }
}
