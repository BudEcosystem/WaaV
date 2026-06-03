//! Regression probe for a CONNECT-LEVEL provider bug class surfaced while live-testing Sarvam STT:
//! a hand-built `http::Request` passed to `connect_async` MUST already carry the 5 WebSocket
//! upgrade headers (`Host`, `Connection`, `Upgrade`, `Sec-WebSocket-Version`, `Sec-WebSocket-Key`).
//!
//! tungstenite's `IntoClientRequest for http::Request<()>` is the identity (`Ok(self)`) — it injects
//! NOTHING — and the client handshake's `generate_request` returns `Error::Protocol(InvalidHeader)`
//! if any of those 5 is absent. A bare `Request::builder().uri(url).header("Authorization", ...)`
//! therefore fails EVERY connect attempt; under a reconnect supervisor that error is swallowed and
//! surfaces only as an outer "connection timeout" (exactly how the Sarvam + alibaba_cloud bugs
//! presented). Passing a URL *string* (or `url.into_client_request()`) is safe — that path builds
//! the full handshake with all 5 headers.
//!
//! This test pins tungstenite's contract on THIS machine + crate version so a future provider that
//! hand-builds a bare upgrade request is caught structurally, without needing a vendor key.

use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::Request;

fn ensure_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// A reachable wss endpoint whose TLS terminates fast; we only care about the CLIENT-SIDE handshake
/// header validation, which runs before any bytes are sent, so auth/credentials are irrelevant.
const WSS_URL: &str = "wss://dashscope.aliyuncs.com/api-ws/v1/inference";

/// The BARE pattern (what broken providers did): no WS upgrade headers.
fn bare_request() -> Request<()> {
    Request::builder()
        .uri(WSS_URL)
        .header("Authorization", "Bearer test")
        .header("User-Agent", "WaaV-Gateway/1.0")
        .body(())
        .unwrap()
}

/// The FIXED pattern: derive the full handshake via `into_client_request`, then add auth on top.
fn fixed_request() -> Request<()> {
    let mut req = WSS_URL.into_client_request().unwrap();
    req.headers_mut().insert(
        "Authorization",
        "Bearer test".parse().unwrap(),
    );
    req.headers_mut()
        .insert("User-Agent", "WaaV-Gateway/1.0".parse().unwrap());
    req
}

#[tokio::test]
async fn bare_upgrade_request_fails_client_side_header_validation() {
    ensure_crypto();
    // The bare request is missing all 5 WS headers; tungstenite rejects it during the client
    // handshake. This must be an immediate protocol/header error, NOT a successful connect.
    let err = connect_async(bare_request())
        .await
        .expect_err("a bare request (no WS upgrade headers) MUST NOT connect");
    let msg = format!("{err:?}");
    // The first missing header tungstenite checks is `Host`.
    assert!(
        msg.contains("Host") || msg.to_lowercase().contains("header") || msg.contains("Protocol"),
        "expected a missing-WS-header protocol error, got: {msg}"
    );
    eprintln!("bare request correctly rejected: {msg}");
}

#[tokio::test]
async fn fixed_request_passes_header_validation() {
    ensure_crypto();
    // The fixed request carries all 5 headers, so it passes client-side handshake validation. It may
    // still fail later for network/TLS/HTTP reasons (no real creds), but it MUST NOT fail with the
    // missing-header protocol error the bare request hits — proving the header bug is gone.
    match connect_async(fixed_request()).await {
        Ok(_) => eprintln!("fixed request connected (unexpected but fine)"),
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
