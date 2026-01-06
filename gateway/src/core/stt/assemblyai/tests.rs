//! Tests for AssemblyAI STT implementation.
//!
//! This module contains comprehensive unit tests for:
//! - Configuration handling
//! - Message parsing
//! - Client state management
//! - Error handling

use super::*;
use crate::core::stt::base::{BaseSTT, STTConfig, STTError};
use bytes::Bytes;

// =============================================================================
// Configuration Tests
// =============================================================================

mod config_tests {
    use super::*;

    #[test]
    fn test_encoding_roundtrip() {
        // Test all encoding variants
        assert_eq!(
            AssemblyAIEncoding::PcmS16le
                .as_str()
                .parse::<AssemblyAIEncoding>()
                .unwrap(),
            AssemblyAIEncoding::PcmS16le
        );
        assert_eq!(
            AssemblyAIEncoding::PcmMulaw
                .as_str()
                .parse::<AssemblyAIEncoding>()
                .unwrap(),
            AssemblyAIEncoding::PcmMulaw
        );
    }

    #[test]
    fn test_encoding_aliases() {
        // Test common aliases
        assert_eq!(
            "mulaw".parse::<AssemblyAIEncoding>().unwrap(),
            AssemblyAIEncoding::PcmMulaw
        );
        assert_eq!(
            "ulaw".parse::<AssemblyAIEncoding>().unwrap(),
            AssemblyAIEncoding::PcmMulaw
        );
        assert_eq!(
            "MULAW".parse::<AssemblyAIEncoding>().unwrap(),
            AssemblyAIEncoding::PcmMulaw
        );
    }

    #[test]
    fn test_speech_model_roundtrip() {
        assert_eq!(
            AssemblyAISpeechModel::UniversalStreamingEnglish
                .as_str()
                .parse::<AssemblyAISpeechModel>()
                .unwrap(),
            AssemblyAISpeechModel::UniversalStreamingEnglish
        );
        assert_eq!(
            AssemblyAISpeechModel::UniversalStreamingMultilingual
                .as_str()
                .parse::<AssemblyAISpeechModel>()
                .unwrap(),
            AssemblyAISpeechModel::UniversalStreamingMultilingual
        );
    }

    #[test]
    fn test_region_urls() {
        // Verify all region URLs are valid
        for region in [AssemblyAIRegion::Default, AssemblyAIRegion::Eu] {
            let url = region.websocket_base_url();
            assert!(url.starts_with("wss://"));
            assert!(url.contains("assemblyai.com"));

            let host = region.host();
            assert!(host.contains("assemblyai.com"));
        }
    }

    #[test]
    fn test_config_from_base_preserves_sample_rate() {
        let base = STTConfig {
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 8000, // Non-default sample rate
            ..Default::default()
        };

        let config = AssemblyAISTTConfig::from_base(base);
        assert_eq!(config.base.sample_rate, 8000);
    }

    #[test]
    fn test_config_url_includes_all_params() {
        let config = AssemblyAISTTConfig {
            base: STTConfig {
                sample_rate: 16000,
                ..Default::default()
            },
            speech_model: AssemblyAISpeechModel::UniversalStreamingMultilingual,
            encoding: AssemblyAIEncoding::PcmMulaw,
            format_turns: true,
            end_of_turn_confidence_threshold: Some(0.75),
            region: AssemblyAIRegion::Eu,
            include_word_timestamps: true,
        };

        let url = config.build_websocket_url();

        // Check all expected parameters
        assert!(url.contains("streaming.eu.assemblyai.com"));
        assert!(url.contains("sample_rate=16000"));
        assert!(url.contains("encoding=pcm_mulaw"));
        assert!(url.contains("speech_model=universal-streaming-multilingual"));
        assert!(url.contains("format_turns=true"));
        assert!(url.contains("end_of_turn_confidence_threshold=0.75"));
    }

    #[test]
    fn test_config_url_omits_none_threshold() {
        let config = AssemblyAISTTConfig {
            base: STTConfig {
                sample_rate: 16000,
                ..Default::default()
            },
            end_of_turn_confidence_threshold: None,
            ..Default::default()
        };

        let url = config.build_websocket_url();
        assert!(!url.contains("end_of_turn_confidence_threshold"));
    }
}

// =============================================================================
// Message Tests
// =============================================================================

mod message_tests {
    use super::*;

    #[test]
    fn test_parse_begin_with_all_fields() {
        let json = r#"{
            "type": "Begin",
            "id": "session-abc-123",
            "expires_at": 1704067200
        }"#;

        let msg = AssemblyAIMessage::parse(json).unwrap();
        match msg {
            AssemblyAIMessage::Begin(begin) => {
                assert_eq!(begin.id, "session-abc-123");
                assert_eq!(begin.expires_at, 1704067200);
            }
            _ => panic!("Expected Begin message"),
        }
    }

    #[test]
    fn test_parse_turn_partial() {
        // Turn with end_of_turn=false (partial/interim result)
        let json = r#"{
            "type": "Turn",
            "turn_order": 0,
            "transcript": "Hello",
            "end_of_turn": false,
            "words": [{"start": 0, "end": 400, "confidence": 0.9, "text": "Hello"}]
        }"#;

        let msg = AssemblyAIMessage::parse(json).unwrap();

        // Check is_final_transcript first before pattern matching (which moves)
        assert!(!msg.is_final_transcript());

        match msg {
            AssemblyAIMessage::Turn(turn) => {
                assert!(!turn.end_of_turn);
                assert_eq!(turn.transcript, "Hello");
            }
            _ => panic!("Expected Turn message"),
        }
    }

    #[test]
    fn test_parse_turn_final() {
        // Turn with end_of_turn=true (final result)
        let json = r#"{
            "type": "Turn",
            "turn_order": 0,
            "transcript": "Hello world",
            "end_of_turn": true,
            "words": []
        }"#;

        let msg = AssemblyAIMessage::parse(json).unwrap();
        assert!(msg.is_final_transcript());
    }

    #[test]
    fn test_parse_turn_with_language_detection() {
        let json = r#"{
            "type": "Turn",
            "turn_order": 0,
            "transcript": "Bonjour",
            "end_of_turn": true,
            "words": [],
            "language": "fr",
            "language_confidence": 0.95
        }"#;

        let msg = AssemblyAIMessage::parse(json).unwrap();
        match msg {
            AssemblyAIMessage::Turn(turn) => {
                assert_eq!(turn.language, Some("fr".to_string()));
                assert!((turn.language_confidence.unwrap() - 0.95).abs() < 0.001);
            }
            _ => panic!("Expected Turn message"),
        }
    }

    #[test]
    fn test_parse_termination_normal() {
        let json = r#"{
            "type": "Termination",
            "audio_duration_ms": 10000,
            "terminated_normally": true
        }"#;

        let msg = AssemblyAIMessage::parse(json).unwrap();
        assert!(msg.is_termination());

        match msg {
            AssemblyAIMessage::Termination(term) => {
                assert_eq!(term.audio_duration_ms, 10000);
                assert!(term.terminated_normally);
            }
            _ => panic!("Expected Termination message"),
        }
    }

    #[test]
    fn test_parse_error_with_code() {
        let json = r#"{
            "type": "Error",
            "error_code": "invalid_audio",
            "error": "Audio format not supported"
        }"#;

        let msg = AssemblyAIMessage::parse(json).unwrap();
        assert!(msg.is_error());

        match msg {
            AssemblyAIMessage::Error(err) => {
                assert_eq!(err.error_code, Some("invalid_audio".to_string()));
                assert_eq!(err.error, "Audio format not supported");
            }
            _ => panic!("Expected Error message"),
        }
    }

    #[test]
    fn test_parse_error_without_code() {
        let json = r#"{
            "type": "Error",
            "error": "Unknown error occurred"
        }"#;

        let msg = AssemblyAIMessage::parse(json).unwrap();
        match msg {
            AssemblyAIMessage::Error(err) => {
                assert!(err.error_code.is_none());
                assert_eq!(err.error, "Unknown error occurred");
            }
            _ => panic!("Expected Error message"),
        }
    }

    #[test]
    fn test_serialize_terminate() {
        let msg = TerminateMessage::default();
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"Terminate"}"#);
    }

    #[test]
    fn test_serialize_force_endpoint() {
        let msg = ForceEndpointMessage::default();
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"ForceEndpoint"}"#);
    }

    #[test]
    fn test_serialize_update_configuration_with_threshold() {
        let msg = UpdateConfigurationMessage::new(Some(0.6));
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"UpdateConfiguration\""));
        assert!(json.contains("\"end_of_turn_confidence_threshold\":0.6"));
    }

    #[test]
    fn test_serialize_update_configuration_without_threshold() {
        let msg = UpdateConfigurationMessage::new(None);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"UpdateConfiguration\""));
        assert!(!json.contains("end_of_turn_confidence_threshold"));
    }

    #[test]
    fn test_word_timing_parse() {
        let json = r#"{"start": 1500, "end": 2000, "confidence": 0.99, "text": "world"}"#;
        let word: Word = serde_json::from_str(json).unwrap();

        assert_eq!(word.start, 1500);
        assert_eq!(word.end, 2000);
        assert!((word.confidence - 0.99).abs() < 0.001);
        assert_eq!(word.text, "world");
    }
}

// =============================================================================
// Client Tests
// =============================================================================

mod client_tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::{RwLock, mpsc};
    use tokio_tungstenite::tungstenite::protocol::Message;

    #[tokio::test]
    async fn test_new_creates_disconnected_client() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let stt = AssemblyAISTT::new(config).unwrap();

        assert!(!stt.is_ready());
        assert!(stt.get_config().is_some());
        assert!(stt.get_session_id().await.is_none());
    }

    #[test]
    fn test_new_rejects_empty_api_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = AssemblyAISTT::new(config);
        assert!(result.is_err());

        match result {
            Err(STTError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("API key is required"));
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[test]
    fn test_new_rejects_sample_rate_below_minimum() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 7999, // Below MIN_SAMPLE_RATE (8000)
            ..Default::default()
        };

        let result = AssemblyAISTT::new(config);
        assert!(result.is_err());

        match result {
            Err(STTError::ConfigurationError(msg)) => {
                assert!(msg.contains("outside supported range"));
                assert!(msg.contains("8000"));
            }
            _ => panic!("Expected ConfigurationError"),
        }
    }

    #[test]
    fn test_new_rejects_sample_rate_above_maximum() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 48001, // Above MAX_SAMPLE_RATE (48000)
            ..Default::default()
        };

        let result = AssemblyAISTT::new(config);
        assert!(result.is_err());

        match result {
            Err(STTError::ConfigurationError(msg)) => {
                assert!(msg.contains("outside supported range"));
                assert!(msg.contains("48000"));
            }
            _ => panic!("Expected ConfigurationError"),
        }
    }

    #[test]
    fn test_new_accepts_minimum_sample_rate() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 8000, // MIN_SAMPLE_RATE
            ..Default::default()
        };

        let result = AssemblyAISTT::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_new_accepts_maximum_sample_rate() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 48000, // MAX_SAMPLE_RATE
            ..Default::default()
        };

        let result = AssemblyAISTT::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_provider_info() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        let stt = AssemblyAISTT::new(config).unwrap();
        assert_eq!(stt.get_provider_info(), "AssemblyAI Streaming STT v3");
    }

    #[test]
    fn test_get_config_returns_base_config() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            language: "fr".to_string(),
            sample_rate: 8000,
            ..Default::default()
        };

        let stt = AssemblyAISTT::new(config.clone()).unwrap();
        let retrieved = stt.get_config().unwrap();

        assert_eq!(retrieved.language, "fr");
        assert_eq!(retrieved.sample_rate, 8000);
    }

    #[tokio::test]
    async fn test_send_audio_fails_when_not_connected() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        let mut stt = AssemblyAISTT::new(config).unwrap();
        let audio = Bytes::from(vec![0u8; 1024]);

        let result = stt.send_audio(audio).await;
        assert!(result.is_err());

        match result {
            Err(STTError::ConnectionFailed(msg)) => {
                assert!(msg.contains("Not connected"));
            }
            _ => panic!("Expected ConnectionFailed error"),
        }
    }

    #[tokio::test]
    async fn test_force_endpoint_fails_when_not_connected() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        let stt = AssemblyAISTT::new(config).unwrap();

        let result = stt.force_endpoint().await;
        assert!(result.is_err());

        match result {
            Err(STTError::ConnectionFailed(msg)) => {
                assert!(msg.contains("Not connected") || msg.contains("no control channel"));
            }
            _ => panic!("Expected ConnectionFailed error"),
        }
    }

    #[tokio::test]
    async fn test_handle_message_begin_sets_session_id() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let session_id = Arc::new(RwLock::new(None));

        let msg =
            Message::Text(r#"{"type":"Begin","id":"session-xyz","expires_at":1704067200}"#.into());

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

        assert!(result.is_ok());
        assert!(result.unwrap()); // Should continue
        assert_eq!(*session_id.read().await, Some("session-xyz".to_string()));
    }

    #[tokio::test]
    async fn test_handle_message_turn_sends_result() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Text(
            r#"{"type":"Turn","turn_order":0,"transcript":"Test transcript","end_of_turn":true,"words":[{"start":0,"end":1000,"confidence":0.95,"text":"Test"},{"start":1000,"end":2000,"confidence":0.92,"text":"transcript"}]}"#.into(),
        );

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

        assert!(result.is_ok());
        assert!(result.unwrap()); // Should continue

        let stt_result = rx.try_recv().unwrap();
        assert_eq!(stt_result.transcript, "Test transcript");
        assert!(stt_result.is_final);
        assert!(stt_result.is_speech_final);
        // Average of 0.95 and 0.92
        assert!(stt_result.confidence > 0.9);
    }

    #[tokio::test]
    async fn test_handle_message_termination_returns_false() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Text(
            r#"{"type":"Termination","audio_duration_ms":5000,"terminated_normally":true}"#.into(),
        );

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

        assert!(result.is_ok());
        assert!(!result.unwrap()); // Should NOT continue
    }

    #[tokio::test]
    async fn test_handle_message_auth_error() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Text(
            r#"{"type":"Error","error_code":"invalid_api_key","error":"Invalid API key"}"#.into(),
        );

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

        assert!(result.is_err());
        match result {
            Err(STTError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("Invalid API key"));
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_handle_message_rate_limit_error() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Text(
            r#"{"type":"Error","error_code":"rate_limit_exceeded","error":"Too many requests"}"#
                .into(),
        );

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

        assert!(result.is_err());
        match result {
            Err(STTError::ProviderError(msg)) => {
                assert!(msg.contains("Rate limit"));
            }
            _ => panic!("Expected ProviderError"),
        }
    }

    #[tokio::test]
    async fn test_handle_message_audio_error() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Text(
            r#"{"type":"Error","error_code":"invalid_audio","error":"Invalid audio format"}"#
                .into(),
        );

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

        assert!(result.is_err());
        match result {
            Err(STTError::InvalidAudioFormat(msg)) => {
                assert!(msg.contains("Invalid audio format"));
            }
            _ => panic!("Expected InvalidAudioFormat error"),
        }
    }

    #[tokio::test]
    async fn test_handle_message_close() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Close(None);

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

        assert!(result.is_ok());
        assert!(!result.unwrap()); // Should NOT continue
    }

    #[tokio::test]
    async fn test_handle_message_ping_continues() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Ping(vec![].into());

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

        assert!(result.is_ok());
        assert!(result.unwrap()); // Should continue
    }

    #[tokio::test]
    async fn test_handle_message_pong_continues() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Pong(vec![].into());

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

        assert!(result.is_ok());
        assert!(result.unwrap()); // Should continue
    }

    #[tokio::test]
    async fn test_handle_message_unknown_type_continues() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Text(r#"{"type":"FutureType","data":"value"}"#.into());

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

        assert!(result.is_ok());
        assert!(result.unwrap()); // Should continue (forward compatibility)
    }

    #[tokio::test]
    async fn test_handle_message_malformed_json() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Text("not valid json".into());

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

        // Should not error, just log warning and continue
        assert!(result.is_ok());
        assert!(result.unwrap()); // Should continue
    }
}

// =============================================================================
// Integration Tests (require mocking or real API)
// =============================================================================

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[tokio::test]
    async fn test_full_lifecycle_disconnected() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let mut stt = AssemblyAISTT::new(config).unwrap();

        // Initial state
        assert!(!stt.is_ready());
        assert!(stt.get_session_id().await.is_none());

        // Disconnect when not connected should succeed
        let result = stt.disconnect().await;
        assert!(result.is_ok());
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_callback_registration() {
        use std::sync::Arc;

        let config = STTConfig {
            api_key: "test_key".to_string(),
            ..Default::default()
        };

        let mut stt = AssemblyAISTT::new(config).unwrap();

        // Register result callback
        let result_callback = Arc::new(|_result: crate::core::stt::base::STTResult| {
            Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        let result = stt.on_result(result_callback).await;
        assert!(result.is_ok());

        // Register error callback
        let error_callback = Arc::new(|_error: STTError| {
            Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        let result = stt.on_error(error_callback).await;
        assert!(result.is_ok());
    }
}
