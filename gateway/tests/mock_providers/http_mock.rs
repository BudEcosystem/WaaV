//! HTTP Mock Server for TTS Providers
//!
//! Simulates HTTP-based TTS providers like ElevenLabs, PlayHT, OpenAI TTS

use super::{ChaosConfig, LatencyProfile, MockStats};
use axum::{
    body::Body,
    extract::State,
    http::{Response, StatusCode},
    routing::post,
    Router,
};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;

/// HTTP Mock Server State
pub struct HttpMockState {
    pub latency: LatencyProfile,
    pub chaos: ChaosConfig,
    pub stats: MockStats,
}

impl HttpMockState {
    pub fn new(latency: LatencyProfile, chaos: ChaosConfig) -> Self {
        Self {
            latency,
            chaos,
            stats: MockStats::default(),
        }
    }

    pub fn elevenlabs() -> Self {
        Self::new(LatencyProfile::elevenlabs_tts(), ChaosConfig::production())
    }

    pub fn elevenlabs_chaos() -> Self {
        Self::new(LatencyProfile::elevenlabs_tts(), ChaosConfig::stress())
    }
}

/// TTS endpoint handler
async fn tts_handler(State(state): State<Arc<HttpMockState>>) -> Response<Body> {
    let start = Instant::now();

    // Check for chaos conditions
    if state.chaos.should_drop() {
        state.stats.record_drop();
        // Simulate connection drop by returning nothing
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(Body::empty())
            .unwrap();
    }

    if state.chaos.should_rate_limit() {
        state.stats.record_rate_limit();
        return Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header("Retry-After", "60")
            .body(Body::from("Rate limit exceeded"))
            .unwrap();
    }

    if state.chaos.should_timeout() {
        state.stats.record_timeout();
        // Simulate timeout by sleeping for a very long time
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        return Response::builder()
            .status(StatusCode::GATEWAY_TIMEOUT)
            .body(Body::empty())
            .unwrap();
    }

    if state.chaos.should_fail() {
        state.stats.record_failure();
        return Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from(r#"{"error": "Internal server error"}"#))
            .unwrap();
    }

    // Calculate latency with slow multiplier
    let mut latency = state.latency.sample();
    let multiplier = state.chaos.slow_multiplier();
    if multiplier > 1 {
        latency *= multiplier;
    }

    // Simulate provider processing time
    tokio::time::sleep(latency).await;

    let latency_ms = start.elapsed().as_millis() as u64;
    state.stats.record_success(latency_ms);

    // Return mock audio data (16KB of PCM-like data)
    let mock_audio = vec![0u8; 16384];

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "audio/mpeg")
        .header("X-Mock-Latency-Ms", latency_ms.to_string())
        .body(Body::from(mock_audio))
        .unwrap()
}

/// Voices endpoint handler
async fn voices_handler(State(state): State<Arc<HttpMockState>>) -> Response<Body> {
    let latency = state.latency.sample();
    tokio::time::sleep(latency).await;

    let voices = r#"{
        "voices": [
            {"voice_id": "mock-voice-1", "name": "Mock Voice 1"},
            {"voice_id": "mock-voice-2", "name": "Mock Voice 2"}
        ]
    }"#;

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Body::from(voices))
        .unwrap()
}

/// Stats endpoint
async fn stats_handler(State(state): State<Arc<HttpMockState>>) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain")
        .body(Body::from(state.stats.summary()))
        .unwrap()
}

/// Start HTTP mock server
pub async fn start_http_mock(port: u16, state: HttpMockState) -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(state);

    let app = Router::new()
        // ElevenLabs-like endpoints (use {param} syntax for Axum 0.7+)
        .route("/v1/text-to-speech/{voice_id}", post(tts_handler.clone()))
        .route("/v1/text-to-speech/{voice_id}/stream", post(tts_handler.clone()))
        .route("/v1/voices", axum::routing::get(voices_handler.clone()))
        // OpenAI-like endpoints
        .route("/v1/audio/speech", post(tts_handler.clone()))
        // PlayHT-like endpoints
        .route("/api/v2/tts", post(tts_handler.clone()))
        // Stats
        .route("/stats", axum::routing::get(stats_handler))
        .with_state(state);

    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    println!("HTTP Mock Server listening on port {}", port);

    axum::serve(listener, app).await?;
    Ok(())
}

/// Start HTTP mock server in background
pub fn spawn_http_mock(port: u16, state: HttpMockState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = start_http_mock(port, state).await {
            eprintln!("HTTP Mock Server error: {}", e);
        }
    })
}
