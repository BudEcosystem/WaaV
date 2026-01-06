//! Comprehensive tests for Groq STT provider.
//!
//! This module contains unit tests that do not require API credentials.
//! Integration tests requiring real API calls are in tests/groq_stt_integration.rs.

use super::*;
use crate::core::stt::base::{BaseSTT, STTConfig, STTError};

// =============================================================================
// Model Tests
// =============================================================================

mod model_tests {
    use super::*;

    #[test]
    fn test_model_as_str() {
        assert_eq!(GroqSTTModel::WhisperLargeV3.as_str(), "whisper-large-v3");
        assert_eq!(
            GroqSTTModel::WhisperLargeV3Turbo.as_str(),
            "whisper-large-v3-turbo"
        );
    }

    #[test]
    fn test_model_from_str_exact() {
        assert_eq!(
            GroqSTTModel::from_str_or_default("whisper-large-v3"),
            GroqSTTModel::WhisperLargeV3
        );
        assert_eq!(
            GroqSTTModel::from_str_or_default("whisper-large-v3-turbo"),
            GroqSTTModel::WhisperLargeV3Turbo
        );
    }

    #[test]
    fn test_model_from_str_aliases() {
        assert_eq!(
            GroqSTTModel::from_str_or_default("large-v3"),
            GroqSTTModel::WhisperLargeV3
        );
        assert_eq!(
            GroqSTTModel::from_str_or_default("whisper-v3"),
            GroqSTTModel::WhisperLargeV3
        );
        assert_eq!(
            GroqSTTModel::from_str_or_default("turbo"),
            GroqSTTModel::WhisperLargeV3Turbo
        );
        assert_eq!(
            GroqSTTModel::from_str_or_default("large-v3-turbo"),
            GroqSTTModel::WhisperLargeV3Turbo
        );
    }

    #[test]
    fn test_model_from_str_unknown_returns_default() {
        assert_eq!(
            GroqSTTModel::from_str_or_default("unknown-model"),
            GroqSTTModel::default()
        );
        assert_eq!(
            GroqSTTModel::from_str_or_default(""),
            GroqSTTModel::default()
        );
    }

    #[test]
    fn test_model_default_is_turbo() {
        assert_eq!(GroqSTTModel::default(), GroqSTTModel::WhisperLargeV3Turbo);
    }

    #[test]
    fn test_model_display() {
        assert_eq!(format!("{}", GroqSTTModel::WhisperLargeV3), "whisper-large-v3");
        assert_eq!(
            format!("{}", GroqSTTModel::WhisperLargeV3Turbo),
            "whisper-large-v3-turbo"
        );
    }

    #[test]
    fn test_model_word_error_rate() {
        assert!((GroqSTTModel::WhisperLargeV3.word_error_rate() - 0.103).abs() < 0.001);
        assert!((GroqSTTModel::WhisperLargeV3Turbo.word_error_rate() - 0.12).abs() < 0.001);
    }

    #[test]
    fn test_model_speed_factor() {
        assert_eq!(GroqSTTModel::WhisperLargeV3.speed_factor(), 189);
        assert_eq!(GroqSTTModel::WhisperLargeV3Turbo.speed_factor(), 216);
    }

    #[test]
    fn test_model_cost_per_hour() {
        assert!((GroqSTTModel::WhisperLargeV3.cost_per_hour() - 0.111).abs() < 0.001);
        assert!((GroqSTTModel::WhisperLargeV3Turbo.cost_per_hour() - 0.04).abs() < 0.001);
    }

    #[test]
    fn test_model_serialization() {
        let model = GroqSTTModel::WhisperLargeV3Turbo;
        let json = serde_json::to_string(&model).unwrap();
        assert_eq!(json, "\"whisper-large-v3-turbo\"");

        let deserialized: GroqSTTModel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, model);
    }
}

// =============================================================================
// Response Format Tests
// =============================================================================

mod response_format_tests {
    use super::*;

    #[test]
    fn test_response_format_as_str() {
        assert_eq!(GroqResponseFormat::Json.as_str(), "json");
        assert_eq!(GroqResponseFormat::Text.as_str(), "text");
        assert_eq!(GroqResponseFormat::VerboseJson.as_str(), "verbose_json");
    }

    #[test]
    fn test_response_format_default() {
        assert_eq!(GroqResponseFormat::default(), GroqResponseFormat::Json);
    }

    #[test]
    fn test_response_format_display() {
        assert_eq!(format!("{}", GroqResponseFormat::Json), "json");
        assert_eq!(format!("{}", GroqResponseFormat::VerboseJson), "verbose_json");
    }

    #[test]
    fn test_response_format_serialization() {
        let format = GroqResponseFormat::VerboseJson;
        let json = serde_json::to_string(&format).unwrap();
        assert_eq!(json, "\"verbose_json\"");

        let deserialized: GroqResponseFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, format);
    }
}

// =============================================================================
// Audio Format Tests
// =============================================================================

mod audio_format_tests {
    use super::*;

    #[test]
    fn test_audio_format_mime_types() {
        assert_eq!(AudioInputFormat::Wav.mime_type(), "audio/wav");
        assert_eq!(AudioInputFormat::Flac.mime_type(), "audio/flac");
        assert_eq!(AudioInputFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(AudioInputFormat::Mp4.mime_type(), "audio/mp4");
        assert_eq!(AudioInputFormat::Mpeg.mime_type(), "audio/mpeg");
        assert_eq!(AudioInputFormat::Mpga.mime_type(), "audio/mpeg");
        assert_eq!(AudioInputFormat::M4a.mime_type(), "audio/m4a");
        assert_eq!(AudioInputFormat::Ogg.mime_type(), "audio/ogg");
        assert_eq!(AudioInputFormat::Webm.mime_type(), "audio/webm");
    }

    #[test]
    fn test_audio_format_extensions() {
        assert_eq!(AudioInputFormat::Wav.extension(), "wav");
        assert_eq!(AudioInputFormat::Flac.extension(), "flac");
        assert_eq!(AudioInputFormat::Mp3.extension(), "mp3");
        assert_eq!(AudioInputFormat::Ogg.extension(), "ogg");
        assert_eq!(AudioInputFormat::Webm.extension(), "webm");
    }

    #[test]
    fn test_audio_format_default() {
        assert_eq!(AudioInputFormat::default(), AudioInputFormat::Wav);
    }
}

// =============================================================================
// Timestamp Granularity Tests
// =============================================================================

mod timestamp_tests {
    use super::*;

    #[test]
    fn test_timestamp_granularity_as_str() {
        assert_eq!(TimestampGranularity::Word.as_str(), "word");
        assert_eq!(TimestampGranularity::Segment.as_str(), "segment");
    }

    #[test]
    fn test_timestamp_granularity_serialization() {
        let word = TimestampGranularity::Word;
        let json = serde_json::to_string(&word).unwrap();
        assert_eq!(json, "\"word\"");

        let deserialized: TimestampGranularity = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, word);
    }
}

// =============================================================================
// Config Tests
// =============================================================================

mod config_tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = GroqSTTConfig::default();
        assert_eq!(config.model, GroqSTTModel::WhisperLargeV3Turbo);
        assert_eq!(config.response_format, GroqResponseFormat::VerboseJson);
        assert_eq!(config.temperature, Some(0.0));
        assert_eq!(config.flush_threshold_bytes, 1024 * 1024);
        assert_eq!(config.max_file_size_bytes, DEFAULT_MAX_FILE_SIZE);
        assert!(!config.translate_to_english);
        assert!(config.custom_endpoint.is_none());
    }

    #[test]
    fn test_config_from_base() {
        let base = STTConfig {
            api_key: "test_key".to_string(),
            model: "whisper-large-v3".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };

        let config = GroqSTTConfig::from_base(base);
        assert_eq!(config.model, GroqSTTModel::WhisperLargeV3);
        assert_eq!(config.base.language, "en");
        assert_eq!(config.base.sample_rate, 16000);
    }

    #[test]
    fn test_config_validation_empty_api_key() {
        let config = GroqSTTConfig {
            base: STTConfig {
                api_key: String::new(),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
    }

    #[test]
    fn test_config_validation_invalid_temperature_high() {
        let config = GroqSTTConfig {
            base: STTConfig {
                api_key: "test_key".to_string(),
                ..Default::default()
            },
            temperature: Some(1.5),
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Temperature"));
    }

    #[test]
    fn test_config_validation_invalid_temperature_negative() {
        let config = GroqSTTConfig {
            base: STTConfig {
                api_key: "test_key".to_string(),
                ..Default::default()
            },
            temperature: Some(-0.5),
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Temperature"));
    }

    #[test]
    fn test_config_validation_threshold_exceeds_max() {
        let config = GroqSTTConfig {
            base: STTConfig {
                api_key: "test_key".to_string(),
                ..Default::default()
            },
            flush_threshold_bytes: 30 * 1024 * 1024,
            max_file_size_bytes: 25 * 1024 * 1024,
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceed max file size"));
    }

    #[test]
    fn test_config_validation_valid() {
        let config = GroqSTTConfig {
            base: STTConfig {
                api_key: "test_key".to_string(),
                ..Default::default()
            },
            temperature: Some(0.5),
            ..Default::default()
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_api_url_default() {
        let config = GroqSTTConfig::default();
        assert_eq!(config.api_url(), GROQ_STT_URL);
    }

    #[test]
    fn test_config_api_url_translation() {
        let config = GroqSTTConfig {
            translate_to_english: true,
            ..Default::default()
        };
        assert_eq!(config.api_url(), GROQ_TRANSLATION_URL);
    }

    #[test]
    fn test_config_api_url_custom() {
        let config = GroqSTTConfig {
            custom_endpoint: Some("https://custom.api.com/transcribe".to_string()),
            ..Default::default()
        };
        assert_eq!(config.api_url(), "https://custom.api.com/transcribe");
    }

    #[test]
    fn test_config_builder_with_model() {
        let config = GroqSTTConfig::default().with_model(GroqSTTModel::WhisperLargeV3);
        assert_eq!(config.model, GroqSTTModel::WhisperLargeV3);
    }

    #[test]
    fn test_config_builder_with_response_format() {
        let config = GroqSTTConfig::default().with_response_format(GroqResponseFormat::Text);
        assert_eq!(config.response_format, GroqResponseFormat::Text);
    }

    #[test]
    fn test_config_builder_with_translation() {
        let config = GroqSTTConfig::default().with_translation();
        assert!(config.translate_to_english);
    }

    #[test]
    fn test_config_builder_with_dev_tier() {
        let config = GroqSTTConfig::default().with_dev_tier();
        assert_eq!(config.max_file_size_bytes, DEV_TIER_MAX_FILE_SIZE);
    }
}

// =============================================================================
// Message Tests
// =============================================================================

mod message_tests {
    use super::*;

    #[test]
    fn test_simple_response_parsing() {
        let json = r#"{
            "text": "Hello world",
            "x_groq": {"id": "req_123abc"}
        }"#;

        let response: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello world");
        assert_eq!(response.x_groq.as_ref().unwrap().id, "req_123abc");
    }

    #[test]
    fn test_simple_response_without_metadata() {
        let json = r#"{"text": "Hello world"}"#;
        let response: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello world");
        assert!(response.x_groq.is_none());
    }

    #[test]
    fn test_verbose_response_parsing() {
        let json = r#"{
            "text": "Hello world",
            "language": "en",
            "duration": 2.5,
            "segments": [{
                "id": 0,
                "seek": 0,
                "start": 0.0,
                "end": 2.5,
                "text": "Hello world",
                "avg_logprob": -0.3,
                "no_speech_prob": 0.01,
                "compression_ratio": 1.2,
                "tokens": [1, 2, 3],
                "temperature": 0.0
            }],
            "words": [{
                "word": "Hello",
                "start": 0.0,
                "end": 1.2
            }, {
                "word": "world",
                "start": 1.2,
                "end": 2.5
            }]
        }"#;

        let response: VerboseTranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello world");
        assert_eq!(response.language.as_deref(), Some("en"));
        assert_eq!(response.duration, Some(2.5));
        assert_eq!(response.segments.len(), 1);
        assert_eq!(response.words.len(), 2);

        let segment = &response.segments[0];
        assert_eq!(segment.id, 0);
        assert_eq!(segment.text, "Hello world");
        assert!(segment.avg_logprob.is_some());
    }

    #[test]
    fn test_segment_confidence() {
        let segment = Segment {
            id: 0,
            seek: 0,
            start: 0.0,
            end: 1.0,
            text: "Test".to_string(),
            avg_logprob: Some(-0.3),
            no_speech_prob: Some(0.01),
            compression_ratio: None,
            tokens: vec![],
            temperature: None,
        };

        let confidence = segment.confidence();
        assert!(confidence > 0.0 && confidence <= 1.0);
    }

    #[test]
    fn test_segment_confidence_default() {
        let segment = Segment {
            id: 0,
            seek: 0,
            start: 0.0,
            end: 1.0,
            text: "Test".to_string(),
            avg_logprob: None,
            no_speech_prob: None,
            compression_ratio: None,
            tokens: vec![],
            temperature: None,
        };

        // Default confidence is 0.5 (DEFAULT_UNKNOWN_CONFIDENCE) when avg_logprob is None
        assert!((segment.confidence() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_segment_is_speech() {
        let speech_segment = Segment {
            id: 0,
            seek: 0,
            start: 0.0,
            end: 1.0,
            text: "Hello".to_string(),
            avg_logprob: None,
            no_speech_prob: Some(0.1), // Low probability of no speech
            compression_ratio: None,
            tokens: vec![],
            temperature: None,
        };
        assert!(speech_segment.is_speech());

        let silence_segment = Segment {
            id: 0,
            seek: 0,
            start: 0.0,
            end: 1.0,
            text: "".to_string(),
            avg_logprob: None,
            no_speech_prob: Some(0.9), // High probability of no speech
            compression_ratio: None,
            tokens: vec![],
            temperature: None,
        };
        assert!(!silence_segment.is_speech());
    }

    #[test]
    fn test_error_response_parsing() {
        let json = r#"{
            "error": {
                "message": "Rate limit exceeded for model",
                "type": "rate_limit_error",
                "code": "rate_limit_exceeded"
            }
        }"#;

        let error: GroqErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(error.error.message, "Rate limit exceeded for model");
        assert_eq!(error.error.error_type, "rate_limit_error");
        assert_eq!(error.error.code, Some("rate_limit_exceeded".to_string()));
    }

    #[test]
    fn test_transcription_result_text() {
        let simple = TranscriptionResult::Simple(TranscriptionResponse {
            text: "Hello".to_string(),
            x_groq: None,
        });
        assert_eq!(simple.text(), "Hello");

        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "World".to_string(),
            language: Some("en".to_string()),
            duration: Some(1.0),
            segments: vec![],
            words: vec![],
            x_groq: None,
        });
        assert_eq!(verbose.text(), "World");

        let plain = TranscriptionResult::PlainText("Plain text".to_string());
        assert_eq!(plain.text(), "Plain text");
    }

    #[test]
    fn test_transcription_result_language() {
        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Test".to_string(),
            language: Some("fr".to_string()),
            duration: None,
            segments: vec![],
            words: vec![],
            x_groq: None,
        });
        assert_eq!(verbose.language(), Some("fr"));

        let simple = TranscriptionResult::Simple(TranscriptionResponse {
            text: "Test".to_string(),
            x_groq: None,
        });
        assert_eq!(simple.language(), None);
    }

    #[test]
    fn test_transcription_result_duration() {
        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: Some(5.5),
            segments: vec![],
            words: vec![],
            x_groq: None,
        });
        assert_eq!(verbose.duration(), Some(5.5));

        let simple = TranscriptionResult::Simple(TranscriptionResponse {
            text: "Test".to_string(),
            x_groq: None,
        });
        assert_eq!(simple.duration(), None);
    }
}

// =============================================================================
// WAV Tests
// =============================================================================

mod wav_tests {
    use super::messages::wav;

    #[test]
    fn test_wav_header_size() {
        assert_eq!(wav::HEADER_SIZE, 44);
    }

    #[test]
    fn test_wav_creation_header() {
        let pcm_data = vec![0u8; 100];
        let wav_file = wav::create_wav(&pcm_data, 16000, 1);

        // Check RIFF header
        assert_eq!(&wav_file[0..4], b"RIFF");
        assert_eq!(&wav_file[8..12], b"WAVE");
        assert_eq!(&wav_file[12..16], b"fmt ");
        assert_eq!(&wav_file[36..40], b"data");
    }

    #[test]
    fn test_wav_creation_size() {
        let pcm_data = vec![0u8; 100];
        let wav_file = wav::create_wav(&pcm_data, 16000, 1);

        assert_eq!(wav_file.len(), wav::HEADER_SIZE + pcm_data.len());
    }

    #[test]
    fn test_wav_creation_stereo() {
        let pcm_data = vec![0u8; 200];
        let wav_file = wav::create_wav(&pcm_data, 44100, 2);

        // Check channels (bytes 22-23)
        let channels = u16::from_le_bytes([wav_file[22], wav_file[23]]);
        assert_eq!(channels, 2);

        // Check sample rate (bytes 24-27)
        let sample_rate = u32::from_le_bytes([
            wav_file[24],
            wav_file[25],
            wav_file[26],
            wav_file[27],
        ]);
        assert_eq!(sample_rate, 44100);
    }

    #[test]
    fn test_wav_creation_empty() {
        let pcm_data = vec![];
        let wav_file = wav::create_wav(&pcm_data, 16000, 1);

        assert_eq!(wav_file.len(), wav::HEADER_SIZE);
    }
}

// =============================================================================
// Client Tests
// =============================================================================

mod client_tests {
    use super::*;
    use bytes::Bytes;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_client_creation_with_valid_config() {
        let config = STTConfig {
            provider: "groq".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };

        let stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), "Groq Whisper STT");
    }

    #[tokio::test]
    async fn test_client_creation_empty_api_key() {
        let config = STTConfig {
            provider: "groq".to_string(),
            api_key: String::new(),
            ..Default::default()
        };

        let result = <GroqSTT as BaseSTT>::new(config);
        assert!(result.is_err());
        match result {
            Err(STTError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("API key"));
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_client_connect_disconnect() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        assert!(!stt.is_ready());

        stt.connect().await.unwrap();
        assert!(stt.is_ready());

        stt.disconnect().await.unwrap();
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_client_buffer_audio() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };

        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        stt.connect().await.unwrap();

        // Send audio
        let audio: Bytes = vec![0u8; 1024].into();
        stt.send_audio(audio).await.unwrap();

        assert_eq!(stt.audio_buffer.len(), 1024);
        assert_eq!(stt.total_bytes_received, 1024);

        // Send more audio
        let more_audio: Bytes = vec![0u8; 512].into();
        stt.send_audio(more_audio).await.unwrap();

        assert_eq!(stt.audio_buffer.len(), 1536);
        assert_eq!(stt.total_bytes_received, 1536);
    }

    #[tokio::test]
    async fn test_client_send_without_connect() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();

        let audio: Bytes = vec![0u8; 1024].into();
        let result = stt.send_audio(audio).await;

        assert!(result.is_err());
        match result {
            Err(STTError::ConnectionFailed(msg)) => {
                assert!(msg.contains("not connected"));
            }
            _ => panic!("Expected ConnectionFailed error"),
        }
    }

    #[tokio::test]
    async fn test_client_callback_registration() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();

        let callback_count = Arc::new(AtomicU32::new(0));
        let callback_count_clone = callback_count.clone();

        let callback = Arc::new(move |_result: crate::core::stt::base::STTResult| {
            let count = callback_count_clone.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::Relaxed);
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        stt.on_result(callback).await.unwrap();
        assert!(stt.result_callback.lock().await.is_some());
    }

    #[tokio::test]
    async fn test_client_with_groq_config() {
        let groq_config = GroqSTTConfig {
            base: STTConfig {
                api_key: "test_key".to_string(),
                ..Default::default()
            },
            model: GroqSTTModel::WhisperLargeV3,
            response_format: GroqResponseFormat::Text,
            temperature: Some(0.3),
            ..Default::default()
        };

        let stt = GroqSTT::with_config(groq_config).unwrap();
        let config = stt.config.as_ref().unwrap();

        assert_eq!(config.model, GroqSTTModel::WhisperLargeV3);
        assert_eq!(config.response_format, GroqResponseFormat::Text);
        assert_eq!(config.temperature, Some(0.3));
    }

    #[test]
    fn test_client_should_flush_on_threshold() {
        let mut stt = GroqSTT::default();
        stt.config = Some(GroqSTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            flush_strategy: FlushStrategy::OnThreshold,
            flush_threshold_bytes: 1000,
            ..Default::default()
        });

        // Below threshold
        stt.audio_buffer = vec![0u8; 500];
        assert!(!stt.should_flush(None));

        // At threshold
        stt.audio_buffer = vec![0u8; 1000];
        assert!(stt.should_flush(None));

        // Above threshold
        stt.audio_buffer = vec![0u8; 2000];
        assert!(stt.should_flush(None));
    }

    #[test]
    fn test_client_should_not_flush_on_disconnect_strategy() {
        let mut stt = GroqSTT::default();
        stt.config = Some(GroqSTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            flush_strategy: FlushStrategy::OnDisconnect,
            flush_threshold_bytes: 1000,
            ..Default::default()
        });

        // Even with large buffer, OnDisconnect should not flush
        stt.audio_buffer = vec![0u8; 5000];
        assert!(!stt.should_flush(None));
    }

    #[test]
    fn test_client_rms_calculation_silent() {
        let silent_audio = vec![0u8; 1000];
        let rms = GroqSTT::calculate_rms_energy(&silent_audio);
        assert!(rms < 0.001);
    }

    #[test]
    fn test_client_rms_calculation_loud() {
        let mut loud_audio = Vec::new();
        for _ in 0..500 {
            loud_audio.extend_from_slice(&i16::MAX.to_le_bytes());
        }
        let rms = GroqSTT::calculate_rms_energy(&loud_audio);
        assert!(rms > 0.9);
    }

    #[test]
    fn test_client_is_audio_silent() {
        let silent = vec![0u8; 1000];
        assert!(GroqSTT::is_audio_silent(&silent, 0.01));

        let mut loud = Vec::new();
        for _ in 0..500 {
            loud.extend_from_slice(&10000i16.to_le_bytes());
        }
        assert!(!GroqSTT::is_audio_silent(&loud, 0.01));
    }

    #[test]
    fn test_client_is_retryable_error() {
        // Retryable errors
        assert!(GroqSTT::is_retryable_error(&STTError::NetworkError(
            "timeout".to_string()
        )));
        assert!(GroqSTT::is_retryable_error(&STTError::ProviderError(
            "429 rate limit".to_string()
        )));
        assert!(GroqSTT::is_retryable_error(&STTError::ProviderError(
            "503 Service Unavailable".to_string()
        )));

        // Non-retryable errors
        assert!(!GroqSTT::is_retryable_error(&STTError::AuthenticationFailed(
            "invalid key".to_string()
        )));
        assert!(!GroqSTT::is_retryable_error(&STTError::ConfigurationError(
            "invalid config".to_string()
        )));
    }

    #[test]
    fn test_client_estimated_cost() {
        let mut stt = GroqSTT::default();
        stt.config = Some(GroqSTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                sample_rate: 16000,
                channels: 1,
                ..Default::default()
            },
            model: GroqSTTModel::WhisperLargeV3Turbo,
            ..Default::default()
        });

        // Add 1 minute of audio (16kHz, 16-bit mono = 1,920,000 bytes)
        stt.audio_buffer = vec![0u8; 1_920_000];
        let cost = stt.estimated_cost();

        // 1 minute at $0.04/hour for turbo = ~$0.000667
        assert!(cost > 0.0005);
        assert!(cost < 0.001);
    }

    #[test]
    fn test_client_model_accessor() {
        let mut stt = GroqSTT::default();
        assert!(stt.model().is_none());

        stt.config = Some(GroqSTTConfig::default());
        assert_eq!(stt.model(), Some(&GroqSTTModel::WhisperLargeV3Turbo));
    }

    #[tokio::test]
    async fn test_client_update_config() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
            ..Default::default()
        };

        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        stt.connect().await.unwrap();
        assert!(stt.is_ready());

        // Update config
        let new_config = STTConfig {
            api_key: "new_key".to_string(),
            model: "whisper-large-v3".to_string(),
            ..Default::default()
        };

        stt.update_config(new_config).await.unwrap();
        assert!(stt.is_ready());

        let stored_config = stt.config.as_ref().unwrap();
        assert_eq!(stored_config.base.api_key, "new_key");
        assert_eq!(stored_config.model, GroqSTTModel::WhisperLargeV3);
    }
}

// =============================================================================
// Constants Tests
// =============================================================================

mod constants_tests {
    use super::*;

    #[test]
    fn test_api_urls() {
        assert_eq!(
            GROQ_STT_URL,
            "https://api.groq.com/openai/v1/audio/transcriptions"
        );
        assert_eq!(
            GROQ_TRANSLATION_URL,
            "https://api.groq.com/openai/v1/audio/translations"
        );
    }

    #[test]
    fn test_file_size_limits() {
        assert_eq!(DEFAULT_MAX_FILE_SIZE, 25 * 1024 * 1024); // 25MB
        assert_eq!(DEV_TIER_MAX_FILE_SIZE, 100 * 1024 * 1024); // 100MB
    }

    #[test]
    fn test_max_prompt_tokens() {
        assert_eq!(MAX_PROMPT_TOKENS, 224);
    }
}

// =============================================================================
// Rate Limit Info Tests
// =============================================================================

mod rate_limit_tests {
    use super::*;

    #[test]
    fn test_rate_limit_info_default() {
        let info = RateLimitInfo::default();
        assert!(info.remaining_requests.is_none());
        assert!(info.remaining_tokens.is_none());
        assert!(info.reset_requests_at.is_none());
        assert!(info.reset_tokens_at.is_none());
        assert!(info.retry_after_ms.is_none());
    }

    #[test]
    fn test_rate_limit_parse_duration_string_ms() {
        let result = RateLimitInfo::parse_duration_string("500ms");
        assert_eq!(result, Some(500));
    }

    #[test]
    fn test_rate_limit_parse_duration_string_seconds() {
        let result = RateLimitInfo::parse_duration_string("5s");
        assert_eq!(result, Some(5000));
    }

    #[test]
    fn test_rate_limit_parse_duration_string_minutes() {
        let result = RateLimitInfo::parse_duration_string("2m");
        assert_eq!(result, Some(120_000));
    }

    #[test]
    fn test_rate_limit_parse_duration_string_invalid() {
        assert!(RateLimitInfo::parse_duration_string("invalid").is_none());
        assert!(RateLimitInfo::parse_duration_string("").is_none());
        assert!(RateLimitInfo::parse_duration_string("5h").is_none()); // hours not supported
    }

    #[test]
    fn test_rate_limit_parse_retry_after_seconds() {
        // Integer seconds should be converted to ms
        let result = RateLimitInfo::parse_retry_after("5");
        assert_eq!(result, Some(5000));
    }

    #[test]
    fn test_rate_limit_parse_retry_after_duration_string() {
        let result = RateLimitInfo::parse_retry_after("500ms");
        assert_eq!(result, Some(500));
    }

    #[test]
    fn test_rate_limit_from_headers() {
        use reqwest::header::{HeaderMap, HeaderValue};

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-remaining-requests",
            HeaderValue::from_static("100"),
        );
        headers.insert(
            "x-ratelimit-remaining-tokens",
            HeaderValue::from_static("50000"),
        );
        headers.insert("retry-after", HeaderValue::from_static("30"));

        let info = RateLimitInfo::from_headers(&headers);
        assert_eq!(info.remaining_requests, Some(100));
        assert_eq!(info.remaining_tokens, Some(50000));
        assert_eq!(info.retry_after_ms, Some(30_000)); // 30 seconds in ms
    }

    #[test]
    fn test_rate_limit_from_headers_empty() {
        let headers = reqwest::header::HeaderMap::new();
        let info = RateLimitInfo::from_headers(&headers);
        assert!(info.remaining_requests.is_none());
        assert!(info.remaining_tokens.is_none());
        assert!(info.retry_after_ms.is_none());
    }
}

// =============================================================================
// WAV Validation Tests
// =============================================================================

mod wav_validation_tests {
    use super::messages::wav::{try_create_wav, WavError};

    #[test]
    fn test_wav_zero_sample_rate() {
        let pcm_data = vec![0u8; 100];
        let result = try_create_wav(&pcm_data, 0, 1);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), WavError::ZeroSampleRate);
    }

    #[test]
    fn test_wav_zero_channels() {
        let pcm_data = vec![0u8; 100];
        let result = try_create_wav(&pcm_data, 16000, 0);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), WavError::ZeroChannels);
    }

    #[test]
    fn test_wav_valid_params() {
        let pcm_data = vec![0u8; 100];
        let result = try_create_wav(&pcm_data, 16000, 1);
        assert!(result.is_ok());
        let wav = result.unwrap();
        assert_eq!(wav.len(), 44 + 100); // header + data
    }

    #[test]
    fn test_wav_error_display() {
        assert_eq!(
            WavError::ZeroSampleRate.to_string(),
            "Sample rate cannot be zero"
        );
        assert_eq!(
            WavError::ZeroChannels.to_string(),
            "Number of channels cannot be zero"
        );
        assert_eq!(
            WavError::DataTooLarge.to_string(),
            "PCM data exceeds maximum WAV file size (4GB)"
        );
    }

    #[test]
    #[should_panic(expected = "Invalid WAV parameters")]
    fn test_wav_create_panics_on_zero_sample_rate() {
        let pcm_data = vec![0u8; 100];
        let _ = super::messages::wav::create_wav(&pcm_data, 0, 1);
    }
}

// =============================================================================
// Silence Detection Config Tests
// =============================================================================

mod silence_detection_config_tests {
    use super::*;

    #[test]
    fn test_silence_config_default() {
        let config = SilenceDetectionConfig::default();
        assert!((config.rms_threshold - 0.01).abs() < f32::EPSILON);
        assert_eq!(config.silence_duration_ms, 1000);
        assert_eq!(config.min_audio_duration_ms, 500);
    }

    #[test]
    fn test_silence_config_validate_valid() {
        let config = SilenceDetectionConfig {
            rms_threshold: 0.05,
            silence_duration_ms: 500,
            min_audio_duration_ms: 1000,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_silence_config_validate_invalid_rms_threshold_negative() {
        let config = SilenceDetectionConfig {
            rms_threshold: -0.01,
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("RMS threshold"));
    }

    #[test]
    fn test_silence_config_validate_invalid_rms_threshold_high() {
        let config = SilenceDetectionConfig {
            rms_threshold: 1.5,
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("RMS threshold"));
    }

    #[test]
    fn test_silence_config_validate_invalid_duration_too_short() {
        let config = SilenceDetectionConfig {
            silence_duration_ms: 50, // Below 100ms
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Silence duration"));
    }

    #[test]
    fn test_silence_config_validate_invalid_duration_too_long() {
        let config = SilenceDetectionConfig {
            silence_duration_ms: 70000, // Above 60000ms
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Silence duration"));
    }

    #[test]
    fn test_silence_config_validate_invalid_min_audio_duration() {
        let config = SilenceDetectionConfig {
            min_audio_duration_ms: 35000, // Above 30000ms
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Minimum audio duration"));
    }

    #[test]
    fn test_silence_config_serialization() {
        let config = SilenceDetectionConfig {
            rms_threshold: 0.02,
            silence_duration_ms: 500,
            min_audio_duration_ms: 1000,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SilenceDetectionConfig = serde_json::from_str(&json).unwrap();

        assert!((deserialized.rms_threshold - 0.02).abs() < f32::EPSILON);
        assert_eq!(deserialized.silence_duration_ms, 500);
        assert_eq!(deserialized.min_audio_duration_ms, 1000);
    }

    #[test]
    fn test_silence_config_deserialization_defaults() {
        // Empty JSON object should use defaults
        let config: SilenceDetectionConfig = serde_json::from_str("{}").unwrap();
        assert!((config.rms_threshold - 0.01).abs() < f32::EPSILON);
        assert_eq!(config.silence_duration_ms, 1000);
        assert_eq!(config.min_audio_duration_ms, 500);
    }
}

// =============================================================================
// Flush Strategy Tests
// =============================================================================

mod flush_strategy_tests {
    use super::*;

    #[test]
    fn test_flush_strategy_default() {
        assert_eq!(FlushStrategy::default(), FlushStrategy::OnDisconnect);
    }

    #[test]
    fn test_flush_strategy_serialization() {
        let strategies = [
            (FlushStrategy::OnDisconnect, "\"on_disconnect\""),
            (FlushStrategy::OnThreshold, "\"on_threshold\""),
            (FlushStrategy::OnSilence, "\"on_silence\""),
        ];

        for (strategy, expected_json) in strategies {
            let json = serde_json::to_string(&strategy).unwrap();
            assert_eq!(json, expected_json);

            let deserialized: FlushStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, strategy);
        }
    }
}

// =============================================================================
// Buffer Management Tests
// =============================================================================

mod buffer_management_tests {
    use super::*;
    use bytes::Bytes;

    #[tokio::test]
    async fn test_buffer_len_empty() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        let stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        assert_eq!(stt.buffer_len(), 0);
    }

    #[tokio::test]
    async fn test_buffer_len_with_data() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        stt.connect().await.unwrap();

        let audio: Bytes = vec![0u8; 1000].into();
        stt.send_audio(audio).await.unwrap();
        assert_eq!(stt.buffer_len(), 1000);
    }

    #[tokio::test]
    async fn test_is_buffer_empty() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        assert!(stt.is_buffer_empty());

        stt.connect().await.unwrap();
        let audio: Bytes = vec![0u8; 100].into();
        stt.send_audio(audio).await.unwrap();
        assert!(!stt.is_buffer_empty());
    }

    #[tokio::test]
    async fn test_clear_buffer() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        stt.connect().await.unwrap();

        let audio: Bytes = vec![0u8; 1000].into();
        stt.send_audio(audio).await.unwrap();
        assert!(!stt.is_buffer_empty());

        stt.clear_buffer();

        assert!(stt.is_buffer_empty());
        assert_eq!(stt.buffer_len(), 0);
    }

    #[tokio::test]
    async fn test_take_buffer() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        stt.connect().await.unwrap();

        let original_data = vec![1u8, 2, 3, 4, 5];
        let audio: Bytes = original_data.clone().into();
        stt.send_audio(audio).await.unwrap();

        let taken = stt.take_buffer();

        assert_eq!(taken, original_data);
        assert!(stt.is_buffer_empty());
    }

    #[tokio::test]
    async fn test_buffer_reference() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        stt.connect().await.unwrap();

        let data = vec![10u8, 20, 30];
        let audio: Bytes = data.clone().into();
        stt.send_audio(audio).await.unwrap();

        let buffer_ref = stt.buffer();
        assert_eq!(buffer_ref, &data[..]);
    }

    #[tokio::test]
    async fn test_take_buffer_clears_state() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        let mut stt = <GroqSTT as BaseSTT>::new(config).unwrap();
        stt.connect().await.unwrap();

        // Send audio
        let audio: Bytes = vec![0u8; 1000].into();
        stt.send_audio(audio).await.unwrap();

        // Take the buffer
        let taken = stt.take_buffer();
        assert_eq!(taken.len(), 1000);

        // Buffer should be empty but still usable
        assert!(stt.is_buffer_empty());

        // Can send more audio
        let more_audio: Bytes = vec![1u8; 500].into();
        stt.send_audio(more_audio).await.unwrap();
        assert_eq!(stt.buffer_len(), 500);
    }
}

// =============================================================================
// Duration Weighted Confidence Tests
// =============================================================================

mod confidence_tests {
    use super::*;

    #[test]
    fn test_transcription_result_confidence_weighted_by_duration() {
        // Create a verbose response with segments of different durations and confidences
        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: None,
            segments: vec![
                Segment {
                    id: 0,
                    seek: 0,
                    start: 0.0,
                    end: 1.0, // 1 second
                    text: "Short".to_string(),
                    avg_logprob: Some(-0.5), // Higher confidence
                    no_speech_prob: None,
                    compression_ratio: None,
                    tokens: vec![],
                    temperature: None,
                },
                Segment {
                    id: 1,
                    seek: 0,
                    start: 1.0,
                    end: 4.0, // 3 seconds
                    text: "Longer segment".to_string(),
                    avg_logprob: Some(-1.5), // Lower confidence
                    no_speech_prob: None,
                    compression_ratio: None,
                    tokens: vec![],
                    temperature: None,
                },
            ],
            words: vec![],
            x_groq: None,
        });

        let confidence = verbose.confidence();

        // Weighted average should favor the longer segment (3 seconds vs 1 second)
        // So overall confidence should be closer to the lower confidence segment
        assert!(confidence > 0.0 && confidence < 1.0);

        // Get individual segment confidences for comparison
        let seg0_conf = verbose.segments().unwrap()[0].confidence();
        let seg1_conf = verbose.segments().unwrap()[1].confidence();

        // The weighted average should be between the two, but closer to seg1
        // due to duration weighting (seg1 is 3x longer)
        assert!(confidence < seg0_conf);
        assert!(confidence > seg1_conf);
    }

    #[test]
    fn test_transcription_result_confidence_empty_segments() {
        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: None,
            segments: vec![],
            words: vec![],
            x_groq: None,
        });

        // Should return DEFAULT_UNKNOWN_CONFIDENCE when no segments
        assert!((verbose.confidence() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_transcription_result_confidence_zero_duration_segments() {
        // Test fallback when all segment durations are zero
        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: None,
            segments: vec![
                Segment {
                    id: 0,
                    seek: 0,
                    start: 0.0,
                    end: 0.0, // Zero duration
                    text: "A".to_string(),
                    avg_logprob: Some(-0.3),
                    no_speech_prob: None,
                    compression_ratio: None,
                    tokens: vec![],
                    temperature: None,
                },
                Segment {
                    id: 1,
                    seek: 0,
                    start: 0.0,
                    end: 0.0, // Zero duration
                    text: "B".to_string(),
                    avg_logprob: Some(-0.7),
                    no_speech_prob: None,
                    compression_ratio: None,
                    tokens: vec![],
                    temperature: None,
                },
            ],
            words: vec![],
            x_groq: None,
        });

        // Should fallback to simple average
        let confidence = verbose.confidence();
        assert!(confidence > 0.0 && confidence < 1.0);
    }
}

// =============================================================================
// Default Unknown Confidence Constant Tests
// =============================================================================

mod confidence_constant_tests {
    use super::*;

    #[test]
    fn test_default_confidence_constants_match() {
        // Verify that the f32 and f64 constants are equivalent
        let f32_const = DEFAULT_UNKNOWN_CONFIDENCE;
        let f64_const = MESSAGE_DEFAULT_UNKNOWN_CONFIDENCE;

        // f32 should be derived from f64
        assert!((f32_const as f64 - f64_const).abs() < 0.0001);
    }

    #[test]
    fn test_default_confidence_value() {
        // Both should be 0.5
        assert!((DEFAULT_UNKNOWN_CONFIDENCE - 0.5).abs() < f32::EPSILON);
        assert!((MESSAGE_DEFAULT_UNKNOWN_CONFIDENCE - 0.5).abs() < f64::EPSILON);
    }
}
