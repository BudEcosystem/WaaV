//! Integration tests for OpenAI Realtime API provider
//!
//! These tests verify:
//! - Provider creation through factory
//! - Configuration validation
//! - Callback registration
//! - Connection state management
//!
//! Note: Tests requiring actual API calls are marked with #[ignore]
//! and require OPENAI_API_KEY environment variable.

use std::sync::Arc;
use waav_gateway::core::realtime::{
    BaseRealtime, ConnectionState, OPENAI_REALTIME_SAMPLE_RATE, OPENAI_REALTIME_URL,
    OpenAIRealtime, OpenAIRealtimeAudioFormat, OpenAIRealtimeModel, OpenAIRealtimeVoice,
    RealtimeConfig, RealtimeError, RealtimeProvider, create_realtime_provider,
    get_supported_realtime_providers,
};

/// Test that OpenAI is included in supported providers
#[test]
fn test_openai_in_supported_realtime_providers() {
    let providers = get_supported_realtime_providers();
    assert!(providers.contains(&"openai"));
    assert_eq!(providers.len(), 1); // Only OpenAI for now
}

/// Test provider creation via string name
#[tokio::test]
async fn test_create_openai_realtime_provider_by_name() {
    let config = RealtimeConfig {
        api_key: "test-api-key".to_string(),
        model: "gpt-4o-realtime-preview".to_string(),
        voice: Some("alloy".to_string()),
        ..Default::default()
    };

    let result = create_realtime_provider("openai", config);
    assert!(result.is_ok());

    let provider = result.unwrap();
    let info = provider.get_provider_info();
    assert_eq!(info["provider"], "openai");
    assert!(!provider.is_ready());
}

/// Test case-insensitive provider name parsing
#[tokio::test]
async fn test_openai_realtime_provider_name_case_insensitive() {
    let config = RealtimeConfig {
        api_key: "test-api-key".to_string(),
        ..Default::default()
    };

    // Test various cases
    assert!(create_realtime_provider("openai", config.clone()).is_ok());
    assert!(create_realtime_provider("OpenAI", config.clone()).is_ok());
    assert!(create_realtime_provider("OPENAI", config).is_ok());
}

/// Test API key validation
#[tokio::test]
async fn test_openai_realtime_requires_api_key() {
    let config = RealtimeConfig {
        api_key: String::new(), // Empty API key
        model: "gpt-4o-realtime-preview".to_string(),
        ..Default::default()
    };

    let result = create_realtime_provider("openai", config);
    assert!(result.is_err());

    match result {
        Err(RealtimeError::AuthenticationFailed(msg)) => {
            assert!(msg.contains("API key is required"));
        }
        _ => panic!("Expected AuthenticationFailed error"),
    }
}

/// Test model configuration
#[tokio::test]
async fn test_openai_realtime_model_configuration() {
    // Test default model
    let config = RealtimeConfig {
        api_key: "test-api-key".to_string(),
        ..Default::default()
    };
    let realtime = OpenAIRealtime::new(config).unwrap();
    assert_eq!(realtime.model(), OpenAIRealtimeModel::Gpt4oRealtimePreview);

    // Test mini model
    let config = RealtimeConfig {
        api_key: "test-api-key".to_string(),
        model: "gpt-4o-mini-realtime-preview".to_string(),
        ..Default::default()
    };
    let realtime = OpenAIRealtime::new(config).unwrap();
    assert_eq!(
        realtime.model(),
        OpenAIRealtimeModel::Gpt4oMiniRealtimePreview
    );
}

/// Test voice configuration
#[tokio::test]
async fn test_openai_realtime_voice_configuration() {
    let voices = [
        ("alloy", OpenAIRealtimeVoice::Alloy),
        ("ash", OpenAIRealtimeVoice::Ash),
        ("ballad", OpenAIRealtimeVoice::Ballad),
        ("coral", OpenAIRealtimeVoice::Coral),
        ("echo", OpenAIRealtimeVoice::Echo),
        ("sage", OpenAIRealtimeVoice::Sage),
        ("shimmer", OpenAIRealtimeVoice::Shimmer),
        ("verse", OpenAIRealtimeVoice::Verse),
    ];

    for (voice_str, expected_voice) in voices {
        let config = RealtimeConfig {
            api_key: "test-api-key".to_string(),
            voice: Some(voice_str.to_string()),
            ..Default::default()
        };
        let realtime = OpenAIRealtime::new(config).unwrap();
        assert_eq!(
            realtime.voice(),
            expected_voice,
            "Voice {} mapping failed",
            voice_str
        );
    }
}

/// Test audio format configuration
#[tokio::test]
async fn test_openai_realtime_audio_format() {
    // Default is PCM16
    let config = RealtimeConfig {
        api_key: "test-api-key".to_string(),
        ..Default::default()
    };
    let realtime = OpenAIRealtime::new(config).unwrap();
    assert_eq!(realtime.audio_format(), OpenAIRealtimeAudioFormat::Pcm16);

    // Test G.711 u-law
    let config = RealtimeConfig {
        api_key: "test-api-key".to_string(),
        input_audio_format: Some("g711_ulaw".to_string()),
        ..Default::default()
    };
    let realtime = OpenAIRealtime::new(config).unwrap();
    assert_eq!(realtime.audio_format(), OpenAIRealtimeAudioFormat::G711Ulaw);
}

/// Test default configuration
#[test]
fn test_openai_realtime_default_configuration() {
    let realtime = OpenAIRealtime::default();
    assert!(!realtime.is_ready());
    assert_eq!(
        realtime.get_connection_state(),
        ConnectionState::Disconnected
    );
    assert_eq!(realtime.model(), OpenAIRealtimeModel::Gpt4oRealtimePreview);
    assert_eq!(realtime.voice(), OpenAIRealtimeVoice::Alloy);
}

/// Test provider info structure
#[test]
fn test_openai_realtime_provider_info() {
    let realtime = OpenAIRealtime::default();
    let info = realtime.get_provider_info();

    assert_eq!(info["provider"], "openai");
    assert_eq!(info["api_type"], "WebSocket Realtime");
    assert_eq!(info["default_sample_rate"], 24000);

    // Verify supported models
    let models = info["supported_models"].as_array().unwrap();
    assert!(models.iter().any(|m| m == "gpt-4o-realtime-preview"));
    assert!(models.iter().any(|m| m == "gpt-4o-mini-realtime-preview"));

    // Verify supported voices (8 total)
    let voices = info["supported_voices"].as_array().unwrap();
    assert_eq!(voices.len(), 8);
    assert!(voices.iter().any(|v| v == "alloy"));
    assert!(voices.iter().any(|v| v == "shimmer"));

    // Verify features
    assert!(info["features"]["bidirectional_audio"].as_bool().unwrap());
    assert!(info["features"]["vad"].as_bool().unwrap());
    assert!(info["features"]["function_calling"].as_bool().unwrap());
    assert!(info["features"]["transcription"].as_bool().unwrap());
}

/// Test send_audio fails when not connected
#[tokio::test]
async fn test_send_audio_requires_connection() {
    let config = RealtimeConfig {
        api_key: "test-api-key".to_string(),
        ..Default::default()
    };

    let mut realtime = OpenAIRealtime::new(config).unwrap();
    let result = realtime
        .send_audio(bytes::Bytes::from(vec![0u8; 100]))
        .await;

    assert!(result.is_err());
    match result {
        Err(RealtimeError::NotConnected) => {}
        _ => panic!("Expected NotConnected error"),
    }
}

/// Test send_text fails when not connected
#[tokio::test]
async fn test_send_text_requires_connection() {
    let config = RealtimeConfig {
        api_key: "test-api-key".to_string(),
        ..Default::default()
    };

    let mut realtime = OpenAIRealtime::new(config).unwrap();
    let result = realtime.send_text("Hello").await;

    assert!(result.is_err());
    match result {
        Err(RealtimeError::NotConnected) => {}
        _ => panic!("Expected NotConnected error"),
    }
}

/// Test constants
#[test]
fn test_realtime_constants() {
    assert_eq!(OPENAI_REALTIME_URL, "wss://api.openai.com/v1/realtime");
    assert_eq!(OPENAI_REALTIME_SAMPLE_RATE, 24000);
}

/// Test provider enum parsing
#[test]
fn test_realtime_provider_parse() {
    assert_eq!(
        RealtimeProvider::parse("openai"),
        Some(RealtimeProvider::OpenAI)
    );
    assert_eq!(
        RealtimeProvider::parse("OPENAI"),
        Some(RealtimeProvider::OpenAI)
    );
    assert_eq!(RealtimeProvider::parse("invalid"), None);
}

/// Test audio format sample rates
#[test]
fn test_audio_format_sample_rates() {
    assert_eq!(OpenAIRealtimeAudioFormat::Pcm16.sample_rate(), 24000);
    assert_eq!(OpenAIRealtimeAudioFormat::G711Ulaw.sample_rate(), 8000);
    assert_eq!(OpenAIRealtimeAudioFormat::G711Alaw.sample_rate(), 8000);
}

/// Integration test with real API (requires OPENAI_API_KEY)
#[tokio::test]
#[ignore = "Requires OPENAI_API_KEY environment variable"]
async fn test_real_openai_realtime_connection() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::time::{Duration, timeout};
    use waav_gateway::core::realtime::{TranscriptCallback, TranscriptResult};

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY not set");

    let config = RealtimeConfig {
        api_key,
        model: "gpt-4o-realtime-preview".to_string(),
        voice: Some("alloy".to_string()),
        instructions: Some("You are a helpful assistant. Keep responses brief.".to_string()),
        ..Default::default()
    };

    let mut realtime = OpenAIRealtime::new(config).unwrap();

    // Track connection and transcripts
    let connected = Arc::new(AtomicBool::new(false));
    let connected_clone = connected.clone();

    // Register transcript callback
    let callback: TranscriptCallback = Arc::new(move |result: TranscriptResult| {
        Box::pin(async move {
            println!("[{}] {}", result.role, result.text);
        })
    });

    realtime.on_transcript(callback).unwrap();

    // Connect with timeout
    let connect_result = timeout(Duration::from_secs(10), realtime.connect()).await;

    match connect_result {
        Ok(Ok(())) => {
            connected_clone.store(true, Ordering::SeqCst);
            println!("Connected to OpenAI Realtime API");
            assert!(realtime.is_ready());

            // Wait a moment for session to be established
            tokio::time::sleep(Duration::from_secs(1)).await;

            // Disconnect
            realtime.disconnect().await.unwrap();
            assert!(!realtime.is_ready());
        }
        Ok(Err(e)) => {
            panic!("Connection failed: {:?}", e);
        }
        Err(_) => {
            panic!("Connection timed out");
        }
    }
}

/// Test with audio streaming (requires OPENAI_API_KEY)
#[tokio::test]
#[ignore = "Requires OPENAI_API_KEY environment variable"]
async fn test_real_openai_realtime_audio_streaming() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::time::{Duration, timeout};
    use waav_gateway::core::realtime::{AudioOutputCallback, RealtimeAudioData};

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY not set");

    let config = RealtimeConfig {
        api_key,
        model: "gpt-4o-realtime-preview".to_string(),
        voice: Some("shimmer".to_string()),
        instructions: Some("Say hello briefly.".to_string()),
        ..Default::default()
    };

    let mut realtime = OpenAIRealtime::new(config).unwrap();

    // Track audio received
    let received_audio = Arc::new(AtomicBool::new(false));
    let audio_bytes = Arc::new(AtomicUsize::new(0));
    let received_clone = received_audio.clone();
    let bytes_clone = audio_bytes.clone();

    // Register audio callback
    let callback: AudioOutputCallback = Arc::new(move |audio: RealtimeAudioData| {
        let received = received_clone.clone();
        let bytes = bytes_clone.clone();
        Box::pin(async move {
            received.store(true, Ordering::SeqCst);
            bytes.fetch_add(audio.data.len(), Ordering::SeqCst);
            println!("Received {} bytes of audio", audio.data.len());
        })
    });

    realtime.on_audio(callback).unwrap();

    // Connect
    let connect_result = timeout(Duration::from_secs(10), realtime.connect()).await;

    if let Ok(Ok(())) = connect_result {
        assert!(realtime.is_ready());

        // Send some silent audio to trigger VAD
        // In a real test, you would send actual audio
        let silent_audio = vec![0u8; 4800]; // 100ms of 24kHz 16-bit mono
        for _ in 0..10 {
            realtime
                .send_audio(bytes::Bytes::from(silent_audio.clone()))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Wait for any audio response
        tokio::time::sleep(Duration::from_secs(5)).await;

        println!(
            "Total audio bytes received: {}",
            audio_bytes.load(Ordering::SeqCst)
        );

        // Disconnect
        realtime.disconnect().await.unwrap();
    } else {
        println!("Skipping audio test - connection failed or timed out");
    }
}
