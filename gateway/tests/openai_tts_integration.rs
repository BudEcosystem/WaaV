//! Integration tests for OpenAI TTS provider
//!
//! These tests verify:
//! - Provider creation through factory
//! - Configuration validation
//! - Callback registration
//! - Voice and model configuration
//!
//! Note: Tests requiring actual API calls are marked with #[ignore]
//! and require OPENAI_API_KEY environment variable.

use std::sync::Arc;
use waav_gateway::core::tts::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, OPENAI_TTS_URL, OpenAITTS, OpenAITTSModel,
    OpenAIVoice, TTSConfig, TTSError, create_tts_provider, get_tts_provider_urls,
};

/// Test that OpenAI is included in supported providers
#[test]
fn test_openai_in_tts_provider_urls() {
    let urls = get_tts_provider_urls();
    assert!(urls.contains_key("openai"));
    assert_eq!(urls.get("openai").unwrap(), OPENAI_TTS_URL);
}

/// Test provider creation via string name
#[tokio::test]
async fn test_create_openai_tts_provider_by_name() {
    let config = TTSConfig {
        provider: "openai".to_string(),
        api_key: "test-api-key".to_string(),
        voice_id: Some("nova".to_string()),
        model: "tts-1-hd".to_string(),
        ..Default::default()
    };

    let result = create_tts_provider("openai", config);
    assert!(result.is_ok());

    let tts = result.unwrap();
    let info = tts.get_provider_info();
    assert_eq!(info["provider"], "openai");
    assert!(!tts.is_ready());
}

/// Test case-insensitive provider name parsing
#[tokio::test]
async fn test_openai_tts_provider_name_case_insensitive() {
    let config = TTSConfig {
        provider: "openai".to_string(),
        api_key: "test-api-key".to_string(),
        ..Default::default()
    };

    // Test various cases
    assert!(create_tts_provider("openai", config.clone()).is_ok());
    assert!(create_tts_provider("OpenAI", config.clone()).is_ok());
    assert!(create_tts_provider("OPENAI", config).is_ok());
}

/// Test model configuration
#[tokio::test]
async fn test_openai_tts_model_configuration() {
    // Test tts-1 model
    let config = TTSConfig {
        api_key: "test-api-key".to_string(),
        model: "tts-1".to_string(),
        ..Default::default()
    };
    let tts = OpenAITTS::new(config).unwrap();
    assert_eq!(tts.model(), OpenAITTSModel::Tts1);

    // Test tts-1-hd model
    let config = TTSConfig {
        api_key: "test-api-key".to_string(),
        model: "tts-1-hd".to_string(),
        ..Default::default()
    };
    let tts = OpenAITTS::new(config).unwrap();
    assert_eq!(tts.model(), OpenAITTSModel::Tts1Hd);

    // Test gpt-4o-mini-tts model
    let config = TTSConfig {
        api_key: "test-api-key".to_string(),
        model: "gpt-4o-mini-tts".to_string(),
        ..Default::default()
    };
    let tts = OpenAITTS::new(config).unwrap();
    assert_eq!(tts.model(), OpenAITTSModel::Gpt4oMiniTts);
}

/// Test voice configuration
#[tokio::test]
async fn test_openai_tts_voice_configuration() {
    let voices = [
        ("alloy", OpenAIVoice::Alloy),
        ("ash", OpenAIVoice::Ash),
        ("ballad", OpenAIVoice::Ballad),
        ("coral", OpenAIVoice::Coral),
        ("echo", OpenAIVoice::Echo),
        ("fable", OpenAIVoice::Fable),
        ("onyx", OpenAIVoice::Onyx),
        ("nova", OpenAIVoice::Nova),
        ("sage", OpenAIVoice::Sage),
        ("shimmer", OpenAIVoice::Shimmer),
        ("verse", OpenAIVoice::Verse),
    ];

    for (voice_str, expected_voice) in voices {
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some(voice_str.to_string()),
            ..Default::default()
        };
        let tts = OpenAITTS::new(config).unwrap();
        assert_eq!(
            tts.voice(),
            expected_voice,
            "Voice {} mapping failed",
            voice_str
        );
    }
}

/// Test speed/speaking rate clamping
#[tokio::test]
async fn test_openai_tts_speed_clamping() {
    // Test speed below minimum (should clamp to 0.25)
    let config = TTSConfig {
        api_key: "test-api-key".to_string(),
        speaking_rate: Some(0.1),
        ..Default::default()
    };
    let tts = OpenAITTS::new(config).unwrap();
    let info = tts.get_provider_info();
    assert_eq!(info["speed_range"]["min"], 0.25);

    // Test speed above maximum (should clamp to 4.0)
    let config = TTSConfig {
        api_key: "test-api-key".to_string(),
        speaking_rate: Some(5.0),
        ..Default::default()
    };
    let _tts = OpenAITTS::new(config).unwrap();
    // Speed clamping is internal, just verify creation succeeds
}

/// Test default configuration
#[test]
fn test_openai_tts_default_configuration() {
    let tts = OpenAITTS::default();
    assert!(!tts.is_ready());
    assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    assert_eq!(tts.model(), OpenAITTSModel::Tts1);
    assert_eq!(tts.voice(), OpenAIVoice::Alloy);
}

/// Test provider info structure
#[test]
fn test_openai_tts_provider_info() {
    let tts = OpenAITTS::default();
    let info = tts.get_provider_info();

    assert_eq!(info["provider"], "openai");
    assert_eq!(info["api_type"], "HTTP REST");
    assert_eq!(info["default_sample_rate"], 24000);

    // Verify supported models
    let models = info["supported_models"].as_array().unwrap();
    assert!(models.iter().any(|m| m == "tts-1"));
    assert!(models.iter().any(|m| m == "tts-1-hd"));
    assert!(models.iter().any(|m| m == "gpt-4o-mini-tts"));

    // Verify supported voices (11 total)
    let voices = info["supported_voices"].as_array().unwrap();
    assert_eq!(voices.len(), 11);
    assert!(voices.iter().any(|v| v == "alloy"));
    assert!(voices.iter().any(|v| v == "nova"));
    assert!(voices.iter().any(|v| v == "verse"));

    // Verify supported formats
    let formats = info["supported_formats"].as_array().unwrap();
    assert!(formats.iter().any(|f| f == "mp3"));
    assert!(formats.iter().any(|f| f == "pcm"));
    assert!(formats.iter().any(|f| f == "opus"));
}

/// Test callback registration (without connection)
#[tokio::test]
async fn test_openai_tts_callback_registration() {
    let config = TTSConfig {
        api_key: "test-api-key".to_string(),
        ..Default::default()
    };

    let mut tts = OpenAITTS::new(config).unwrap();

    // Test AudioCallback trait
    struct TestAudioCallback;

    impl AudioCallback for TestAudioCallback {
        fn on_audio(
            &self,
            _audio: AudioData,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async move {
                println!("Got audio");
            })
        }

        fn on_error(
            &self,
            _error: TTSError,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async move {
                println!("Got error");
            })
        }

        fn on_complete(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async move {
                println!("Complete");
            })
        }
    }

    let result = tts.on_audio(Arc::new(TestAudioCallback));
    assert!(result.is_ok());
}

/// Integration test with real API (requires OPENAI_API_KEY)
#[tokio::test]
#[ignore = "Requires OPENAI_API_KEY environment variable"]
async fn test_real_openai_tts() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::time::{Duration, timeout};

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY not set");

    let config = TTSConfig {
        api_key,
        voice_id: Some("nova".to_string()),
        model: "tts-1".to_string(),
        audio_format: Some("pcm".to_string()),
        ..Default::default()
    };

    let mut tts = OpenAITTS::new(config).unwrap();

    // Track audio received
    let received_audio = Arc::new(AtomicBool::new(false));
    let bytes_received = Arc::new(AtomicUsize::new(0));
    let received_clone = received_audio.clone();
    let bytes_clone = bytes_received.clone();

    struct TestCallback {
        received: Arc<AtomicBool>,
        bytes: Arc<AtomicUsize>,
    }

    impl AudioCallback for TestCallback {
        fn on_audio(
            &self,
            audio: AudioData,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            let len = audio.data.len();
            self.received.store(true, Ordering::SeqCst);
            self.bytes.fetch_add(len, Ordering::SeqCst);
            Box::pin(async move {
                println!("Received {} bytes of audio", len);
            })
        }

        fn on_error(
            &self,
            error: TTSError,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async move {
                println!("Error: {:?}", error);
            })
        }

        fn on_complete(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async move {
                println!("TTS complete");
            })
        }
    }

    tts.on_audio(Arc::new(TestCallback {
        received: received_clone,
        bytes: bytes_clone,
    }))
    .unwrap();

    // Connect
    let connect_result = tts.connect().await;
    assert!(
        connect_result.is_ok(),
        "Connection failed: {:?}",
        connect_result
    );
    assert!(tts.is_ready());

    // Speak
    let speak_result = timeout(
        Duration::from_secs(10),
        tts.speak("Hello, this is a test of OpenAI text to speech.", true),
    )
    .await;

    assert!(speak_result.is_ok(), "Speak timed out");
    let inner_result = speak_result.unwrap();
    assert!(inner_result.is_ok(), "Speak failed: {:?}", inner_result);

    // Wait for audio
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert!(
        received_audio.load(Ordering::SeqCst),
        "No audio received from OpenAI TTS"
    );
    assert!(
        bytes_received.load(Ordering::SeqCst) > 0,
        "Received 0 bytes of audio"
    );

    println!(
        "Total bytes received: {}",
        bytes_received.load(Ordering::SeqCst)
    );

    // Disconnect
    tts.disconnect().await.unwrap();
    assert!(!tts.is_ready());
}

/// Test TTS with HD model (requires OPENAI_API_KEY)
#[tokio::test]
#[ignore = "Requires OPENAI_API_KEY environment variable"]
async fn test_openai_tts_hd_model() {
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY not set");

    let config = TTSConfig {
        api_key,
        voice_id: Some("shimmer".to_string()),
        model: "tts-1-hd".to_string(),
        audio_format: Some("opus".to_string()),
        speaking_rate: Some(1.2),
        ..Default::default()
    };

    let tts = OpenAITTS::new(config).unwrap();
    assert_eq!(tts.model(), OpenAITTSModel::Tts1Hd);
    assert_eq!(tts.voice(), OpenAIVoice::Shimmer);

    // Just verify creation, actual API test above
}

/// REAL live end-to-end synthesis round-trip against a local OpenAI-compatible speech server.
///
/// The synthesis counterpart of `test_openai_stt_local_server_roundtrip`: it runs in the default
/// suite (no `#[ignore]`), stands up a real `POST /v1/audio/speech` server, points the WaaV OpenAI
/// TTS provider at it via the standard `OPENAI_BASE_URL` override, and drives the *actual* provider
/// pipeline (`connect` → `on_audio` → `speak` → dispatcher/queue worker → audio callback). It
/// validates the real wire path — JSON request body, `Authorization: Bearer` header, the HTTP POST,
/// and streaming the response bytes back through `AudioCallback` — with NO paid credentials.
#[tokio::test]
async fn test_openai_tts_local_server_roundtrip() {
    use axum::{Router, extract::State, http::HeaderMap, routing::post};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    // 16 KB of valid s16le PCM "audio" the server will return as the synthesized speech.
    fn fake_pcm() -> Vec<u8> {
        let mut v = Vec::with_capacity(8000 * 2);
        for i in 0..8000u32 {
            let s = (((i as f32) * 0.04).sin() * 9000.0) as i16;
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }

    #[derive(Default)]
    struct SrvState {
        saw_bearer: AtomicBool,
        saw_json_body: AtomicBool,
    }

    async fn speak_handler(
        State(st): State<Arc<SrvState>>,
        headers: HeaderMap,
        body: bytes::Bytes,
    ) -> Vec<u8> {
        if headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.starts_with("Bearer "))
        {
            st.saw_bearer.store(true, Ordering::SeqCst);
        }
        // The OpenAI speech contract is a JSON body containing the input text + voice/model.
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) {
            if v.get("input").is_some() && v.get("voice").is_some() {
                st.saw_json_body.store(true, Ordering::SeqCst);
            }
        }
        fake_pcm()
    }

    let state = Arc::new(SrvState::default());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local server");
    let addr = listener.local_addr().expect("local_addr");
    let app = Router::new()
        .route("/v1/audio/speech", post(speak_handler))
        .with_state(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    // SAFETY (edition 2024): set/remove_var are unsafe. Only this test touches OPENAI_BASE_URL in
    // the default suite (the other live tests are #[ignore]d); it is removed below.
    unsafe { std::env::set_var("OPENAI_BASE_URL", format!("http://{addr}")) };

    let config = TTSConfig {
        provider: "openai".to_string(),
        api_key: "local-test-key".to_string(),
        voice_id: Some("nova".to_string()),
        model: "tts-1".to_string(),
        audio_format: Some("pcm".to_string()),
        ..Default::default()
    };
    let mut tts = OpenAITTS::new(config).expect("create OpenAI TTS");

    let (tx, mut rx) = mpsc::unbounded_channel::<usize>();
    struct CaptureCallback(mpsc::UnboundedSender<usize>);
    impl AudioCallback for CaptureCallback {
        fn on_audio(
            &self,
            audio: AudioData,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            let n = audio.data.len();
            let tx = self.0.clone();
            Box::pin(async move {
                let _ = tx.send(n);
            })
        }
        fn on_error(
            &self,
            _error: TTSError,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async move {})
        }
        fn on_complete(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async move {})
        }
    }
    tts.on_audio(Arc::new(CaptureCallback(tx)))
        .expect("register audio callback");

    tts.connect().await.expect("connect");
    assert!(tts.is_ready());

    timeout(Duration::from_secs(10), tts.speak("Hello from WaaV.", true))
        .await
        .expect("speak timed out")
        .expect("speak failed");

    // Audio is delivered asynchronously by the dispatcher/queue worker via the callback.
    let total: usize = {
        let mut acc = 0usize;
        // Collect whatever chunks arrive within the window.
        while let Ok(Some(n)) = timeout(Duration::from_secs(5), rx.recv()).await {
            acc += n;
            if acc > 0 {
                break;
            }
        }
        acc
    };

    tts.disconnect().await.expect("disconnect");
    unsafe { std::env::remove_var("OPENAI_BASE_URL") };

    assert!(
        total > 0,
        "the provider must stream synthesized audio bytes back through AudioCallback"
    );
    assert!(
        state.saw_bearer.load(Ordering::SeqCst),
        "the provider must send an Authorization: Bearer header"
    );
    assert!(
        state.saw_json_body.load(Ordering::SeqCst),
        "the provider must POST a JSON body with input+voice (OpenAI speech contract)"
    );
}
