//! Integration tests for OpenAI STT provider (Whisper)
//!
//! These tests verify:
//! - Provider creation through factory
//! - Configuration validation
//! - Callback registration
//! - Error handling
//!
//! Note: Tests requiring actual API calls are marked with #[ignore]
//! and require OPENAI_API_KEY environment variable.

use std::sync::Arc;
use waav_gateway::core::stt::{
    BaseSTT, OpenAIResponseFormat, OpenAISTT, OpenAISTTModel, STTConfig, STTError, STTProvider,
    create_stt_provider, create_stt_provider_from_enum, get_supported_stt_providers,
};

/// Test that OpenAI is included in supported providers
#[test]
fn test_openai_in_supported_providers() {
    let providers = get_supported_stt_providers();
    assert!(providers.contains(&"openai"));
    // deepgram, google, elevenlabs, microsoft-azure, cartesia, openai
    assert_eq!(providers.len(), 6);
}

/// Test provider creation via string name
#[test]
fn test_create_openai_provider_by_name() {
    let config = STTConfig {
        provider: "openai".to_string(),
        api_key: "test-api-key".to_string(),
        language: "en".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: "whisper-1".to_string(),
    };

    let result = create_stt_provider("openai", config);
    assert!(result.is_ok());

    let stt = result.unwrap();
    assert_eq!(stt.get_provider_info(), "OpenAI Whisper STT");
    // is_ready() is false until connect() is called
    assert!(!stt.is_ready());
}

/// Test provider creation via enum
#[test]
fn test_create_openai_provider_by_enum() {
    let config = STTConfig {
        provider: "openai".to_string(),
        api_key: "test-api-key".to_string(),
        language: "en".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: "whisper-1".to_string(),
    };

    let result = create_stt_provider_from_enum(STTProvider::OpenAI, config);
    assert!(result.is_ok());
}

/// Test case-insensitive provider name parsing
#[test]
fn test_openai_provider_name_case_insensitive() {
    let config = STTConfig {
        provider: "openai".to_string(),
        api_key: "test-api-key".to_string(),
        ..Default::default()
    };

    // Test various cases
    assert!(create_stt_provider("openai", config.clone()).is_ok());
    assert!(create_stt_provider("OpenAI", config.clone()).is_ok());
    assert!(create_stt_provider("OPENAI", config).is_ok());
}

/// Test API key validation
#[test]
fn test_openai_requires_api_key() {
    let config = STTConfig {
        provider: "openai".to_string(),
        api_key: String::new(), // Empty API key
        language: "en".to_string(),
        sample_rate: 16000,
        ..Default::default()
    };

    let result = create_stt_provider("openai", config);
    assert!(result.is_err());

    match result {
        Err(STTError::AuthenticationFailed(msg)) => {
            assert!(msg.contains("API key"));
        }
        Err(e) => panic!("Expected AuthenticationFailed, got: {}", e),
        Ok(_) => panic!("Expected error, got success"),
    }
}

/// Test model enum serialization
#[test]
fn test_openai_model_serialization() {
    assert_eq!(OpenAISTTModel::Whisper1.as_str(), "whisper-1");
    assert_eq!(
        OpenAISTTModel::Gpt4oTranscribe.as_str(),
        "gpt-4o-transcribe"
    );
    assert_eq!(
        OpenAISTTModel::Gpt4oMiniTranscribe.as_str(),
        "gpt-4o-mini-transcribe"
    );
}

/// Test response format enum serialization
#[test]
fn test_openai_response_format_serialization() {
    assert_eq!(OpenAIResponseFormat::Json.as_str(), "json");
    assert_eq!(OpenAIResponseFormat::Text.as_str(), "text");
    assert_eq!(OpenAIResponseFormat::Srt.as_str(), "srt");
    assert_eq!(OpenAIResponseFormat::VerboseJson.as_str(), "verbose_json");
    assert_eq!(OpenAIResponseFormat::Vtt.as_str(), "vtt");
}

/// Test model parsing from string using from_str_or_default
#[test]
fn test_openai_model_parsing() {
    // from_str_or_default always returns a valid model (defaults to Whisper1 for invalid)
    assert_eq!(
        OpenAISTTModel::from_str_or_default("whisper-1"),
        OpenAISTTModel::Whisper1
    );
    assert_eq!(
        OpenAISTTModel::from_str_or_default("whisper1"),
        OpenAISTTModel::Whisper1
    ); // Legacy
    assert_eq!(
        OpenAISTTModel::from_str_or_default("gpt-4o-transcribe"),
        OpenAISTTModel::Gpt4oTranscribe
    );
    assert_eq!(
        OpenAISTTModel::from_str_or_default("gpt-4o-mini-transcribe"),
        OpenAISTTModel::Gpt4oMiniTranscribe
    );
    // Invalid model defaults to Whisper1
    assert_eq!(
        OpenAISTTModel::from_str_or_default("invalid-model"),
        OpenAISTTModel::Whisper1
    );
}

/// Test callback registration using Arc callbacks
#[tokio::test]
async fn test_openai_callback_registration() {
    let config = STTConfig {
        provider: "openai".to_string(),
        api_key: "test-api-key".to_string(),
        ..Default::default()
    };

    let result = create_stt_provider("openai", config);
    assert!(result.is_ok());

    let mut stt = result.unwrap();

    // Register result callback with Arc (matching the trait signature)
    let result_callback = stt
        .on_result(Arc::new(|_result| Box::pin(async {})))
        .await;
    assert!(result_callback.is_ok());

    // Register error callback with Arc
    let error_callback = stt.on_error(Arc::new(|_error| Box::pin(async {}))).await;
    assert!(error_callback.is_ok());
}

/// Test default model selection
#[test]
fn test_openai_default_model() {
    let config = STTConfig {
        provider: "openai".to_string(),
        api_key: "test-api-key".to_string(),
        model: String::new(), // Empty model should default to whisper-1
        ..Default::default()
    };

    let result = create_stt_provider("openai", config);
    assert!(result.is_ok());
}

/// Test provider info
#[test]
fn test_openai_provider_info() {
    let config = STTConfig {
        provider: "openai".to_string(),
        api_key: "test-api-key".to_string(),
        ..Default::default()
    };

    let result = create_stt_provider("openai", config);
    assert!(result.is_ok());

    let stt = result.unwrap();
    assert_eq!(stt.get_provider_info(), "OpenAI Whisper STT");
}

/// Test OpenAI STT specific methods using with_config
#[test]
fn test_openai_stt_specific_creation() {
    use waav_gateway::core::stt::openai::{OpenAISTTConfig, ResponseFormat};

    let config = OpenAISTTConfig {
        base: STTConfig {
            provider: "openai".to_string(),
            api_key: "test-api-key".to_string(),
            model: "gpt-4o-transcribe".to_string(),
            language: "en".to_string(),
            ..Default::default()
        },
        model: OpenAISTTModel::Gpt4oTranscribe,
        response_format: ResponseFormat::VerboseJson,
        ..Default::default()
    };

    let stt = OpenAISTT::with_config(config);
    assert!(stt.is_ok());

    let stt = stt.unwrap();
    // is_ready() is false until connect() is called
    assert!(!stt.is_ready());
}

// =============================================================================
// Live API Integration Tests (require OPENAI_API_KEY)
// =============================================================================

/// Test actual transcription with OpenAI API
/// Run with: OPENAI_API_KEY=sk-... cargo test test_openai_live_transcription --ignored -- --nocapture
#[tokio::test]
#[ignore]
async fn test_openai_live_transcription() {
    use bytes::Bytes;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::mpsc;
    use waav_gateway::core::stt::openai::OpenAISTTConfig;

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");

    let config = OpenAISTTConfig::from_base(STTConfig {
        provider: "openai".to_string(),
        api_key,
        language: "en".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: "whisper-1".to_string(),
    });

    let mut stt = OpenAISTT::with_config(config).expect("Failed to create OpenAI STT");

    // Track results
    let result_received = Arc::new(AtomicBool::new(false));
    let result_received_clone = result_received.clone();

    let (result_tx, mut result_rx) = mpsc::unbounded_channel();

    // Register callback with proper async signature
    stt.on_result(Arc::new(move |result| {
        result_received_clone.store(true, Ordering::SeqCst);
        let tx = result_tx.clone();
        Box::pin(async move {
            let _ = tx.send(result);
        })
    }))
    .await
    .expect("Failed to register callback");

    // Connect (no-op for REST-based STT)
    stt.connect().await.expect("Failed to connect");

    // Generate some test audio (silence for this test)
    // In a real test, you'd use actual audio with speech
    let silence = vec![0u8; 16000 * 2]; // 1 second of silence at 16kHz mono 16-bit
    let audio_data = Bytes::from(silence);

    // Send audio
    stt.send_audio(audio_data)
        .await
        .expect("Failed to send audio");

    // Flush to get result (OpenAI-specific method)
    stt.flush().await.expect("Failed to flush");

    // Wait for result with timeout
    let timeout = tokio::time::Duration::from_secs(10);
    match tokio::time::timeout(timeout, result_rx.recv()).await {
        Ok(Some(result)) => {
            println!("Received result: {:?}", result);
            // For silence, we might get empty transcript or error
            // The important thing is we got a response
        }
        Ok(None) => {
            println!("Channel closed without result");
        }
        Err(_) => {
            // Timeout is acceptable for silence input
            println!("No result received (expected for silence)");
        }
    }

    // Disconnect
    stt.disconnect().await.expect("Failed to disconnect");
}

/// Test verbose JSON response format
/// Run with: OPENAI_API_KEY=sk-... cargo test test_openai_verbose_json_format --ignored -- --nocapture
#[tokio::test]
#[ignore]
async fn test_openai_verbose_json_format() {
    use bytes::Bytes;
    use tokio::sync::mpsc;
    use waav_gateway::core::stt::openai::OpenAISTTConfig;

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");

    let config = OpenAISTTConfig::from_base(STTConfig {
        provider: "openai".to_string(),
        api_key,
        language: "en".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: "whisper-1".to_string(),
    });

    let mut stt = OpenAISTT::with_config(config).expect("Failed to create OpenAI STT");

    let (result_tx, mut result_rx) = mpsc::unbounded_channel();

    stt.on_result(Arc::new(move |result| {
        let tx = result_tx.clone();
        Box::pin(async move {
            let _ = tx.send(result);
        })
    }))
    .await
    .expect("Failed to register callback");

    stt.connect().await.expect("Failed to connect");

    // Generate a simple audio pattern (not silence) to potentially get a result
    let mut audio: Vec<u8> = Vec::with_capacity(32000);
    for i in 0..16000 {
        // 1 second at 16kHz
        let sample = ((i as f32 * 0.01).sin() * 1000.0) as i16;
        audio.extend_from_slice(&sample.to_le_bytes());
    }
    let audio_data = Bytes::from(audio);

    stt.send_audio(audio_data)
        .await
        .expect("Failed to send audio");
    stt.flush().await.expect("Failed to flush");

    let timeout = tokio::time::Duration::from_secs(10);
    match tokio::time::timeout(timeout, result_rx.recv()).await {
        Ok(Some(result)) => {
            println!("Verbose result: {:?}", result);
        }
        Ok(None) => println!("Channel closed"),
        Err(_) => println!("Timeout (expected for generated audio)"),
    }

    stt.disconnect().await.expect("Failed to disconnect");
}
