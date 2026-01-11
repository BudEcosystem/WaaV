//! gRPC Mock Server for Google Speech Services
//!
//! Simulates Google Cloud Speech-to-Text and Text-to-Speech gRPC APIs
//! Note: This is a simplified mock that simulates gRPC behavior over HTTP/2

use super::{ChaosConfig, LatencyProfile, MockStats};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

/// gRPC Mock Server State
pub struct GrpcMockState {
    pub stt_latency: LatencyProfile,
    pub tts_latency: LatencyProfile,
    pub chaos: ChaosConfig,
    pub stats: MockStats,
}

impl GrpcMockState {
    pub fn new(stt_latency: LatencyProfile, tts_latency: LatencyProfile, chaos: ChaosConfig) -> Self {
        Self {
            stt_latency,
            tts_latency,
            chaos,
            stats: MockStats::default(),
        }
    }

    pub fn google() -> Self {
        Self::new(
            LatencyProfile::google_stt(),
            LatencyProfile::google_tts(),
            ChaosConfig::production(),
        )
    }

    pub fn google_chaos() -> Self {
        Self::new(
            LatencyProfile::google_stt(),
            LatencyProfile::google_tts(),
            ChaosConfig::stress(),
        )
    }
}

/// Simulated gRPC streaming response for STT
pub struct MockSttStream {
    state: Arc<GrpcMockState>,
    rx: mpsc::Receiver<Vec<u8>>,
}

impl MockSttStream {
    pub fn new(state: Arc<GrpcMockState>) -> (Self, mpsc::Sender<Vec<u8>>) {
        let (tx, rx) = mpsc::channel(100);
        (Self { state, rx }, tx)
    }

    /// Process audio chunk and return transcript
    pub async fn process_chunk(&mut self) -> Option<MockSttResult> {
        let audio = self.rx.recv().await?;
        let start = Instant::now();

        // Check chaos conditions
        if self.state.chaos.should_fail() {
            self.state.stats.record_failure();
            return Some(MockSttResult::Error("INTERNAL".to_string()));
        }

        if self.state.chaos.should_timeout() {
            self.state.stats.record_timeout();
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            return Some(MockSttResult::Error("DEADLINE_EXCEEDED".to_string()));
        }

        // Simulate processing latency
        let mut latency = self.state.stt_latency.sample();
        let multiplier = self.state.chaos.slow_multiplier();
        if multiplier > 1 {
            latency *= multiplier;
        }
        tokio::time::sleep(latency).await;

        let latency_ms = start.elapsed().as_millis() as u64;
        self.state.stats.record_success(latency_ms);

        Some(MockSttResult::Transcript {
            text: "Mock Google transcript".to_string(),
            confidence: 0.95,
            is_final: true,
            latency_ms,
        })
    }
}

/// Mock STT result
#[derive(Debug)]
pub enum MockSttResult {
    Transcript {
        text: String,
        confidence: f32,
        is_final: bool,
        latency_ms: u64,
    },
    Error(String),
}

/// Simulated gRPC TTS response
pub struct MockTtsResponse {
    pub audio_content: Vec<u8>,
    pub latency_ms: u64,
}

/// Process TTS request
pub async fn process_tts_request(
    state: Arc<GrpcMockState>,
    text: &str,
) -> Result<MockTtsResponse, String> {
    let start = Instant::now();

    // Check chaos conditions
    if state.chaos.should_fail() {
        state.stats.record_failure();
        return Err("INTERNAL".to_string());
    }

    if state.chaos.should_timeout() {
        state.stats.record_timeout();
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        return Err("DEADLINE_EXCEEDED".to_string());
    }

    if state.chaos.should_rate_limit() {
        state.stats.record_rate_limit();
        return Err("RESOURCE_EXHAUSTED".to_string());
    }

    // Simulate processing latency
    let mut latency = state.tts_latency.sample();
    let multiplier = state.chaos.slow_multiplier();
    if multiplier > 1 {
        latency *= multiplier;
    }
    tokio::time::sleep(latency).await;

    let latency_ms = start.elapsed().as_millis() as u64;
    state.stats.record_success(latency_ms);

    // Generate mock audio (size based on text length)
    let audio_size = text.len() * 100; // ~100 bytes per character
    let audio_content = vec![0u8; audio_size.max(1024)];

    Ok(MockTtsResponse {
        audio_content,
        latency_ms,
    })
}

// Note: Full gRPC server would require tonic, which adds complexity.
// For load testing, we can use the HTTP mock with gRPC-like latency profiles,
// or implement a proper tonic-based mock server.
//
// The key insight is that gRPC latency characteristics are captured in
// LatencyProfile::google_stt() and LatencyProfile::google_tts().
