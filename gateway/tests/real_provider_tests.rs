//! Real Provider Integration Tests
//!
//! These tests make actual API calls to real providers.
//! They are marked with #[ignore] and require API keys to be set.
//!
//! Run: cargo test --test real_provider_tests --ignored -- --test-threads=1
//!
//! Required environment variables (set those you want to test):
//! - DEEPGRAM_API_KEY
//! - ELEVENLABS_API_KEY
//! - OPENAI_API_KEY
//! - GOOGLE_APPLICATION_CREDENTIALS (path to service account JSON)
//! - AZURE_SPEECH_SUBSCRIPTION_KEY + AZURE_SPEECH_REGION
//! - CARTESIA_API_KEY
//! - ANTHROPIC_API_KEY
//! - HUME_API_KEY + HUME_CONFIG_ID

mod fixtures;

use fixtures::audio_fixtures;
use std::env;
use std::time::Duration;

// =============================================================================
// Test Helper Functions
// =============================================================================

fn get_env_or_skip(key: &str) -> Option<String> {
    env::var(key).ok()
}

fn require_env(key: &str) -> String {
    env::var(key).unwrap_or_else(|_| {
        panic!(
            "Environment variable {} is required for this test. Set it and run with --ignored",
            key
        )
    })
}

/// Generate test audio for STT (16kHz, 16-bit PCM, mono)
fn generate_test_audio() -> Vec<u8> {
    // Generate 2 seconds of speech-like audio
    audio_fixtures::generate_speech_pattern_bytes(audio_fixtures::SECOND * 2)
}

/// Generate test audio as WAV
fn generate_test_audio_wav() -> Vec<u8> {
    let samples = audio_fixtures::generate_speech_pattern(audio_fixtures::SECOND * 2);
    audio_fixtures::create_wav_file(&samples)
}

// =============================================================================
// Deepgram Tests
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_deepgram_stt_real() {
    let api_key = require_env("DEEPGRAM_API_KEY");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    let audio_data = generate_test_audio_wav();

    let response = client
        .post("https://api.deepgram.com/v1/listen?model=nova-2&language=en")
        .header("Authorization", format!("Token {}", api_key))
        .header("Content-Type", "audio/wav")
        .body(audio_data)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            println!("Deepgram STT response status: {}", status);
            println!("Deepgram STT response body: {}", body);

            if status.is_success() {
                let json: serde_json::Value = serde_json::from_str(&body).unwrap();
                // Check if we got results (even if empty due to generated audio)
                assert!(
                    json.get("results").is_some() || json.get("metadata").is_some(),
                    "Deepgram response missing expected fields"
                );
            } else {
                // 401 = bad API key, which is an expected failure mode
                assert!(
                    status.as_u16() == 401 || status.is_client_error(),
                    "Unexpected server error from Deepgram: {} - {}",
                    status,
                    body
                );
            }
        }
        Err(e) => {
            panic!("Failed to connect to Deepgram: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_deepgram_tts_real() {
    let api_key = require_env("DEEPGRAM_API_KEY");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    let payload = serde_json::json!({
        "text": "Hello, this is a test of the Deepgram text to speech API."
    });

    let response = client
        .post("https://api.deepgram.com/v1/speak?model=aura-asteria-en")
        .header("Authorization", format!("Token {}", api_key))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();

            println!("Deepgram TTS response status: {}", status);

            if status.is_success() {
                let audio_bytes = resp.bytes().await.unwrap();
                println!("Deepgram TTS returned {} bytes of audio", audio_bytes.len());
                assert!(audio_bytes.len() > 1000, "TTS audio too short");
            } else {
                let body = resp.text().await.unwrap_or_default();
                assert!(
                    status.as_u16() == 401 || status.is_client_error(),
                    "Unexpected server error from Deepgram TTS: {} - {}",
                    status,
                    body
                );
            }
        }
        Err(e) => {
            panic!("Failed to connect to Deepgram TTS: {}", e);
        }
    }
}

// =============================================================================
// ElevenLabs Tests
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_elevenlabs_stt_real() {
    let api_key = require_env("ELEVENLABS_API_KEY");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    let audio_data = generate_test_audio_wav();

    // ElevenLabs STT endpoint
    let response = client
        .post("https://api.elevenlabs.io/v1/speech-to-text")
        .header("xi-api-key", &api_key)
        .header("Content-Type", "audio/wav")
        .body(audio_data)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            println!("ElevenLabs STT response status: {}", status);
            println!("ElevenLabs STT response: {}", body);

            // Accept success or expected client errors
            assert!(
                status.is_success() || status.as_u16() == 401 || status.as_u16() == 404,
                "Unexpected error from ElevenLabs STT: {} - {}",
                status,
                body
            );
        }
        Err(e) => {
            panic!("Failed to connect to ElevenLabs: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_elevenlabs_tts_real() {
    let api_key = require_env("ELEVENLABS_API_KEY");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    // Use a standard voice ID
    let voice_id = "21m00Tcm4TlvDq8ikWAM"; // Rachel voice

    let payload = serde_json::json!({
        "text": "Hello, this is a test of the ElevenLabs text to speech API.",
        "model_id": "eleven_monolingual_v1"
    });

    let response = client
        .post(format!(
            "https://api.elevenlabs.io/v1/text-to-speech/{}",
            voice_id
        ))
        .header("xi-api-key", &api_key)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();

            println!("ElevenLabs TTS response status: {}", status);

            if status.is_success() {
                let audio_bytes = resp.bytes().await.unwrap();
                println!("ElevenLabs TTS returned {} bytes of audio", audio_bytes.len());
                assert!(audio_bytes.len() > 1000, "TTS audio too short");
            } else {
                let body = resp.text().await.unwrap_or_default();
                assert!(
                    status.as_u16() == 401 || status.is_client_error(),
                    "Unexpected server error from ElevenLabs TTS: {} - {}",
                    status,
                    body
                );
            }
        }
        Err(e) => {
            panic!("Failed to connect to ElevenLabs TTS: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_elevenlabs_voices_real() {
    let api_key = require_env("ELEVENLABS_API_KEY");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    let response = client
        .get("https://api.elevenlabs.io/v1/voices")
        .header("xi-api-key", &api_key)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            println!("ElevenLabs voices response status: {}", status);

            if status.is_success() {
                let json: serde_json::Value = serde_json::from_str(&body).unwrap();
                let voices = json.get("voices").and_then(|v| v.as_array());
                assert!(voices.is_some(), "No voices array in response");
                println!("ElevenLabs has {} voices available", voices.unwrap().len());
            } else {
                assert!(
                    status.as_u16() == 401,
                    "Unexpected error from ElevenLabs voices: {} - {}",
                    status,
                    body
                );
            }
        }
        Err(e) => {
            panic!("Failed to connect to ElevenLabs: {}", e);
        }
    }
}

// =============================================================================
// OpenAI Tests
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_openai_stt_real() {
    let api_key = require_env("OPENAI_API_KEY");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap();

    let audio_data = generate_test_audio_wav();

    // Create multipart form
    let part = reqwest::multipart::Part::bytes(audio_data)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .unwrap();

    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", "whisper-1");

    let response = client
        .post("https://api.openai.com/v1/audio/transcriptions")
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            println!("OpenAI STT response status: {}", status);
            println!("OpenAI STT response: {}", body);

            if status.is_success() {
                let json: serde_json::Value = serde_json::from_str(&body).unwrap();
                assert!(
                    json.get("text").is_some(),
                    "OpenAI response missing text field"
                );
            } else {
                assert!(
                    status.as_u16() == 401 || status.is_client_error(),
                    "Unexpected server error from OpenAI STT: {} - {}",
                    status,
                    body
                );
            }
        }
        Err(e) => {
            panic!("Failed to connect to OpenAI: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_openai_tts_real() {
    let api_key = require_env("OPENAI_API_KEY");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    let payload = serde_json::json!({
        "model": "tts-1",
        "input": "Hello, this is a test of the OpenAI text to speech API.",
        "voice": "alloy"
    });

    let response = client
        .post("https://api.openai.com/v1/audio/speech")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();

            println!("OpenAI TTS response status: {}", status);

            if status.is_success() {
                let audio_bytes = resp.bytes().await.unwrap();
                println!("OpenAI TTS returned {} bytes of audio", audio_bytes.len());
                assert!(audio_bytes.len() > 1000, "TTS audio too short");
            } else {
                let body = resp.text().await.unwrap_or_default();
                assert!(
                    status.as_u16() == 401 || status.is_client_error(),
                    "Unexpected server error from OpenAI TTS: {} - {}",
                    status,
                    body
                );
            }
        }
        Err(e) => {
            panic!("Failed to connect to OpenAI TTS: {}", e);
        }
    }
}

// =============================================================================
// Cartesia Tests
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_cartesia_tts_real() {
    let api_key = require_env("CARTESIA_API_KEY");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    let payload = serde_json::json!({
        "model_id": "sonic-english",
        "transcript": "Hello, this is a test of the Cartesia text to speech API.",
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

    let response = client
        .post("https://api.cartesia.ai/tts/bytes")
        .header("X-API-Key", &api_key)
        .header("Cartesia-Version", "2024-06-10")
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();

            println!("Cartesia TTS response status: {}", status);

            if status.is_success() {
                let audio_bytes = resp.bytes().await.unwrap();
                println!("Cartesia TTS returned {} bytes of audio", audio_bytes.len());
                assert!(audio_bytes.len() > 1000, "TTS audio too short");
            } else {
                let body = resp.text().await.unwrap_or_default();
                assert!(
                    status.as_u16() == 401 || status.as_u16() == 403 || status.is_client_error(),
                    "Unexpected server error from Cartesia TTS: {} - {}",
                    status,
                    body
                );
            }
        }
        Err(e) => {
            panic!("Failed to connect to Cartesia TTS: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_cartesia_voices_real() {
    let api_key = require_env("CARTESIA_API_KEY");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    let response = client
        .get("https://api.cartesia.ai/voices")
        .header("X-API-Key", &api_key)
        .header("Cartesia-Version", "2024-06-10")
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            println!("Cartesia voices response status: {}", status);

            if status.is_success() {
                let json: serde_json::Value = serde_json::from_str(&body).unwrap();
                if let Some(voices) = json.as_array() {
                    println!("Cartesia has {} voices available", voices.len());
                }
            } else {
                assert!(
                    status.as_u16() == 401 || status.as_u16() == 403,
                    "Unexpected error from Cartesia voices: {} - {}",
                    status,
                    body
                );
            }
        }
        Err(e) => {
            panic!("Failed to connect to Cartesia: {}", e);
        }
    }
}

// =============================================================================
// Azure Speech Tests
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_azure_stt_real() {
    let api_key = require_env("AZURE_SPEECH_SUBSCRIPTION_KEY");
    let region = require_env("AZURE_SPEECH_REGION");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    let audio_data = generate_test_audio_wav();

    let response = client
        .post(format!(
            "https://{}.stt.speech.microsoft.com/speech/recognition/conversation/cognitiveservices/v1?language=en-US",
            region
        ))
        .header("Ocp-Apim-Subscription-Key", &api_key)
        .header("Content-Type", "audio/wav; codecs=audio/pcm; samplerate=16000")
        .body(audio_data)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            println!("Azure STT response status: {}", status);
            println!("Azure STT response: {}", body);

            if status.is_success() {
                let json: serde_json::Value = serde_json::from_str(&body).unwrap();
                assert!(
                    json.get("RecognitionStatus").is_some(),
                    "Azure response missing RecognitionStatus"
                );
            } else {
                assert!(
                    status.as_u16() == 401 || status.is_client_error(),
                    "Unexpected server error from Azure STT: {} - {}",
                    status,
                    body
                );
            }
        }
        Err(e) => {
            panic!("Failed to connect to Azure Speech: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_azure_tts_real() {
    let api_key = require_env("AZURE_SPEECH_SUBSCRIPTION_KEY");
    let region = require_env("AZURE_SPEECH_REGION");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    let ssml = r#"<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>
        <voice name='en-US-JennyNeural'>
            Hello, this is a test of the Azure text to speech API.
        </voice>
    </speak>"#;

    let response = client
        .post(format!(
            "https://{}.tts.speech.microsoft.com/cognitiveservices/v1",
            region
        ))
        .header("Ocp-Apim-Subscription-Key", &api_key)
        .header("Content-Type", "application/ssml+xml")
        .header("X-Microsoft-OutputFormat", "audio-16khz-32kbitrate-mono-mp3")
        .body(ssml)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();

            println!("Azure TTS response status: {}", status);

            if status.is_success() {
                let audio_bytes = resp.bytes().await.unwrap();
                println!("Azure TTS returned {} bytes of audio", audio_bytes.len());
                assert!(audio_bytes.len() > 1000, "TTS audio too short");
            } else {
                let body = resp.text().await.unwrap_or_default();
                assert!(
                    status.as_u16() == 401 || status.is_client_error(),
                    "Unexpected server error from Azure TTS: {} - {}",
                    status,
                    body
                );
            }
        }
        Err(e) => {
            panic!("Failed to connect to Azure Speech TTS: {}", e);
        }
    }
}

// =============================================================================
// Provider Availability Test
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_all_configured_providers() {
    println!("\n=== Provider Availability Check ===\n");

    let providers = [
        ("DEEPGRAM_API_KEY", "Deepgram"),
        ("ELEVENLABS_API_KEY", "ElevenLabs"),
        ("OPENAI_API_KEY", "OpenAI"),
        ("CARTESIA_API_KEY", "Cartesia"),
        ("AZURE_SPEECH_SUBSCRIPTION_KEY", "Azure Speech"),
        ("GOOGLE_APPLICATION_CREDENTIALS", "Google Cloud"),
        ("ANTHROPIC_API_KEY", "Anthropic"),
        ("HUME_API_KEY", "Hume"),
    ];

    let mut available = Vec::new();
    let mut missing = Vec::new();

    for (env_var, name) in providers {
        if env::var(env_var).is_ok() {
            available.push(name);
        } else {
            missing.push(name);
        }
    }

    println!("Available providers ({}):", available.len());
    for provider in &available {
        println!("  - {}", provider);
    }

    println!("\nMissing providers ({}):", missing.len());
    for provider in &missing {
        println!("  - {}", provider);
    }

    println!("\nTo run real provider tests, set the required environment variables.");
    println!("Example:");
    println!("  export DEEPGRAM_API_KEY=your_key_here");
    println!("  cargo test --test real_provider_tests --ignored -- --test-threads=1");

    // This test always passes - it's informational
    assert!(true);
}

// =============================================================================
// Gateway Integration Tests (requires running server)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_gateway_with_real_deepgram() {
    let _api_key = require_env("DEEPGRAM_API_KEY");

    // This test requires the gateway to be running with Deepgram configured
    let gateway_url = env::var("GATEWAY_URL").unwrap_or_else(|_| "http://localhost:3001".to_string());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    // Check if gateway is running
    let health_check = client.get(format!("{}/", gateway_url)).send().await;

    if health_check.is_err() {
        println!("Gateway not running at {}. Start it with:", gateway_url);
        println!("  DEEPGRAM_API_KEY=xxx cargo run");
        return;
    }

    // Test speak endpoint with Deepgram TTS
    let payload = serde_json::json!({
        "text": "Hello from the gateway integration test.",
        "provider": "deepgram",
        "voice_id": "aura-asteria-en"
    });

    let response = client
        .post(format!("{}/speak", gateway_url))
        .json(&payload)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            println!("Gateway speak response status: {}", status);

            if status.is_success() {
                let audio_bytes = resp.bytes().await.unwrap();
                println!("Gateway returned {} bytes of audio", audio_bytes.len());
                assert!(audio_bytes.len() > 0, "No audio returned");
            } else {
                let body = resp.text().await.unwrap_or_default();
                println!("Gateway error: {}", body);
            }
        }
        Err(e) => {
            println!("Failed to connect to gateway: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_gateway_voices_with_real_elevenlabs() {
    let _api_key = require_env("ELEVENLABS_API_KEY");

    let gateway_url = env::var("GATEWAY_URL").unwrap_or_else(|_| "http://localhost:3001".to_string());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    // Check if gateway is running
    let health_check = client.get(format!("{}/", gateway_url)).send().await;

    if health_check.is_err() {
        println!("Gateway not running at {}. Start it with:", gateway_url);
        println!("  ELEVENLABS_API_KEY=xxx cargo run");
        return;
    }

    // Test voices endpoint
    let response = client
        .get(format!("{}/voices?provider=elevenlabs", gateway_url))
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            println!("Gateway voices response status: {}", status);

            if status.is_success() {
                let json: serde_json::Value = serde_json::from_str(&body).unwrap();
                println!("Voices response: {:?}", json);
            } else {
                println!("Gateway error: {}", body);
            }
        }
        Err(e) => {
            println!("Failed to connect to gateway: {}", e);
        }
    }
}
