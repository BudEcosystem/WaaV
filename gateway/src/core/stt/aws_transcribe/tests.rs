//! Tests for Amazon Transcribe Streaming STT provider.

use super::*;
use crate::core::stt::base::{BaseSTT, STTConfig};

// =============================================================================
// Configuration Tests
// =============================================================================

#[test]
fn test_aws_region_all_variants() {
    let regions = AwsRegion::all();
    assert!(regions.len() >= 15);
    assert!(regions.contains(&AwsRegion::UsEast1));
    assert!(regions.contains(&AwsRegion::EuWest1));
    assert!(regions.contains(&AwsRegion::ApNortheast1));
}

#[test]
fn test_aws_region_roundtrip() {
    for region in AwsRegion::all() {
        let s = region.as_str();
        let parsed = AwsRegion::from_str_or_default(s);
        assert_eq!(*region, parsed);
    }
}

#[test]
fn test_media_encoding_roundtrip() {
    let encodings = [
        MediaEncoding::Pcm,
        MediaEncoding::Flac,
        MediaEncoding::OggOpus,
    ];
    for encoding in encodings {
        let s = encoding.as_str();
        let parsed = MediaEncoding::from_str_or_default(s);
        assert_eq!(encoding, parsed);
    }
}

#[test]
fn test_partial_results_stability_roundtrip() {
    let stabilities = [
        PartialResultsStability::High,
        PartialResultsStability::Medium,
        PartialResultsStability::Low,
    ];
    for stability in stabilities {
        let s = stability.as_str();
        let parsed = PartialResultsStability::from_str_or_default(s);
        assert_eq!(stability, parsed);
    }
}

#[test]
fn test_config_with_language() {
    let config = AwsTranscribeSTTConfig::with_language("ja-JP");
    assert_eq!(config.base.language, "ja-JP");
    assert_eq!(config.region, AwsRegion::UsEast1);
    assert!(config.enable_partial_results_stabilization);
}

#[test]
fn test_config_validation_valid_config() {
    let config = AwsTranscribeSTTConfig::default();
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_validation_invalid_sample_rate_low() {
    let mut config = AwsTranscribeSTTConfig::default();
    config.base.sample_rate = 4000;
    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Sample rate"));
}

#[test]
fn test_config_validation_invalid_sample_rate_high() {
    let mut config = AwsTranscribeSTTConfig::default();
    config.base.sample_rate = 96000;
    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Sample rate"));
}

#[test]
fn test_config_validation_speaker_labels_missing_max() {
    let mut config = AwsTranscribeSTTConfig::default();
    config.show_speaker_label = true;
    // max_speaker_labels not set
    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("max_speaker_labels"));
}

#[test]
fn test_config_validation_speaker_labels_invalid_max() {
    let mut config = AwsTranscribeSTTConfig::default();
    config.show_speaker_label = true;
    config.max_speaker_labels = Some(1); // Too low (min is 2)
    let result = config.validate();
    assert!(result.is_err());
}

#[test]
fn test_config_validation_speaker_labels_valid() {
    let mut config = AwsTranscribeSTTConfig::default();
    config.show_speaker_label = true;
    config.max_speaker_labels = Some(5);
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_calculate_chunk_size_default() {
    let config = AwsTranscribeSTTConfig::default();
    // 100ms at 16kHz mono 16-bit = 3200 bytes
    assert_eq!(config.calculate_chunk_size(), 3200);
}

#[test]
fn test_config_calculate_chunk_size_48khz() {
    let mut config = AwsTranscribeSTTConfig::default();
    config.base.sample_rate = 48000;
    // 100ms at 48kHz mono 16-bit = 9600 bytes
    assert_eq!(config.calculate_chunk_size(), 9600);
}

#[test]
fn test_config_calculate_chunk_size_8khz() {
    let mut config = AwsTranscribeSTTConfig::default();
    config.base.sample_rate = 8000;
    // 100ms at 8kHz mono 16-bit = 1600 bytes
    assert_eq!(config.calculate_chunk_size(), 1600);
}

#[test]
fn test_config_calculate_chunk_size_stereo() {
    let mut config = AwsTranscribeSTTConfig::default();
    config.base.channels = 2;
    // 100ms at 16kHz stereo 16-bit = 6400 bytes
    assert_eq!(config.calculate_chunk_size(), 6400);
}

#[test]
fn test_config_has_explicit_credentials() {
    let mut config = AwsTranscribeSTTConfig::default();
    assert!(!config.has_explicit_credentials());

    config.aws_access_key_id = Some("AKIAIOSFODNN7EXAMPLE".to_string());
    assert!(!config.has_explicit_credentials()); // Still need secret

    config.aws_secret_access_key = Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string());
    assert!(config.has_explicit_credentials());
}

// =============================================================================
// Message Parsing Tests
// =============================================================================

#[test]
fn test_transcript_result_best_transcript() {
    let result = TranscribeResult {
        result_id: Some("123".to_string()),
        start_time: Some(0.0),
        end_time: Some(1.5),
        is_partial: Some(false),
        alternatives: Some(vec![Alternative {
            transcript: Some("Hello world".to_string()),
            items: None,
            entities: None,
        }]),
        channel_id: None,
        language_code: None,
        language_identification: None,
    };

    assert_eq!(result.best_transcript(), Some("Hello world"));
    assert!(result.is_final());
    assert_eq!(result.duration(), Some(1.5));
}

#[test]
fn test_transcript_result_partial() {
    let result = TranscribeResult {
        result_id: None,
        start_time: None,
        end_time: None,
        is_partial: Some(true),
        alternatives: None,
        channel_id: None,
        language_code: None,
        language_identification: None,
    };

    assert!(!result.is_final());
    assert!(result.best_transcript().is_none());
}

#[test]
fn test_transcript_result_confidence_calculation() {
    let result = TranscribeResult {
        result_id: None,
        start_time: None,
        end_time: None,
        is_partial: Some(false),
        alternatives: Some(vec![Alternative {
            transcript: Some("test".to_string()),
            items: Some(vec![Item {
                start_time: None,
                end_time: None,
                item_type: None,
                content: Some("test".to_string()),
                vocabulary_filter_match: None,
                confidence: Some(0.95),
                speaker: None,
                stable: None,
            }]),
            entities: None,
        }]),
        channel_id: None,
        language_code: None,
        language_identification: None,
    };

    let confidence = result.confidence();
    assert!((confidence - 0.95).abs() < 0.001);
}

#[test]
fn test_transcript_event_has_final_results() {
    let event = TranscriptEvent {
        transcript: Some(Transcript {
            results: Some(vec![TranscribeResult {
                result_id: None,
                start_time: None,
                end_time: None,
                is_partial: Some(false),
                alternatives: None,
                channel_id: None,
                language_code: None,
                language_identification: None,
            }]),
        }),
    };

    assert!(event.has_final_results());
}

#[test]
fn test_transcript_event_no_final_results() {
    let event = TranscriptEvent {
        transcript: Some(Transcript {
            results: Some(vec![TranscribeResult {
                result_id: None,
                start_time: None,
                end_time: None,
                is_partial: Some(true),
                alternatives: None,
                channel_id: None,
                language_code: None,
                language_identification: None,
            }]),
        }),
    };

    assert!(!event.has_final_results());
}

#[test]
fn test_stable_words_extraction() {
    let result = TranscribeResult {
        result_id: None,
        start_time: None,
        end_time: None,
        is_partial: Some(true),
        alternatives: Some(vec![Alternative {
            transcript: Some("hello world test".to_string()),
            items: Some(vec![
                Item {
                    start_time: None,
                    end_time: None,
                    item_type: None,
                    content: Some("hello".to_string()),
                    vocabulary_filter_match: None,
                    confidence: None,
                    speaker: None,
                    stable: Some(true),
                },
                Item {
                    start_time: None,
                    end_time: None,
                    item_type: None,
                    content: Some("world".to_string()),
                    vocabulary_filter_match: None,
                    confidence: None,
                    speaker: None,
                    stable: Some(true),
                },
                Item {
                    start_time: None,
                    end_time: None,
                    item_type: None,
                    content: Some("test".to_string()),
                    vocabulary_filter_match: None,
                    confidence: None,
                    speaker: None,
                    stable: Some(false),
                },
            ]),
            entities: None,
        }]),
        channel_id: None,
        language_code: None,
        language_identification: None,
    };

    let stable = result.stable_words();
    assert_eq!(stable, vec!["hello", "world"]);
}

#[test]
fn test_transcribe_error_display() {
    let error = TranscribeError {
        code: Some("BadRequestException".to_string()),
        message: Some("Invalid audio format".to_string()),
    };
    assert_eq!(
        error.to_string(),
        "BadRequestException: Invalid audio format"
    );

    let error_code_only = TranscribeError {
        code: Some("BadRequestException".to_string()),
        message: None,
    };
    assert_eq!(error_code_only.to_string(), "BadRequestException");

    let error_msg_only = TranscribeError {
        code: None,
        message: Some("Invalid audio format".to_string()),
    };
    assert_eq!(error_msg_only.to_string(), "Invalid audio format");

    let error_empty = TranscribeError {
        code: None,
        message: None,
    };
    assert_eq!(error_empty.to_string(), "Unknown error");
}

// =============================================================================
// Client Tests (Unit)
// =============================================================================

#[tokio::test]
async fn test_client_creation_valid_config() {
    let config = STTConfig {
        provider: "aws-transcribe".to_string(),
        api_key: String::new(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm".to_string(),
        model: String::new(),
    };

    let stt = AwsTranscribeSTT::new(config);
    assert!(stt.is_ok());
    let stt = stt.unwrap();
    assert!(!stt.is_ready());
    assert_eq!(stt.get_provider_info(), "Amazon Transcribe Streaming");
}

#[tokio::test]
async fn test_client_creation_invalid_sample_rate() {
    let config = STTConfig {
        provider: "aws-transcribe".to_string(),
        api_key: String::new(),
        language: "en-US".to_string(),
        sample_rate: 2000, // Too low
        channels: 1,
        punctuation: true,
        encoding: "pcm".to_string(),
        model: String::new(),
    };

    let result = AwsTranscribeSTT::new(config);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_send_audio_not_connected() {
    let config = STTConfig {
        provider: "aws-transcribe".to_string(),
        api_key: String::new(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm".to_string(),
        model: String::new(),
    };

    let mut stt = AwsTranscribeSTT::new(config).unwrap();
    let audio_data = bytes::Bytes::from(vec![0u8; 1024]);

    let result = stt.send_audio(audio_data).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_get_session_id_before_connect() {
    let config = STTConfig {
        provider: "aws-transcribe".to_string(),
        api_key: String::new(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm".to_string(),
        model: String::new(),
    };

    let stt = AwsTranscribeSTT::new(config).unwrap();
    assert!(stt.get_session_id().await.is_none());
}

#[tokio::test]
async fn test_client_with_custom_config() {
    let aws_config = AwsTranscribeSTTConfig {
        base: STTConfig {
            provider: "aws-transcribe".to_string(),
            api_key: String::new(),
            language: "ja-JP".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm".to_string(),
            model: String::new(),
        },
        region: AwsRegion::ApNortheast1,
        enable_partial_results_stabilization: true,
        partial_results_stability: PartialResultsStability::Medium,
        show_speaker_label: true,
        max_speaker_labels: Some(4),
        chunk_duration_ms: 150,
        ..Default::default()
    };

    let stt = AwsTranscribeSTT::new_with_config(aws_config);
    assert!(stt.is_ok());
}

#[tokio::test]
async fn test_client_get_config() {
    let config = STTConfig {
        provider: "aws-transcribe".to_string(),
        api_key: String::new(),
        language: "fr-FR".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm".to_string(),
        model: String::new(),
    };

    let stt = AwsTranscribeSTT::new(config).unwrap();
    let stored_config = stt.get_config();
    assert!(stored_config.is_some());
    assert_eq!(stored_config.unwrap().language, "fr-FR");
}

// =============================================================================
// Content Redaction Tests
// =============================================================================

#[test]
fn test_content_redaction_type() {
    assert_eq!(ContentRedactionType::Pii.as_str(), "PII");
}

#[test]
fn test_vocabulary_filter_method() {
    assert_eq!(VocabularyFilterMethod::Remove.as_str(), "remove");
    assert_eq!(VocabularyFilterMethod::Mask.as_str(), "mask");
    assert_eq!(VocabularyFilterMethod::Tag.as_str(), "tag");

    assert_eq!(
        VocabularyFilterMethod::from_str_or_default("mask"),
        VocabularyFilterMethod::Mask
    );
    assert_eq!(
        VocabularyFilterMethod::from_str_or_default("unknown"),
        VocabularyFilterMethod::Remove
    );
}

// =============================================================================
// Serialization Tests
// =============================================================================

#[test]
fn test_config_serialization() {
    let config = AwsTranscribeSTTConfig::default();
    let json = serde_json::to_string(&config);
    assert!(json.is_ok());

    let json_str = json.unwrap();
    assert!(json_str.contains("\"language\":\"en-US\""));
    assert!(json_str.contains("\"sample_rate\":16000"));
}

#[test]
fn test_config_deserialization() {
    let json = r#"{
        "provider": "aws-transcribe",
        "api_key": "",
        "language": "de-DE",
        "sample_rate": 16000,
        "channels": 1,
        "punctuation": true,
        "encoding": "pcm",
        "model": "",
        "region": "eu-central-1",
        "enable_partial_results_stabilization": true,
        "partial_results_stability": "medium"
    }"#;

    let config: Result<AwsTranscribeSTTConfig, _> = serde_json::from_str(json);
    assert!(config.is_ok());
    let config = config.unwrap();
    assert_eq!(config.base.language, "de-DE");
    assert_eq!(config.region, AwsRegion::EuCentral1);
    assert_eq!(
        config.partial_results_stability,
        PartialResultsStability::Medium
    );
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_empty_transcript_handling() {
    let result = TranscribeResult {
        result_id: None,
        start_time: None,
        end_time: None,
        is_partial: Some(false),
        alternatives: Some(vec![]),
        channel_id: None,
        language_code: None,
        language_identification: None,
    };

    assert!(result.best_transcript().is_none());
    assert_eq!(result.confidence(), 0.0);
}

#[test]
fn test_no_items_confidence() {
    let result = TranscribeResult {
        result_id: None,
        start_time: None,
        end_time: None,
        is_partial: Some(false),
        alternatives: Some(vec![Alternative {
            transcript: Some("test".to_string()),
            items: None, // No items
            entities: None,
        }]),
        channel_id: None,
        language_code: None,
        language_identification: None,
    };

    assert_eq!(result.confidence(), 0.0);
}

#[test]
fn test_empty_items_confidence() {
    let result = TranscribeResult {
        result_id: None,
        start_time: None,
        end_time: None,
        is_partial: Some(false),
        alternatives: Some(vec![Alternative {
            transcript: Some("test".to_string()),
            items: Some(vec![]), // Empty items
            entities: None,
        }]),
        channel_id: None,
        language_code: None,
        language_identification: None,
    };

    assert_eq!(result.confidence(), 0.0);
}

#[test]
fn test_multiple_alternatives() {
    let result = TranscribeResult {
        result_id: None,
        start_time: None,
        end_time: None,
        is_partial: Some(false),
        alternatives: Some(vec![
            Alternative {
                transcript: Some("hello".to_string()),
                items: None,
                entities: None,
            },
            Alternative {
                transcript: Some("helo".to_string()), // Typo variant
                items: None,
                entities: None,
            },
        ]),
        channel_id: None,
        language_code: None,
        language_identification: None,
    };

    // Should return the first (highest confidence) alternative
    assert_eq!(result.best_transcript(), Some("hello"));
}
