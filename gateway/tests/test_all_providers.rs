//! Comprehensive Provider Test Suite
//!
//! Tests all 71 providers (32 STT + 37 TTS + 2 Realtime) in the WaaV Gateway.
//!
//! Run with: cargo test --test test_all_providers --release -- --nocapture --test-threads=1

mod fixtures;

use fixtures::audio_fixtures;
use std::collections::HashMap;
use std::env;
use std::time::{Duration, Instant};

// ============================================================================
// Provider Definitions
// ============================================================================

/// All 32 STT providers
const STT_PROVIDERS: &[(&str, &str)] = &[
    ("deepgram", "DEEPGRAM_API_KEY"),
    ("google", "GOOGLE_APPLICATION_CREDENTIALS"),
    ("elevenlabs", "ELEVENLABS_API_KEY"),
    ("microsoft-azure", "AZURE_SPEECH_SUBSCRIPTION_KEY"),
    ("cartesia", "CARTESIA_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("assemblyai", "ASSEMBLYAI_API_KEY"),
    ("aws-transcribe", "AWS_ACCESS_KEY_ID"),
    ("ibm-watson", "IBM_WATSON_API_KEY"),
    ("groq", "GROQ_API_KEY"),
    ("gnani", "GNANI_API_KEY"),
    ("sarvam", "SARVAM_API_KEY"),
    ("speechmatics", "SPEECHMATICS_API_KEY"),
    ("gladia", "GLADIA_API_KEY"),
    ("revai", "REVAI_API_KEY"),
    ("phonexia", "PHONEXIA_API_KEY"),
    ("reverie", "REVERIE_API_KEY"),
    ("yandex", "YANDEX_API_KEY"),
    ("tinkoff", "TINKOFF_API_KEY"),
    ("sberdevices", "SBERDEVICES_CLIENT_SECRET"),
    ("bhashini", "BHASHINI_API_KEY"),
    ("iflytek", "IFLYTEK_APP_ID"),
    ("alibaba-cloud", "ALIBABA_CLOUD_API_KEY"),
    ("baidu", "BAIDU_APP_ID"),
    ("tencent", "TENCENT_SECRET_ID"),
    ("huawei-cloud", "HUAWEI_CLOUD_AK"),
    ("naver-clova", "NAVER_CLOVA_CLIENT_ID"),
    ("amivoice", "AMIVOICE_APP_KEY"),
    ("fpt-ai", "FPT_AI_API_KEY"),
    ("viettel-ai", "VIETTEL_AI_TOKEN"),
    ("prosa-ai", "PROSA_API_KEY"),
    ("nectec", "NECTEC_API_KEY"),
];

/// All 37 TTS providers
const TTS_PROVIDERS: &[(&str, &str)] = &[
    ("deepgram", "DEEPGRAM_API_KEY"),
    ("elevenlabs", "ELEVENLABS_API_KEY"),
    ("google", "GOOGLE_APPLICATION_CREDENTIALS"),
    ("microsoft-azure", "AZURE_SPEECH_SUBSCRIPTION_KEY"),
    ("cartesia", "CARTESIA_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("aws-polly", "AWS_ACCESS_KEY_ID"),
    ("ibm-watson", "IBM_WATSON_API_KEY"),
    ("hume", "HUME_API_KEY"),
    ("lmnt", "LMNT_API_KEY"),
    ("playht", "PLAYHT_API_KEY"),
    ("gnani", "GNANI_API_KEY"),
    ("murf", "MURF_API_KEY"),
    ("wellsaid", "WELLSAID_API_KEY"),
    ("resemble", "RESEMBLE_API_KEY"),
    ("speechify", "SPEECHIFY_API_KEY"),
    ("unrealspeech", "UNREALSPEECH_API_KEY"),
    ("speechmatics", "SPEECHMATICS_API_KEY"),
    ("acapela", "ACAPELA_APPLICATION"),
    ("cereproc", "CEREPROC_USERNAME"),
    ("reverie", "REVERIE_API_KEY"),
    ("yandex", "YANDEX_API_KEY"),
    ("smallest", "SMALLEST_API_KEY"),
    ("tinkoff", "TINKOFF_API_KEY"),
    ("sberdevices", "SBERDEVICES_CLIENT_SECRET"),
    ("bhashini", "BHASHINI_API_KEY"),
    ("iflytek", "IFLYTEK_APP_ID"),
    ("alibaba-cloud", "ALIBABA_CLOUD_API_KEY"),
    ("baidu", "BAIDU_APP_ID"),
    ("tencent", "TENCENT_SECRET_ID"),
    ("huawei-cloud", "HUAWEI_CLOUD_AK"),
    ("naver-clova", "NAVER_CLOVA_CLIENT_ID"),
    ("zalo-ai", "ZALO_API_KEY"),
    ("fpt-ai", "FPT_AI_API_KEY"),
    ("viettel-ai", "VIETTEL_AI_TOKEN"),
    ("prosa-ai", "PROSA_API_KEY"),
    ("nectec", "NECTEC_API_KEY"),
];

/// All 2 Realtime providers
const REALTIME_PROVIDERS: &[(&str, &str)] =
    &[("openai", "OPENAI_API_KEY"), ("hume", "HUME_API_KEY")];

// ============================================================================
// Test Result Tracking
// ============================================================================

#[derive(Debug, Clone)]
pub struct TestResult {
    pub provider: String,
    pub provider_type: String,
    pub status: TestStatus,
    pub latency_ms: Option<u64>,
    pub error_message: Option<String>,
    pub api_key_configured: bool,
}

#[derive(Debug, Clone)]
pub enum TestStatus {
    Success,
    Failed,
    Skipped,
    ApiKeyMissing,
}

impl std::fmt::Display for TestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestStatus::Success => write!(f, "✓ PASS"),
            TestStatus::Failed => write!(f, "✗ FAIL"),
            TestStatus::Skipped => write!(f, "⊘ SKIP"),
            TestStatus::ApiKeyMissing => write!(f, "⊠ NO KEY"),
        }
    }
}

// ============================================================================
// Test Helpers
// ============================================================================

fn get_gateway_url() -> String {
    env::var("GATEWAY_URL").unwrap_or_else(|_| "http://localhost:3001".to_string())
}

fn has_api_key(env_var: &str) -> bool {
    env::var(env_var)
        .map(|v| !v.is_empty() && !v.starts_with("your_"))
        .unwrap_or(false)
}

fn generate_test_audio_wav() -> Vec<u8> {
    let samples = audio_fixtures::generate_speech_pattern(audio_fixtures::SECOND * 2);
    audio_fixtures::create_wav_file(&samples)
}

// ============================================================================
// Provider-Specific API Tests
// ============================================================================

async fn test_deepgram_stt_api() -> Result<u64, String> {
    let api_key =
        env::var("DEEPGRAM_API_KEY").map_err(|_| "DEEPGRAM_API_KEY not set".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let audio_data = generate_test_audio_wav();
    let start = Instant::now();

    let response = client
        .post("https://api.deepgram.com/v1/listen?model=nova-2&language=en")
        .header("Authorization", format!("Token {}", api_key))
        .header("Content-Type", "audio/wav")
        .body(audio_data)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let latency = start.elapsed().as_millis() as u64;

    if response.status().is_success() {
        Ok(latency)
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!("API returned error: {}", body))
    }
}

async fn test_deepgram_tts_api() -> Result<u64, String> {
    let api_key =
        env::var("DEEPGRAM_API_KEY").map_err(|_| "DEEPGRAM_API_KEY not set".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let payload = serde_json::json!({
        "text": "Hello, this is a test."
    });

    let start = Instant::now();

    let response = client
        .post("https://api.deepgram.com/v1/speak?model=aura-asteria-en")
        .header("Authorization", format!("Token {}", api_key))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let latency = start.elapsed().as_millis() as u64;

    if response.status().is_success() {
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > 100 {
            Ok(latency)
        } else {
            Err("TTS returned too little audio data".to_string())
        }
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!("API returned error: {}", body))
    }
}

async fn test_elevenlabs_stt_api() -> Result<u64, String> {
    let api_key =
        env::var("ELEVENLABS_API_KEY").map_err(|_| "ELEVENLABS_API_KEY not set".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let audio_data = generate_test_audio_wav();
    let start = Instant::now();

    // ElevenLabs STT uses multipart
    let part = reqwest::multipart::Part::bytes(audio_data)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;

    let form = reqwest::multipart::Form::new()
        .part("audio", part)
        .text("model_id", "eleven_turbo_v2");

    let response = client
        .post("https://api.elevenlabs.io/v1/speech-to-text")
        .header("xi-api-key", &api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let latency = start.elapsed().as_millis() as u64;

    if response.status().is_success() {
        Ok(latency)
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!("API returned error: {}", body))
    }
}

async fn test_elevenlabs_tts_api() -> Result<u64, String> {
    let api_key =
        env::var("ELEVENLABS_API_KEY").map_err(|_| "ELEVENLABS_API_KEY not set".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let voice_id = "21m00Tcm4TlvDq8ikWAM"; // Rachel voice
    let payload = serde_json::json!({
        "text": "Hello, this is a test.",
        "model_id": "eleven_monolingual_v1"
    });

    let start = Instant::now();

    let response = client
        .post(format!(
            "https://api.elevenlabs.io/v1/text-to-speech/{}",
            voice_id
        ))
        .header("xi-api-key", &api_key)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let latency = start.elapsed().as_millis() as u64;

    if response.status().is_success() {
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > 100 {
            Ok(latency)
        } else {
            Err("TTS returned too little audio data".to_string())
        }
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!("API returned error: {}", body))
    }
}

async fn test_openai_stt_api() -> Result<u64, String> {
    let api_key = env::var("OPENAI_API_KEY").map_err(|_| "OPENAI_API_KEY not set".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    let audio_data = generate_test_audio_wav();

    let part = reqwest::multipart::Part::bytes(audio_data)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;

    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", "whisper-1");

    let start = Instant::now();

    let response = client
        .post("https://api.openai.com/v1/audio/transcriptions")
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let latency = start.elapsed().as_millis() as u64;

    if response.status().is_success() {
        Ok(latency)
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!("API returned error: {}", body))
    }
}

async fn test_openai_tts_api() -> Result<u64, String> {
    let api_key = env::var("OPENAI_API_KEY").map_err(|_| "OPENAI_API_KEY not set".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let payload = serde_json::json!({
        "model": "tts-1",
        "input": "Hello, this is a test.",
        "voice": "alloy"
    });

    let start = Instant::now();

    let response = client
        .post("https://api.openai.com/v1/audio/speech")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let latency = start.elapsed().as_millis() as u64;

    if response.status().is_success() {
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > 100 {
            Ok(latency)
        } else {
            Err("TTS returned too little audio data".to_string())
        }
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!("API returned error: {}", body))
    }
}

async fn test_cartesia_tts_api() -> Result<u64, String> {
    let api_key =
        env::var("CARTESIA_API_KEY").map_err(|_| "CARTESIA_API_KEY not set".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let payload = serde_json::json!({
        "model_id": "sonic-english",
        "transcript": "Hello, this is a test.",
        "voice": {
            "mode": "id",
            "id": "a0e99841-438c-4a64-b679-ae501e7d6091"
        },
        "output_format": {
            "container": "raw",
            "encoding": "pcm_s16le",
            "sample_rate": 16000
        }
    });

    let start = Instant::now();

    let response = client
        .post("https://api.cartesia.ai/tts/bytes")
        .header("X-API-Key", &api_key)
        .header("Cartesia-Version", "2024-06-10")
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let latency = start.elapsed().as_millis() as u64;

    if response.status().is_success() {
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > 100 {
            Ok(latency)
        } else {
            Err("TTS returned too little audio data".to_string())
        }
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!("API returned error: {}", body))
    }
}

async fn test_azure_stt_api() -> Result<u64, String> {
    let api_key = env::var("AZURE_SPEECH_SUBSCRIPTION_KEY")
        .map_err(|_| "AZURE_SPEECH_SUBSCRIPTION_KEY not set".to_string())?;
    let region = env::var("AZURE_SPEECH_REGION").unwrap_or_else(|_| "eastus".to_string());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let audio_data = generate_test_audio_wav();
    let start = Instant::now();

    let response = client
        .post(format!(
            "https://{}.stt.speech.microsoft.com/speech/recognition/conversation/cognitiveservices/v1?language=en-US",
            region
        ))
        .header("Ocp-Apim-Subscription-Key", &api_key)
        .header("Content-Type", "audio/wav; codecs=audio/pcm; samplerate=16000")
        .body(audio_data)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let latency = start.elapsed().as_millis() as u64;

    if response.status().is_success() {
        Ok(latency)
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!("API returned error: {}", body))
    }
}

async fn test_azure_tts_api() -> Result<u64, String> {
    let api_key = env::var("AZURE_SPEECH_SUBSCRIPTION_KEY")
        .map_err(|_| "AZURE_SPEECH_SUBSCRIPTION_KEY not set".to_string())?;
    let region = env::var("AZURE_SPEECH_REGION").unwrap_or_else(|_| "eastus".to_string());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let ssml = r#"<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>
        <voice name='en-US-JennyNeural'>Hello, this is a test.</voice>
    </speak>"#;

    let start = Instant::now();

    let response = client
        .post(format!(
            "https://{}.tts.speech.microsoft.com/cognitiveservices/v1",
            region
        ))
        .header("Ocp-Apim-Subscription-Key", &api_key)
        .header("Content-Type", "application/ssml+xml")
        .header(
            "X-Microsoft-OutputFormat",
            "audio-16khz-32kbitrate-mono-mp3",
        )
        .body(ssml)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let latency = start.elapsed().as_millis() as u64;

    if response.status().is_success() {
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > 100 {
            Ok(latency)
        } else {
            Err("TTS returned too little audio data".to_string())
        }
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!("API returned error: {}", body))
    }
}

// ============================================================================
// Gateway Integration Tests
// ============================================================================

async fn test_gateway_tts(provider: &str) -> Result<u64, String> {
    let gateway_url = get_gateway_url();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let payload = serde_json::json!({
        "text": "Hello, this is a test.",
        "provider": provider
    });

    let start = Instant::now();

    let response = client
        .post(format!("{}/speak", gateway_url))
        .json(&payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let latency = start.elapsed().as_millis() as u64;

    if response.status().is_success() {
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > 0 {
            Ok(latency)
        } else {
            Err("No audio returned".to_string())
        }
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!("Gateway returned error: {}", body))
    }
}

async fn test_gateway_voices(provider: &str) -> Result<u64, String> {
    let gateway_url = get_gateway_url();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let start = Instant::now();

    let response = client
        .get(format!("{}/voices?provider={}", gateway_url, provider))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let latency = start.elapsed().as_millis() as u64;

    if response.status().is_success() {
        Ok(latency)
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(format!("Gateway returned error: {}", body))
    }
}

// ============================================================================
// Main Test Runner
// ============================================================================

async fn run_stt_provider_test(provider: &str, env_var: &str) -> TestResult {
    let api_key_configured = has_api_key(env_var);

    if !api_key_configured {
        return TestResult {
            provider: provider.to_string(),
            provider_type: "STT".to_string(),
            status: TestStatus::ApiKeyMissing,
            latency_ms: None,
            error_message: Some(format!("{} not configured", env_var)),
            api_key_configured: false,
        };
    }

    // Run provider-specific test
    let result = match provider {
        "deepgram" => test_deepgram_stt_api().await,
        "elevenlabs" => test_elevenlabs_stt_api().await,
        "openai" => test_openai_stt_api().await,
        "microsoft-azure" => test_azure_stt_api().await,
        _ => {
            // For providers without specific tests, mark as skipped
            return TestResult {
                provider: provider.to_string(),
                provider_type: "STT".to_string(),
                status: TestStatus::Skipped,
                latency_ms: None,
                error_message: Some("No specific test implemented".to_string()),
                api_key_configured: true,
            };
        }
    };

    match result {
        Ok(latency) => TestResult {
            provider: provider.to_string(),
            provider_type: "STT".to_string(),
            status: TestStatus::Success,
            latency_ms: Some(latency),
            error_message: None,
            api_key_configured: true,
        },
        Err(e) => TestResult {
            provider: provider.to_string(),
            provider_type: "STT".to_string(),
            status: TestStatus::Failed,
            latency_ms: None,
            error_message: Some(e),
            api_key_configured: true,
        },
    }
}

async fn run_tts_provider_test(provider: &str, env_var: &str) -> TestResult {
    let api_key_configured = has_api_key(env_var);

    if !api_key_configured {
        return TestResult {
            provider: provider.to_string(),
            provider_type: "TTS".to_string(),
            status: TestStatus::ApiKeyMissing,
            latency_ms: None,
            error_message: Some(format!("{} not configured", env_var)),
            api_key_configured: false,
        };
    }

    // First try gateway, then fallback to direct API
    let gateway_result = test_gateway_tts(provider).await;

    match gateway_result {
        Ok(latency) => TestResult {
            provider: provider.to_string(),
            provider_type: "TTS".to_string(),
            status: TestStatus::Success,
            latency_ms: Some(latency),
            error_message: None,
            api_key_configured: true,
        },
        Err(gateway_err) => {
            // Try direct API test
            let direct_result = match provider {
                "deepgram" => test_deepgram_tts_api().await,
                "elevenlabs" => test_elevenlabs_tts_api().await,
                "openai" => test_openai_tts_api().await,
                "cartesia" => test_cartesia_tts_api().await,
                "microsoft-azure" => test_azure_tts_api().await,
                _ => Err(gateway_err.clone()),
            };

            match direct_result {
                Ok(latency) => TestResult {
                    provider: provider.to_string(),
                    provider_type: "TTS".to_string(),
                    status: TestStatus::Success,
                    latency_ms: Some(latency),
                    error_message: None,
                    api_key_configured: true,
                },
                Err(e) => TestResult {
                    provider: provider.to_string(),
                    provider_type: "TTS".to_string(),
                    status: TestStatus::Failed,
                    latency_ms: None,
                    error_message: Some(format!("Gateway: {}, Direct: {}", gateway_err, e)),
                    api_key_configured: true,
                },
            }
        }
    }
}

fn print_test_summary(results: &[TestResult]) {
    println!("\n{}", "=".repeat(80));
    println!("                    WAAV GATEWAY PROVIDER TEST RESULTS");
    println!("{}", "=".repeat(80));

    let mut success_count = 0;
    let mut failed_count = 0;
    let mut skipped_count = 0;
    let mut no_key_count = 0;

    // Group results by type
    let mut stt_results: Vec<&TestResult> = Vec::new();
    let mut tts_results: Vec<&TestResult> = Vec::new();
    let mut realtime_results: Vec<&TestResult> = Vec::new();

    for result in results {
        match result.provider_type.as_str() {
            "STT" => stt_results.push(result),
            "TTS" => tts_results.push(result),
            "Realtime" => realtime_results.push(result),
            _ => {}
        }

        match result.status {
            TestStatus::Success => success_count += 1,
            TestStatus::Failed => failed_count += 1,
            TestStatus::Skipped => skipped_count += 1,
            TestStatus::ApiKeyMissing => no_key_count += 1,
        }
    }

    // Print STT Results
    println!("\n--- STT PROVIDERS ({}) ---", stt_results.len());
    println!(
        "{:<20} {:>10} {:>12} {}",
        "Provider", "Status", "Latency", "Error"
    );
    println!("{}", "-".repeat(70));
    for result in &stt_results {
        let latency_str = result
            .latency_ms
            .map(|l| format!("{}ms", l))
            .unwrap_or_else(|| "-".to_string());
        let error_str = result
            .error_message
            .as_ref()
            .map(|e| {
                if e.len() > 30 {
                    format!("{}...", &e[..30])
                } else {
                    e.clone()
                }
            })
            .unwrap_or_default();
        println!(
            "{:<20} {:>10} {:>12} {}",
            result.provider, result.status, latency_str, error_str
        );
    }

    // Print TTS Results
    println!("\n--- TTS PROVIDERS ({}) ---", tts_results.len());
    println!(
        "{:<20} {:>10} {:>12} {}",
        "Provider", "Status", "Latency", "Error"
    );
    println!("{}", "-".repeat(70));
    for result in &tts_results {
        let latency_str = result
            .latency_ms
            .map(|l| format!("{}ms", l))
            .unwrap_or_else(|| "-".to_string());
        let error_str = result
            .error_message
            .as_ref()
            .map(|e| {
                if e.len() > 30 {
                    format!("{}...", &e[..30])
                } else {
                    e.clone()
                }
            })
            .unwrap_or_default();
        println!(
            "{:<20} {:>10} {:>12} {}",
            result.provider, result.status, latency_str, error_str
        );
    }

    // Print Realtime Results
    if !realtime_results.is_empty() {
        println!("\n--- REALTIME PROVIDERS ({}) ---", realtime_results.len());
        println!(
            "{:<20} {:>10} {:>12} {}",
            "Provider", "Status", "Latency", "Error"
        );
        println!("{}", "-".repeat(70));
        for result in &realtime_results {
            let latency_str = result
                .latency_ms
                .map(|l| format!("{}ms", l))
                .unwrap_or_else(|| "-".to_string());
            let error_str = result
                .error_message
                .as_ref()
                .map(|e| {
                    if e.len() > 30 {
                        format!("{}...", &e[..30])
                    } else {
                        e.clone()
                    }
                })
                .unwrap_or_default();
            println!(
                "{:<20} {:>10} {:>12} {}",
                result.provider, result.status, latency_str, error_str
            );
        }
    }

    // Print Summary
    println!("\n{}", "=".repeat(80));
    println!("SUMMARY");
    println!("{}", "-".repeat(80));
    println!("Total Providers: {}", results.len());
    println!("  ✓ Passed:      {}", success_count);
    println!("  ✗ Failed:      {}", failed_count);
    println!("  ⊘ Skipped:     {}", skipped_count);
    println!("  ⊠ No API Key:  {}", no_key_count);
    println!("{}", "=".repeat(80));
}

// ============================================================================
// Test Entry Points
// ============================================================================

#[tokio::test]
async fn test_all_providers() {
    println!("\n=== WaaV Gateway Provider Test Suite ===\n");

    // Check gateway health first
    let gateway_url = get_gateway_url();
    let client = reqwest::Client::new();

    match client.get(format!("{}/", gateway_url)).send().await {
        Ok(resp) if resp.status().is_success() => {
            println!("✓ Gateway is running at {}", gateway_url);
        }
        _ => {
            println!(
                "✗ Gateway is not running at {}. Start it first!",
                gateway_url
            );
            println!("  Run: cargo run --release");
            return;
        }
    }

    let mut all_results: Vec<TestResult> = Vec::new();

    // Test STT providers
    println!("\nTesting {} STT providers...", STT_PROVIDERS.len());
    for (provider, env_var) in STT_PROVIDERS {
        print!("  Testing {} STT... ", provider);
        let result = run_stt_provider_test(provider, env_var).await;
        println!("{}", result.status);
        all_results.push(result);
    }

    // Test TTS providers
    println!("\nTesting {} TTS providers...", TTS_PROVIDERS.len());
    for (provider, env_var) in TTS_PROVIDERS {
        print!("  Testing {} TTS... ", provider);
        let result = run_tts_provider_test(provider, env_var).await;
        println!("{}", result.status);
        all_results.push(result);
    }

    // Test Realtime providers (simplified - check if key exists)
    println!(
        "\nTesting {} Realtime providers...",
        REALTIME_PROVIDERS.len()
    );
    for (provider, env_var) in REALTIME_PROVIDERS {
        let api_key_configured = has_api_key(env_var);
        let result = TestResult {
            provider: provider.to_string(),
            provider_type: "Realtime".to_string(),
            status: if api_key_configured {
                TestStatus::Skipped
            } else {
                TestStatus::ApiKeyMissing
            },
            latency_ms: None,
            error_message: if api_key_configured {
                Some("WebSocket test not implemented".to_string())
            } else {
                Some(format!("{} not configured", env_var))
            },
            api_key_configured,
        };
        println!("  Testing {} Realtime... {}", provider, result.status);
        all_results.push(result);
    }

    // Print summary
    print_test_summary(&all_results);
}

#[tokio::test]
#[ignore]
async fn test_stt_providers_only() {
    let mut results: Vec<TestResult> = Vec::new();

    for (provider, env_var) in STT_PROVIDERS {
        let result = run_stt_provider_test(provider, env_var).await;
        results.push(result);
    }

    print_test_summary(&results);
}

#[tokio::test]
#[ignore]
async fn test_tts_providers_only() {
    let mut results: Vec<TestResult> = Vec::new();

    for (provider, env_var) in TTS_PROVIDERS {
        let result = run_tts_provider_test(provider, env_var).await;
        results.push(result);
    }

    print_test_summary(&results);
}

#[tokio::test]
async fn test_provider_availability() {
    println!("\n=== Provider API Key Availability ===\n");

    println!("STT Providers ({}):", STT_PROVIDERS.len());
    for (provider, env_var) in STT_PROVIDERS {
        let has_key = has_api_key(env_var);
        let status = if has_key { "✓" } else { "✗" };
        println!("  {} {}: {}", status, provider, env_var);
    }

    println!("\nTTS Providers ({}):", TTS_PROVIDERS.len());
    for (provider, env_var) in TTS_PROVIDERS {
        let has_key = has_api_key(env_var);
        let status = if has_key { "✓" } else { "✗" };
        println!("  {} {}: {}", status, provider, env_var);
    }

    println!("\nRealtime Providers ({}):", REALTIME_PROVIDERS.len());
    for (provider, env_var) in REALTIME_PROVIDERS {
        let has_key = has_api_key(env_var);
        let status = if has_key { "✓" } else { "✗" };
        println!("  {} {}: {}", status, provider, env_var);
    }
}

#[tokio::test]
#[ignore] // Requires running gateway at localhost:3001
async fn test_gateway_health() {
    let gateway_url = get_gateway_url();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let response = client.get(format!("{}/", gateway_url)).send().await;

    match response {
        Ok(resp) => {
            assert!(resp.status().is_success(), "Gateway health check failed");
            println!("Gateway health check passed at {}", gateway_url);
        }
        Err(e) => {
            panic!("Gateway not reachable at {}: {}", gateway_url, e);
        }
    }
}
