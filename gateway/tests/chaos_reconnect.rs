//! Reconnection chaos tests (W-D1).
//!
//! These prove the reconnection supervisor recovers a *featured* streaming session across a
//! mid-stream transport drop without losing finals:
//!
//! - `generic_*` exercise the transport-agnostic [`ReconnectableStream`] directly with an
//!   in-memory mock that drops mid-stream and emits a numbered sequence. The supervisor must
//!   reconnect, call `restore_session` on the fresh connection, and the union of finals seen
//!   before and after the drop must equal the full set (nothing lost).
//! - `deepgram_recovers_after_midstream_kill` drives the REAL `DeepgramSTT` provider through
//!   an in-repo mock WebSocket server (via `endpoint_override`) that drops the socket
//!   mid-stream. It asserts the provider reconnects within `max_delay`, that the reconnect
//!   re-sends the *featured* query (`diarize=true` + `keyterm=...` — the session restore),
//!   and that no finals are lost (union of pre+post == full set).

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

use waav_gateway::core::stt::standard::{StandardSTTConfig, SttFeatures, create_stt_standard};
use waav_gateway::core::stt::{STTConfig, STTResult, STTResultCallback};
use waav_gateway::core::websocket::ReconnectionConfig;
use waav_gateway::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    SupervisorExit, WsTransport,
};

// =============================================================================
// Part A — generic supervisor with an in-memory mock transport
// =============================================================================

/// An in-memory mock transport. On the first `run()` it "emits" finals `0..split` and then
/// drops (Reconnectable); on the second `run()` it emits `split..total` and completes. The
/// finals it produces are appended to a shared sink so the test can assert no loss.
struct DroppingTransport {
    connect_index: usize,
    split: usize,
    total: usize,
    finals: Arc<Mutex<Vec<usize>>>,
    restore_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl WsTransport for DroppingTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // Re-sending the featured handshake on (re)connect.
        self.restore_calls.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        if self.connect_index == 0 {
            // First connection: deliver the first half, then drop mid-stream.
            let mut f = self.finals.lock().await;
            for i in 0..self.split {
                f.push(i);
            }
            ReconnectOutcome::Reconnectable(StreamError::new("mid-stream socket reset"))
        } else {
            // Reconnected: deliver the rest and finish cleanly.
            let mut f = self.finals.lock().await;
            for i in self.split..self.total {
                f.push(i);
            }
            ReconnectOutcome::Completed
        }
    }
}

#[tokio::test]
async fn generic_reconnects_after_midstream_drop_no_finals_lost() {
    const SPLIT: usize = 5;
    const TOTAL: usize = 10;

    let finals = Arc::new(Mutex::new(Vec::<usize>::new()));
    let restore_calls = Arc::new(AtomicUsize::new(0));
    let connect_count = Arc::new(AtomicUsize::new(0));

    let recon = ReconnectionConfig {
        jitter: false,
        initial_delay_ms: 2,
        max_delay_ms: 10,
        ..ReconnectionConfig::aggressive()
    };
    let cfg = ReconnectableStreamConfig::new("mock", recon);
    let stream = ReconnectableStream::new(cfg);

    let f = Arc::clone(&finals);
    let rc = Arc::clone(&restore_calls);
    let cc = Arc::clone(&connect_count);
    let start = Instant::now();
    let exit = stream
        .run(move || {
            let f = Arc::clone(&f);
            let rc = Arc::clone(&rc);
            let cc = Arc::clone(&cc);
            async move {
                let idx = cc.fetch_add(1, Ordering::AcqRel);
                Ok::<_, StreamError>(DroppingTransport {
                    connect_index: idx,
                    split: SPLIT,
                    total: TOTAL,
                    finals: f,
                    restore_calls: rc,
                })
            }
        })
        .await;
    let elapsed = start.elapsed();

    assert_eq!(exit, SupervisorExit::Completed);
    // Reconnect happened within the configured max backoff (a couple of ms here).
    assert!(
        elapsed < Duration::from_secs(2),
        "reconnect took too long: {elapsed:?}"
    );
    // restore_session must run on EVERY connect — the second call is the session restore.
    assert_eq!(restore_calls.load(Ordering::Acquire), 2);
    assert_eq!(connect_count.load(Ordering::Acquire), 2, "connected twice");

    // No finals lost: the union of pre-drop + post-reconnect equals the full numbered set.
    let got: HashSet<usize> = finals.lock().await.iter().copied().collect();
    let expected: HashSet<usize> = (0..TOTAL).collect();
    assert_eq!(got, expected, "finals lost across the reconnect: {got:?}");
}

// =============================================================================
// Part B — the REAL DeepgramSTT provider through an in-repo mock WS server
// =============================================================================

/// What the mock server captured/observed across the test.
#[derive(Default)]
struct MockObservations {
    /// The request URI (with query) of every connection, in order.
    connect_uris: Vec<String>,
}

/// A mock Deepgram-style WS server that:
///   1. captures every connection's request URI (so we can assert the featured query is
///      re-sent on reconnect — the session restore),
///   2. on the FIRST connection emits finals `0..split` then **drops the socket** mid-stream,
///   3. on the SECOND connection emits `split..total` then keeps the socket open.
/// Returns the bound port and a handle to the shared observations.
async fn spawn_dropping_deepgram_mock(
    split: usize,
    total: usize,
) -> (u16, Arc<Mutex<MockObservations>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let obs = Arc::new(Mutex::new(MockObservations::default()));
    let obs_ret = Arc::clone(&obs);
    let conn_count = Arc::new(AtomicUsize::new(0));

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let obs = Arc::clone(&obs);
            let conn_count = Arc::clone(&conn_count);
            tokio::spawn(async move {
                // Capture the request URI (path + query) during the WS handshake.
                let captured_uri = Arc::new(std::sync::Mutex::new(String::new()));
                let cap = Arc::clone(&captured_uri);
                let callback = move |req: &Request, resp: Response| {
                    *cap.lock().unwrap() = req.uri().to_string();
                    Ok(resp)
                };
                let ws = match accept_hdr_async(stream, callback).await {
                    Ok(ws) => ws,
                    Err(_) => return,
                };
                let uri = captured_uri.lock().unwrap().clone();
                let which = conn_count.fetch_add(1, Ordering::AcqRel);
                obs.lock().await.connect_uris.push(uri);

                let (mut write, mut read) = ws.split();

                // Send Deepgram-style Metadata first.
                let _ = write
                    .send(Message::Text(
                        r#"{"type":"Metadata","request_id":"mock"}"#.into(),
                    ))
                    .await;

                // Drive a transcript pump that emits finals on a timer regardless of audio,
                // and reads (to consume audio / detect close).
                let (lo, hi, drop_after) = if which == 0 {
                    (0, split, true)
                } else {
                    (split, total, false)
                };

                let mut idx = lo;
                let mut ticker = tokio::time::interval(Duration::from_millis(15));
                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            if idx < hi {
                                let transcript = format!(
                                    r#"{{"type":"Results","is_final":true,"speech_final":false,"channel":{{"alternatives":[{{"transcript":"final {idx}","confidence":0.99}}]}}}}"#
                                );
                                if write.send(Message::Text(transcript.into())).await.is_err() {
                                    return;
                                }
                                idx += 1;
                            } else if drop_after {
                                // First connection: we've emitted our half — drop the socket
                                // abruptly (no close frame) to simulate a mid-stream kill.
                                return;
                            }
                            // Second connection: keep the socket open and idle after emitting.
                        }
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Close(_))) | None => return,
                                Some(Ok(Message::Text(t))) => {
                                    // CloseStream is the client's graceful shutdown.
                                    if t.contains("CloseStream") { return; }
                                }
                                Some(Ok(_)) => {} // audio / ping etc.
                                Some(Err(_)) => return,
                            }
                        }
                    }
                }
            });
        }
    });

    (port, obs_ret)
}

#[tokio::test]
async fn deepgram_recovers_after_midstream_kill() {
    const SPLIT: usize = 5;
    const TOTAL: usize = 10;

    let (port, obs) = spawn_dropping_deepgram_mock(SPLIT, TOTAL).await;
    let endpoint = format!("ws://127.0.0.1:{port}");

    // Build the standardized, *featured* config: diarization + a keyterm. These must survive
    // the reconnect (the restore_session re-sends the same query).
    let std_cfg = StandardSTTConfig {
        base: STTConfig {
            provider: "deepgram".into(),
            api_key: "test-key".into(),
            model: "nova-3".into(),
            language: "en-US".into(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".into(),
        },
        features: SttFeatures {
            diarization: Some(true),
            keyterms: Some(vec!["WaaV".into()]),
            ..Default::default()
        },
        extras: Default::default(),
    }
    .with_endpoint_override(&endpoint);

    let mut provider = create_stt_standard("deepgram", std_cfg).expect("build deepgram");

    // Collect every final transcript the provider surfaces.
    let collected: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    let cb: STTResultCallback = Arc::new(move |r: STTResult| {
        let sink = Arc::clone(&sink);
        Box::pin(async move {
            if r.is_final {
                // transcripts are "final N" — parse the index.
                if let Some(n) = r.transcript.rsplit(' ').next().and_then(|s| s.parse().ok()) {
                    sink.lock().await.push(n);
                }
            }
        })
    });
    provider.on_result(cb).await.unwrap();

    let start = Instant::now();
    provider.connect().await.expect("initial connect");

    // Feed audio frames continuously so the provider's send path stays alive; the mock emits
    // finals on its own timer.
    let audio = bytes::Bytes::from(vec![0u8; 640]);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let _ = provider.send_audio(audio.clone()).await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        let have = collected.lock().await.len();
        if have >= TOTAL {
            break;
        }
        if Instant::now() > deadline {
            break;
        }
    }
    let elapsed_to_full = start.elapsed();

    let _ = provider.disconnect().await;

    // 1. The session reconnected: the mock saw (at least) two connections.
    let uris = obs.lock().await.connect_uris.clone();
    assert!(
        uris.len() >= 2,
        "expected a reconnect (>=2 connections), saw {}: {uris:?}",
        uris.len()
    );

    // 2. The reconnect re-sent the FEATURED query (session restore): diarize + keyterm on the
    //    SECOND connection, not a bare reconnect.
    let restore_uri = &uris[1];
    assert!(
        restore_uri.contains("diarize=true"),
        "restore did not re-send diarization: {restore_uri}"
    );
    assert!(
        restore_uri.contains("keyterm=WaaV"),
        "restore did not re-send the keyterm: {restore_uri}"
    );

    // 3. Reconnect happened within a small multiple of max_delay (aggressive preset caps at
    //    5s; the whole recovery to the full set should be well under the 8s test budget).
    assert!(
        elapsed_to_full < Duration::from_secs(8),
        "recovery to full set took too long: {elapsed_to_full:?}"
    );

    // 4. No finals lost: union of pre-drop + post-reconnect == the full numbered set.
    let got: HashSet<usize> = collected.lock().await.iter().copied().collect();
    let expected: HashSet<usize> = (0..TOTAL).collect();
    assert_eq!(
        got, expected,
        "finals lost across the Deepgram reconnect: got {got:?}, expected {expected:?}"
    );
}

// =============================================================================
// Part C — REAL CartesiaSTT through a mock WS server (newly-migrated provider, W-D1 fleet)
// =============================================================================

/// A mock Cartesia-style WS server that:
///   1. captures every connection's request URI (so we can assert the featured query — model +
///      encoding — is re-sent on reconnect, i.e. the URL *is* the session restore for Cartesia),
///   2. on the FIRST connection emits finals `0..split` then **drops the socket** mid-stream,
///   3. on the SECOND connection emits `split..total` then keeps the socket open.
/// Cartesia speaks raw binary audio in and `{"type":"transcript","text":..,"is_final":..}` out.
async fn spawn_dropping_cartesia_mock(
    split: usize,
    total: usize,
) -> (u16, Arc<Mutex<MockObservations>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let obs = Arc::new(Mutex::new(MockObservations::default()));
    let obs_ret = Arc::clone(&obs);
    let conn_count = Arc::new(AtomicUsize::new(0));

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let obs = Arc::clone(&obs);
            let conn_count = Arc::clone(&conn_count);
            tokio::spawn(async move {
                let captured_uri = Arc::new(std::sync::Mutex::new(String::new()));
                let cap = Arc::clone(&captured_uri);
                let callback = move |req: &Request, resp: Response| {
                    *cap.lock().unwrap() = req.uri().to_string();
                    Ok(resp)
                };
                let ws = match accept_hdr_async(stream, callback).await {
                    Ok(ws) => ws,
                    Err(_) => return,
                };
                let uri = captured_uri.lock().unwrap().clone();
                let which = conn_count.fetch_add(1, Ordering::AcqRel);
                obs.lock().await.connect_uris.push(uri);

                let (mut write, mut read) = ws.split();

                let (lo, hi, drop_after) = if which == 0 {
                    (0, split, true)
                } else {
                    (split, total, false)
                };

                let mut idx = lo;
                let mut ticker = tokio::time::interval(Duration::from_millis(15));
                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            if idx < hi {
                                let transcript = format!(
                                    r#"{{"type":"transcript","text":"final {idx}","is_final":true}}"#
                                );
                                if write.send(Message::Text(transcript.into())).await.is_err() {
                                    return;
                                }
                                idx += 1;
                            } else if drop_after {
                                // First connection: emitted our half — drop abruptly (no close
                                // frame) to simulate a mid-stream kill.
                                return;
                            }
                        }
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Close(_))) | None => return,
                                Some(Ok(Message::Text(t))) => {
                                    // Cartesia's graceful shutdown is the JSON string "done".
                                    if t.contains("done") { return; }
                                }
                                Some(Ok(_)) => {} // raw binary audio / ping etc.
                                Some(Err(_)) => return,
                            }
                        }
                    }
                }
            });
        }
    });

    (port, obs_ret)
}

/// Drives the REAL `CartesiaSTT` provider (a newly-migrated W-D1 provider) through the mock that
/// kills the socket mid-stream. Asserts: it reconnects (>=2 connections), the reconnect re-sends
/// the *featured* connect URL (model + encoding — Cartesia's session restore IS the URL), recovery
/// is timely, and NO finals are lost across the drop.
#[tokio::test]
async fn cartesia_recovers_after_midstream_kill() {
    const SPLIT: usize = 5;
    const TOTAL: usize = 10;

    let (port, obs) = spawn_dropping_cartesia_mock(SPLIT, TOTAL).await;
    let endpoint = format!("ws://127.0.0.1:{port}");

    let std_cfg = StandardSTTConfig {
        base: STTConfig {
            provider: "cartesia".into(),
            api_key: "test-key".into(),
            model: "ink-whisper".into(),
            language: "en".into(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".into(),
        },
        features: SttFeatures {
            endpointing_ms: Some(500),
            ..Default::default()
        },
        extras: Default::default(),
    }
    .with_endpoint_override(&endpoint);

    let mut provider = create_stt_standard("cartesia", std_cfg).expect("build cartesia");

    let collected: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    let cb: STTResultCallback = Arc::new(move |r: STTResult| {
        let sink = Arc::clone(&sink);
        Box::pin(async move {
            if r.is_final {
                if let Some(n) = r.transcript.rsplit(' ').next().and_then(|s| s.parse().ok()) {
                    sink.lock().await.push(n);
                }
            }
        })
    });
    provider.on_result(cb).await.unwrap();

    let start = Instant::now();
    provider.connect().await.expect("initial connect");

    let audio = bytes::Bytes::from(vec![0u8; 640]);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let _ = provider.send_audio(audio.clone()).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        if collected.lock().await.len() >= TOTAL {
            break;
        }
        if Instant::now() > deadline {
            break;
        }
    }
    let elapsed_to_full = start.elapsed();
    let _ = provider.disconnect().await;

    // 1. The session reconnected: the mock saw (at least) two connections.
    let uris = obs.lock().await.connect_uris.clone();
    assert!(
        uris.len() >= 2,
        "expected a reconnect (>=2 connections), saw {}: {uris:?}",
        uris.len()
    );

    // 2. The reconnect re-sent the FEATURED connect URL (Cartesia carries the featured session in
    //    the URL): model + encoding present on the SECOND connection, not a bare reconnect.
    let restore_uri = &uris[1];
    assert!(
        restore_uri.contains("model=ink-whisper"),
        "restore did not re-send the model: {restore_uri}"
    );
    assert!(
        restore_uri.contains("encoding=pcm_s16le"),
        "restore did not re-send the encoding: {restore_uri}"
    );

    // 3. Recovery was timely (aggressive preset caps backoff at 5s; full set well under 8s).
    assert!(
        elapsed_to_full < Duration::from_secs(8),
        "recovery to full set took too long: {elapsed_to_full:?}"
    );

    // 4. No finals lost: union of pre-drop + post-reconnect == the full numbered set.
    let got: HashSet<usize> = collected.lock().await.iter().copied().collect();
    let expected: HashSet<usize> = (0..TOTAL).collect();
    assert_eq!(
        got, expected,
        "finals lost across the Cartesia reconnect: got {got:?}, expected {expected:?}"
    );
}

// =============================================================================
// Part D — REAL AssemblyAISTT through a mock WS server (W-D1 fleet migration)
// =============================================================================

/// A mock AssemblyAI v3-style WS server that:
///   1. captures every connection's request URI (so we can assert the featured `/v3/ws` query —
///      AssemblyAI carries the featured session in the URL, so the re-dial IS the restore),
///   2. opens every session with a `Begin` frame (the client blocks on the first Begin, and a
///      restored session only becomes writable after its Begin re-arms readiness),
///   3. on the FIRST connection emits final Turns `0..split` then **drops the socket** mid-stream,
///   4. on the SECOND connection emits `split..total` then keeps the socket open.
/// AssemblyAI speaks raw binary audio in and `{"type":"Turn",...}` out; `{"type":"Terminate"}` is
/// the client's graceful shutdown.
async fn spawn_dropping_assemblyai_mock(
    split: usize,
    total: usize,
) -> (u16, Arc<Mutex<MockObservations>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let obs = Arc::new(Mutex::new(MockObservations::default()));
    let obs_ret = Arc::clone(&obs);
    let conn_count = Arc::new(AtomicUsize::new(0));

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let obs = Arc::clone(&obs);
            let conn_count = Arc::clone(&conn_count);
            tokio::spawn(async move {
                let captured_uri = Arc::new(std::sync::Mutex::new(String::new()));
                let cap = Arc::clone(&captured_uri);
                let callback = move |req: &Request, resp: Response| {
                    *cap.lock().unwrap() = req.uri().to_string();
                    Ok(resp)
                };
                let ws = match accept_hdr_async(stream, callback).await {
                    Ok(ws) => ws,
                    Err(_) => return,
                };
                let uri = captured_uri.lock().unwrap().clone();
                let which = conn_count.fetch_add(1, Ordering::AcqRel);
                obs.lock().await.connect_uris.push(uri);

                let (mut write, mut read) = ws.split();

                // Every AssemblyAI session opens with a Begin frame; the restored session needs
                // one too (it re-arms the client's readiness after the reconnect).
                let begin =
                    format!(r#"{{"type":"Begin","id":"sess-{which}","expires_at":1704067200}}"#);
                if write.send(Message::Text(begin.into())).await.is_err() {
                    return;
                }

                let (lo, hi, drop_after) = if which == 0 {
                    (0, split, true)
                } else {
                    (split, total, false)
                };

                let mut idx = lo;
                let mut ticker = tokio::time::interval(Duration::from_millis(15));
                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            if idx < hi {
                                let turn = format!(
                                    r#"{{"type":"Turn","turn_order":{idx},"transcript":"final {idx}","end_of_turn":true,"words":[]}}"#
                                );
                                if write.send(Message::Text(turn.into())).await.is_err() {
                                    return;
                                }
                                idx += 1;
                            } else if drop_after {
                                // First connection: emitted our half — drop abruptly (no close
                                // frame) to simulate a mid-stream kill.
                                return;
                            }
                        }
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Close(_))) | None => return,
                                Some(Ok(Message::Text(t))) => {
                                    // AssemblyAI's graceful shutdown is `{"type":"Terminate"}`.
                                    if t.contains("Terminate") { return; }
                                }
                                Some(Ok(_)) => {} // raw binary audio / ping etc.
                                Some(Err(_)) => return,
                            }
                        }
                    }
                }
            });
        }
    });

    (port, obs_ret)
}

/// Drives the REAL `AssemblyAISTT` provider (migrated off its hand-rolled reconnect loop onto the
/// generic [`ReconnectableStream`] supervisor) through the mock that kills the socket mid-stream.
/// Asserts: it reconnects (>=2 connections), the reconnect re-dials the *featured* `/v3/ws` URL
/// (speech model + keyterms — AssemblyAI's session restore IS the URL), the restored session is
/// writable again after its Begin, recovery is timely, and NO final turns are lost.
#[tokio::test]
async fn assemblyai_recovers_after_midstream_kill() {
    const SPLIT: usize = 5;
    const TOTAL: usize = 10;

    let (port, obs) = spawn_dropping_assemblyai_mock(SPLIT, TOTAL).await;
    let endpoint = format!("ws://127.0.0.1:{port}");

    // Featured config: keyterms prompting rides the /v3/ws connect query (`keyterms_prompt`),
    // so the reconnect must re-send it — a bare re-dial would lose the biasing.
    let std_cfg = StandardSTTConfig {
        base: STTConfig {
            provider: "assemblyai".into(),
            api_key: "test-key".into(),
            model: String::new(),
            language: "en".into(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".into(),
        },
        features: SttFeatures {
            keyterms: Some(vec!["WaaV".into()]),
            ..Default::default()
        },
        extras: Default::default(),
    }
    .with_endpoint_override(&endpoint);

    let mut provider = create_stt_standard("assemblyai", std_cfg).expect("build assemblyai");

    let collected: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    let cb: STTResultCallback = Arc::new(move |r: STTResult| {
        let sink = Arc::clone(&sink);
        Box::pin(async move {
            if r.is_final {
                if let Some(n) = r.transcript.rsplit(' ').next().and_then(|s| s.parse().ok()) {
                    sink.lock().await.push(n);
                }
            }
        })
    });
    provider.on_result(cb).await.unwrap();

    let start = Instant::now();
    provider.connect().await.expect("initial connect");

    let audio = bytes::Bytes::from(vec![0u8; 640]);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let _ = provider.send_audio(audio.clone()).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        if collected.lock().await.len() >= TOTAL {
            break;
        }
        if Instant::now() > deadline {
            break;
        }
    }
    let elapsed_to_full = start.elapsed();

    // After the reconnect's Begin, the restored session must be writable again (is_ready was
    // re-armed) — the legacy hand-rolled loop left the client unready forever after a reconnect.
    let send_after_reconnect = provider.send_audio(audio.clone()).await;
    let _ = provider.disconnect().await;

    // 1. The session reconnected: the mock saw (at least) two connections.
    let uris = obs.lock().await.connect_uris.clone();
    assert!(
        uris.len() >= 2,
        "expected a reconnect (>=2 connections), saw {}: {uris:?}",
        uris.len()
    );

    // 2. The reconnect re-dialed the FEATURED /v3/ws URL (AssemblyAI carries the featured
    //    session in the URL): v3 path + speech model + the keyterm on the SECOND connection.
    let restore_uri = &uris[1];
    assert!(
        restore_uri.contains("/v3/ws"),
        "restore did not target the v3 streaming endpoint: {restore_uri}"
    );
    assert!(
        restore_uri.contains("speech_model=universal-streaming-english"),
        "restore did not re-send the speech model: {restore_uri}"
    );
    assert!(
        restore_uri.contains("keyterms_prompt=") && restore_uri.contains("WaaV"),
        "restore did not re-send the keyterms prompt: {restore_uri}"
    );

    // 3. The restored session accepts audio again (readiness re-armed by the new Begin).
    assert!(
        send_after_reconnect.is_ok(),
        "restored session rejected audio: {send_after_reconnect:?}"
    );

    // 4. Recovery was timely (aggressive preset caps backoff at 5s; full set well under 8s).
    assert!(
        elapsed_to_full < Duration::from_secs(8),
        "recovery to full set took too long: {elapsed_to_full:?}"
    );

    // 5. No final turns lost: union of pre-drop + post-reconnect == the full numbered set.
    let got: HashSet<usize> = collected.lock().await.iter().copied().collect();
    let expected: HashSet<usize> = (0..TOTAL).collect();
    assert_eq!(
        got, expected,
        "finals lost across the AssemblyAI reconnect: got {got:?}, expected {expected:?}"
    );
}

// =============================================================================
// Part E — D-G1 reconnect audio-replay: the un-finalized tail crosses the gap
// =============================================================================

/// Which provider protocol the replay mock speaks.
#[derive(Clone, Copy)]
enum ReplayProto {
    Deepgram,
    AssemblyAi,
}

/// Binary audio captured per connection, in arrival order.
#[derive(Default)]
struct ReplayObservations {
    audio_per_conn: Vec<Vec<Vec<u8>>>,
}

/// A mock STT server choreographed around DISTINCT tagged audio chunks:
/// - conn 0: on receiving the 0xA1 chunk → send a FINAL transcript (the
///   client clears its replay ring on it); on receiving the 0xA3 chunk →
///   drop the socket abruptly (0xA2 and 0xA3 are now the un-finalized tail).
/// - conn 1: capture everything; on receiving a 0xA4 chunk → send a FINAL so
///   the test can observe completion.
async fn spawn_replay_mock(proto: ReplayProto) -> (u16, Arc<Mutex<ReplayObservations>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let obs = Arc::new(Mutex::new(ReplayObservations::default()));
    let obs_ret = Arc::clone(&obs);
    let conn_count = Arc::new(AtomicUsize::new(0));

    fn final_frame(proto: ReplayProto, idx: usize) -> String {
        match proto {
            ReplayProto::Deepgram => format!(
                r#"{{"type":"Results","is_final":true,"speech_final":false,"channel":{{"alternatives":[{{"transcript":"final {idx}","confidence":0.99}}]}}}}"#
            ),
            ReplayProto::AssemblyAi => format!(
                r#"{{"type":"Turn","turn_order":{idx},"transcript":"final {idx}","end_of_turn":true,"words":[]}}"#
            ),
        }
    }

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let obs = Arc::clone(&obs);
            let conn_count = Arc::clone(&conn_count);
            tokio::spawn(async move {
                let ws = match accept_hdr_async(stream, |_req: &Request, resp: Response| Ok(resp))
                    .await
                {
                    Ok(ws) => ws,
                    Err(_) => return,
                };
                let which = conn_count.fetch_add(1, Ordering::AcqRel);
                obs.lock().await.audio_per_conn.push(Vec::new());

                let (mut write, mut read) = ws.split();

                if matches!(proto, ReplayProto::AssemblyAi) {
                    let begin = format!(
                        r#"{{"type":"Begin","id":"sess-{which}","expires_at":1704067200}}"#
                    );
                    if write.send(Message::Text(begin.into())).await.is_err() {
                        return;
                    }
                }

                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Binary(b)) => {
                            let tag = b.first().copied().unwrap_or(0);
                            obs.lock().await.audio_per_conn[which].push(b.to_vec());
                            match (which, tag) {
                                (0, 0xA1) => {
                                    // Ack everything so far: the client clears its ring.
                                    if write
                                        .send(Message::Text(final_frame(proto, 0).into()))
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                                (0, 0xA3) => {
                                    // Mid-stream kill with 0xA2+0xA3 un-finalized.
                                    return;
                                }
                                (_, 0xA4) => {
                                    let _ = write
                                        .send(Message::Text(final_frame(proto, 1).into()))
                                        .await;
                                }
                                _ => {}
                            }
                        }
                        Ok(Message::Close(_)) | Err(_) => return,
                        Ok(Message::Text(t)) => {
                            if t.contains("CloseStream") || t.contains("Terminate") {
                                return;
                            }
                        }
                        Ok(_) => {}
                    }
                }
            });
        }
    });

    (port, obs_ret)
}

fn tagged(tag: u8) -> bytes::Bytes {
    bytes::Bytes::from(vec![tag; 320])
}

/// Drives a REAL provider through the replay mock and asserts the D-G1
/// contract: the un-finalized tail (0xA2, 0xA3) is replayed FIRST on the new
/// connection, in order, and the finalized chunk (0xA1) is NOT replayed.
async fn assert_replays_unfinalized_tail(provider_name: &str, proto: ReplayProto) {
    let (port, obs) = spawn_replay_mock(proto).await;
    let endpoint = format!("ws://127.0.0.1:{port}");

    let std_cfg = StandardSTTConfig {
        base: STTConfig {
            provider: provider_name.into(),
            api_key: "test-key".into(),
            model: if matches!(proto, ReplayProto::Deepgram) { "nova-3".into() } else { String::new() },
            language: "en".into(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".into(),
        },
        features: SttFeatures::default(),
        extras: Default::default(),
    }
    .with_endpoint_override(&endpoint);

    let mut provider = create_stt_standard(provider_name, std_cfg).expect("build provider");

    let finals = Arc::new(AtomicUsize::new(0));
    let finals_cb = Arc::clone(&finals);
    let cb: STTResultCallback = Arc::new(move |r: STTResult| {
        let finals = Arc::clone(&finals_cb);
        Box::pin(async move {
            if r.is_final {
                finals.fetch_add(1, Ordering::SeqCst);
            }
        })
    });
    provider.on_result(cb).await.unwrap();
    provider.connect().await.expect("initial connect");

    // 1) Send the chunk the mock ACKS with a final → the ring clears.
    provider.send_audio(tagged(0xA1)).await.expect("send A1");
    let deadline = Instant::now() + Duration::from_secs(5);
    while finals.load(Ordering::SeqCst) < 1 {
        assert!(Instant::now() < deadline, "never received the first final ack");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // 2) The at-risk tail: sent after the last final, then the socket dies.
    provider.send_audio(tagged(0xA2)).await.expect("send A2");
    provider.send_audio(tagged(0xA3)).await.expect("send A3");

    // 3) Keep nudging with 0xA4 until the reconnected session acks (the
    //    sends may fail during the gap — that is the point).
    let deadline = Instant::now() + Duration::from_secs(8);
    while finals.load(Ordering::SeqCst) < 2 {
        assert!(
            Instant::now() < deadline,
            "no final from the reconnected session — replay/reconnect failed"
        );
        let _ = provider.send_audio(tagged(0xA4)).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = provider.disconnect().await;

    let obs = obs.lock().await;
    assert!(
        obs.audio_per_conn.len() >= 2,
        "expected a reconnect (>=2 connections), saw {}",
        obs.audio_per_conn.len()
    );
    let conn2 = &obs.audio_per_conn[1];
    assert!(
        conn2.len() >= 2,
        "second connection received too little audio: {} frames",
        conn2.len()
    );
    // The REPLAYED tail comes first, oldest first, before any fresh audio.
    assert_eq!(
        conn2[0][0], 0xA2,
        "first frame on the new connection must be the replayed 0xA2 chunk"
    );
    assert_eq!(conn2[0].len(), 320, "replayed chunk must be byte-identical");
    assert_eq!(
        conn2[1][0], 0xA3,
        "second frame must be the replayed 0xA3 chunk (order preserved)"
    );
    // The finalized chunk must NOT be replayed (cleared by the final ack).
    assert!(
        conn2.iter().all(|f| f[0] != 0xA1),
        "0xA1 was finalized before the drop — replaying it would duplicate words"
    );
}

#[tokio::test]
async fn deepgram_replays_unfinalized_audio_after_kill() {
    assert_replays_unfinalized_tail("deepgram", ReplayProto::Deepgram).await;
}

#[tokio::test]
async fn assemblyai_replays_unfinalized_audio_after_kill() {
    assert_replays_unfinalized_tail("assemblyai", ReplayProto::AssemblyAi).await;
}
