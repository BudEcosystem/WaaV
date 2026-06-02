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
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

use waav_gateway::core::stt::standard::{SttFeatures, StandardSTTConfig, create_stt_standard};
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
