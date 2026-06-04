//! Credential-free END-TO-END integration via `endpoint_override` + a local mock WS server.
//!
//! This drives the REAL provider through the full loop — `create_stt_standard` → `connect()` →
//! `send_audio()` → the provider's WS handshake + protocol → response parse → `on_result` callback —
//! against an in-repo mock server, with NO vendor key. It's the strongest credential-free proof that
//! a provider's integration actually works for the 63 vendors we don't hold keys for.
//!
//! `chaos_reconnect.rs` already exercises this harness for Deepgram + Cartesia (under socket-drop
//! chaos); this adds a clean happy-path proof for Rev AI (a 3rd-party vendor we have no key for) and
//! is the template for extending credential-free e2e coverage provider-by-provider.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;
use waav_gateway::core::stt::standard::{StandardSTTConfig, create_stt_standard};
use waav_gateway::core::stt::{STTConfig, STTResult};

fn ensure_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Spawn a local mock Rev AI WS server: accept the upgrade, emit one `final` transcript message that
/// decodes to `transcript_value`, then drain inbound audio so the client keeps streaming. Returns
/// the bound port. The mock speaks Rev AI's real wire shape
/// (`{"type":"final","elements":[{"type":"text","value":..}]}`).
async fn spawn_revai_mock(transcript_value: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(ws) = tokio_tungstenite::accept_async(stream).await
        {
            let (mut write, mut read) = ws.split();
            // Rev AI sends a `connected` session-ack first; the client's connect() blocks on it.
            let _ = write
                .send(Message::Text(
                    r#"{"type":"connected","id":"mock-session-1"}"#.into(),
                ))
                .await;
            let final_msg = format!(
                r#"{{"type":"final","ts":0.0,"end_ts":1.0,"elements":[{{"type":"text","value":"{transcript_value}","confidence":0.99}}]}}"#
            );
            // Emit the `final` once real audio starts flowing (the client's read loop is now active),
            // then keep draining so the client can keep streaming.
            let mut sent_final = false;
            while let Some(Ok(frame)) = read.next().await {
                if !sent_final && matches!(frame, Message::Binary(_)) {
                    let _ = write.send(Message::Text(final_msg.clone().into())).await;
                    sent_final = true;
                }
            }
        }
    });
    port
}

#[tokio::test]
async fn revai_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_revai_mock("hello world").await;
    let endpoint = format!("ws://127.0.0.1:{port}");

    // Build the REAL Rev AI provider through the standardized keystone, pointed at the mock.
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "revai".into(),
        api_key: "test-key".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(&endpoint);

    let mut stt = create_stt_standard("revai", std).expect("build revai via keystone");

    let best = Arc::new(tokio::sync::Mutex::new(String::new()));
    let b2 = best.clone();
    stt.on_result(Arc::new(move |r: STTResult| {
        let b = b2.clone();
        Box::pin(async move {
            let t = r.transcript.trim().to_string();
            if !t.is_empty() {
                *b.lock().await = t;
            }
        })
    }))
    .await
    .unwrap();

    // Full loop: connect to the mock (real handshake), stream a little audio, let the provider parse
    // the mock's `final` message and fire the callback.
    stt.connect().await.expect("connect to mock endpoint");
    for _ in 0..5 {
        let _ = stt.send_audio(bytes::Bytes::from(vec![0u8; 640])).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    stt.disconnect().await.ok();

    let got = best.lock().await.clone();
    println!("Rev AI mock e2e surfaced transcript: {got:?}");
    assert_eq!(
        got, "hello world",
        "Rev AI did not surface the mock transcript end-to-end (full integration broken)"
    );
}

/// Spawn a local mock Reverie WS server: after the first audio frame, emit a Reverie transcript
/// message (`{"id":..,"text":..,"final":true}`) — its real wire shape — then drain audio.
async fn spawn_reverie_mock(transcript_value: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(ws) = tokio_tungstenite::accept_async(stream).await
        {
            let (mut write, mut read) = ws.split();
            let msg = format!(
                r#"{{"id":"mock-session-1","text":"{transcript_value}","final":true,"cause":"silence"}}"#
            );
            let mut sent = false;
            while let Some(Ok(frame)) = read.next().await {
                if !sent && matches!(frame, Message::Binary(_)) {
                    let _ = write.send(Message::Text(msg.clone().into())).await;
                    sent = true;
                }
            }
        }
    });
    port
}

#[tokio::test]
async fn reverie_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_reverie_mock("hello world").await;
    let endpoint = format!("ws://127.0.0.1:{port}");

    // Reverie reads its required `app_id` from the `model` field of the base config.
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "reverie".into(),
        api_key: "test-key".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: "test-app-id".into(),
    })
    .with_endpoint_override(&endpoint);

    let mut stt = create_stt_standard("reverie", std).expect("build reverie via keystone");

    let best = Arc::new(tokio::sync::Mutex::new(String::new()));
    let b2 = best.clone();
    stt.on_result(Arc::new(move |r: STTResult| {
        let b = b2.clone();
        Box::pin(async move {
            let t = r.transcript.trim().to_string();
            if !t.is_empty() {
                *b.lock().await = t;
            }
        })
    }))
    .await
    .unwrap();

    stt.connect().await.expect("connect to mock endpoint");
    for _ in 0..5 {
        let _ = stt.send_audio(bytes::Bytes::from(vec![0u8; 640])).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    stt.disconnect().await.ok();

    let got = best.lock().await.clone();
    println!("Reverie mock e2e surfaced transcript: {got:?}");
    assert_eq!(
        got, "hello world",
        "Reverie did not surface the mock transcript end-to-end (full integration broken)"
    );
}

/// Spawn a local mock Tencent ASR WS server: after the first audio frame, emit a Tencent ASR
/// response (`{"code":0,...,"result":{"slice_type":2,...,"voice_text_str":..},"final":1}`) — its real
/// wire shape — then drain audio.
async fn spawn_tencent_mock(transcript_value: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(ws) = tokio_tungstenite::accept_async(stream).await
        {
            let (mut write, mut read) = ws.split();
            let msg = format!(
                r#"{{"code":0,"message":"success","voice_id":"v1","result":{{"slice_type":2,"index":0,"start_time":0,"end_time":1000,"voice_text_str":"{transcript_value}"}},"final":1}}"#
            );
            let mut sent = false;
            while let Some(Ok(frame)) = read.next().await {
                if !sent && matches!(frame, Message::Binary(_)) {
                    let _ = write.send(Message::Text(msg.clone().into())).await;
                    sent = true;
                }
            }
        }
    });
    port
}

#[tokio::test]
async fn tencent_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_tencent_mock("hello world").await;
    let endpoint = format!("ws://127.0.0.1:{port}");

    // Tencent packs its three credentials into api_key as `secret_id|secret_key|app_id`.
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "tencent".into(),
        api_key: "test-sid|test-skey|test-appid".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(&endpoint);

    let mut stt = create_stt_standard("tencent", std).expect("build tencent via keystone");

    let best = Arc::new(tokio::sync::Mutex::new(String::new()));
    let b2 = best.clone();
    stt.on_result(Arc::new(move |r: STTResult| {
        let b = b2.clone();
        Box::pin(async move {
            let t = r.transcript.trim().to_string();
            if !t.is_empty() {
                *b.lock().await = t;
            }
        })
    }))
    .await
    .unwrap();

    stt.connect().await.expect("connect to mock endpoint");
    for _ in 0..5 {
        let _ = stt.send_audio(bytes::Bytes::from(vec![0u8; 640])).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    stt.disconnect().await.ok();

    let got = best.lock().await.clone();
    println!("Tencent mock e2e surfaced transcript: {got:?}");
    assert_eq!(
        got, "hello world",
        "Tencent did not surface the mock transcript end-to-end (full integration broken)"
    );
}

/// Shared driver: connect, stream 5 small audio chunks, wait, disconnect, return the surfaced
/// transcript. `audio_is_text` providers (iFlytek) send audio as JSON Text not Binary.
async fn drive_and_capture(stt: &mut dyn waav_gateway::core::stt::BaseSTT) -> String {
    let best = Arc::new(tokio::sync::Mutex::new(String::new()));
    let b2 = best.clone();
    stt.on_result(Arc::new(move |r: STTResult| {
        let b = b2.clone();
        Box::pin(async move {
            let t = r.transcript.trim().to_string();
            if !t.is_empty() {
                *b.lock().await = t;
            }
        })
    }))
    .await
    .unwrap();
    stt.connect().await.expect("connect to mock endpoint");
    for _ in 0..6 {
        let _ = stt.send_audio(bytes::Bytes::from(vec![0u8; 1280])).await;
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    stt.disconnect().await.ok();
    let g = best.lock().await.clone();
    g
}

/// iFlytek mock: client streams audio as JSON `Message::Text` (base64 inside), so we trigger on the
/// first Text frame and reply with iFlytek's real response shape (ws[].cw[].w, status==2 final).
async fn spawn_iflytek_mock(transcript_value: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(ws) = tokio_tungstenite::accept_async(stream).await
        {
            let (mut write, mut read) = ws.split();
            let msg = format!(
                r#"{{"code":0,"message":"success","sid":"mock-sid-1","data":{{"result":{{"ws":[{{"bg":0,"cw":[{{"w":"{transcript_value}"}}]}}],"sn":1,"ls":true}},"status":2}}}}"#
            );
            let mut sent = false;
            while let Some(Ok(frame)) = read.next().await {
                if !sent && matches!(frame, Message::Text(_)) {
                    let _ = write.send(Message::Text(msg.clone().into())).await;
                    sent = true;
                }
            }
        }
    });
    port
}

#[tokio::test]
async fn iflytek_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_iflytek_mock("hello world").await;
    let endpoint = format!("ws://127.0.0.1:{port}");
    // iFlytek packs 3 creds into api_key as `app_id|api_key|api_secret`.
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "iflytek".into(),
        api_key: "test-app-id|test-api-key|test-api-secret".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(&endpoint);
    let mut stt = create_stt_standard("iflytek", std).expect("build iflytek via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("iFlytek mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "iFlytek full integration broken");
}

/// Prosa mock: streams audio as `Message::Binary`; replies with the `{"type":"result",...}` shape.
async fn spawn_prosa_mock(transcript_value: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(ws) = tokio_tungstenite::accept_async(stream).await
        {
            let (mut write, mut read) = ws.split();
            let msg = format!(
                r#"{{"type":"result","transcript":"{transcript_value}","time_start":0.0,"time_end":1.0}}"#
            );
            let mut sent = false;
            while let Some(Ok(frame)) = read.next().await {
                if !sent && matches!(frame, Message::Binary(_)) {
                    let _ = write.send(Message::Text(msg.clone().into())).await;
                    sent = true;
                }
            }
        }
    });
    port
}

#[tokio::test]
async fn prosa_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_prosa_mock("hello world").await;
    let endpoint = format!("ws://127.0.0.1:{port}");
    // Prosa only dials the WS when the streaming model is selected (model field).
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "prosa_ai".into(),
        api_key: "test-key".into(),
        language: "id".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: "stt-general-online".into(),
    })
    .with_endpoint_override(&endpoint);
    let mut stt = create_stt_standard("prosa_ai", std).expect("build prosa via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("Prosa mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Prosa full integration broken");
}

/// Speechmatics mock: the client requests the `json` WS subprotocol, so we MUST echo it via
/// `accept_hdr_async` (a plain accept fails the handshake). Replies with an `AddTranscript` message.
async fn spawn_speechmatics_mock(transcript_value: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let cb = |_req: &Request, mut response: Response| {
                response
                    .headers_mut()
                    .append("Sec-WebSocket-Protocol", HeaderValue::from_static("json"));
                Ok(response)
            };
            if let Ok(ws) = accept_hdr_async(stream, cb).await {
                let (mut write, mut read) = ws.split();
                let msg = format!(
                    r#"{{"message":"AddTranscript","format":"2.9","metadata":{{"start_time":0.0,"end_time":1.0,"transcript":"{transcript_value}"}},"results":[]}}"#
                );
                let mut sent = false;
                while let Some(Ok(frame)) = read.next().await {
                    if !sent && matches!(frame, Message::Binary(_)) {
                        let _ = write.send(Message::Text(msg.clone().into())).await;
                        sent = true;
                    }
                }
            }
        }
    });
    port
}

#[tokio::test]
async fn speechmatics_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_speechmatics_mock("hello world").await;
    let endpoint = format!("ws://127.0.0.1:{port}");
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "speechmatics".into(),
        api_key: "test-key".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(&endpoint);
    let mut stt = create_stt_standard("speechmatics", std).expect("build speechmatics via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("Speechmatics mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Speechmatics full integration broken");
}

/// AmiVoice mock: connect() blocks on the session-start ack, so the mock sends a bare `s`
/// (SessionStartOk) IMMEDIATELY on connect (not gated on audio, or connect deadlocks); then on the
/// first audio frame it emits a final result `A <json>` (AmiVoice's real `A `+JSON wire shape).
async fn spawn_amivoice_mock(transcript_value: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(ws) = tokio_tungstenite::accept_async(stream).await
        {
            let (mut write, mut read) = ws.split();
            let _ = write.send(Message::Text("s".into())).await;
            let final_msg = format!(
                r#"A {{"results":[{{"text":"{transcript_value}","tokens":[{{"written":"{transcript_value}","confidence":0.99}}]}}],"text":"{transcript_value}","code":"0","message":"success"}}"#
            );
            let mut sent = false;
            while let Some(Ok(frame)) = read.next().await {
                if !sent && matches!(frame, Message::Binary(_)) {
                    let _ = write.send(Message::Text(final_msg.clone().into())).await;
                    sent = true;
                }
            }
        }
    });
    port
}

#[tokio::test]
async fn amivoice_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_amivoice_mock("hello world").await;
    let endpoint = format!("ws://127.0.0.1:{port}");
    // AmiVoice reads its APPKEY from api_key (emitted as `authorization=` on the `s` start command).
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "amivoice".into(),
        api_key: "test-key".into(),
        language: "ja".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(&endpoint);
    let mut stt = create_stt_standard("amivoice", std).expect("build amivoice via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("AmiVoice mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "AmiVoice full integration broken");
}

/// Azure mock: USP protocol. connect() self-unblocks on its own handshake+config-send (no server
/// ack needed), no WS subprotocol. Audio is USP Binary frames; reply on the first Binary with a USP
/// `Path:speech.phrase` Text frame (CRLF header block + JSON) whose DisplayText surfaces the text.
async fn spawn_azure_mock(transcript_value: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(ws) = tokio_tungstenite::accept_async(stream).await
        {
            let (mut write, mut read) = ws.split();
            let final_msg = format!(
                "X-RequestId:mockreq1\r\nPath:speech.phrase\r\nContent-Type:application/json\r\n\r\n{{\"RecognitionStatus\":\"Success\",\"Offset\":0,\"Duration\":10000000,\"DisplayText\":\"{transcript_value}\"}}"
            );
            let mut sent = false;
            while let Some(Ok(frame)) = read.next().await {
                if !sent && matches!(frame, Message::Binary(_)) {
                    let _ = write.send(Message::Text(final_msg.clone().into())).await;
                    sent = true;
                }
            }
        }
    });
    port
}

#[tokio::test]
async fn azure_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_azure_mock("hello world").await;
    let endpoint = format!("ws://127.0.0.1:{port}");
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "azure".into(),
        api_key: "test-key".into(),
        language: "en-US".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(&endpoint);
    let mut stt = create_stt_standard("azure", std).expect("build azure via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("Azure mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Azure full integration broken");
}

/// Alibaba DashScope (Paraformer) mock: plain accept (no subprotocol); audio is Binary; connect()
/// does NOT block on a server ack. Reply with a `result-generated` event (sentence_end=true=final).
async fn spawn_dashscope_mock(transcript_value: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(ws) = tokio_tungstenite::accept_async(stream).await
        {
            let (mut write, mut read) = ws.split();
            let msg = format!(
                r#"{{"header":{{"task_id":"mock-task-1","event":"result-generated"}},"payload":{{"output":{{"sentence":{{"begin_time":0,"end_time":1000,"text":"{transcript_value}","words":[],"sentence_end":true}}}}}}}}"#
            );
            let mut sent = false;
            while let Some(Ok(frame)) = read.next().await {
                if !sent && matches!(frame, Message::Binary(_)) {
                    let _ = write.send(Message::Text(msg.clone().into())).await;
                    sent = true;
                }
            }
        }
    });
    port
}

#[tokio::test]
async fn dashscope_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_dashscope_mock("hello world").await;
    let endpoint = format!("ws://127.0.0.1:{port}");
    // model="paraformer-realtime-v2" selects the simpler inference/run-task sub-protocol.
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "alibaba_cloud".into(),
        api_key: "test-key".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm".into(),
        model: "paraformer-realtime-v2".into(),
    })
    .with_endpoint_override(&endpoint);
    let mut stt = create_stt_standard("alibaba_cloud", std).expect("build dashscope via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("DashScope mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "DashScope full integration broken");
}

/// Baidu mock: realtime WS (OAuth is dead code on this path). Audio is Binary; connect() doesn't
/// block on a server ack. Reply with a `FIN_TEXT` final result.
async fn spawn_baidu_mock(transcript_value: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(ws) = tokio_tungstenite::accept_async(stream).await
        {
            let (mut write, mut read) = ws.split();
            let msg = format!(
                r#"{{"err_no":0,"err_msg":"success","type":"FIN_TEXT","result":"{transcript_value}","sn":"mock-1"}}"#
            );
            let mut sent = false;
            while let Some(Ok(frame)) = read.next().await {
                if !sent && matches!(frame, Message::Binary(_)) {
                    let _ = write.send(Message::Text(msg.clone().into())).await;
                    sent = true;
                }
            }
        }
    });
    port
}

#[tokio::test]
async fn baidu_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_baidu_mock("hello world").await;
    let endpoint = format!("ws://127.0.0.1:{port}");
    // Baidu packs api_key|secret_key into api_key; model "mandarin" → dev_pid 1537.
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "baidu".into(),
        api_key: "test-app-id|test-app-key".into(),
        language: "zh".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm".into(),
        model: "mandarin".into(),
    })
    .with_endpoint_override(&endpoint);
    let mut stt = create_stt_standard("baidu", std).expect("build baidu via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("Baidu mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Baidu full integration broken");
}

/// NAVER CLOVA is a REST/batch provider (not WS): it buffers audio and POSTs it on disconnect().
/// So the mock is an axum HTTP server returning `{"text":...}`, and the e2e triggers the POST via
/// disconnect(), not a streamed frame.
async fn spawn_naver_mock(transcript_value: &'static str) -> u16 {
    use axum::{Router, http::header, routing::post};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = format!(r#"{{"text":"{transcript_value}"}}"#);
    let app = Router::new().route(
        "/recog/v1/stt",
        post(move || {
            let body = body.clone();
            async move { ([(header::CONTENT_TYPE, "application/json")], body) }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

#[tokio::test]
async fn naver_clova_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_naver_mock("hello world").await;
    let endpoint = format!("http://127.0.0.1:{port}"); // HTTP, not ws://
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "naver_clova".into(),
        api_key: "test-client-id|test-client-secret".into(),
        language: "ko".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(&endpoint);
    let mut stt = create_stt_standard("naver_clova", std).expect("build naver via keystone");

    let best = Arc::new(tokio::sync::Mutex::new(String::new()));
    let b2 = best.clone();
    stt.on_result(Arc::new(move |r: STTResult| {
        let b = b2.clone();
        Box::pin(async move {
            let t = r.transcript.trim().to_string();
            if !t.is_empty() {
                *b.lock().await = t;
            }
        })
    }))
    .await
    .unwrap();

    stt.connect().await.expect("connect (local, no I/O)");
    for _ in 0..5 {
        let _ = stt.send_audio(bytes::Bytes::from(vec![0u8; 640])).await;
    }
    // REST: the batch POST + on_result callback fire on disconnect().
    stt.disconnect().await.ok();

    let got = best.lock().await.clone();
    println!("NAVER CLOVA mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "NAVER CLOVA full integration broken");
}
