//! WebSocket Mock Server for STT/TTS Providers
//!
//! Simulates WebSocket-based providers like Deepgram, Cartesia, LMNT

use super::{ChaosConfig, LatencyProfile, MockStats};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message};

/// WebSocket Mock Server State
pub struct WebSocketMockState {
    pub stt_latency: LatencyProfile,
    pub tts_latency: LatencyProfile,
    pub chaos: ChaosConfig,
    pub stats: MockStats,
    pub connection_count: AtomicU64,
}

impl WebSocketMockState {
    pub fn new(stt_latency: LatencyProfile, tts_latency: LatencyProfile, chaos: ChaosConfig) -> Self {
        Self {
            stt_latency,
            tts_latency,
            chaos,
            stats: MockStats::default(),
            connection_count: AtomicU64::new(0),
        }
    }

    pub fn deepgram() -> Self {
        Self::new(
            LatencyProfile::deepgram_stt(),
            LatencyProfile::deepgram_tts(),
            ChaosConfig::production(),
        )
    }

    pub fn deepgram_chaos() -> Self {
        Self::new(
            LatencyProfile::deepgram_stt(),
            LatencyProfile::deepgram_tts(),
            ChaosConfig::stress(),
        )
    }

    pub fn cartesia() -> Self {
        Self::new(
            LatencyProfile::deepgram_stt(),
            LatencyProfile::cartesia_tts(),
            ChaosConfig::production(),
        )
    }
}

/// Handle a single WebSocket connection (Deepgram STT style)
async fn handle_stt_connection(
    stream: TcpStream,
    state: Arc<WebSocketMockState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws_stream = accept_async(stream).await?;
    let (mut write, mut read) = ws_stream.split();

    state.connection_count.fetch_add(1, Ordering::Relaxed);
    let conn_id = state.connection_count.load(Ordering::Relaxed);

    // Check for connection drop chaos
    if state.chaos.should_drop() {
        state.stats.record_drop();
        return Ok(());
    }

    // Send initial metadata (Deepgram style)
    let metadata = json!({
        "type": "Metadata",
        "transaction_key": format!("mock-{}", conn_id),
        "request_id": format!("req-{}", conn_id),
        "sha256": "mock-sha256",
        "created": "2024-01-01T00:00:00.000Z",
        "duration": 0.0,
        "channels": 1,
        "models": ["nova-2"],
    });
    write.send(Message::Text(metadata.to_string().into())).await?;

    let mut audio_chunk_count = 0u64;

    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Binary(_audio_data)) => {
                audio_chunk_count += 1;
                let start = Instant::now();

                // Check for chaos conditions
                if state.chaos.should_fail() {
                    state.stats.record_failure();
                    let error = json!({
                        "type": "Error",
                        "message": "Mock provider error",
                        "code": "MOCK_ERROR"
                    });
                    write.send(Message::Text(error.to_string().into())).await?;
                    continue;
                }

                if state.chaos.should_timeout() {
                    state.stats.record_timeout();
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    continue;
                }

                // Simulate STT processing latency
                let mut latency = state.stt_latency.sample();
                let multiplier = state.chaos.slow_multiplier();
                if multiplier > 1 {
                    latency *= multiplier;
                }
                tokio::time::sleep(latency).await;

                // Send transcript result (Deepgram format)
                let transcript = json!({
                    "type": "Results",
                    "channel_index": [0, 1],
                    "duration": 0.5,
                    "start": (audio_chunk_count as f64 - 1.0) * 0.5,
                    "is_final": true,
                    "speech_final": audio_chunk_count % 5 == 0,
                    "channel": {
                        "alternatives": [{
                            "transcript": format!("Mock transcript chunk {}", audio_chunk_count),
                            "confidence": 0.95,
                            "words": [{
                                "word": "mock",
                                "start": 0.0,
                                "end": 0.2,
                                "confidence": 0.95
                            }]
                        }]
                    }
                });

                let latency_ms = start.elapsed().as_millis() as u64;
                state.stats.record_success(latency_ms);

                write.send(Message::Text(transcript.to_string().into())).await?;
            }
            Ok(Message::Text(text)) => {
                // Handle control messages
                if let Ok(msg) = serde_json::from_str::<Value>(&text) {
                    if msg.get("type").and_then(|t| t.as_str()) == Some("CloseStream") {
                        break;
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(data)) => {
                write.send(Message::Pong(data)).await?;
            }
            Err(e) => {
                eprintln!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    state.connection_count.fetch_sub(1, Ordering::Relaxed);
    Ok(())
}

/// Handle a TTS WebSocket connection (Deepgram Aura style)
async fn handle_tts_connection(
    stream: TcpStream,
    state: Arc<WebSocketMockState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws_stream = accept_async(stream).await?;
    let (mut write, mut read) = ws_stream.split();

    state.connection_count.fetch_add(1, Ordering::Relaxed);

    // Check for connection drop chaos
    if state.chaos.should_drop() {
        state.stats.record_drop();
        return Ok(());
    }

    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let start = Instant::now();

                // Parse speak request
                if let Ok(request) = serde_json::from_str::<Value>(&text) {
                    let text_to_speak = request.get("text").and_then(|t| t.as_str()).unwrap_or("");

                    // Check chaos conditions
                    if state.chaos.should_fail() {
                        state.stats.record_failure();
                        let error = json!({"type": "Error", "message": "Mock TTS error"});
                        write.send(Message::Text(error.to_string().into())).await?;
                        continue;
                    }

                    // Simulate TTS processing latency
                    let mut latency = state.tts_latency.sample();
                    let multiplier = state.chaos.slow_multiplier();
                    if multiplier > 1 {
                        latency *= multiplier;
                    }
                    tokio::time::sleep(latency).await;

                    // Send audio chunks (simulate streaming)
                    let chunk_count = (text_to_speak.len() / 10).max(3);
                    for _i in 0..chunk_count {
                        // 1KB audio chunk
                        let audio_chunk: Bytes = vec![0u8; 1024].into();
                        write.send(Message::Binary(audio_chunk)).await?;

                        // Small delay between chunks for streaming simulation
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }

                    // Send completion
                    let complete = json!({"type": "Flushed"});
                    write.send(Message::Text(complete.to_string().into())).await?;

                    let latency_ms = start.elapsed().as_millis() as u64;
                    state.stats.record_success(latency_ms);
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(data)) => {
                write.send(Message::Pong(data)).await?;
            }
            Err(e) => {
                eprintln!("TTS WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    state.connection_count.fetch_sub(1, Ordering::Relaxed);
    Ok(())
}

/// Start WebSocket mock server for STT
pub async fn start_stt_websocket_mock(port: u16, state: Arc<WebSocketMockState>) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    println!("STT WebSocket Mock Server listening on port {}", port);

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_stt_connection(stream, state).await {
                eprintln!("STT connection error: {}", e);
            }
        });
    }
}

/// Start WebSocket mock server for TTS
pub async fn start_tts_websocket_mock(port: u16, state: Arc<WebSocketMockState>) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    println!("TTS WebSocket Mock Server listening on port {}", port);

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_tts_connection(stream, state).await {
                eprintln!("TTS connection error: {}", e);
            }
        });
    }
}

/// Spawn STT WebSocket mock in background
pub fn spawn_stt_websocket_mock(port: u16, state: Arc<WebSocketMockState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = start_stt_websocket_mock(port, state).await {
            eprintln!("STT WebSocket Mock error: {}", e);
        }
    })
}

/// Spawn TTS WebSocket mock in background
pub fn spawn_tts_websocket_mock(port: u16, state: Arc<WebSocketMockState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = start_tts_websocket_mock(port, state).await {
            eprintln!("TTS WebSocket Mock error: {}", e);
        }
    })
}
