//! WebSocket Mock Server for STT/TTS Providers
//!
//! Simulates WebSocket-based providers like Deepgram, Cartesia, LMNT

use super::{ChaosConfig, LatencyProfile, MockStats};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Number of `{"type":"Speak"}` frames received by the TTS mock (protocol-accurate path).
    pub tts_speak_frames: AtomicU64,
    /// Number of `{"type":"Flush"}` frames received by the TTS mock.
    pub tts_flush_frames: AtomicU64,
    /// Number of `{"type":"Clear"}` frames received by the TTS mock.
    pub tts_clear_frames: AtomicU64,
    /// Accumulated text of the most recent flushed synthesis (what `Flush` synthesized).
    pub tts_last_flushed_text: std::sync::Mutex<String>,
}

impl WebSocketMockState {
    pub fn new(
        stt_latency: LatencyProfile,
        tts_latency: LatencyProfile,
        chaos: ChaosConfig,
    ) -> Self {
        Self {
            stt_latency,
            tts_latency,
            chaos,
            stats: MockStats::default(),
            connection_count: AtomicU64::new(0),
            tts_speak_frames: AtomicU64::new(0),
            tts_flush_frames: AtomicU64::new(0),
            tts_clear_frames: AtomicU64::new(0),
            tts_last_flushed_text: std::sync::Mutex::new(String::new()),
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
    write
        .send(Message::Text(metadata.to_string().into()))
        .await?;

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
                    "speech_final": audio_chunk_count.is_multiple_of(5),
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

                write
                    .send(Message::Text(transcript.to_string().into()))
                    .await?;
            }
            Ok(Message::Text(text)) => {
                // Handle control messages
                if let Ok(msg) = serde_json::from_str::<Value>(&text)
                    && msg.get("type").and_then(|t| t.as_str()) == Some("CloseStream") {
                        break;
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

/// Number of audio chunks the mock streams for a given text (1 per 10 chars, min 3).
pub fn tts_mock_chunk_count(text_len: usize) -> usize {
    (text_len / 10).max(3)
}

/// Size of each mock audio chunk in bytes.
pub const TTS_MOCK_CHUNK_BYTES: usize = 1024;
/// Inter-chunk pacing of the mock's streamed audio.
pub const TTS_MOCK_CHUNK_INTERVAL_MS: u64 = 20;

/// Build one mock audio chunk; the first byte carries the chunk index so clients
/// can assert ordered delivery.
fn tts_mock_chunk(index: usize) -> Bytes {
    let mut chunk = vec![0u8; TTS_MOCK_CHUNK_BYTES];
    chunk[0] = (index % 256) as u8;
    chunk.into()
}

/// Handle a TTS WebSocket connection — Deepgram Aura `/v1/speak` WS protocol.
///
/// Protocol-accurate path (frames with a `"type"` field):
/// - `{"type":"Speak","text":…}` buffers text (counted in `tts_speak_frames`).
/// - `{"type":"Flush"}` synthesizes the buffer: streams binary chunks (first byte =
///   chunk index) paced at [`TTS_MOCK_CHUNK_INTERVAL_MS`], then sends
///   `{"type":"Flushed","sequence_id":N}`.
/// - `{"type":"Clear"}` drops buffered/queued audio immediately and replies
///   `{"type":"Cleared"}` — incoming frames PREEMPT chunk streaming (biased select),
///   so a mid-stream Clear actually stops the stream like the real provider.
/// - `{"type":"Close"}` closes the socket.
///
/// Legacy path: a JSON frame with `"text"` but no `"type"` keeps the old behavior
/// (immediate audio burst + `Flushed`) for pre-existing consumers.
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

    // Send initial metadata like the real endpoint does on connect.
    let metadata = json!({
        "type": "Metadata",
        "request_id": format!("mock-tts-{}", state.connection_count.load(Ordering::Relaxed)),
        "model_name": "aura-mock-en",
    });
    write
        .send(Message::Text(metadata.to_string().into()))
        .await?;

    // Buffered Speak text awaiting a Flush.
    let mut buffered_text = String::new();
    // Audio chunks queued for paced streaming, plus whether a Flushed is due after.
    let mut chunk_queue: std::collections::VecDeque<Bytes> = std::collections::VecDeque::new();
    let mut flushed_due = false;
    let mut sequence_id: u64 = 0;
    let mut flush_started: Option<Instant> = None;
    let mut pacer = tokio::time::interval(Duration::from_millis(TTS_MOCK_CHUNK_INTERVAL_MS));
    pacer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            // Incoming control frames preempt chunk streaming (Clear must cancel).
            biased;

            msg = read.next() => {
                let Some(msg) = msg else { break };
                match msg {
                    Ok(Message::Text(text)) => {
                        let Ok(request) = serde_json::from_str::<Value>(&text) else { continue };
                        match request.get("type").and_then(|t| t.as_str()) {
                            Some("Speak") => {
                                state.tts_speak_frames.fetch_add(1, Ordering::Relaxed);
                                buffered_text
                                    .push_str(request.get("text").and_then(|t| t.as_str()).unwrap_or(""));
                            }
                            Some("Flush") => {
                                state.tts_flush_frames.fetch_add(1, Ordering::Relaxed);
                                if state.chaos.should_fail() {
                                    state.stats.record_failure();
                                    let error = json!({"type": "Error", "description": "Mock TTS error"});
                                    write.send(Message::Text(error.to_string().into())).await?;
                                    buffered_text.clear();
                                    continue;
                                }
                                *state.tts_last_flushed_text.lock().unwrap() = buffered_text.clone();
                                // First-chunk synthesis latency.
                                let mut latency = state.tts_latency.sample();
                                let multiplier = state.chaos.slow_multiplier();
                                if multiplier > 1 {
                                    latency *= multiplier;
                                }
                                tokio::time::sleep(latency).await;
                                for i in 0..tts_mock_chunk_count(buffered_text.len()) {
                                    chunk_queue.push_back(tts_mock_chunk(i));
                                }
                                buffered_text.clear();
                                flushed_due = true;
                                flush_started = Some(Instant::now());
                            }
                            Some("Clear") => {
                                state.tts_clear_frames.fetch_add(1, Ordering::Relaxed);
                                buffered_text.clear();
                                chunk_queue.clear();
                                flushed_due = false;
                                flush_started = None;
                                let cleared = json!({"type": "Cleared", "sequence_id": sequence_id});
                                write.send(Message::Text(cleared.to_string().into())).await?;
                            }
                            Some("Close") => {
                                let _ = write.send(Message::Close(None)).await;
                                break;
                            }
                            Some(_) => {}
                            None => {
                                // Legacy loose path: {"text": …} without a "type" —
                                // immediate audio burst + Flushed (old mock behavior).
                                let Some(text_to_speak) =
                                    request.get("text").and_then(|t| t.as_str())
                                else {
                                    continue;
                                };
                                let start = Instant::now();
                                if state.chaos.should_fail() {
                                    state.stats.record_failure();
                                    let error = json!({"type": "Error", "message": "Mock TTS error"});
                                    write.send(Message::Text(error.to_string().into())).await?;
                                    continue;
                                }
                                let mut latency = state.tts_latency.sample();
                                let multiplier = state.chaos.slow_multiplier();
                                if multiplier > 1 {
                                    latency *= multiplier;
                                }
                                tokio::time::sleep(latency).await;
                                for i in 0..tts_mock_chunk_count(text_to_speak.len()) {
                                    write.send(Message::Binary(tts_mock_chunk(i))).await?;
                                    tokio::time::sleep(Duration::from_millis(
                                        TTS_MOCK_CHUNK_INTERVAL_MS,
                                    ))
                                    .await;
                                }
                                let complete = json!({"type": "Flushed"});
                                write.send(Message::Text(complete.to_string().into())).await?;
                                state
                                    .stats
                                    .record_success(start.elapsed().as_millis() as u64);
                            }
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

            // Paced streaming of queued audio; after the queue drains, send Flushed.
            _ = pacer.tick(), if !chunk_queue.is_empty() || flushed_due => {
                if let Some(chunk) = chunk_queue.pop_front() {
                    write.send(Message::Binary(chunk)).await?;
                } else if flushed_due {
                    sequence_id += 1;
                    let complete = json!({"type": "Flushed", "sequence_id": sequence_id});
                    write.send(Message::Text(complete.to_string().into())).await?;
                    flushed_due = false;
                    if let Some(started) = flush_started.take() {
                        state
                            .stats
                            .record_success(started.elapsed().as_millis() as u64);
                    }
                }
            }
        }
    }

    state.connection_count.fetch_sub(1, Ordering::Relaxed);
    Ok(())
}

/// Start WebSocket mock server for STT
pub async fn start_stt_websocket_mock(
    port: u16,
    state: Arc<WebSocketMockState>,
) -> Result<(), Box<dyn std::error::Error>> {
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
pub async fn start_tts_websocket_mock(
    port: u16,
    state: Arc<WebSocketMockState>,
) -> Result<(), Box<dyn std::error::Error>> {
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
pub fn spawn_stt_websocket_mock(
    port: u16,
    state: Arc<WebSocketMockState>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = start_stt_websocket_mock(port, state).await {
            eprintln!("STT WebSocket Mock error: {}", e);
        }
    })
}

/// Spawn TTS WebSocket mock in background
pub fn spawn_tts_websocket_mock(
    port: u16,
    state: Arc<WebSocketMockState>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = start_tts_websocket_mock(port, state).await {
            eprintln!("TTS WebSocket Mock error: {}", e);
        }
    })
}

/// Spawn the TTS WebSocket mock on an OS-assigned ephemeral port, returning the
/// bound port (avoids fixed-port collisions when tests run in parallel).
pub async fn spawn_tts_websocket_mock_ephemeral(
    state: Arc<WebSocketMockState>,
) -> std::io::Result<(u16, tokio::task::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let state = state.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_tts_connection(stream, state).await {
                    eprintln!("TTS connection error: {}", e);
                }
            });
        }
    });
    Ok((port, handle))
}
