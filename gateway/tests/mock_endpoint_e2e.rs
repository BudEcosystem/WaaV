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
