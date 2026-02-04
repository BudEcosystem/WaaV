//! Integration tests for AmiVoice STT provider.
//!
//! These tests verify the functionality of the AmiVoice Cloud Platform
//! Speech-to-Text integration.

use super::*;
use crate::core::stt::base::{BaseSTT, STTConfig, STTError, STTResult};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Mutex;

// =============================================================================
// Test Helpers
// =============================================================================

fn make_test_config() -> STTConfig {
    STTConfig {
        api_key: "test_appkey_12345".to_string(),
        language: "ja".to_string(),
        sample_rate: 16000,
        model: "-a-general".to_string(),
        ..Default::default()
    }
}

fn make_config_with_model(model: &str) -> STTConfig {
    STTConfig {
        api_key: "test_appkey".to_string(),
        model: model.to_string(),
        ..make_test_config()
    }
}

/// Mock callback that tracks invocations.
struct MockResultCallback {
    call_count: AtomicUsize,
    results: Mutex<Vec<STTResult>>,
    is_final_received: AtomicBool,
}

impl MockResultCallback {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            call_count: AtomicUsize::new(0),
            results: Mutex::new(Vec::new()),
            is_final_received: AtomicBool::new(false),
        })
    }

    #[allow(dead_code)]
    fn get_call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    fn received_final(&self) -> bool {
        self.is_final_received.load(Ordering::SeqCst)
    }
}

// =============================================================================
// Configuration Tests
// =============================================================================

#[test]
fn test_config_default_values() {
    let config = AmiVoiceSTTConfig::default();

    assert!(config.app_key.is_empty());
    assert_eq!(config.engine, AmiVoiceEngine::HybridJapaneseGeneral);
    assert_eq!(config.audio_format, AmiVoiceAudioFormat::Pcm16kHz);
    assert!(!config.no_logging);
    assert!(config.interim_results);
    assert_eq!(config.result_updated_interval, 1000);
}

#[test]
fn test_config_from_base() {
    let base = STTConfig {
        api_key: "my_appkey".to_string(),
        sample_rate: 8000,
        model: "-a-medical".to_string(),
        ..Default::default()
    };

    let config = AmiVoiceSTTConfig::from_base(base);

    assert_eq!(config.app_key, "my_appkey");
    assert_eq!(config.engine, AmiVoiceEngine::HybridJapaneseMedical);
    assert_eq!(config.audio_format, AmiVoiceAudioFormat::Pcm8kHz);
}

#[test]
fn test_config_validation_empty_appkey() {
    let config = AmiVoiceSTTConfig::default();
    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("APPKEY"));
}

#[test]
fn test_config_validation_invalid_interval() {
    let mut config = AmiVoiceSTTConfig::default();
    config.app_key = "test_key".to_string();
    config.result_updated_interval = 50; // Less than 100ms

    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("100ms"));
}

#[test]
fn test_config_validation_success() {
    let mut config = AmiVoiceSTTConfig::default();
    config.app_key = "test_key".to_string();

    let result = config.validate();
    assert!(result.is_ok());
}

// =============================================================================
// Engine Tests
// =============================================================================

#[test]
fn test_engine_ids() {
    assert_eq!(
        AmiVoiceEngine::HybridJapaneseGeneral.engine_id(),
        "-a-general"
    );
    assert_eq!(
        AmiVoiceEngine::HybridJapaneseMedical.engine_id(),
        "-a-medical"
    );
    assert_eq!(
        AmiVoiceEngine::E2EJapaneseGeneral.engine_id(),
        "-a2-ja-general"
    );
    assert_eq!(
        AmiVoiceEngine::E2EMultilingual.engine_id(),
        "-a2-multi-general"
    );
    assert_eq!(
        AmiVoiceEngine::HybridEnglishGeneral.engine_id(),
        "-a-general-en"
    );
    assert_eq!(
        AmiVoiceEngine::HybridKoreanGeneral.engine_id(),
        "-a-general-ko"
    );
}

#[test]
fn test_engine_display_names() {
    assert!(
        !AmiVoiceEngine::HybridJapaneseGeneral
            .display_name()
            .is_empty()
    );
    assert!(
        AmiVoiceEngine::HybridJapaneseMedical
            .display_name()
            .contains("Medical")
    );
    assert!(
        AmiVoiceEngine::E2EJapaneseGeneral
            .display_name()
            .contains("E2E")
    );
}

#[test]
fn test_engine_word_registration_support() {
    // Hybrid engines support word registration
    assert!(AmiVoiceEngine::HybridJapaneseGeneral.supports_word_registration());
    assert!(AmiVoiceEngine::HybridJapaneseMedical.supports_word_registration());
    assert!(AmiVoiceEngine::HybridEnglishGeneral.supports_word_registration());

    // E2E engines do NOT support word registration
    assert!(!AmiVoiceEngine::E2EJapaneseGeneral.supports_word_registration());
    assert!(!AmiVoiceEngine::E2EMultilingual.supports_word_registration());
    assert!(!AmiVoiceEngine::E2EBatchJapaneseGeneral.supports_word_registration());
}

#[test]
fn test_engine_batch_detection() {
    assert!(!AmiVoiceEngine::HybridJapaneseGeneral.is_batch_engine());
    assert!(!AmiVoiceEngine::E2EJapaneseGeneral.is_batch_engine());

    assert!(AmiVoiceEngine::E2EBatchJapaneseGeneral.is_batch_engine());
    assert!(AmiVoiceEngine::E2EBatchChineseGeneral.is_batch_engine());
    assert!(AmiVoiceEngine::E2EBatchMultilingual.is_batch_engine());
}

#[test]
fn test_engine_primary_language() {
    assert_eq!(
        AmiVoiceEngine::HybridJapaneseGeneral.primary_language(),
        "ja"
    );
    assert_eq!(
        AmiVoiceEngine::HybridEnglishGeneral.primary_language(),
        "en"
    );
    assert_eq!(
        AmiVoiceEngine::HybridChineseGeneral.primary_language(),
        "zh"
    );
    assert_eq!(AmiVoiceEngine::HybridKoreanGeneral.primary_language(), "ko");
    assert_eq!(AmiVoiceEngine::E2EMultilingual.primary_language(), "multi");
}

#[test]
fn test_engine_from_str_relaxed() {
    // Exact engine IDs
    assert_eq!(
        AmiVoiceEngine::from_str_relaxed("-a-general"),
        Some(AmiVoiceEngine::HybridJapaneseGeneral)
    );
    assert_eq!(
        AmiVoiceEngine::from_str_relaxed("-a2-ja-general"),
        Some(AmiVoiceEngine::E2EJapaneseGeneral)
    );

    // Friendly names
    assert_eq!(
        AmiVoiceEngine::from_str_relaxed("general"),
        Some(AmiVoiceEngine::HybridJapaneseGeneral)
    );
    assert_eq!(
        AmiVoiceEngine::from_str_relaxed("medical"),
        Some(AmiVoiceEngine::HybridJapaneseMedical)
    );
    assert_eq!(
        AmiVoiceEngine::from_str_relaxed("e2e-ja"),
        Some(AmiVoiceEngine::E2EJapaneseGeneral)
    );

    // Case insensitive
    assert_eq!(
        AmiVoiceEngine::from_str_relaxed("GENERAL"),
        Some(AmiVoiceEngine::HybridJapaneseGeneral)
    );

    // Invalid
    assert_eq!(AmiVoiceEngine::from_str_relaxed("invalid"), None);
}

#[test]
fn test_all_engines() {
    let engines = AmiVoiceEngine::all();

    // Should have all 19 engines
    assert_eq!(engines.len(), 19);

    // Check presence of key engines
    assert!(engines.contains(&AmiVoiceEngine::HybridJapaneseGeneral));
    assert!(engines.contains(&AmiVoiceEngine::E2EJapaneseGeneral));
    assert!(engines.contains(&AmiVoiceEngine::E2EMultilingual));
    assert!(engines.contains(&AmiVoiceEngine::HybridJapaneseMedical));
}

// =============================================================================
// Audio Format Tests
// =============================================================================

#[test]
fn test_audio_format_codes() {
    assert_eq!(AmiVoiceAudioFormat::Pcm16kHz.format_code(), "16K");
    assert_eq!(AmiVoiceAudioFormat::Pcm8kHz.format_code(), "8K");
    assert_eq!(AmiVoiceAudioFormat::Lsb16kHz.format_code(), "LSB16K");
    assert_eq!(AmiVoiceAudioFormat::Lsb8kHz.format_code(), "LSB8K");
}

#[test]
fn test_audio_format_sample_rates() {
    assert_eq!(AmiVoiceAudioFormat::Pcm16kHz.sample_rate(), 16000);
    assert_eq!(AmiVoiceAudioFormat::Pcm8kHz.sample_rate(), 8000);
    assert_eq!(AmiVoiceAudioFormat::Lsb16kHz.sample_rate(), 16000);
    assert_eq!(AmiVoiceAudioFormat::Lsb8kHz.sample_rate(), 8000);
}

#[test]
fn test_audio_format_from_sample_rate() {
    assert_eq!(
        AmiVoiceAudioFormat::from_sample_rate(16000),
        AmiVoiceAudioFormat::Pcm16kHz
    );
    assert_eq!(
        AmiVoiceAudioFormat::from_sample_rate(8000),
        AmiVoiceAudioFormat::Pcm8kHz
    );
    // Higher rates default to 16K
    assert_eq!(
        AmiVoiceAudioFormat::from_sample_rate(44100),
        AmiVoiceAudioFormat::Pcm16kHz
    );
    // Lower rates default to 8K
    assert_eq!(
        AmiVoiceAudioFormat::from_sample_rate(4000),
        AmiVoiceAudioFormat::Pcm8kHz
    );
}

// =============================================================================
// URL Tests
// =============================================================================

#[test]
fn test_websocket_url_with_logging() {
    let config = AmiVoiceSTTConfig {
        no_logging: false,
        ..Default::default()
    };

    assert_eq!(config.get_websocket_url(), AMIVOICE_WS_URL);
    assert!(!config.get_websocket_url().contains("nolog"));
}

#[test]
fn test_websocket_url_without_logging() {
    let config = AmiVoiceSTTConfig {
        no_logging: true,
        ..Default::default()
    };

    assert_eq!(config.get_websocket_url(), AMIVOICE_WS_NOLOG_URL);
    assert!(config.get_websocket_url().contains("nolog"));
}

#[test]
fn test_http_url_with_logging() {
    let config = AmiVoiceSTTConfig {
        no_logging: false,
        ..Default::default()
    };

    assert_eq!(config.get_http_url(), AMIVOICE_HTTP_URL);
}

#[test]
fn test_http_url_without_logging() {
    let config = AmiVoiceSTTConfig {
        no_logging: true,
        ..Default::default()
    };

    assert_eq!(config.get_http_url(), AMIVOICE_HTTP_NOLOG_URL);
}

// =============================================================================
// Start Command Tests
// =============================================================================

#[test]
fn test_build_start_command_basic() {
    let config = AmiVoiceSTTConfig {
        app_key: "TEST_KEY".to_string(),
        engine: AmiVoiceEngine::HybridJapaneseGeneral,
        audio_format: AmiVoiceAudioFormat::Pcm16kHz,
        interim_results: false,
        ..Default::default()
    };

    let cmd = config.build_start_command();

    assert!(cmd.starts_with("s "));
    assert!(cmd.contains("16K"));
    assert!(cmd.contains("-a-general"));
    assert!(cmd.contains("authorization=TEST_KEY"));
}

#[test]
fn test_build_start_command_with_interim() {
    let config = AmiVoiceSTTConfig {
        app_key: "TEST_KEY".to_string(),
        interim_results: true,
        result_updated_interval: 500,
        ..Default::default()
    };

    let cmd = config.build_start_command();

    assert!(cmd.contains("resultUpdatedInterval=500"));
}

#[test]
fn test_build_start_command_with_diarization() {
    let config = AmiVoiceSTTConfig {
        app_key: "TEST_KEY".to_string(),
        enable_diarization: true,
        ..Default::default()
    };

    let cmd = config.build_start_command();

    assert!(cmd.contains("segmenterProperties=\"useDiarizer=1\""));
}

#[test]
fn test_build_start_command_with_profile() {
    let config = AmiVoiceSTTConfig {
        app_key: "TEST_KEY".to_string(),
        profile_id: Some("user123".to_string()),
        ..Default::default()
    };

    let cmd = config.build_start_command();

    assert!(cmd.contains("profileId=user123"));
}

// =============================================================================
// Message Parsing Tests
// =============================================================================

#[test]
fn test_parse_session_start_ok() {
    let msg = AmiVoiceMessage::parse("s").unwrap();
    assert!(matches!(msg, AmiVoiceMessage::SessionStartOk));
    assert!(!msg.is_error());
}

#[test]
fn test_parse_session_start_error() {
    let msg = AmiVoiceMessage::parse("s Invalid APPKEY").unwrap();

    match &msg {
        AmiVoiceMessage::SessionStartError(e) => {
            assert_eq!(e, "Invalid APPKEY");
        }
        _ => panic!("Expected SessionStartError"),
    }

    assert!(msg.is_error());
    assert_eq!(msg.error_message(), Some("Invalid APPKEY"));
}

#[test]
fn test_parse_speech_events() {
    let start_msg = AmiVoiceMessage::parse("S 1000").unwrap();
    match start_msg {
        AmiVoiceMessage::SpeechStart { timestamp_ms } => {
            assert_eq!(timestamp_ms, 1000);
        }
        _ => panic!("Expected SpeechStart"),
    }

    let end_msg = AmiVoiceMessage::parse("E 2500").unwrap();
    match end_msg {
        AmiVoiceMessage::SpeechEnd { timestamp_ms } => {
            assert_eq!(timestamp_ms, 2500);
        }
        _ => panic!("Expected SpeechEnd"),
    }
}

#[test]
fn test_parse_processing_start() {
    let msg = AmiVoiceMessage::parse("C").unwrap();
    assert!(matches!(msg, AmiVoiceMessage::ProcessingStart));
}

#[test]
fn test_parse_interim_result() {
    let json =
        r#"{"results":[{"text":"テスト","tokens":[]}],"text":"テスト","code":"0","message":""}"#;
    let msg = AmiVoiceMessage::parse(&format!("U {}", json)).unwrap();

    match msg {
        AmiVoiceMessage::InterimResult(result) => {
            assert_eq!(result.text, "テスト");
            assert!(result.is_success());
        }
        _ => panic!("Expected InterimResult"),
    }
}

#[test]
fn test_parse_final_result() {
    let json = r#"{"results":[{"text":"認識結果","tokens":[{"written":"認識","confidence":0.95,"starttime":100,"endtime":300},{"written":"結果","confidence":0.92,"starttime":300,"endtime":500}]}],"text":"認識結果","code":"0","message":"success"}"#;
    let msg = AmiVoiceMessage::parse(&format!("A {}", json)).unwrap();

    match msg {
        AmiVoiceMessage::FinalResult(result) => {
            assert_eq!(result.text, "認識結果");
            assert_eq!(result.results.len(), 1);
            assert_eq!(result.results[0].tokens.len(), 2);
        }
        _ => panic!("Expected FinalResult"),
    }
}

#[test]
fn test_parse_server_info() {
    let msg = AmiVoiceMessage::parse("G Server status: OK").unwrap();

    match msg {
        AmiVoiceMessage::ServerInfo(info) => {
            assert_eq!(info, "Server status: OK");
        }
        _ => panic!("Expected ServerInfo"),
    }
}

#[test]
fn test_parse_session_end() {
    let ok_msg = AmiVoiceMessage::parse("e").unwrap();
    assert!(matches!(ok_msg, AmiVoiceMessage::SessionEndOk));

    let err_msg = AmiVoiceMessage::parse("e Timeout").unwrap();
    match err_msg {
        AmiVoiceMessage::SessionEndError(e) => {
            assert_eq!(e, "Timeout");
        }
        _ => panic!("Expected SessionEndError"),
    }
}

#[test]
fn test_parse_unknown_message() {
    let result = AmiVoiceMessage::parse("X unknown");
    assert!(result.is_err());
}

#[test]
fn test_parse_empty_message() {
    let result = AmiVoiceMessage::parse("");
    assert!(result.is_err());
}

// =============================================================================
// Result Conversion Tests
// =============================================================================

#[test]
fn test_result_to_stt_result() {
    let result = AmiVoiceResult {
        results: vec![AmiVoiceRecognitionResult {
            text: "こんにちは".to_string(),
            tokens: vec![AmiVoiceToken {
                written: "こんにちは".to_string(),
                confidence: Some(0.95),
                starttime: Some(100),
                endtime: Some(500),
                spoken: Some("こんにちは".to_string()),
            }],
            tags: vec![],
            rulename: String::new(),
        }],
        text: "こんにちは".to_string(),
        code: "0".to_string(),
        message: "success".to_string(),
    };

    let stt_result = result.to_stt_result(true);

    assert_eq!(stt_result.transcript, "こんにちは");
    assert!(stt_result.is_final);
    assert!(stt_result.is_speech_final);
    assert!((stt_result.confidence - 0.95).abs() < 0.001);

    // Word timings are available through AmiVoiceResult directly
    let word_timings = result.get_word_timings();
    assert_eq!(word_timings.len(), 1);
    assert_eq!(word_timings[0].0, "こんにちは");
    assert_eq!(word_timings[0].1, Some(100));
    assert_eq!(word_timings[0].2, Some(500));
}

#[test]
fn test_result_confidence_calculation() {
    let result = AmiVoiceResult {
        results: vec![AmiVoiceRecognitionResult {
            text: "test".to_string(),
            tokens: vec![
                AmiVoiceToken {
                    written: "a".to_string(),
                    confidence: Some(0.9),
                    starttime: None,
                    endtime: None,
                    spoken: None,
                },
                AmiVoiceToken {
                    written: "b".to_string(),
                    confidence: Some(0.8),
                    starttime: None,
                    endtime: None,
                    spoken: None,
                },
                AmiVoiceToken {
                    written: "c".to_string(),
                    confidence: Some(0.7),
                    starttime: None,
                    endtime: None,
                    spoken: None,
                },
            ],
            tags: vec![],
            rulename: String::new(),
        }],
        text: "abc".to_string(),
        code: "0".to_string(),
        message: "".to_string(),
    };

    let stt_result = result.to_stt_result(false);

    // Average of 0.9, 0.8, 0.7 = 0.8
    assert!((stt_result.confidence - 0.8).abs() < 0.001);
}

#[test]
fn test_result_is_success() {
    let success = AmiVoiceResult {
        code: "0".to_string(),
        ..Default::default()
    };
    assert!(success.is_success());

    let empty_code = AmiVoiceResult {
        code: String::new(),
        ..Default::default()
    };
    assert!(empty_code.is_success());

    let error = AmiVoiceResult {
        code: "-1".to_string(),
        ..Default::default()
    };
    assert!(!error.is_success());
}

#[test]
fn test_result_get_timing() {
    let result = AmiVoiceResult {
        results: vec![AmiVoiceRecognitionResult {
            text: "test".to_string(),
            tokens: vec![
                AmiVoiceToken {
                    written: "a".to_string(),
                    confidence: Some(0.9),
                    starttime: Some(100),
                    endtime: Some(200),
                    spoken: None,
                },
                AmiVoiceToken {
                    written: "b".to_string(),
                    confidence: Some(0.8),
                    starttime: Some(200),
                    endtime: Some(400),
                    spoken: None,
                },
            ],
            tags: vec![],
            rulename: String::new(),
        }],
        text: "ab".to_string(),
        code: "0".to_string(),
        message: "".to_string(),
    };

    let (start, end) = result.get_timing();
    assert_eq!(start, Some(100));
    assert_eq!(end, Some(400));
}

// =============================================================================
// Client Tests
// =============================================================================

#[test]
fn test_client_new_valid_config() {
    let config = make_test_config();
    let result = AmiVoiceSTT::new(config);

    assert!(result.is_ok());
    let stt = result.unwrap();
    assert!(!stt.is_ready());
}

#[test]
fn test_client_new_empty_api_key() {
    let config = STTConfig {
        api_key: String::new(),
        ..make_test_config()
    };

    let result = AmiVoiceSTT::new(config);
    assert!(result.is_err());

    match result {
        Err(STTError::AuthenticationFailed(msg)) => {
            assert!(msg.contains("APPKEY"));
        }
        _ => panic!("Expected AuthenticationFailed error"),
    }
}

#[test]
fn test_client_provider_info() {
    let stt = AmiVoiceSTT::new(make_test_config()).unwrap();
    let info = stt.get_provider_info();

    assert!(info.contains("AmiVoice"));
    assert!(info.contains("Advanced Media"));
}

#[test]
fn test_client_set_engine() {
    let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

    stt.set_engine(AmiVoiceEngine::HybridJapaneseMedical);

    let config = stt.get_amivoice_config().unwrap();
    assert_eq!(config.engine, AmiVoiceEngine::HybridJapaneseMedical);
}

#[test]
fn test_client_set_interim_results() {
    let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

    // Initially true
    assert!(stt.get_amivoice_config().unwrap().interim_results);

    stt.set_interim_results(false);
    assert!(!stt.get_amivoice_config().unwrap().interim_results);

    stt.set_interim_results(true);
    assert!(stt.get_amivoice_config().unwrap().interim_results);
}

#[test]
fn test_client_set_no_logging() {
    let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

    stt.set_no_logging(true);
    assert!(stt.get_amivoice_config().unwrap().no_logging);

    let url = stt.get_amivoice_config().unwrap().get_websocket_url();
    assert!(url.contains("nolog"));
}

#[test]
fn test_client_set_profile_words() {
    let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

    let words = r#"{"words":[{"written":"WaaV","spoken":"わーぶ"}]}"#;
    stt.set_profile_words(Some(words.to_string()));

    let config = stt.get_amivoice_config().unwrap();
    assert!(config.profile_words.is_some());
    assert!(config.profile_words.as_ref().unwrap().contains("WaaV"));
}

#[test]
fn test_client_set_diarization() {
    let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

    stt.set_diarization(true);
    assert!(stt.get_amivoice_config().unwrap().enable_diarization);

    let cmd = stt.get_amivoice_config().unwrap().build_start_command();
    assert!(cmd.contains("useDiarizer=1"));
}

#[test]
fn test_client_get_config() {
    let config = make_test_config();
    let stt = AmiVoiceSTT::new(config.clone()).unwrap();

    let retrieved_config = stt.get_config().unwrap();
    assert_eq!(retrieved_config.api_key, config.api_key);
    assert_eq!(retrieved_config.language, config.language);
    assert_eq!(retrieved_config.sample_rate, config.sample_rate);
}

#[tokio::test]
async fn test_client_send_audio_not_connected() {
    let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();
    let audio = bytes::Bytes::from_static(b"test audio data");

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
async fn test_client_disconnect_when_not_connected() {
    let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

    // Should succeed even when not connected
    let result = stt.disconnect().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_client_callback_registration() {
    let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();
    let callback = MockResultCallback::new();
    let callback_clone = callback.clone();

    let result = stt
        .on_result(Arc::new(move |result| {
            let cb = callback_clone.clone();
            Box::pin(async move {
                cb.call_count.fetch_add(1, Ordering::SeqCst);
                if result.is_final {
                    cb.is_final_received.store(true, Ordering::SeqCst);
                }
                cb.results.lock().await.push(result);
            })
        }))
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_client_error_callback_registration() {
    let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();
    let error_received = Arc::new(AtomicBool::new(false));
    let error_received_clone = error_received.clone();

    let result = stt
        .on_error(Arc::new(move |_error| {
            let flag = error_received_clone.clone();
            Box::pin(async move {
                flag.store(true, Ordering::SeqCst);
            })
        }))
        .await;

    assert!(result.is_ok());
}

// =============================================================================
// Model Parsing Tests
// =============================================================================

#[test]
fn test_model_parsing_in_config() {
    let test_cases = vec![
        ("-a-general", AmiVoiceEngine::HybridJapaneseGeneral),
        ("-a-medical", AmiVoiceEngine::HybridJapaneseMedical),
        ("-a-finance", AmiVoiceEngine::HybridJapaneseFinance),
        ("-a2-ja-general", AmiVoiceEngine::E2EJapaneseGeneral),
        ("-a2-multi-general", AmiVoiceEngine::E2EMultilingual),
        ("-a-general-en", AmiVoiceEngine::HybridEnglishGeneral),
        ("-a-general-zh", AmiVoiceEngine::HybridChineseGeneral),
        ("-a-general-ko", AmiVoiceEngine::HybridKoreanGeneral),
    ];

    for (model, expected_engine) in test_cases {
        let config = make_config_with_model(model);
        let stt = AmiVoiceSTT::new(config).unwrap();
        let amivoice_config = stt.get_amivoice_config().unwrap();

        assert_eq!(
            amivoice_config.engine, expected_engine,
            "Model '{}' should map to engine {:?}",
            model, expected_engine
        );
    }
}

// =============================================================================
// Default Implementation Tests
// =============================================================================

#[test]
fn test_default_stt() {
    let stt = AmiVoiceSTT::default();

    // Should not be ready when created with default
    assert!(!stt.is_ready());
}

impl Default for AmiVoiceResult {
    fn default() -> Self {
        Self {
            results: vec![],
            text: String::new(),
            code: String::new(),
            message: String::new(),
        }
    }
}
