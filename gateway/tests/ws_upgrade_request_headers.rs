//! Regression guard for a CONNECT-LEVEL provider bug class surfaced while live-testing Sarvam STT
//! (and which separately rendered Alibaba DashScope STT + TTS 100% unconnectable):
//!
//! a hand-built `http::Request` passed to `connect_async` MUST already carry the 5 WebSocket
//! upgrade headers (`Host`, `Connection`, `Upgrade`, `Sec-WebSocket-Version`, `Sec-WebSocket-Key`).
//!
//! tungstenite's `IntoClientRequest for http::Request<()>` is the identity (`Ok(self)`) — it injects
//! NOTHING — and the client handshake's `generate_request` returns `Error::Protocol(InvalidHeader)`
//! if any of those 5 is absent, BEFORE writing a single byte to the socket. A bare
//! `Request::builder().uri(url).header("Authorization", ...)` therefore fails EVERY connect attempt;
//! under a reconnect supervisor that per-attempt error is swallowed and surfaces only as an outer
//! "connection timeout" (exactly how the Sarvam + alibaba_cloud bugs presented). Passing a URL
//! *string* (or `url.into_client_request()`) is safe — that path builds the full handshake with all
//! 5 headers.
//!
//! This test pins tungstenite's contract on THIS machine + crate version using a LOCAL throwaway TCP
//! listener (ws://, no network, no TLS), so it runs deterministically in CI with no vendor key.

use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::Request;

/// Bind a localhost listener that accepts one connection and then just drains/holds it. The client
/// handshake's header validation runs before (bare request) or independently of (fixed request) any
/// server response, so the listener never needs to speak WebSocket.
async fn spawn_dummy_listener() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            if let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    // Read whatever the client sends (its handshake request, if any) ONCE, then drop
                    // the socket to close the connection. We never send a valid 101, so a client that
                    // got past header validation hits EOF and errors (a non-InvalidHeader error) —
                    // crucially WITHOUT hanging waiting for a response that never comes.
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                });
            }
        }
    });
    port
}

/// The BARE pattern (what the broken providers did): no WS upgrade headers.
fn bare_request(port: u16) -> Request<()> {
    Request::builder()
        .uri(format!("ws://127.0.0.1:{port}/"))
        .header("Authorization", "Bearer test")
        .header("User-Agent", "WaaV-Gateway/1.0")
        .body(())
        .unwrap()
}

/// The FIXED pattern: derive the full handshake via `into_client_request`, then add auth on top.
fn fixed_request(port: u16) -> Request<()> {
    let mut req = format!("ws://127.0.0.1:{port}/")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Authorization", "Bearer test".parse().unwrap());
    req.headers_mut()
        .insert("User-Agent", "WaaV-Gateway/1.0".parse().unwrap());
    req
}

#[tokio::test]
async fn bare_upgrade_request_fails_client_side_header_validation() {
    let port = spawn_dummy_listener().await;
    // The bare request is missing all 5 WS headers; tungstenite rejects it during the client
    // handshake with an InvalidHeader protocol error — it must NOT connect.
    let err = connect_async(bare_request(port))
        .await
        .expect_err("a bare request (no WS upgrade headers) MUST NOT connect");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("InvalidHeader") || msg.contains("Protocol"),
        "expected a missing-WS-header protocol error, got: {msg}"
    );
    eprintln!("bare request correctly rejected: {msg}");
}

#[tokio::test]
async fn fixed_request_passes_header_validation() {
    let port = spawn_dummy_listener().await;
    // The fixed request carries all 5 headers, so it clears client-side handshake validation and
    // proceeds to exchange bytes. The dummy server never returns a valid 101, so it still fails —
    // but NOT with the missing-header protocol error the bare request hits. That delta proves the
    // header bug is gone (a real WS server would complete the upgrade).
    match connect_async(fixed_request(port)).await {
        Ok(_) => eprintln!("fixed request connected (unexpected for the dummy server, but fine)"),
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                !msg.contains("InvalidHeader"),
                "fixed request must clear header validation, but got an InvalidHeader error: {msg}"
            );
            eprintln!("fixed request cleared header validation; later-stage error: {msg}");
        }
    }
}
