//! IBM Watson STT tests.
//!
//! This module contains unit tests for the IBM Watson Speech-to-Text
//! provider implementation.

use super::*;
use crate::core::stt::base::{BaseSTT, STTConfig, STTError, STTResultCallback};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// =============================================================================
// Configuration Tests
// =============================================================================

#[test]
fn test_ibm_watson_stt_creation() {
    let config = STTConfig {
        provider: "ibm-watson".to_string(),
        api_key: "test_api_key".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: "default".to_string(),
    };

    let stt = <IbmWatsonSTT as BaseSTT>::new(config).unwrap();
    assert!(!stt.is_ready());
    assert_eq!(stt.get_provider_info(), "IBM Watson Speech-to-Text");
}

#[test]
fn test_ibm_watson_stt_empty_api_key_error() {
    let config = STTConfig {
        provider: "ibm-watson".to_string(),
        api_key: String::new(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: "default".to_string(),
    };

    let result = <IbmWatsonSTT as BaseSTT>::new(config);
    assert!(result.is_err());
    if let Err(STTError::AuthenticationFailed(msg)) = result {
        assert!(msg.contains("API key"));
    } else {
        panic!("Expected AuthenticationFailed error");
    }
}

#[test]
fn test_ibm_watson_stt_config_access() {
    let config = STTConfig {
        provider: "ibm-watson".to_string(),
        api_key: "test_key".to_string(),
        language: "de-DE".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: false,
        encoding: "linear16".to_string(),
        model: "default".to_string(),
    };

    let stt = <IbmWatsonSTT as BaseSTT>::new(config).unwrap();

    let stored_config = stt.get_config().unwrap();
    assert_eq!(stored_config.api_key, "test_key");
    assert_eq!(stored_config.language, "de-DE");
    assert_eq!(stored_config.sample_rate, 16000);
}

#[test]
fn test_ibm_watson_stt_ibm_config_access() {
    let config = STTConfig {
        provider: "ibm-watson".to_string(),
        api_key: "test_key".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        ..Default::default()
    };

    let stt = <IbmWatsonSTT as BaseSTT>::new(config).unwrap();

    let ibm_config = stt.get_ibm_config().unwrap();
    // Default region should be UsSouth
    assert_eq!(ibm_config.region, config::IbmRegion::UsSouth);
    // Default model for en-US should be EnUsMultimedia
    assert_eq!(ibm_config.model, config::IbmModel::EnUsMultimedia);
    // Default interim_results should be true
    assert!(ibm_config.interim_results);
}

#[test]
fn test_ibm_watson_stt_set_instance_id() {
    let config = STTConfig {
        provider: "ibm-watson".to_string(),
        api_key: "test_key".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        ..Default::default()
    };

    let mut stt = <IbmWatsonSTT as BaseSTT>::new(config).unwrap();

    // Instance ID should be empty initially
    assert!(stt.get_ibm_config().unwrap().instance_id.is_empty());

    // Set instance ID
    stt.set_instance_id("test-instance-123".to_string());

    assert_eq!(
        stt.get_ibm_config().unwrap().instance_id,
        "test-instance-123"
    );
}

#[test]
fn test_ibm_watson_stt_set_region() {
    let config = STTConfig {
        provider: "ibm-watson".to_string(),
        api_key: "test_key".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        ..Default::default()
    };

    let mut stt = <IbmWatsonSTT as BaseSTT>::new(config).unwrap();

    // Default region
    assert_eq!(stt.get_ibm_config().unwrap().region, config::IbmRegion::UsSouth);

    // Set to EU region
    stt.set_region(config::IbmRegion::EuDe);

    assert_eq!(stt.get_ibm_config().unwrap().region, config::IbmRegion::EuDe);
}

#[test]
fn test_default_state() {
    let stt = IbmWatsonSTT::default();
    assert!(!stt.is_ready());
    // Cannot access config directly as it's private; verify via get_config
    assert!(stt.get_config().is_none());
}

// =============================================================================
// Connection Tests
// =============================================================================

#[tokio::test]
async fn test_send_audio_not_connected_error() {
    let config = STTConfig {
        provider: "ibm-watson".to_string(),
        api_key: "test_key".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        ..Default::default()
    };

    let mut stt = <IbmWatsonSTT as BaseSTT>::new(config).unwrap();
    stt.set_instance_id("test-instance".to_string());

    let result = stt.send_audio(vec![0u8; 1024].into()).await;
    assert!(result.is_err());
    if let Err(STTError::ConnectionFailed(msg)) = result {
        assert!(msg.contains("Not connected"));
    } else {
        panic!("Expected ConnectionFailed error");
    }
}

#[tokio::test]
async fn test_connect_missing_instance_id() {
    let config = STTConfig {
        provider: "ibm-watson".to_string(),
        api_key: "test_key".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        ..Default::default()
    };

    let mut stt = <IbmWatsonSTT as BaseSTT>::new(config).unwrap();
    // Don't set instance_id

    let result = stt.connect().await;
    assert!(result.is_err());
    if let Err(STTError::ConfigurationError(msg)) = result {
        assert!(msg.contains("instance_id"));
    } else {
        panic!("Expected ConfigurationError about instance_id");
    }
}

// =============================================================================
// Callback Tests
// =============================================================================

#[tokio::test]
async fn test_callback_registration() {
    let config = STTConfig {
        provider: "ibm-watson".to_string(),
        api_key: "test_key".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        ..Default::default()
    };

    let mut stt = <IbmWatsonSTT as BaseSTT>::new(config).unwrap();

    let callback_registered = Arc::new(AtomicBool::new(false));
    let callback_flag = callback_registered.clone();

    let callback: STTResultCallback = Arc::new(move |_result| {
        callback_flag.store(true, Ordering::SeqCst);
        Box::pin(async {})
    });

    // Should succeed without error
    let result = stt.on_result(callback).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_error_callback_registration() {
    let config = STTConfig {
        provider: "ibm-watson".to_string(),
        api_key: "test_key".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        ..Default::default()
    };

    let mut stt = <IbmWatsonSTT as BaseSTT>::new(config).unwrap();

    let callback: crate::core::stt::base::STTErrorCallback =
        Arc::new(move |_error| Box::pin(async {}));

    // Should succeed without error
    let result = stt.on_error(callback).await;
    assert!(result.is_ok());
}

// =============================================================================
// Configuration Model Tests
// =============================================================================

#[test]
fn test_model_selection_by_language() {
    // English (US) -> EnUsMultimedia
    let config = STTConfig {
        language: "en-US".to_string(),
        ..Default::default()
    };
    let ibm_config = config::IbmWatsonSTTConfig::from_base(config, "test".to_string());
    assert_eq!(ibm_config.model, config::IbmModel::EnUsMultimedia);

    // German -> DeDeMultimedia
    let config = STTConfig {
        language: "de-DE".to_string(),
        ..Default::default()
    };
    let ibm_config = config::IbmWatsonSTTConfig::from_base(config, "test".to_string());
    assert_eq!(ibm_config.model, config::IbmModel::DeDeMultimedia);

    // Japanese -> JaJpMultimedia
    let config = STTConfig {
        language: "ja-JP".to_string(),
        ..Default::default()
    };
    let ibm_config = config::IbmWatsonSTTConfig::from_base(config, "test".to_string());
    assert_eq!(ibm_config.model, config::IbmModel::JaJpMultimedia);

    // Unknown language falls back to en-US
    let config = STTConfig {
        language: "xx-XX".to_string(),
        ..Default::default()
    };
    let ibm_config = config::IbmWatsonSTTConfig::from_base(config, "test".to_string());
    assert_eq!(ibm_config.model, config::IbmModel::EnUsMultimedia);
}

#[test]
fn test_encoding_selection() {
    // Linear16
    let config = STTConfig {
        encoding: "linear16".to_string(),
        ..Default::default()
    };
    let ibm_config = config::IbmWatsonSTTConfig::from_base(config, "test".to_string());
    assert_eq!(ibm_config.encoding, config::IbmAudioEncoding::Linear16);

    // Mulaw
    let config = STTConfig {
        encoding: "mulaw".to_string(),
        ..Default::default()
    };
    let ibm_config = config::IbmWatsonSTTConfig::from_base(config, "test".to_string());
    assert_eq!(ibm_config.encoding, config::IbmAudioEncoding::Mulaw);

    // FLAC
    let config = STTConfig {
        encoding: "flac".to_string(),
        ..Default::default()
    };
    let ibm_config = config::IbmWatsonSTTConfig::from_base(config, "test".to_string());
    assert_eq!(ibm_config.encoding, config::IbmAudioEncoding::Flac);
}

// =============================================================================
// Message Parsing Tests
// =============================================================================

#[test]
fn test_parse_listening_state() {
    let json = r#"{"state": "listening"}"#;
    let msg = messages::IbmWatsonMessage::parse(json).unwrap();
    assert!(msg.is_listening());
}

#[test]
fn test_parse_final_result() {
    let json = r#"{
        "results": [
            {
                "alternatives": [
                    {
                        "transcript": "hello world",
                        "confidence": 0.95
                    }
                ],
                "final": true
            }
        ],
        "result_index": 0
    }"#;

    let msg = messages::IbmWatsonMessage::parse(json).unwrap();
    assert!(msg.is_results());

    if let messages::IbmWatsonMessage::Results(results) = msg {
        let stt_result = results.to_stt_result().unwrap();
        assert_eq!(stt_result.transcript, "hello world");
        assert!(stt_result.is_final);
        assert!((stt_result.confidence - 0.95).abs() < 0.001);
    }
}

#[test]
fn test_parse_interim_result() {
    let json = r#"{
        "results": [
            {
                "alternatives": [
                    {
                        "transcript": "hello"
                    }
                ],
                "final": false
            }
        ],
        "result_index": 0
    }"#;

    let msg = messages::IbmWatsonMessage::parse(json).unwrap();
    if let messages::IbmWatsonMessage::Results(results) = msg {
        let stt_result = results.to_stt_result().unwrap();
        assert!(!stt_result.is_final);
    }
}

#[test]
fn test_parse_error_message() {
    let json = r#"{
        "error": "session timed out",
        "code": 408
    }"#;

    let msg = messages::IbmWatsonMessage::parse(json).unwrap();
    assert!(msg.is_error());

    if let messages::IbmWatsonMessage::Error(error) = msg {
        assert!(error.is_critical());
        assert!(error.is_inactivity_timeout());
    }
}

#[test]
fn test_parse_multiple_results() {
    let json = r#"{
        "results": [
            {
                "alternatives": [
                    {"transcript": "first result", "confidence": 0.9}
                ],
                "final": true
            },
            {
                "alternatives": [
                    {"transcript": "second result", "confidence": 0.85}
                ],
                "final": true
            }
        ],
        "result_index": 0
    }"#;

    let msg = messages::IbmWatsonMessage::parse(json).unwrap();
    if let messages::IbmWatsonMessage::Results(results) = msg {
        let all_results = results.all_transcripts();
        assert_eq!(all_results.len(), 2);
        assert_eq!(all_results[0].transcript, "first result");
        assert_eq!(all_results[1].transcript, "second result");
    }
}

// =============================================================================
// WebSocket URL Building Tests
// =============================================================================

#[test]
fn test_websocket_url_building() {
    let config = config::IbmWatsonSTTConfig {
        region: config::IbmRegion::UsSouth,
        instance_id: "test-instance-123".to_string(),
        model: config::IbmModel::EnUsMultimedia,
        ..Default::default()
    };

    let url = config.build_websocket_url("test-token-xyz");

    assert!(url.starts_with("wss://api.us-south.speech-to-text.watson.cloud.ibm.com"));
    assert!(url.contains("instances/test-instance-123"));
    assert!(url.contains("access_token=test-token-xyz"));
    assert!(url.contains("model=en-US_Multimedia"));
}

#[test]
fn test_websocket_url_different_regions() {
    let base_config = config::IbmWatsonSTTConfig {
        instance_id: "test-instance".to_string(),
        model: config::IbmModel::EnUsMultimedia,
        ..Default::default()
    };

    // EU Germany
    let mut config = base_config.clone();
    config.region = config::IbmRegion::EuDe;
    let url = config.build_websocket_url("token");
    assert!(url.contains("api.eu-de.speech-to-text.watson.cloud.ibm.com"));

    // Japan
    let mut config = base_config.clone();
    config.region = config::IbmRegion::JpTok;
    let url = config.build_websocket_url("token");
    assert!(url.contains("api.jp-tok.speech-to-text.watson.cloud.ibm.com"));

    // Australia
    let mut config = base_config;
    config.region = config::IbmRegion::AuSyd;
    let url = config.build_websocket_url("token");
    assert!(url.contains("api.au-syd.speech-to-text.watson.cloud.ibm.com"));
}

// =============================================================================
// Start Message Building Tests
// =============================================================================

#[test]
fn test_start_message_building() {
    let config = config::IbmWatsonSTTConfig {
        base: STTConfig {
            sample_rate: 16000,
            ..Default::default()
        },
        encoding: config::IbmAudioEncoding::Linear16,
        interim_results: true,
        word_timestamps: true,
        smart_formatting: true,
        speaker_labels: true,
        profanity_filter: true,
        inactivity_timeout: 60,
        ..Default::default()
    };

    let msg = config.build_start_message();

    assert_eq!(msg["action"], "start");
    assert_eq!(msg["interim_results"], true);
    assert_eq!(msg["timestamps"], true);
    assert_eq!(msg["smart_formatting"], true);
    assert_eq!(msg["speaker_labels"], true);
    assert_eq!(msg["profanity_filter"], true);
    assert_eq!(msg["inactivity_timeout"], 60);
    assert!(msg["content-type"]
        .as_str()
        .unwrap()
        .contains("audio/l16"));
}

// =============================================================================
// Feature Flag Tests
// =============================================================================

#[test]
fn test_config_with_all_features_enabled() {
    let config = config::IbmWatsonSTTConfig {
        base: STTConfig {
            sample_rate: 16000,
            ..Default::default()
        },
        interim_results: true,
        word_timestamps: true,
        word_confidence: true,
        speaker_labels: true,
        smart_formatting: true,
        profanity_filter: true,
        redaction: true,
        low_latency: true,
        split_transcript_at_phrase_end: true,
        background_audio_suppression: Some(0.5),
        speech_detector_sensitivity: Some(0.6),
        end_of_phrase_silence_time: Some(2.0),
        character_insertion_bias: Some(-0.1),
        ..Default::default()
    };

    let msg = config.build_start_message();

    // All flags should be set
    assert_eq!(msg["interim_results"], true);
    assert_eq!(msg["timestamps"], true);
    assert_eq!(msg["word_confidence"], true);
    assert_eq!(msg["speaker_labels"], true);
    assert_eq!(msg["smart_formatting"], true);
    assert_eq!(msg["profanity_filter"], true);
    assert_eq!(msg["redaction"], true);
    assert_eq!(msg["low_latency"], true);
    assert_eq!(msg["split_transcript_at_phrase_end"], true);

    // Optional parameters
    assert!((msg["background_audio_suppression"].as_f64().unwrap() - 0.5).abs() < 0.001);
    assert!((msg["speech_detector_sensitivity"].as_f64().unwrap() - 0.6).abs() < 0.001);
    assert!((msg["end_of_phrase_silence_time"].as_f64().unwrap() - 2.0).abs() < 0.001);
    assert!((msg["character_insertion_bias"].as_f64().unwrap() - (-0.1)).abs() < 0.001);
}

// =============================================================================
// Model Recommended Sample Rate Tests
// =============================================================================

#[test]
fn test_model_recommended_sample_rates() {
    // Multimedia models should recommend 16kHz
    assert_eq!(
        config::IbmModel::EnUsMultimedia.recommended_sample_rate(),
        16000
    );
    assert_eq!(
        config::IbmModel::DeDeMultimedia.recommended_sample_rate(),
        16000
    );
    assert_eq!(
        config::IbmModel::JaJpMultimedia.recommended_sample_rate(),
        16000
    );

    // Telephony models should recommend 8kHz
    assert_eq!(
        config::IbmModel::EnUsTelephony.recommended_sample_rate(),
        8000
    );
    assert_eq!(
        config::IbmModel::DeDeTelephony.recommended_sample_rate(),
        8000
    );
    assert_eq!(
        config::IbmModel::JaJpTelephony.recommended_sample_rate(),
        8000
    );
}

// =============================================================================
// Audio Content Type Tests
// =============================================================================

#[test]
fn test_audio_content_types() {
    assert_eq!(
        config::IbmAudioEncoding::Linear16.content_type(16000),
        "audio/l16;rate=16000;channels=1"
    );
    assert_eq!(
        config::IbmAudioEncoding::Mulaw.content_type(8000),
        "audio/mulaw;rate=8000"
    );
    assert_eq!(
        config::IbmAudioEncoding::Alaw.content_type(8000),
        "audio/alaw;rate=8000"
    );
    assert_eq!(
        config::IbmAudioEncoding::Flac.content_type(16000),
        "audio/flac"
    );
    assert_eq!(
        config::IbmAudioEncoding::OggOpus.content_type(16000),
        "audio/ogg;codecs=opus"
    );
    assert_eq!(
        config::IbmAudioEncoding::WebmOpus.content_type(16000),
        "audio/webm;codecs=opus"
    );
    assert_eq!(
        config::IbmAudioEncoding::Mp3.content_type(16000),
        "audio/mp3"
    );
}

// =============================================================================
// Region Tests
// =============================================================================

#[test]
fn test_all_regions() {
    let regions = [
        (config::IbmRegion::UsSouth, "us-south"),
        (config::IbmRegion::UsEast, "us-east"),
        (config::IbmRegion::EuDe, "eu-de"),
        (config::IbmRegion::EuGb, "eu-gb"),
        (config::IbmRegion::AuSyd, "au-syd"),
        (config::IbmRegion::JpTok, "jp-tok"),
        (config::IbmRegion::KrSeo, "kr-seo"),
    ];

    for (region, expected_str) in regions {
        assert_eq!(region.as_str(), expected_str);
        assert!(!region.stt_hostname().is_empty());
        assert!(!region.tts_hostname().is_empty());
        assert!(region.stt_hostname().contains("speech-to-text"));
        assert!(region.tts_hostname().contains("text-to-speech"));
    }
}

// =============================================================================
// Disconnect Tests
// =============================================================================

#[tokio::test]
async fn test_disconnect_not_connected() {
    let config = STTConfig {
        provider: "ibm-watson".to_string(),
        api_key: "test_key".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        ..Default::default()
    };

    let mut stt = <IbmWatsonSTT as BaseSTT>::new(config).unwrap();

    // Disconnect should succeed even when not connected
    let result = stt.disconnect().await;
    assert!(result.is_ok());
}

// =============================================================================
// Custom Model Tests
// =============================================================================

#[test]
fn test_custom_model() {
    let model = config::IbmModel::Custom("my-custom-model-v2".to_string());
    assert_eq!(model.as_str(), "my-custom-model-v2");
    // Custom models default to 16kHz
    assert_eq!(model.recommended_sample_rate(), 16000);
}
