use std::sync::Arc;

use crate::core::stt::base::{BaseSTT, STTConfig, STTError, STTResult};
use crate::core::stt::google::provider::ConnectionState;
use crate::core::stt::google::{GoogleSTT, GoogleSTTConfig};

#[test]
fn test_google_stt_default() {
    let stt = GoogleSTT::default();

    assert!(stt.config.is_none());
    assert!(!stt.is_ready());
    assert!(stt.audio_sender.is_none());
    assert!(stt.shutdown_tx.is_none());
    assert!(stt.result_tx.is_none());
    assert!(stt.connection_handle.is_none());
    assert!(stt.result_forward_handle.is_none());
    assert!(stt.auth_client.is_none());
}

#[test]
fn test_google_stt_new_missing_project_id() {
    // With JSON credentials that don't have project_id and no project_id in model,
    // the creation should fail with "project_id is required"
    let json_creds = r#"{"type": "service_account"}"#; // Missing project_id
    let config = STTConfig {
        provider: "google".to_string(),
        api_key: json_creds.to_string(),
        model: "latest_long".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
    };

    let result = <GoogleSTT as BaseSTT>::new(config);
    assert!(result.is_err());

    if let Err(STTError::ConfigurationError(msg)) = result {
        assert!(
            msg.contains("project_id is required"),
            "Expected error about missing project_id, got: {}",
            msg
        );
    } else {
        panic!("Expected ConfigurationError");
    }
}

#[test]
fn test_google_stt_new_with_project_id_in_credentials() {
    // When credentials JSON contains project_id, it should be extracted
    let json_creds = r#"{"type": "service_account", "project_id": "creds-project-123", "client_email": "test@test.iam.gserviceaccount.com", "private_key": "fake-key"}"#;
    let config = STTConfig {
        provider: "google".to_string(),
        api_key: json_creds.to_string(),
        model: "latest_long".to_string(), // No project_id in model
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
    };

    let result = <GoogleSTT as BaseSTT>::new(config);

    // The creation might fail due to invalid credentials format, but
    // it should NOT fail due to missing project_id
    if let Err(STTError::ConfigurationError(msg)) = &result {
        assert!(
            !msg.contains("project_id is required"),
            "Should have extracted project_id from credentials JSON"
        );
    }
}

#[tokio::test]
async fn test_google_stt_new_with_project_id_in_model() {
    let config = STTConfig {
        provider: "google".to_string(),
        api_key: String::new(),
        model: "test-project-123:latest_long".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
    };

    let result = <GoogleSTT as BaseSTT>::new(config);

    if let Err(STTError::ConfigurationError(msg)) = &result {
        assert!(
            !msg.contains("project_id is required"),
            "Should have extracted project_id from model field"
        );
    }
}

// A self-contained, valid service-account credential (real 2048-bit RSA key, generated for
// tests only — not used against any real project) so `new_standard` can build the auth client
// and we can assert the standardized features land in the provider-specific config.
const TEST_SERVICE_ACCOUNT_JSON: &str = r#"{
  "type": "service_account",
  "project_id": "creds-project-123",
  "private_key_id": "test-key-id",
  "client_email": "test@creds-project-123.iam.gserviceaccount.com",
  "client_id": "1234567890",
  "private_key": "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC0dEowKy753Kjw\nP8a593MGNp91QpbTYl6VGND6bRXvkXr1h4Bb37e0OWofS/skiWI6omPU34bEuTgC\n83kfKILtWkb5URVs58QX7TO9w84eA+ePxHGoWL7Myohwrh5fWruk4f4t3rXqjlAK\ne5+yslho0ufEMB4thPDAMK7yQt3grCK6Ybe+en96aiaz4yR4xxKkI5l5KEVacF/i\n4wDszefD8n7+f9K+VhIZRTbuowRlwwn3/79BNmLNcSt5MgTQeA/Ayhl6uyrXNkTm\npj5rRum/13G0QAzPJJiYmYm/zNBR94Mvk4JM9T9kOZjEH4s1q7E/vfRoX6+qeVud\nduosFrz7AgMBAAECggEAIuXCWyJeyU9VFHEg+2HRSshRehnQlTyW0fqkn2ltLpFR\n2B3GQv42xpG75iWJgf1Xk8NHzykTJQQ0ws1XBSGOgFxPEXQO0qrXj1D+CprNR5y1\nsWXqHQZcj7ozPKdPlF01oKNbxn8layDudbiGn8ZBtrPiwlwT2fW1oVVI3+zyf7o2\n7QGbhq6ZXz9U3TAwTa+5nB8R8JNAO2xwwFKrtQLB9mtNuuYzk5mV8PV6sqKEqk+x\n4AjUYgO3BJ6jO+Cg9DfMV6WYQF8E9CvR4raOhKMFcjZQ/rAZz5QhFDLbWltxN6Qd\nnMiMshuP0rHOsUkQWr5suf8Wp4hqsb8zYgXklVHrwQKBgQDpIhzdV1RKCprUHioV\nGa1H3Z2lvrndMAZ8OgtdeagfImzICsloF3KvTpAAdkWN8cia/sjP3MKA8UzJGgF0\nb9N3XI4VsBIXcuL6Sz3hc0M2o017L0yCesNK/ffxygjwrHOZ4SUCerK4whddr7g1\n6l6DfoL6peiyT6+NSnUYKtyVMQKBgQDGJ21xKUHroiGR6vspMCh4fREKPKY4XJLu\nBrz5tIrCGmguUrBY690shtn7SQjsriTEN9EVjRc6xLfrY+tD4zvAhINwn6bTvdS5\nco6DmvKRwiyUX4VdqAEktwg2dLgLstHQKS6FGaMolRe9YWgRTwrv8BhsJzw/MxRb\ncu+Wt4gZ6wKBgQCzY9FsLC+qzaA3yoI9PEXO/+O3zxv77GGBI7TtF5jbZETqZQp3\ns1tHNB+wi1GYGM1xHs5szAVK7OJV+FHYQ9gnh6u5WoOBUaEAUfdqzKOSnnQXbtzj\npg0yXlx0zC626ywE4270CnANpSQPrhAERLS3YBjvP8zfsFt4UCvsDccwcQKBgFS0\nX/1CpLJEgVMt/qVxt6sh01nr6SYotIpZiQi5G6OzxBshL88jLE2va5kWdGEwY/kY\n3yD2ShrOIszVzqkbhtxaCRHovVjASiHoDXHGl7ClL4dReeI6Qhrevv0AUfh2PWhd\nYkx1VCCx8w76h5D2l/dPTDFXaFKf1DDvZemolN53AoGAftepQ1IBdcm23x4Z+/0X\nR8IOzZWxdgLO7llUPxrYr4xXKB/lTdwMwTezGAQSwGG8amTXH3Kh2TItg1kcEqnw\n0pv1w3pEXtsXF5+2JJZlpkDholWm4Sr371WLoibqZxur4i3s/oXc7g5sV8FK4Gc+\nQLrApvC3ECyPJuv/tZ/KPrM=\n-----END PRIVATE KEY-----\n"
}"#;

// W1 keystone: Google's standardized features (interim results, voice-activity events) and the
// non-standard `project_id` (via extras) must survive through `new_standard` into the
// provider-specific config — previously dropped by the flat factory path.
#[tokio::test]
async fn new_standard_unlocks_features_and_project_id() {
    use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
    let mut extras = serde_json::Map::new();
    extras.insert("project_id".into(), serde_json::json!("proj-xyz"));
    let std = StandardSTTConfig {
        base: STTConfig {
            provider: "google".into(),
            api_key: TEST_SERVICE_ACCOUNT_JSON.into(),
            ..Default::default()
        },
        features: SttFeatures {
            interim_results: Some(false),
            vad_events: Some(false),
            ..Default::default()
        },
        extras: ProviderExtras(extras),
    };
    let stt = GoogleSTT::new_standard(&std).expect("new_standard should build a valid client");
    let cfg = stt.config.as_ref().expect("config should be set");
    assert!(!cfg.interim_results); // interim_results feature
    assert!(!cfg.enable_voice_activity_events); // vad_events feature
    assert_eq!(cfg.project_id, "proj-xyz"); // from provider_extras passthrough

    // Empty api_key is rejected through the standardized path too (parity with Deepgram).
    let bad = StandardSTTConfig::from_base(STTConfig {
        provider: "google".into(),
        api_key: String::new(),
        ..Default::default()
    });
    assert!(GoogleSTT::new_standard(&bad).is_err());
}

#[test]
fn test_google_stt_provider_info() {
    let stt = GoogleSTT::default();
    assert_eq!(stt.get_provider_info(), "Google Cloud Speech-to-Text v2");
}

#[test]
fn test_google_stt_get_config_none() {
    let stt = GoogleSTT::default();
    assert!(stt.get_config().is_none());
}

#[test]
fn test_google_stt_is_ready_disconnected() {
    let stt = GoogleSTT::default();
    assert!(!stt.is_ready());
}

#[tokio::test]
async fn test_google_stt_send_audio_not_connected() {
    let mut stt = GoogleSTT::default();

    let result = stt.send_audio(vec![0u8; 100].into()).await;
    assert!(result.is_err());

    if let Err(STTError::ConnectionFailed(msg)) = result {
        assert!(msg.contains("Not connected"));
    } else {
        panic!("Expected ConnectionFailed error");
    }
}

#[tokio::test]
async fn test_google_stt_on_result_callback() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let mut stt = GoogleSTT::default();
    let callback_called = Arc::new(AtomicBool::new(false));
    let callback_called_clone = callback_called.clone();

    let callback = Arc::new(move |_result: STTResult| {
        callback_called_clone.store(true, Ordering::SeqCst);
        Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });

    let result = stt.on_result(callback).await;
    assert!(result.is_ok());

    let callback_stored = stt.result_callback.read().await.is_some();
    assert!(callback_stored);
}

#[tokio::test]
async fn test_google_stt_disconnect_when_not_connected() {
    let mut stt = GoogleSTT::default();

    let result = stt.disconnect().await;
    assert!(result.is_ok());
}

#[test]
fn test_google_stt_create_google_config() {
    let base_config = STTConfig {
        provider: "google".to_string(),
        api_key: String::new(),
        model: "latest_long".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
    };

    let google_config =
        GoogleSTT::create_google_config(base_config.clone(), "my-project".to_string());

    assert_eq!(google_config.project_id, "my-project");
    assert_eq!(google_config.location, "global");
    assert!(google_config.recognizer_id.is_none());
    assert!(google_config.interim_results);
    assert!(google_config.enable_voice_activity_events);
    assert!(google_config.speech_start_timeout.is_none());
    assert!(google_config.speech_end_timeout.is_none());
    assert!(!google_config.single_utterance);
    assert_eq!(google_config.base.model, "latest_long");
    assert_eq!(google_config.base.language, "en-US");
}

#[test]
fn test_google_stt_project_id_extraction_with_colon() {
    let model = "my-project-123:latest_long";
    let parts: Vec<&str> = model.splitn(2, ':').collect();
    assert_eq!(parts[0], "my-project-123");
    assert_eq!(parts[1], "latest_long");
}

#[test]
fn test_google_stt_project_id_extraction_without_colon() {
    let model = "latest_long";
    if model.contains(':') {
        panic!("Model should not contain colon");
    }
    assert_eq!(model, "latest_long");
}

#[test]
fn test_connection_state_debug() {
    let disconnected = ConnectionState::Disconnected;
    let connecting = ConnectionState::Connecting;
    let connected = ConnectionState::Connected;
    let error = ConnectionState::Error("test error".to_string());

    assert_eq!(format!("{:?}", disconnected), "Disconnected");
    assert_eq!(format!("{:?}", connecting), "Connecting");
    assert_eq!(format!("{:?}", connected), "Connected");
    assert!(format!("{:?}", error).contains("Error"));
}

#[test]
fn test_connection_state_clone() {
    let original = ConnectionState::Connected;
    let cloned = original.clone();
    assert!(matches!(cloned, ConnectionState::Connected));

    let error = ConnectionState::Error("test".to_string());
    let error_clone = error.clone();
    if let ConnectionState::Error(msg) = error_clone {
        assert_eq!(msg, "test");
    } else {
        panic!("Expected Error variant");
    }
}

#[test]
fn test_google_stt_state_transitions() {
    let stt = GoogleSTT::default();

    assert!(matches!(stt.state, ConnectionState::Disconnected));
    assert!(!stt.is_ready());
}

#[tokio::test]
async fn test_google_stt_send_audio_requires_connection() {
    let mut stt = GoogleSTT::default();

    let result = stt.send_audio(vec![1u8, 2, 3, 4].into()).await;
    assert!(result.is_err());
    if let Err(STTError::ConnectionFailed(msg)) = result {
        assert!(msg.contains("Not connected"));
    } else {
        panic!("Expected ConnectionFailed error");
    }
}

#[tokio::test]
async fn test_google_stt_callback_registration() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mut stt = GoogleSTT::default();
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let callback = Arc::new(move |_result: STTResult| {
        call_count_clone.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });

    let result = stt.on_result(callback).await;
    assert!(result.is_ok());

    let stored = stt.result_callback.read().await;
    assert!(stored.is_some());
}

#[test]
fn test_project_id_parsing_with_multiple_colons() {
    let model = "my-project:v1:latest_long";
    let parts: Vec<&str> = model.splitn(2, ':').collect();
    assert_eq!(parts[0], "my-project");
    assert_eq!(parts[1], "v1:latest_long");
}

#[test]
fn test_project_id_parsing_empty_model_after_colon() {
    let model = "my-project:";
    let parts: Vec<&str> = model.splitn(2, ':').collect();
    assert_eq!(parts[0], "my-project");
    assert_eq!(parts[1], "");
}

#[test]
fn test_helper_functions() {
    let stt_config = create_test_stt_config();
    assert_eq!(stt_config.provider, "google");
    assert_eq!(stt_config.language, "en-US");

    let google_config = create_test_google_config();
    assert_eq!(google_config.project_id, "test-project");
    assert_eq!(google_config.location, "global");
}

fn create_test_stt_config() -> STTConfig {
    STTConfig {
        provider: "google".to_string(),
        api_key: String::new(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: "latest_long".to_string(),
    }
}

fn create_test_google_config() -> GoogleSTTConfig {
    GoogleSTTConfig {
        base: create_test_stt_config(),
        project_id: "test-project".to_string(),
        location: "global".to_string(),
        recognizer_id: None,
        interim_results: true,
        enable_voice_activity_events: true,
        speech_start_timeout: None,
        speech_end_timeout: None,
        single_utterance: false,
        ..Default::default()
    }
}
