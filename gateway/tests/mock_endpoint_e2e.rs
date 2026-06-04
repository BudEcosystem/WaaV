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

/// Shared REST driver: connect (local, no I/O), buffer some audio, then disconnect() — which is
/// what triggers the batch POST + on_result callback for the disconnect-flush REST providers.
async fn drive_rest_and_capture(stt: &mut dyn waav_gateway::core::stt::BaseSTT) -> String {
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
    for _ in 0..6 {
        let _ = stt.send_audio(bytes::Bytes::from(vec![0u8; 1280])).await;
    }
    stt.disconnect().await.ok();
    let g = best.lock().await.clone();
    g
}

/// Generic axum HTTP mock: serve `body` on `path` (one POST route). Returns the bound port.
async fn spawn_http_mock(path: &'static str, body: String) -> u16 {
    use axum::{Router, http::header, routing::post};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().route(
        path,
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
async fn groq_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_http_mock(
        "/openai/v1/audio/transcriptions",
        r#"{"text":"hello world"}"#.to_string(),
    )
    .await;
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "groq".into(),
        api_key: "test-key".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut stt = create_stt_standard("groq", std).expect("build groq via keystone");
    let got = drive_rest_and_capture(stt.as_mut()).await;
    println!("Groq mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Groq full integration broken");
}

#[tokio::test]
async fn openai_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_http_mock(
        "/v1/audio/transcriptions",
        r#"{"text":"hello world"}"#.to_string(),
    )
    .await;
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "openai".into(),
        api_key: "test-openai-key".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: "whisper-1".into(),
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut stt = create_stt_standard("openai", std).expect("build openai via keystone");
    let got = drive_rest_and_capture(stt.as_mut()).await;
    println!("OpenAI mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "OpenAI full integration broken");
}

#[tokio::test]
async fn fpt_ai_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_http_mock(
        "/hmi/asr/general",
        r#"{"status":0,"hypotheses":[{"utterance":"hello world"}],"id":"mock-1"}"#.to_string(),
    )
    .await;
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "fpt_ai".into(),
        api_key: "test-key".into(),
        language: "vi".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut stt = create_stt_standard("fpt_ai", std).expect("build fpt via keystone");
    let got = drive_rest_and_capture(stt.as_mut()).await;
    println!("FPT.AI mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "FPT.AI full integration broken");
}

#[tokio::test]
async fn viettel_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_http_mock(
        "/voice/api/asr/v1/rest/decode_file",
        r#"{"status":0,"result":"hello world"}"#.to_string(),
    )
    .await;
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "viettel_ai".into(),
        api_key: "test-token".into(),
        language: "vi".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut stt = create_stt_standard("viettel_ai", std).expect("build viettel via keystone");
    let got = drive_rest_and_capture(stt.as_mut()).await;
    println!("Viettel mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Viettel full integration broken");
}

#[tokio::test]
async fn yandex_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_http_mock(
        "/speech/v1/stt:recognize",
        r#"{"result":"hello world"}"#.to_string(),
    )
    .await;
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "yandex".into(),
        api_key: "test-key".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "lpcm".into(),
        model: String::new(),
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut stt = create_stt_standard("yandex", std).expect("build yandex via keystone");

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

    stt.connect().await.expect("connect");
    for _ in 0..6 {
        let _ = stt.send_audio(bytes::Bytes::from(vec![0u8; 1280])).await;
    }
    // Yandex POSTs from a background task (~500ms cadence once the buffer ≥ 1600 bytes), NOT on
    // disconnect() — so poll for the callback before tearing the session down.
    let mut got = String::new();
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        got = best.lock().await.clone();
        if !got.is_empty() {
            break;
        }
    }
    stt.disconnect().await.ok();
    println!("Yandex mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Yandex full integration broken");
}

/// Sberdevices SaluteSpeech mock: TWO routes on one host — the OAuth token endpoint (hit on
/// connect()) and the speech:recognize endpoint (hit by a background task). One axum router with two
/// `.route(...)` entries serves both (the single endpoint_override rewrites both Sber hosts to here).
async fn spawn_sber_mock(transcript_value: &'static str) -> u16 {
    use axum::{Router, http::header, routing::post};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let recog = format!(r#"{{"result":["{transcript_value}"],"status":200}}"#);
    let app = Router::new()
        .route(
            "/api/v2/oauth",
            post(|| async {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"access_token":"mock-access-token","expires_at":9999999999999}"#,
                )
            }),
        )
        .route(
            "/rest/v1/speech:recognize",
            post(move || {
                let body = recog.clone();
                async move { ([(header::CONTENT_TYPE, "application/json")], body) }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

#[tokio::test]
async fn sberdevices_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_sber_mock("hello world").await;
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "sberdevices".into(),
        api_key: "test_client:test_secret".into(),
        language: "ru-RU".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm".into(),
        model: String::new(),
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut stt = create_stt_standard("sberdevices", std).expect("build sber via keystone");

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

    stt.connect().await.expect("connect (hits the OAuth route)");
    for _ in 0..6 {
        let _ = stt.send_audio(bytes::Bytes::from(vec![0u8; 1280])).await;
    }
    // Sber's recognize POST fires from a background task (~500ms cadence once buffer ≥ 3200 bytes),
    // not on disconnect() — poll for the callback before tearing the session down.
    let mut got = String::new();
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        got = best.lock().await.clone();
        if !got.is_empty() {
            break;
        }
    }
    stt.disconnect().await.ok();
    println!("Sberdevices mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Sberdevices full integration broken");
}

/// IBM Watson axum WS handler: reply to the client's `start` text frame with `{"state":"listening"}`
/// (its connect() blocks on it), then emit a final results message on the first audio (Binary) frame.
async fn ibm_ws_handler(mut socket: axum::extract::ws::WebSocket, results: String) {
    use axum::extract::ws::Message as Aws;
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Aws::Text(_) => {
                let _ = socket
                    .send(Aws::Text(r#"{"state":"listening"}"#.into()))
                    .await;
            }
            Aws::Binary(_) => {
                let _ = socket.send(Aws::Text(results.clone().into())).await;
            }
            _ => {}
        }
    }
}

/// IBM Watson mock: connect() does an IAM HTTP token POST THEN a WS dial — so ONE axum server serves
/// BOTH (POST /identity/token + a WS upgrade on the recognize path). The single endpoint_override
/// rewrites both hosts to here; the config re-applies http:// for IAM and ws:// for the WS dial.
async fn spawn_ibm_mock(transcript_value: &'static str) -> u16 {
    use axum::extract::ws::WebSocketUpgrade;
    use axum::{
        Router,
        http::header,
        routing::{get, post},
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let results = format!(
        r#"{{"results":[{{"alternatives":[{{"transcript":"{transcript_value}","confidence":0.95}}],"final":true}}],"result_index":0}}"#
    );
    let app = Router::new()
        .route(
            "/identity/token",
            post(|| async {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"access_token":"mock-token","expires_in":3600}"#,
                )
            }),
        )
        .route(
            "/instances/{id}/v1/recognize",
            get(move |ws: WebSocketUpgrade| {
                let results = results.clone();
                async move { ws.on_upgrade(move |socket| ibm_ws_handler(socket, results)) }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

#[tokio::test]
async fn ibm_watson_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_ibm_mock("hello world").await;
    // IBM reads instance_id from extras; the override (authority only) drives both the IAM POST and
    // the WS dial against this one mock.
    let mut std = StandardSTTConfig::from_base(STTConfig {
        provider: "ibm_watson".into(),
        api_key: "test-key".into(),
        language: "en-US".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(format!("127.0.0.1:{port}"));
    std.extras
        .0
        .insert("instance_id".into(), serde_json::json!("inst-test"));
    let mut stt = create_stt_standard("ibm_watson", std).expect("build ibm via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("IBM Watson mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "IBM Watson full integration broken");
}

// ============================================================================
// STT WS tail: assemblyai, cartesia, elevenlabs, sarvam, phonexia (parametric
// WS mock) + gladia (2-step init-POST + WS, combined axum mock).
// ============================================================================

/// Parametric STT WS mock: optionally send `ack` first (handshake the client blocks on), then emit
/// `transcript` once the first inbound frame of the matching kind arrives (`trigger_text` → the
/// provider sends audio as JSON Text; otherwise Binary). Drains the rest so the client keeps streaming.
async fn spawn_stt_ws_mock(ack: Option<&'static str>, transcript: String, trigger_text: bool) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(ws) = tokio_tungstenite::accept_async(stream).await
        {
            let (mut write, mut read) = ws.split();
            if let Some(a) = ack {
                let _ = write.send(Message::Text(a.into())).await;
            }
            // Emit the transcript once the client starts streaming (its first data frame, Text or
            // Binary — providers vary; some send a Text control frame before audio). `trigger_text`
            // is retained only to document the provider's audio frame type.
            let _ = trigger_text;
            let mut sent = false;
            while let Some(Ok(frame)) = read.next().await {
                let hit = matches!(frame, Message::Text(_) | Message::Binary(_));
                if !sent && hit {
                    let _ = write.send(Message::Text(transcript.clone().into())).await;
                    sent = true;
                }
            }
        }
    });
    port
}

#[tokio::test]
async fn assemblyai_full_integration_via_mock_endpoint() {
    ensure_crypto();
    // AssemblyAI v3: connect() blocks on a `Begin` ack, then a `Turn` with end_of_turn=true is final.
    let port = spawn_stt_ws_mock(
        Some(r#"{"type":"Begin","id":"s","expires_at":1704067200}"#),
        r#"{"type":"Turn","turn_order":0,"transcript":"hello world","end_of_turn":true}"#.to_string(),
        false,
    )
    .await;
    let endpoint = format!("ws://127.0.0.1:{port}");
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "assemblyai".into(),
        api_key: "test-key".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(&endpoint);
    let mut stt = create_stt_standard("assemblyai", std).expect("build assemblyai via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("AssemblyAI mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "AssemblyAI full integration broken");
}

#[tokio::test]
async fn cartesia_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_stt_ws_mock(
        None,
        r#"{"type":"transcript","text":"hello world","is_final":true}"#.to_string(),
        false,
    )
    .await;
    let endpoint = format!("ws://127.0.0.1:{port}");
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "cartesia".into(),
        api_key: "test-key".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm_s16le".into(),
        model: "ink-whisper".into(),
    })
    .with_endpoint_override(&endpoint);
    let mut stt = create_stt_standard("cartesia", std).expect("build cartesia via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("Cartesia mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Cartesia full integration broken");
}

#[tokio::test]
async fn elevenlabs_full_integration_via_mock_endpoint() {
    ensure_crypto();
    // ElevenLabs blocks on `session_started`; audio is JSON Text; `committed_transcript` is final.
    let port = spawn_stt_ws_mock(
        Some(r#"{"message_type":"session_started","session_id":"s"}"#),
        r#"{"message_type":"committed_transcript","text":"hello world"}"#.to_string(),
        true,
    )
    .await;
    let endpoint = format!("ws://127.0.0.1:{port}");
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "elevenlabs".into(),
        api_key: "test-key".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(&endpoint);
    let mut stt = create_stt_standard("elevenlabs", std).expect("build elevenlabs via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("ElevenLabs mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "ElevenLabs full integration broken");
}

#[tokio::test]
async fn sarvam_full_integration_via_mock_endpoint() {
    ensure_crypto();
    // Sarvam: no ack; audio is JSON Text; `{"type":"data",...}` carries the transcript.
    let port = spawn_stt_ws_mock(
        None,
        r#"{"type":"data","data":{"transcript":"hello world","is_final":true}}"#.to_string(),
        true,
    )
    .await;
    let endpoint = format!("ws://127.0.0.1:{port}");
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "sarvam".into(),
        api_key: "test-key".into(),
        language: "en-IN".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "pcm_s16le".into(),
        model: String::new(),
    })
    .with_endpoint_override(&endpoint);
    let mut stt = create_stt_standard("sarvam", std).expect("build sarvam via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("Sarvam mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Sarvam full integration broken");
}

#[tokio::test]
async fn phonexia_full_integration_via_mock_endpoint() {
    ensure_crypto();
    // Phonexia STT is gated off by default (its WS protocol is not validated against the real
    // vendor — PRODUCTION_PLAN W3). This e2e proves the IMPLEMENTED protocol's wire path
    // (connect -> audio -> parse -> callback) end-to-end against a mock speaking that same protocol;
    // it does NOT certify the protocol matches the real Phonexia server. The opt-in flag is required.
    unsafe {
        std::env::set_var("WAAV_PHONEXIA_ALLOW_UNVERIFIED", "1");
    }
    // Phonexia is on-prem: the SERVER URL is carried in `api_key` (no endpoint_override needed).
    let port = spawn_stt_ws_mock(
        None,
        r#"{"is_last":true,"segments":[{"words":[{"text":"hello world","confidence":0.95}]}]}"#
            .to_string(),
        false,
    )
    .await;
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "phonexia".into(),
        // The api_key field carries the Phonexia server URL; point it at the mock.
        api_key: format!("http://127.0.0.1:{port}"),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    });
    let mut stt = create_stt_standard("phonexia", std).expect("build phonexia via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("Phonexia mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Phonexia full integration broken");
}

/// Gladia WS handler: gladia sends audio as JSON Text (`{"type":"audio_chunk",...}`); emit the
/// transcript on the first Text frame.
async fn gladia_ws_handler(mut socket: axum::extract::ws::WebSocket, transcript: String) {
    use axum::extract::ws::Message as Aws;
    let mut sent = false;
    while let Some(Ok(msg)) = socket.recv().await {
        if let Aws::Text(_) = msg
            && !sent
        {
            let _ = socket.send(Aws::Text(transcript.clone().into())).await;
            sent = true;
        }
    }
}

/// Gladia mock: connect() does a session-init POST (`/v2/live`) that returns a WS `url`, then dials
/// it. ONE axum server serves both (POST returns a `ws://this-port/ws` url + a WS upgrade on `/ws`).
async fn spawn_gladia_mock(transcript_value: &'static str) -> u16 {
    use axum::extract::ws::WebSocketUpgrade;
    use axum::{
        Router,
        http::header,
        routing::{get, post},
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let transcript = format!(
        r#"{{"type":"transcript","session_id":"s","created_at":"2026-06-04T00:00:00.000Z","data":{{"id":"00-1","is_final":true,"utterance":{{"text":"{transcript_value}","language":"en","start":0.0,"end":1.5,"confidence":0.95,"channel":0}}}}}}"#
    );
    let init_body =
        format!(r#"{{"id":"sess","created_at":"2026-06-04T00:00:00.000Z","url":"ws://127.0.0.1:{port}/ws"}}"#);
    let app = Router::new()
        .route(
            "/v2/live",
            post(move || {
                let b = init_body.clone();
                async move { ([(header::CONTENT_TYPE, "application/json")], b) }
            }),
        )
        .route(
            "/ws",
            get(move |ws: WebSocketUpgrade| {
                let t = transcript.clone();
                async move { ws.on_upgrade(move |s| gladia_ws_handler(s, t)) }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

#[tokio::test]
async fn gladia_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_gladia_mock("hello world").await;
    // endpoint_override redirects the session-init POST; the init response's `url` (ws://this-port/ws)
    // drives the WS dial back to the same mock.
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "gladia".into(),
        api_key: "test-key".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut stt = create_stt_standard("gladia", std).expect("build gladia via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("Gladia mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Gladia full integration broken");
}

// ============================================================================
// STT 2-step REST (nectec, bhashini) + token-WS (huawei_cloud).
// ============================================================================

#[tokio::test]
async fn nectec_full_integration_via_mock_endpoint() {
    use axum::{Router, http::header, routing::post};
    ensure_crypto();
    // NECTEC is batch: buffer audio, POST once on flush()/disconnect(); partii5 returns {"content":...}.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().fallback(post(|| async {
        (
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"content":"hello world"}"#,
        )
    }));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "nectec".into(),
        api_key: "test-key".into(),
        language: "th".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: "partii5".into(),
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut stt = create_stt_standard("nectec", std).expect("build nectec via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("NECTEC mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "NECTEC full integration broken");
}

#[tokio::test]
async fn bhashini_full_integration_via_mock_endpoint() {
    use axum::{Router, http::header, routing::post};
    ensure_crypto();
    // Bhashini is 2-step: pipeline-config POST returns a callbackUrl + inference key; the compute
    // POST to that callbackUrl returns the ASR text. One axum server serves both.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config_body = format!(
        r#"{{"pipelineInferenceAPIEndPoint":{{"callbackUrl":"http://127.0.0.1:{port}/compute","inferenceApiKey":{{"name":"Authorization","value":"infk"}}}},"pipelineResponseConfig":[{{"taskType":"asr","config":[{{"serviceId":"svc"}}]}}]}}"#
    );
    let app = Router::new()
        .route(
            "/ulca/apis/v0/model/getModelsPipeline",
            post(move || {
                let b = config_body.clone();
                async move { ([(header::CONTENT_TYPE, "application/json")], b) }
            }),
        )
        .route(
            "/compute",
            post(|| async {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"pipelineResponse":[{"taskType":"asr","output":[{"source":"hello world"}],"audio":[]}]}"#,
                )
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "bhashini".into(),
        // userId|ulcaApiKey|inferenceApiKey packing.
        api_key: "test-user|test-ulca|test-inference".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut stt = create_stt_standard("bhashini", std).expect("build bhashini via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("Bhashini mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Bhashini full integration broken");
}

/// Huawei ASR WS handler: reply `STARTED` to the START control frame (unblocks connect), then emit
/// the final `END` transcript on the first binary audio frame.
async fn huawei_ws_handler(mut socket: axum::extract::ws::WebSocket, transcript: String) {
    use axum::extract::ws::Message as Aws;
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Aws::Text(_) => {
                let _ = socket
                    .send(Aws::Text(r#"{"resp_type":"STARTED","error_code":0}"#.into()))
                    .await;
            }
            Aws::Binary(_) => {
                let _ = socket.send(Aws::Text(transcript.clone().into())).await;
            }
            _ => {}
        }
    }
}

/// Huawei STT mock: connect() does an IAM token POST (token in the `X-Subject-Token` RESPONSE HEADER)
/// THEN a WS dial. ONE axum server serves both (POST /v3/auth/tokens + a WS upgrade on the rasr path).
async fn spawn_huawei_stt_mock(project_id: &str, transcript_value: &'static str) -> u16 {
    use axum::extract::ws::WebSocketUpgrade;
    use axum::http::HeaderName;
    use axum::{
        Router,
        routing::{get, post},
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let transcript = format!(
        r#"{{"resp_type":"END","error_code":0,"result":{{"text":"{transcript_value}","score":0.95,"is_final":true}}}}"#
    );
    let ws_path = format!("/v1/{project_id}/rasr/short-stream");
    let app = Router::new()
        .route(
            "/v3/auth/tokens",
            post(|| async {
                ([(HeaderName::from_static("x-subject-token"), "mock-token")], "{}")
            }),
        )
        .route(
            &ws_path,
            get(move |ws: WebSocketUpgrade| {
                let t = transcript.clone();
                async move { ws.on_upgrade(move |s| huawei_ws_handler(s, t)) }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

#[tokio::test]
async fn huawei_cloud_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_huawei_stt_mock("proj1", "hello world").await;
    // username|password|domain_name|project_id packing; override base is http:// (reqwest needs it
    // for the IAM POST; get_realtime_url normalizes the WS dial back to ws://).
    let std = StandardSTTConfig::from_base(STTConfig {
        provider: "huawei_cloud".into(),
        api_key: "test-user|test-pass|test-domain|proj1".into(),
        language: "en".into(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".into(),
        model: String::new(),
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut stt = create_stt_standard("huawei_cloud", std).expect("build huawei via keystone");
    let got = drive_and_capture(stt.as_mut()).await;
    println!("Huawei Cloud mock e2e surfaced transcript: {got:?}");
    assert_eq!(got, "hello world", "Huawei Cloud full integration broken");
}
