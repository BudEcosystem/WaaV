//! Tests for OpenAI STT module.
//!
//! This module contains comprehensive unit tests for the OpenAI Whisper
//! STT implementation, covering:
//! - Configuration validation
//! - Client lifecycle (creation, connect, disconnect)
//! - Audio buffering behavior
//! - Response parsing
//! - Error handling

use super::*;
use crate::core::stt::base::{BaseSTT, STTConfig, STTError, STTResult};
use bytes::Bytes;
use std::sync::Arc;

// =============================================================================
// Configuration Tests
// =============================================================================

mod config_tests {
    use super::*;

    #[test]
    fn test_model_serialization() {
        // Test model enum serialization
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

    #[test]
    fn test_model_parsing() {
        assert_eq!(
            OpenAISTTModel::from_str_or_default("whisper-1"),
            OpenAISTTModel::Whisper1
        );
        assert_eq!(
            OpenAISTTModel::from_str_or_default("WHISPER-1"),
            OpenAISTTModel::Whisper1
        );
        assert_eq!(
            OpenAISTTModel::from_str_or_default("gpt-4o-transcribe"),
            OpenAISTTModel::Gpt4oTranscribe
        );
        assert_eq!(
            OpenAISTTModel::from_str_or_default("invalid"),
            OpenAISTTModel::Whisper1
        ); // Default
    }

    #[test]
    fn test_response_format_variants() {
        assert_eq!(ResponseFormat::Json.as_str(), "json");
        assert_eq!(ResponseFormat::Text.as_str(), "text");
        assert_eq!(ResponseFormat::VerboseJson.as_str(), "verbose_json");
        assert_eq!(ResponseFormat::Srt.as_str(), "srt");
        assert_eq!(ResponseFormat::Vtt.as_str(), "vtt");
    }

    #[test]
    fn test_audio_format_mime_types() {
        assert_eq!(AudioInputFormat::Wav.mime_type(), "audio/wav");
        assert_eq!(AudioInputFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(AudioInputFormat::Mp4.mime_type(), "audio/mp4");
        assert_eq!(AudioInputFormat::Webm.mime_type(), "audio/webm");
    }

    #[test]
    fn test_audio_format_extensions() {
        assert_eq!(AudioInputFormat::Wav.extension(), "wav");
        assert_eq!(AudioInputFormat::Mp3.extension(), "mp3");
        assert_eq!(AudioInputFormat::M4a.extension(), "m4a");
    }

    #[test]
    fn test_config_validation_valid() {
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "sk-test-key".to_string(),
                ..Default::default()
            },
            temperature: Some(0.5),
            ..Default::default()
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_empty_api_key() {
        let config = OpenAISTTConfig {
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
    fn test_config_validation_temperature_range() {
        // Valid temperature at boundary
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            temperature: Some(0.0),
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        // Valid temperature at upper boundary
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            temperature: Some(1.0),
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        // Invalid temperature above range
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            temperature: Some(1.5),
            ..Default::default()
        };
        assert!(config.validate().is_err());

        // Invalid temperature below range
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            temperature: Some(-0.1),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_threshold_exceeds_max() {
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            flush_threshold_bytes: 30 * 1024 * 1024, // 30MB
            max_file_size_bytes: 25 * 1024 * 1024,   // 25MB
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceed"));
    }

    #[test]
    fn test_config_from_base() {
        let base = STTConfig {
            api_key: "test".to_string(),
            model: "gpt-4o-transcribe".to_string(),
            language: "es".to_string(),
            sample_rate: 44100,
            ..Default::default()
        };

        let config = OpenAISTTConfig::from_base(base);

        assert_eq!(config.model, OpenAISTTModel::Gpt4oTranscribe);
        assert_eq!(config.base.language, "es");
        assert_eq!(config.base.sample_rate, 44100);
    }

    #[test]
    fn test_config_api_url() {
        let config = OpenAISTTConfig::default();
        assert_eq!(
            config.api_url(),
            "https://api.openai.com/v1/audio/transcriptions"
        );
    }
}

// =============================================================================
// Message/Response Tests
// =============================================================================

mod message_tests {
    use super::*;

    #[test]
    fn test_simple_response_parsing() {
        let json = r#"{"text": "Hello, world!"}"#;
        let response: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello, world!");
    }

    #[test]
    fn test_verbose_response_parsing() {
        let json = r#"{
            "text": "Hello, world!",
            "language": "en",
            "duration": 1.5,
            "segments": [
                {
                    "id": 0,
                    "start": 0.0,
                    "end": 1.5,
                    "text": "Hello, world!",
                    "tokens": [1, 2, 3],
                    "avg_logprob": -0.15,
                    "compression_ratio": 1.2,
                    "no_speech_prob": 0.01
                }
            ],
            "words": [
                {"word": "Hello,", "start": 0.0, "end": 0.6},
                {"word": "world!", "start": 0.7, "end": 1.5}
            ]
        }"#;

        let response: VerboseTranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello, world!");
        assert_eq!(response.language, Some("en".to_string()));
        assert_eq!(response.duration, Some(1.5));
        assert_eq!(response.segments.len(), 1);
        assert_eq!(response.words.len(), 2);
    }

    #[test]
    fn test_error_response_parsing() {
        let json = r#"{
            "error": {
                "message": "Invalid API key provided",
                "type": "invalid_request_error",
                "param": null,
                "code": "invalid_api_key"
            }
        }"#;

        let response: OpenAIErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.error.message, "Invalid API key provided");
        assert_eq!(response.error.error_type, "invalid_request_error");
        assert_eq!(response.error.code, Some("invalid_api_key".to_string()));
    }

    #[test]
    fn test_transcription_result_text_access() {
        let simple = TranscriptionResult::Simple(TranscriptionResponse {
            text: "Simple text".to_string(),
        });
        assert_eq!(simple.text(), "Simple text");

        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Verbose text".to_string(),
            language: None,
            duration: None,
            segments: vec![],
            words: vec![],
        });
        assert_eq!(verbose.text(), "Verbose text");

        let plain = TranscriptionResult::PlainText("Plain text".to_string());
        assert_eq!(plain.text(), "Plain text");
    }

    #[test]
    fn test_transcription_result_confidence() {
        // Test with good log probability
        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Test".to_string(),
            language: None,
            duration: None,
            segments: vec![TranscriptionSegment {
                id: 0,
                start: 0.0,
                end: 1.0,
                text: "Test".to_string(),
                tokens: vec![],
                avg_logprob: Some(-0.1), // Good quality
                compression_ratio: None,
                no_speech_prob: None,
                temperature: None,
                seek: None,
            }],
            words: vec![],
        });

        let confidence = verbose.confidence();
        assert!(confidence > 0.8);

        // Test without segments (default confidence)
        let simple = TranscriptionResult::Simple(TranscriptionResponse {
            text: "Test".to_string(),
        });
        assert_eq!(simple.confidence(), 1.0);
    }

    #[test]
    fn test_transcription_result_words() {
        let verbose = TranscriptionResult::Verbose(VerboseTranscriptionResponse {
            text: "Hello world".to_string(),
            language: None,
            duration: None,
            segments: vec![],
            words: vec![
                TranscriptionWord {
                    word: "Hello".to_string(),
                    start: 0.0,
                    end: 0.5,
                },
                TranscriptionWord {
                    word: "world".to_string(),
                    start: 0.6,
                    end: 1.0,
                },
            ],
        });

        let words = verbose.words().unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].word, "Hello");
        assert_eq!(words[1].word, "world");

        // Test without words
        let simple = TranscriptionResult::Simple(TranscriptionResponse {
            text: "Test".to_string(),
        });
        assert!(simple.words().is_none());
    }
}

// =============================================================================
// WAV Header Tests
// =============================================================================

mod wav_tests {
    use super::*;

    #[test]
    fn test_wav_header_structure() {
        let header = wav::create_header(1000, 16000, 1, 16);

        // Check RIFF header
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(&header[8..12], b"WAVE");

        // Check fmt chunk
        assert_eq!(&header[12..16], b"fmt ");

        // Check data chunk
        assert_eq!(&header[36..40], b"data");
    }

    #[test]
    fn test_wav_header_sample_rate() {
        let header = wav::create_header(1000, 44100, 2, 16);

        // Sample rate at bytes 24-28 (little-endian)
        let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
        assert_eq!(sample_rate, 44100);
    }

    #[test]
    fn test_wav_header_channels() {
        let header = wav::create_header(1000, 16000, 2, 16);

        // Channels at bytes 22-24 (little-endian)
        let channels = u16::from_le_bytes([header[22], header[23]]);
        assert_eq!(channels, 2);
    }

    #[test]
    fn test_wav_creation() {
        let pcm_data = vec![0u8; 160]; // 5ms at 16kHz 16-bit mono
        let wav = wav::create_wav(&pcm_data, 16000, 1);

        // Total size should be header (44) + data
        assert_eq!(wav.len(), 44 + 160);

        // Check that header is correct
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
    }

    #[test]
    fn test_wav_file_size_field() {
        let pcm_data = vec![0u8; 1000];
        let header = wav::create_header(1000, 16000, 1, 16);

        // File size at bytes 4-8 (should be 36 + data_size)
        let file_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        assert_eq!(file_size, 36 + 1000);
    }
}

// =============================================================================
// Client Lifecycle Tests
// =============================================================================

mod client_tests {
    use super::*;

    #[tokio::test]
    async fn test_client_creation_valid() {
        let config = STTConfig {
            api_key: "sk-test-key".to_string(),
            model: "whisper-1".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        };

        let result = OpenAISTT::new(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), "OpenAI Whisper STT");
    }

    #[tokio::test]
    async fn test_client_creation_empty_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = OpenAISTT::new(config);
        assert!(result.is_err());

        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[tokio::test]
    async fn test_client_connect_disconnect() {
        let config = STTConfig {
            api_key: "sk-test-key".to_string(),
            ..Default::default()
        };

        let mut stt = OpenAISTT::new(config).unwrap();

        // Initially not ready
        assert!(!stt.is_ready());

        // Connect
        stt.connect().await.unwrap();
        assert!(stt.is_ready());

        // Disconnect
        stt.disconnect().await.unwrap();
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_client_double_disconnect() {
        let config = STTConfig {
            api_key: "sk-test-key".to_string(),
            ..Default::default()
        };

        let mut stt = OpenAISTT::new(config).unwrap();
        stt.connect().await.unwrap();
        stt.disconnect().await.unwrap();

        // Second disconnect should be idempotent
        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_audio_without_connect() {
        let config = STTConfig {
            api_key: "sk-test-key".to_string(),
            ..Default::default()
        };

        let mut stt = OpenAISTT::new(config).unwrap();

        let audio: Bytes = vec![0u8; 100].into();
        let result = stt.send_audio(audio).await;

        assert!(result.is_err());
        if let Err(STTError::ConnectionFailed(_)) = result {
            // Expected
        } else {
            panic!("Expected ConnectionFailed error");
        }
    }

    #[tokio::test]
    async fn test_audio_buffering() {
        let config = STTConfig {
            api_key: "sk-test-key".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let mut stt = OpenAISTT::new(config).unwrap();
        stt.connect().await.unwrap();

        // Send first chunk
        let audio1: Bytes = vec![0u8; 500].into();
        stt.send_audio(audio1).await.unwrap();
        assert_eq!(stt.audio_buffer.len(), 500);

        // Send second chunk
        let audio2: Bytes = vec![1u8; 300].into();
        stt.send_audio(audio2).await.unwrap();
        assert_eq!(stt.audio_buffer.len(), 800);

        // Total bytes tracked
        assert_eq!(stt.total_bytes_received, 800);
    }

    #[tokio::test]
    async fn test_callback_registration() {
        let config = STTConfig {
            api_key: "sk-test-key".to_string(),
            ..Default::default()
        };

        let mut stt = OpenAISTT::new(config).unwrap();

        // Register result callback
        let callback = Arc::new(|_result: STTResult| {
            Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        let result = stt.on_result(callback).await;
        assert!(result.is_ok());
        assert!(stt.result_callback.lock().await.is_some());
    }

    #[tokio::test]
    async fn test_error_callback_registration() {
        let config = STTConfig {
            api_key: "sk-test-key".to_string(),
            ..Default::default()
        };

        let mut stt = OpenAISTT::new(config).unwrap();

        // Register error callback
        let callback = Arc::new(|_error: STTError| {
            Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        let result = stt.on_error(callback).await;
        assert!(result.is_ok());
        assert!(stt.error_callback.lock().await.is_some());
    }

    #[tokio::test]
    async fn test_update_config() {
        let config = STTConfig {
            api_key: "sk-test-key".to_string(),
            language: "en".to_string(),
            ..Default::default()
        };

        let mut stt = OpenAISTT::new(config).unwrap();
        stt.connect().await.unwrap();

        // Update config
        let new_config = STTConfig {
            api_key: "sk-test-key".to_string(),
            language: "es".to_string(),
            model: "gpt-4o-transcribe".to_string(),
            ..Default::default()
        };

        stt.update_config(new_config).await.unwrap();

        // Verify new config
        let stored_config = stt.config.as_ref().unwrap();
        assert_eq!(stored_config.base.language, "es");
        assert_eq!(stored_config.model, OpenAISTTModel::Gpt4oTranscribe);
        assert!(stt.is_ready());
    }

    #[tokio::test]
    async fn test_with_provider_config() {
        let openai_config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "sk-test-key".to_string(),
                language: "fr".to_string(),
                ..Default::default()
            },
            model: OpenAISTTModel::Gpt4oMiniTranscribe,
            response_format: ResponseFormat::VerboseJson,
            temperature: Some(0.3),
            ..Default::default()
        };

        let stt = OpenAISTT::with_config(openai_config).unwrap();

        let config = stt.config.as_ref().unwrap();
        assert_eq!(config.model, OpenAISTTModel::Gpt4oMiniTranscribe);
        assert_eq!(config.response_format, ResponseFormat::VerboseJson);
        assert_eq!(config.temperature, Some(0.3));
        assert_eq!(config.base.language, "fr");
    }
}

// =============================================================================
// Flush Strategy Tests
// =============================================================================

mod flush_tests {
    use super::*;

    #[test]
    fn test_flush_on_disconnect_strategy() {
        let mut stt = OpenAISTT::default();
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            flush_strategy: FlushStrategy::OnDisconnect,
            flush_threshold_bytes: 100,
            ..Default::default()
        };
        stt.config = Some(config);

        // Even with lots of data, should not flush
        stt.audio_buffer = vec![0u8; 1000];
        assert!(!stt.should_flush(None));
    }

    #[test]
    fn test_flush_on_threshold_below() {
        let mut stt = OpenAISTT::default();
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            flush_strategy: FlushStrategy::OnThreshold,
            flush_threshold_bytes: 500,
            ..Default::default()
        };
        stt.config = Some(config);

        // Below threshold
        stt.audio_buffer = vec![0u8; 400];
        assert!(!stt.should_flush(None));
    }

    #[test]
    fn test_flush_on_threshold_at() {
        let mut stt = OpenAISTT::default();
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            flush_strategy: FlushStrategy::OnThreshold,
            flush_threshold_bytes: 500,
            ..Default::default()
        };
        stt.config = Some(config);

        // At threshold
        stt.audio_buffer = vec![0u8; 500];
        assert!(stt.should_flush(None));
    }

    #[test]
    fn test_flush_on_threshold_above() {
        let mut stt = OpenAISTT::default();
        let config = OpenAISTTConfig {
            base: STTConfig {
                api_key: "test".to_string(),
                ..Default::default()
            },
            flush_strategy: FlushStrategy::OnThreshold,
            flush_threshold_bytes: 500,
            ..Default::default()
        };
        stt.config = Some(config);

        // Above threshold
        stt.audio_buffer = vec![0u8; 600];
        assert!(stt.should_flush(None));
    }

    #[test]
    fn test_flush_no_config() {
        let mut stt = OpenAISTT::default();
        // Without config, should never flush
        assert!(!stt.should_flush(None));
    }
}
